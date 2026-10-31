//! The spectator's feed (ADR 0032): this machine's own picture and sound,
//! encoded off the game thread and reported as the bytes a protocol message
//! carries. Only while someone watches; the flow says how many.
//!
//! One channel carries both. A picture is offered only while the encoder
//! has none in hand, so a slow encode costs a spectator a frame and the
//! player nothing beyond one copy of the core's buffer; sound is always
//! taken, because a gap in it is audible and a dropped picture is not.
//!
//! The picture is H.264 (openh264, built from source) in Annex B form with
//! the parameter sets on every key frame, which is what the overlay's
//! `VideoDecoder` takes without a description. A key frame comes every
//! [`KEY_EVERY`] pictures, and sooner when the link dropped some, so a
//! watcher who lost pictures is not left waiting. The sound is mixed to
//! mono near [`SOUND_RATE`] and coded as IMA ADPCM, four bits a sample,
//! with the coder's state at the head of every chunk so a lost chunk is a
//! gap and the next one still decodes. Together they cost about 0.6 Mbit/s;
//! a link that carries less drops what it cannot send (ADR 0034).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, RateControlMode};
use openh264::formats::{ArgbSliceU32, YUVBuffer};
use openh264::{OpenH264API, Timestamp};

use crate::game::Report;

/// What the picture costs the sender's uplink. Half a megabit carries a
/// retro picture at its own rate; a home uplink has room for it, and a
/// phone's drops what it cannot carry rather than falling behind.
const BITRATE: u32 = 500_000;

/// A key frame every two seconds at 60 Hz: the longest a watcher who lost
/// pictures waits for the next one that decodes on its own.
const KEY_EVERY: u32 = 120;

/// The sound's rate, roughly: the core's own divided down to the nearest
/// whole step above it. A retro core's sound loses little at 16 kHz mono,
/// and costs a sixth of 48 kHz stereo.
const SOUND_RATE: u32 = 16_000;

/// Set when the link dropped pictures: the next one encoded is a key frame.
static KEY_WANTED: AtomicBool = AtomicBool::new(false);

/// For the link: pictures were dropped, so start a new group (ADR 0034).
pub fn want_key() {
    KEY_WANTED.store(true, Ordering::Release);
}

/// One core frame, as the core drew it: `0x00RRGGBB`, unscaled.
pub struct Picture {
    pub round: u64,
    pub frame: u64,
    pub width: usize,
    pub height: usize,
    pub aspect: f32,
    pub pixels: Vec<u32>,
    /// Start afresh: a watcher just arrived.
    pub key: bool,
}

/// The samples one or more frames made: interleaved stereo at the core's rate.
pub struct Sound {
    pub round: u64,
    pub frame: u64,
    pub rate: u32,
    pub samples: Vec<i16>,
}

enum Input {
    Picture(Picture),
    Sound(Sound),
}

#[derive(Clone)]
pub struct Streamer {
    tx: Sender<Input>,
    /// Pictures handed over and not yet encoded: never more than one.
    pending: Arc<AtomicUsize>,
}

impl Streamer {
    pub fn spawn(reports: Sender<Report>) -> Streamer {
        let (tx, rx) = mpsc::channel::<Input>();
        let pending = Arc::new(AtomicUsize::new(0));
        let in_hand = Arc::clone(&pending);
        thread::Builder::new()
            .name("encoder".into())
            .spawn(move || {
                let mut video = Video::default();
                let mut thin = Thin::default();
                let mut sound = Adpcm::default();
                let mut complained = false;
                for input in rx {
                    match input {
                        Input::Picture(mut p) => {
                            p.key |= KEY_WANTED.swap(false, Ordering::AcqRel);
                            let encoded = video.encode(&p);
                            in_hand.store(0, Ordering::Release);
                            match encoded {
                                Ok(Some((key, data))) => {
                                    let _ = reports.send(Report::Video {
                                        round: p.round,
                                        frame: p.frame,
                                        key,
                                        width: p.width as u32,
                                        height: p.height as u32,
                                        aspect: p.aspect,
                                        data,
                                    });
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    if !complained {
                                        complained = true;
                                        eprintln!("[stream] cannot encode: {e}");
                                    }
                                }
                            }
                        }
                        Input::Sound(s) => {
                            if s.samples.len() < 2 {
                                continue;
                            }
                            let (rate, mono) = thin.mono(&s.samples, s.rate);
                            if mono.is_empty() {
                                continue;
                            }
                            let data = sound.encode(&mono, 1);
                            let _ = reports.send(Report::Audio { round: s.round, frame: s.frame, rate, channels: 1, data });
                        }
                    }
                }
            })
            .expect("spawn the encoder thread");
        Streamer { tx, pending }
    }

    /// Offers a picture, made only if the encoder is free for it. `false`
    /// when it is still on the last one, and the picture is skipped.
    pub fn offer(&self, make: impl FnOnce() -> Picture) -> bool {
        if self.pending.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return false;
        }
        if self.tx.send(Input::Picture(make())).is_err() {
            self.pending.store(0, Ordering::Release);
            return false;
        }
        true
    }

    pub fn sound(&self, s: Sound) {
        let _ = self.tx.send(Input::Sound(s));
    }
}

// ----------------------------------------------------------------- picture

#[derive(Default)]
struct Video {
    encoder: Option<Encoder>,
    started: Option<Instant>,
}

impl Video {
    /// `Some((key, bytes))`, or `None` for a frame the encoder chose to skip.
    fn encode(&mut self, p: &Picture) -> Result<Option<(bool, Vec<u8>)>, String> {
        // H.264 wants even sides; a core with an odd one loses a row or column.
        let (w, h) = (p.width & !1, p.height & !1);
        if w == 0 || h == 0 || p.pixels.len() < p.width * p.height {
            return Err(format!("a {}x{} picture", p.width, p.height));
        }
        let cropped: Vec<u32>;
        let pixels: &[u32] = if (w, h) == (p.width, p.height) {
            &p.pixels[..w * h]
        } else {
            cropped = p.pixels.chunks(p.width).take(h).flat_map(|row| row[..w].iter().copied()).collect();
            &cropped
        };
        let enc = match self.encoder.as_mut() {
            Some(e) => e,
            None => {
                let config = EncoderConfig::new()
                    .bitrate(BitRate::from_bps(BITRATE))
                    .max_frame_rate(FrameRate::from_hz(60.0))
                    .intra_frame_period(IntraFramePeriod::from_num_frames(KEY_EVERY))
                    .rate_control_mode(RateControlMode::Bitrate);
                let e = Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| e.to_string())?;
                self.started = Some(Instant::now());
                self.encoder.insert(e)
            }
        };
        if p.key {
            enc.force_intra_frame();
        }
        let yuv = YUVBuffer::from_rgb_source(ArgbSliceU32::new(pixels, (w, h)));
        let at = self.started.map_or(0, |t| t.elapsed().as_millis() as u64);
        let bs = enc.encode_at(&yuv, Timestamp::from_millis(at)).map_err(|e| e.to_string())?;
        let key = matches!(bs.frame_type(), FrameType::IDR | FrameType::I);
        let data = bs.to_vec();
        Ok((!data.is_empty()).then_some((key, data)))
    }
}

// ------------------------------------------------------------------- sound

/// Interleaved stereo at the core's rate to mono at about [`SOUND_RATE`]:
/// each output sample the mean of `factor` input frames, which also keeps
/// most of what would alias. A frame left over waits for the next chunk.
#[derive(Default)]
struct Thin {
    rate: u32,
    sum: i32,
    count: u32,
}

impl Thin {
    fn mono(&mut self, stereo: &[i16], rate: u32) -> (u32, Vec<i16>) {
        if rate != self.rate {
            *self = Thin { rate, ..Thin::default() };
        }
        let factor = (rate / SOUND_RATE).max(1);
        let mut out = Vec::with_capacity(stereo.len() / 2 / factor as usize + 1);
        for frame in stereo.chunks_exact(2) {
            self.sum += frame[0] as i32 + frame[1] as i32;
            self.count += 1;
            if self.count == factor {
                out.push((self.sum / (2 * factor as i32)) as i16);
                self.sum = 0;
                self.count = 0;
            }
        }
        (rate / factor, out)
    }
}

const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80,
    88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544,
    598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749,
    3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635,
    13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const INDEX_STEPS: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

#[derive(Clone, Copy, Default)]
struct Coder {
    predictor: i32,
    index: i32,
}

impl Coder {
    fn step(&mut self, sample: i16) -> u8 {
        let step = STEPS[self.index as usize];
        let mut diff = sample as i32 - self.predictor;
        let mut nibble = 0u8;
        if diff < 0 {
            nibble = 8;
            diff = -diff;
        }
        let mut delta = step >> 3;
        if diff >= step {
            nibble |= 4;
            diff -= step;
            delta += step;
        }
        if diff >= step >> 1 {
            nibble |= 2;
            diff -= step >> 1;
            delta += step >> 1;
        }
        if diff >= step >> 2 {
            nibble |= 1;
            delta += step >> 2;
        }
        self.predictor = if nibble & 8 != 0 { self.predictor - delta } else { self.predictor + delta };
        self.predictor = self.predictor.clamp(-32768, 32767);
        self.index = (self.index + INDEX_STEPS[nibble as usize]).clamp(0, 88);
        nibble
    }

    /// The mirror of [`Coder::step`], for the test and for the page.
    #[cfg(test)]
    fn decode(&mut self, nibble: u8) -> i16 {
        let step = STEPS[self.index as usize];
        let mut delta = step >> 3;
        if nibble & 4 != 0 {
            delta += step;
        }
        if nibble & 2 != 0 {
            delta += step >> 1;
        }
        if nibble & 1 != 0 {
            delta += step >> 2;
        }
        self.predictor = if nibble & 8 != 0 { self.predictor - delta } else { self.predictor + delta };
        self.predictor = self.predictor.clamp(-32768, 32767);
        self.index = (self.index + INDEX_STEPS[nibble as usize]).clamp(0, 88);
        self.predictor as i16
    }
}

/// IMA ADPCM across chunks. A chunk is: per channel, the coder's predictor
/// (i16 LE) and step index (u8) as it stands at the chunk's start; then the
/// sample count across channels (u32 LE); then the nibbles in sample order,
/// channels interleaved, low nibble first.
#[derive(Default)]
struct Adpcm {
    coders: [Coder; 2],
}

impl Adpcm {
    fn encode(&mut self, samples: &[i16], channels: usize) -> Vec<u8> {
        let channels = channels.clamp(1, 2);
        let count = samples.len() - samples.len() % channels;
        let mut out = Vec::with_capacity(3 * channels + 4 + count / 2 + 1);
        for c in &self.coders[..channels] {
            out.extend_from_slice(&(c.predictor as i16).to_le_bytes());
            out.push(c.index as u8);
        }
        out.extend_from_slice(&(count as u32).to_le_bytes());
        let mut low: Option<u8> = None;
        for (i, &s) in samples[..count].iter().enumerate() {
            let n = self.coders[i % channels].step(s);
            match low.take() {
                None => low = Some(n),
                Some(l) => out.push(l | (n << 4)),
            }
        }
        if let Some(l) = low {
            out.push(l);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn decode_pub(data: &[u8], channels: usize) -> Vec<i16> {
        decode(data, channels)
    }

    fn decode(data: &[u8], channels: usize) -> Vec<i16> {
        let mut coders = [Coder::default(); 2];
        let mut at = 0;
        for c in coders.iter_mut().take(channels) {
            c.predictor = i16::from_le_bytes([data[at], data[at + 1]]) as i32;
            c.index = data[at + 2] as i32;
            at += 3;
        }
        let count = u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        at += 4;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let byte = data[at + i / 2];
            let nibble = if i % 2 == 0 { byte & 15 } else { byte >> 4 };
            out.push(coders[i % channels].decode(nibble));
        }
        out
    }

    #[test]
    fn adpcm_follows_a_tone_and_restarts_from_any_chunk() {
        let rate = 32000.0;
        let tone: Vec<i16> = (0..2 * 1600)
            .map(|i| {
                let t = (i / 2) as f64 / rate;
                let a = if i % 2 == 0 { 9000.0 } else { 4000.0 };
                (a * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16
            })
            .collect();
        let mut coder = Adpcm::default();
        let first = coder.encode(&tone[..1600], 2);
        let second = coder.encode(&tone[1600..], 2);
        assert_eq!(first.len(), 6 + 4 + 800);
        let back: Vec<i16> = decode(&first, 2).into_iter().chain(decode(&second, 2)).collect();
        assert_eq!(back.len(), tone.len());
        // The first few samples ramp from silence; after that it tracks.
        let worst = tone[200..].iter().zip(&back[200..]).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst < 1500, "worst error {worst}");
        // The second chunk decodes on its own: its header carries the state.
        let alone = decode(&second, 2);
        assert_eq!(alone, &back[1600..]);
    }

    #[test]
    fn sound_thins_to_mono_near_sixteen_kilohertz() {
        let mut t = Thin::default();
        // 48 kHz: three frames to a sample. Seven frames give two, and the
        // seventh waits for the next chunk.
        let stereo: Vec<i16> = (0..7).flat_map(|i| [i * 100, i * 100 + 50]).collect();
        let (rate, mono) = t.mono(&stereo, 48_000);
        assert_eq!((rate, mono.len()), (16_000, 2));
        assert_eq!(mono[0], (50 + 100 + 150 + 200 + 250) / 6, "the first three frames, both channels");
        let (_, more) = t.mono(&[700, 750, 800, 850], 48_000);
        assert_eq!(more, [(600 + 650 + 700 + 750 + 800 + 850) / 6]);
        // SNES: 32040 halves.
        assert_eq!(Thin::default().mono(&[0; 8], 32_040).0, 16_020);
    }

    #[test]
    fn a_first_picture_is_a_key_frame_with_its_parameter_sets() {
        let (w, h) = (256usize, 240usize);
        let pixels: Vec<u32> = (0..w * h).map(|i| ((i % w) as u32) << 16 | ((i / w) as u32) << 8).collect();
        let mut v = Video::default();
        let p = |frame, key| Picture { round: 0, frame, width: w, height: h, aspect: 1.0, pixels: pixels.clone(), key };
        let (key, data) = v.encode(&p(1, false)).expect("openh264 encodes").expect("a frame");
        assert!(key);
        let has = |nal: u8| data.windows(5).any(|s| s[..4] == [0, 0, 0, 1] && s[4] & 0x1f == nal);
        assert!(has(7) && has(8), "SPS and PPS lead the key frame");
        let (key, data) = v.encode(&p(2, false)).unwrap().unwrap();
        assert!(!key && !data.is_empty(), "then a delta frame");
        let (key, _) = v.encode(&p(3, true)).unwrap().unwrap();
        assert!(key, "a key frame on request");
    }
}

#[cfg(test)]
mod cross_check {
    use super::*;

    /// Prints a chunk for the overlay page's decoder to be checked against:
    /// `cargo test -p app-client cross_check -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_a_chunk() {
        let tone: Vec<i16> = (0..64).map(|i| ((i / 2) as f64 * 0.7).sin() as f64 * 12000.0).map(|v| v as i16).collect();
        let mut coder = Adpcm::default();
        let first = coder.encode(&tone[..32], 2);
        let second = coder.encode(&tone[32..], 2);
        println!("CHUNK1 {}", protocol::to_base64(&first));
        println!("CHUNK2 {}", protocol::to_base64(&second));
        let back: Vec<i16> = super::tests::decode_pub(&second, 2);
        println!("BACK2 {:?}", back);
    }
}
