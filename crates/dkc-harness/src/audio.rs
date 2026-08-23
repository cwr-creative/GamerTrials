//! cpal output. The emulator pushes resampled stereo into `libretro::AUDIO`;
//! this drains it.
//!
//! The ring doubles as the frame clock: the main loop refuses to run another
//! frame while the queue is deep, which locks emulation to the sound card
//! rather than to `Instant::now`. Wall-clock pacing drifts against the audio
//! device and produces a click every few seconds.

use std::sync::atomic::Ordering;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

use crate::libretro::{AUDIO, AUDIO_ENABLED, RESAMPLE_STEP};

pub struct Audio {
    pub device_rate: f64,
    pub channels: usize,
    // Dropping the stream stops playback, so it has to be kept alive.
    _stream: cpal::Stream,
}

/// Frames of audio to keep queued. Three frames of a 60 Hz core is ~50 ms:
/// enough that a scheduling hiccup does not underrun, short enough that input
/// still feels attached to the picture.
pub const TARGET_QUEUED_FRAMES: usize = 3;

pub fn start(core_rate: f64) -> Result<Audio, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default audio output device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("no default output config: {e}"))?;

    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    let device_rate = config.sample_rate.0 as f64;

    if channels < 2 {
        return Err(format!("output device is {channels}-channel, need at least 2"));
    }

    // Set the ratio before the stream starts so the first pushed frame resamples
    // correctly rather than being dropped.
    RESAMPLE_STEP.store((core_rate / device_rate).to_bits(), Ordering::Relaxed);

    let err_fn = |e| eprintln!("[audio] stream error: {e}");
    let stream = match sample_format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, channels, err_fn),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, channels, err_fn),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, channels, err_fn),
        other => Err(format!("unsupported sample format {other:?}")),
    }?;

    stream.play().map_err(|e| format!("cannot start audio: {e}"))?;
    AUDIO_ENABLED.store(1, Ordering::Relaxed);

    Ok(Audio {
        device_rate,
        channels,
        _stream: stream,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
                let mut ring = match AUDIO.lock() {
                    Ok(r) => r,
                    Err(e) => e.into_inner(),
                };
                for frame in out.chunks_mut(channels) {
                    // Underrun plays silence. Stretching the last sample instead
                    // sounds worse and hides the problem.
                    let l = ring.samples.pop_front().unwrap_or(0.0);
                    let r = ring.samples.pop_front().unwrap_or(0.0);
                    frame[0] = T::from_sample(l);
                    frame[1] = T::from_sample(r);
                    for extra in frame.iter_mut().skip(2) {
                        *extra = T::from_sample(0.0f32);
                    }
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| format!("cannot build audio stream: {e}"))
}

/// Stereo frames currently queued for playback.
pub fn queued_frames() -> usize {
    let ring = match AUDIO.lock() {
        Ok(r) => r,
        Err(e) => e.into_inner(),
    };
    ring.samples.len() / 2
}
