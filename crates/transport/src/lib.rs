//! One WebSocket's loop, for both ends of the protocol (ADR 0034). The
//! client's link to its server and each of the server's connections run
//! the same [`run`], so a slow link is handled one way in both directions.
//!
//! A socket carries two kinds of message (see [`protocol::Lane`]). Control
//! is the match — events the server judges from, the lobby, pings — and is
//! sent first and never dropped. Stream is a spectator's picture and sound:
//! worth sending only while fresh, so it waits in [`Lanes`] and is dropped
//! once older than [`Rules::stale_after`].
//!
//! Stream is sent only while the link keeps up, which is two checks:
//!
//! * **The socket takes it.** The socket is non-blocking with a small send
//!   buffer, and a stream message is written only once everything before it
//!   has left; until then it waits where it can still be dropped.
//! * **The link is not queueing.** A phone network can hold seconds of data
//!   in buffers no socket sees. While a stream flows the loop pings every
//!   [`Rules::probe_every`], each ping carrying the time it left; a round
//!   trip longer than the shortest one seen is time spent in a queue, and
//!   while that exceeds [`Rules::max_queue`] -- or a ping has gone unanswered
//!   that long -- the stream waits ([`Gauge`]).
//!
//! When a picture is dropped, every later one is too until the next key
//! frame, since none of them decodes without it.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use protocol::{Beat, Heartbeat, Lane, Liveness, Message, ProtocolError, Wire};
use tungstenite::{Message as Frame, WebSocket};

/// The kernel's send buffer. Whatever it holds is beyond dropping, so it is
/// kept small: about a second at a phone's 0.2 Mbit/s, and room for a few
/// Mbit/s at a home link's round trip.
pub const SEND_BUFFER: usize = 16 * 1024;

/// How long an idle loop waits for something to send before it reads again:
/// the latency it adds to an incoming message.
const IDLE: Duration = Duration::from_millis(2);

/// How often the stream's numbers are logged while it flows.
const REPORT_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rules {
    pub liveness: Liveness,
    /// A picture or sound older than this is not sent.
    pub stale_after: Duration,
    /// Queueing the link may add before the stream waits.
    pub max_queue: Duration,
    /// How often to time the link while a stream flows.
    pub probe_every: Duration,
}

impl Rules {
    pub const DEFAULT: Rules = Rules {
        liveness: Liveness::DEFAULT,
        stale_after: Duration::from_millis(400),
        max_queue: Duration::from_millis(250),
        probe_every: Duration::from_millis(100),
    };
}

/// How far the link is queueing, from pings stamped with the millisecond
/// they left (since the loop began) and the pongs that echo them back.
#[derive(Clone, Debug)]
pub struct Gauge {
    max_queue: u64,
    probe_every: u64,
    /// The shortest round trip seen: the link with nothing queued.
    least: Option<u64>,
    last: u64,
    /// The oldest ping not yet answered.
    unanswered: Option<u64>,
    probed: Option<u64>,
}

impl Gauge {
    pub fn new(rules: &Rules) -> Gauge {
        Gauge {
            max_queue: rules.max_queue.as_millis() as u64,
            probe_every: rules.probe_every.as_millis() as u64,
            least: None,
            last: 0,
            unanswered: None,
            probed: None,
        }
    }

    pub fn sent(&mut self, stamp: u64) {
        self.unanswered.get_or_insert(stamp);
        self.probed = Some(stamp);
    }

    /// A pong echoing `stamp` arrived at `now`.
    pub fn answered(&mut self, stamp: u64, now: u64) {
        let rtt = now.saturating_sub(stamp);
        self.last = rtt;
        self.least = Some(self.least.map_or(rtt, |l| l.min(rtt)));
        if self.unanswered.is_some_and(|u| u <= stamp) {
            self.unanswered = None;
        }
    }

    fn queued(&self, rtt: u64) -> u64 {
        rtt.saturating_sub(self.least.unwrap_or(0))
    }

    /// Whether a stream may go at `now`.
    pub fn clear(&self, now: u64) -> bool {
        let waiting = self.unanswered.map_or(0, |u| now.saturating_sub(u));
        self.queued(self.last) <= self.max_queue && self.queued(waiting) <= self.max_queue
    }

    /// Whether to time the link again: one probe out at a time.
    pub fn probe_due(&self, now: u64) -> bool {
        self.unanswered.is_none() && self.probed.map_or(true, |p| now.saturating_sub(p) >= self.probe_every)
    }

    pub fn round_trip(&self) -> Duration {
        Duration::from_millis(self.last)
    }
}

/// Readies a connected stream for [`run`]: non-blocking, no delay, and the
/// small send buffer. Before it, the stream may be used blocking — for the
/// handshake, say.
pub fn prepare(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    socket2::SockRef::from(stream).set_send_buffer_size(SEND_BUFFER)?;
    stream.set_nonblocking(true)
}

/// What the stream came to over a stretch: for the log, and for tests.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    pub stream_bytes: u64,
    pub pictures: u32,
    pub pictures_dropped: u32,
    pub sounds: u32,
    pub sounds_dropped: u32,
}

/// The messages waiting to go, in their two lanes.
pub struct Lanes<M> {
    stale_after: Duration,
    control: VecDeque<M>,
    stream: VecDeque<(Instant, M)>,
    /// A picture was dropped; later ones wait for a key frame.
    broken: bool,
    key_wanted: bool,
    pub stats: Stats,
}

impl<M: Message> Lanes<M> {
    pub fn new(stale_after: Duration) -> Lanes<M> {
        Lanes {
            stale_after,
            control: VecDeque::new(),
            stream: VecDeque::new(),
            broken: false,
            key_wanted: false,
            stats: Stats::default(),
        }
    }

    pub fn push(&mut self, m: M, now: Instant) {
        match m.lane() {
            Lane::Control => self.control.push_back(m),
            _ => {
                self.stream.push_back((now, m));
                self.prune(now);
            }
        }
    }

    pub fn control(&mut self) -> Option<M> {
        self.control.pop_front()
    }

    /// The oldest stream message still worth sending at `now`, if any.
    pub fn stream(&mut self, now: Instant) -> Option<M> {
        self.prune(now);
        while let Some((_, m)) = self.stream.pop_front() {
            match m.lane() {
                Lane::Picture { key } => {
                    if self.broken && !key {
                        self.stats.pictures_dropped += 1;
                        continue;
                    }
                    self.broken = false;
                    self.stats.pictures += 1;
                }
                Lane::Sound => self.stats.sounds += 1,
                Lane::Control => {}
            }
            return Some(m);
        }
        None
    }

    /// Lets go of what has gone stale, as though sending were not possible
    /// until `now`.
    pub fn prune(&mut self, now: Instant) {
        self.prune_older(now, self.stale_after);
    }

    /// Lets go of everything older than `keep`: while the link is queueing,
    /// so what goes when it clears is the newest and not a burst of the
    /// last [`Rules::stale_after`].
    pub fn prune_older(&mut self, now: Instant, keep: Duration) {
        while let Some((at, m)) = self.stream.front() {
            if now.duration_since(*at) <= keep {
                break;
            }
            match m.lane() {
                Lane::Picture { .. } => self.drop_picture(),
                _ => self.stats.sounds_dropped += 1,
            }
            self.stream.pop_front();
        }
    }

    fn drop_picture(&mut self) {
        self.stats.pictures_dropped += 1;
        if !self.broken {
            self.broken = true;
            self.key_wanted = true;
        }
    }

    /// True once after pictures were dropped: the sender should start a
    /// key frame, or the watcher waits for the next one it makes anyway.
    pub fn take_key_wanted(&mut self) -> bool {
        std::mem::take(&mut self.key_wanted)
    }
}

/// Whether the loop goes on after a message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Next {
    Go,
    Stop,
}

/// Why [`run`] returned.
#[derive(Debug)]
pub enum End {
    /// The peer closed the socket.
    Closed,
    /// The owner let go of the sending side, or a message said stop.
    Done,
    /// Nothing heard for this long.
    Silent(Duration),
    /// The peer sent what this side cannot read.
    Malformed(ProtocolError),
    Failed(String),
}

impl std::fmt::Display for End {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            End::Closed => write!(f, "the other end closed the connection"),
            End::Done => write!(f, "done"),
            End::Silent(d) => write!(f, "nothing heard in {}s", d.as_secs()),
            End::Malformed(e) => write!(f, "{e}"),
            End::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// The hooks a loop calls out on.
pub struct Ends<'a, In> {
    /// Each message read. Return [`Next::Stop`] to end the loop.
    pub incoming: &'a mut dyn FnMut(In) -> Next,
    /// Pictures were dropped on this socket.
    pub key_wanted: &'a mut dyn FnMut(),
    /// Names the socket in the stream's log line.
    pub label: &'a str,
}

/// Runs one socket until it ends, reading `In` and sending `Out` from
/// `outgoing`. The stream must have been [`prepare`]d.
pub fn run<S: Read + Write, In: Message, Out: Message>(
    ws: &mut WebSocket<S>,
    rules: Rules,
    outgoing: &Receiver<Out>,
    ends: Ends<'_, In>,
) -> End {
    let start = Instant::now();
    let mut heart = Heartbeat::new(rules.liveness, start);
    let mut lanes: Lanes<Out> = Lanes::new(rules.stale_after);
    // Everything written has left this process.
    let mut flushed = true;
    let mut gauge = Gauge::new(&rules);
    // When a stream message last came to be sent: the link is timed while
    // one flows.
    let mut streaming: Option<Instant> = None;
    let stamp_of = |at: Instant| at.duration_since(start).as_millis() as u64;
    let mut reported = (start, Stats::default());

    loop {
        let mut busy = false;
        let now = Instant::now();

        // Everything waiting to be read, so a ping is never stuck behind
        // what this side has to say.
        loop {
            let frame = match ws.read() {
                Ok(f) => f,
                Err(tungstenite::Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return End::Closed,
                Err(e) => return End::Failed(e.to_string()),
            };
            heart.heard(Instant::now());
            busy = true;
            let m = match frame {
                Frame::Text(t) => protocol::decode::<In>(t.as_str()),
                Frame::Binary(b) => protocol::decode_binary::<In>(&b),
                Frame::Pong(p) => {
                    // The answer echoes the ping's stamp: the round trip is
                    // the time since it.
                    if let Ok(b) = <[u8; 8]>::try_from(&p[..]) {
                        gauge.answered(u64::from_le_bytes(b), stamp_of(Instant::now()));
                    }
                    continue;
                }
                Frame::Close(_) => return End::Closed,
                _ => continue,
            };
            match m {
                Ok(m) => {
                    if (ends.incoming)(m) == Next::Stop {
                        return End::Done;
                    }
                }
                Err(e) => return End::Malformed(e),
            }
        }

        loop {
            match outgoing.try_recv() {
                Ok(m) => {
                    if m.lane() != Lane::Control {
                        streaming = Some(now);
                    }
                    lanes.push(m, now);
                    busy = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return End::Done,
            }
        }

        while let Some(m) = lanes.control() {
            if let Err(e) = write(ws, m) {
                return End::Failed(e);
            }
            flushed = false;
            busy = true;
        }
        let clear = gauge.clear(stamp_of(now));
        if flushed && clear {
            if let Some(m) = lanes.stream(now) {
                let wire = protocol::to_wire(m);
                lanes.stats.stream_bytes += match &wire {
                    Wire::Text(t) => t.len(),
                    Wire::Binary(b) => b.len(),
                } as u64;
                if let Err(e) = write_wire(ws, wire) {
                    return End::Failed(e);
                }
                flushed = false;
                busy = true;
            }
        } else if clear {
            lanes.prune(now);
        } else {
            lanes.prune_older(now, rules.probe_every);
        }
        if lanes.take_key_wanted() {
            (ends.key_wanted)();
        }

        let stream_flowing = streaming.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1));
        let ping = match heart.check(now) {
            Beat::Silent => return End::Silent(heart.silence()),
            Beat::Ping => true,
            Beat::Wait => stream_flowing && gauge.probe_due(stamp_of(now)),
        };
        if ping {
            let stamp = stamp_of(now);
            if let Err(e) = write_frame(ws, Frame::Ping(stamp.to_le_bytes().to_vec().into())) {
                return End::Failed(e);
            }
            gauge.sent(stamp);
            flushed = false;
        }

        if !flushed {
            match ws.flush() {
                Ok(()) => flushed = true,
                Err(tungstenite::Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return End::Failed(e.to_string()),
            }
        }

        if now.duration_since(reported.0) >= REPORT_EVERY {
            let s = lanes.stats;
            let was = reported.1;
            if s != was {
                let secs = now.duration_since(reported.0).as_secs_f64();
                println!(
                    "[{}] stream {:.0} kbit/s, pictures {} sent {} dropped, sound {} sent {} dropped, round trip {} ms",
                    ends.label,
                    (s.stream_bytes - was.stream_bytes) as f64 * 8.0 / secs / 1000.0,
                    s.pictures - was.pictures,
                    s.pictures_dropped - was.pictures_dropped,
                    s.sounds - was.sounds,
                    s.sounds_dropped - was.sounds_dropped,
                    gauge.round_trip().as_millis(),
                );
            }
            reported = (now, s);
        }

        if !busy {
            // Nothing read or queued: wait briefly for something to send.
            match outgoing.recv_timeout(IDLE) {
                Ok(m) => lanes.push(m, Instant::now()),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return End::Done,
            }
        }
    }
}

fn write<S: Read + Write, M: Message>(ws: &mut WebSocket<S>, m: M) -> Result<(), String> {
    write_wire(ws, protocol::to_wire(m))
}

fn write_wire<S: Read + Write>(ws: &mut WebSocket<S>, wire: Wire) -> Result<(), String> {
    write_frame(
        ws,
        match wire {
            Wire::Text(t) => Frame::Text(t.into()),
            Wire::Binary(b) => Frame::Binary(b.into()),
        },
    )
}

/// Queues a frame. On a non-blocking socket a write that would block has
/// still queued it, and the next flush sends it.
fn write_frame<S: Read + Write>(ws: &mut WebSocket<S>, f: Frame) -> Result<(), String> {
    match ws.write(f) {
        Ok(()) => Ok(()),
        Err(tungstenite::Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Sends last messages and closes, waiting at most `wait` for them to
/// leave: for a refusal the peer should read before the socket goes.
pub fn close_with<S: Read + Write, M: Message>(ws: &mut WebSocket<S>, last: impl IntoIterator<Item = M>, wait: Duration) {
    let deadline = Instant::now() + wait;
    for m in last {
        let _ = write(ws, m);
    }
    let _ = ws.close(None);
    while Instant::now() < deadline {
        match ws.flush() {
            Err(tungstenite::Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(IDLE);
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ToClient, ToServer};

    fn picture(frame: u64, key: bool) -> ToServer {
        ToServer::Video { round: 1, frame, key, width: 2, height: 2, aspect: 1.0, data: vec![0].into() }
    }

    fn sound(frame: u64) -> ToServer {
        ToServer::Audio { round: 1, frame, rate: 1, channels: 1, data: vec![0].into() }
    }

    fn frame_of(m: &ToServer) -> u64 {
        match m {
            ToServer::Video { frame, .. } | ToServer::Audio { frame, .. } => *frame,
            _ => u64::MAX,
        }
    }

    #[test]
    fn control_goes_first_and_is_never_dropped() {
        let t0 = Instant::now();
        let mut lanes = Lanes::new(Duration::from_millis(100));
        lanes.push(picture(1, true), t0);
        lanes.push(ToServer::Start, t0);
        assert_eq!(lanes.control(), Some(ToServer::Start));
        let late = t0 + Duration::from_secs(60);
        lanes.push(ToServer::Forfeit { round: 1, frame: 2 }, t0);
        lanes.prune(late);
        assert_eq!(lanes.control(), Some(ToServer::Forfeit { round: 1, frame: 2 }), "however long it waited");
    }

    #[test]
    fn a_stale_stream_is_dropped_and_pictures_wait_for_a_key() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut lanes = Lanes::new(Duration::from_millis(100));
        lanes.push(picture(1, true), ms(0));
        lanes.push(sound(1), ms(0));
        lanes.push(picture(2, false), ms(150));
        lanes.push(picture(3, false), ms(160));
        lanes.push(sound(3), ms(160));
        lanes.push(picture(4, true), ms(170));
        lanes.push(picture(5, false), ms(180));
        // At 200 ms the first two are stale; the deltas after the dropped key
        // cannot decode, so the next thing out is the sound, then the key.
        let out: Vec<u64> = std::iter::from_fn(|| lanes.stream(ms(200))).map(|m| frame_of(&m)).collect();
        assert_eq!(out, [3, 4, 5]);
        assert!(lanes.take_key_wanted(), "a dropped picture asks for a key");
        assert!(!lanes.take_key_wanted(), "once");
        assert_eq!(lanes.stats.pictures_dropped, 3);
        assert_eq!(lanes.stats.sounds_dropped, 1);
        assert_eq!((lanes.stats.pictures, lanes.stats.sounds), (2, 1));
    }

    #[test]
    fn a_gauge_waits_out_a_queue_and_an_unanswered_probe() {
        let mut g = Gauge::new(&Rules::DEFAULT);
        assert!(g.clear(0) && g.probe_due(0), "nothing known: go, and find out");
        g.sent(0);
        assert!(!g.probe_due(50), "one probe out at a time");
        g.answered(0, 80);
        assert!(g.clear(80), "80 ms is this link's floor");
        assert!(!g.probe_due(90) && g.probe_due(100));
        g.sent(100);
        assert!(g.clear(400), "unanswered for 300 ms is 220 queued: within 250");
        assert!(!g.clear(500), "unanswered for 400 ms is queueing");
        g.answered(100, 520);
        assert!(!g.clear(520), "and the answer says so: 420 against a floor of 80");
        g.sent(600);
        g.answered(600, 700);
        assert!(g.clear(700), "the queue drained");
        assert_eq!(g.round_trip(), Duration::from_millis(100));
    }

    #[test]
    fn a_fresh_stream_flows_untouched() {
        let t0 = Instant::now();
        let mut lanes: Lanes<ToClient> = Lanes::new(Duration::from_millis(100));
        for f in 0..5 {
            lanes.push(
                ToClient::Video {
                    round: 1, player: "A".into(), frame: f, key: f == 0, width: 2, height: 2, aspect: 1.0,
                    data: vec![0].into(),
                },
                t0,
            );
        }
        assert_eq!(std::iter::from_fn(|| lanes.stream(t0)).count(), 5);
        assert!(!lanes.take_key_wanted());
    }
}
