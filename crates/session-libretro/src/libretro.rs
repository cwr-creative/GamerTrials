//! Raw libretro FFI plus a `Core` that loads a core DLL directly (ADR 0001).
//!
//! This is the merge of the two harnesses' copies. They turned out to differ
//! by four joypad button ids, three environment calls SNES cores make, and a
//! richer record of what `SET_MEMORY_MAPS` published — all of it additive, so
//! the union is what both consoles now run. Nothing in here is per-game or
//! per-core: what differs between games is which regions they read, and that
//! lives in `memory.rs` and the manifest.
//!
//! libretro callbacks are bare `extern "C"` function pointers with no user data,
//! so frontend state has to be global. It lives in the statics below. Everything
//! except the audio drain runs on the emulator thread.

use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CString};
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use libloading::{Library, Symbol};

// ---------------------------------------------------------------- constants

pub const RETRO_DEVICE_JOYPAD: u32 = 1;

pub const RETRO_MEMORY_SAVE_RAM: u32 = 0;
pub const RETRO_MEMORY_RTC: u32 = 1;
pub const RETRO_MEMORY_SYSTEM_RAM: u32 = 2;
pub const RETRO_MEMORY_VIDEO_RAM: u32 = 3;

/// Every id `retro_get_memory_data` takes, with a printable name. `VIDEO_RAM`
/// is the interesting one for this harness: a core that answers there is
/// offering VRAM/OAM/CGRAM, which is where the sprite table lives.
pub const RETRO_MEMORY_IDS: [(u32, &str); 4] = [
    (RETRO_MEMORY_SAVE_RAM, "SAVE_RAM"),
    (RETRO_MEMORY_RTC, "RTC"),
    (RETRO_MEMORY_SYSTEM_RAM, "SYSTEM_RAM"),
    (RETRO_MEMORY_VIDEO_RAM, "VIDEO_RAM"),
];

// Descriptor flags, from libretro.h. The ALIGN/MINSIZE fields are two-bit
// values shifted up, not single bits, which is why they are masked below
// rather than tested.
const MEMDESC_CONST: u64 = 1 << 0;
const MEMDESC_BIGENDIAN: u64 = 1 << 1;
const MEMDESC_SYSTEM_RAM: u64 = 1 << 2;
const MEMDESC_SAVE_RAM: u64 = 1 << 3;
const MEMDESC_VIDEO_RAM: u64 = 1 << 4;
const MEMDESC_ALIGN_SHIFT: u32 = 16;
const MEMDESC_MINSIZE_SHIFT: u32 = 24;

// Joypad button ids, in libretro order. The libretro layout *is* the SNES pad,
// so unlike the NES harness every id here is used.
pub const JOY_B: u32 = 0;
pub const JOY_Y: u32 = 1;
pub const JOY_SELECT: u32 = 2;
pub const JOY_START: u32 = 3;
pub const JOY_UP: u32 = 4;
pub const JOY_DOWN: u32 = 5;
pub const JOY_LEFT: u32 = 6;
pub const JOY_RIGHT: u32 = 7;
pub const JOY_A: u32 = 8;
pub const JOY_X: u32 = 9;
pub const JOY_L: u32 = 10;
pub const JOY_R: u32 = 11;
/// Cores that negotiated GET_INPUT_BITMASKS ask for every button at once.
const JOY_MASK: u32 = 256;

const PIXEL_FORMAT_0RGB1555: u32 = 0;
const PIXEL_FORMAT_XRGB8888: u32 = 1;
const PIXEL_FORMAT_RGB565: u32 = 2;

// Environment commands we answer. Anything not listed gets `false`, which is
// the libretro way of saying "frontend does not support this" and is safe.
const ENV_GET_CAN_DUPE: u32 = 3;
const ENV_SET_PERFORMANCE_LEVEL: u32 = 8;
const ENV_GET_SYSTEM_DIRECTORY: u32 = 9;
const ENV_SET_PIXEL_FORMAT: u32 = 10;
const ENV_SET_INPUT_DESCRIPTORS: u32 = 11;
const ENV_GET_VARIABLE: u32 = 15;
const ENV_SET_VARIABLES: u32 = 16;
const ENV_GET_VARIABLE_UPDATE: u32 = 17;
const ENV_SET_SUPPORT_NO_GAME: u32 = 18;
const ENV_GET_SAVE_DIRECTORY: u32 = 31;
const ENV_SET_SUBSYSTEM_INFO: u32 = 34;
const ENV_SET_CONTROLLER_INFO: u32 = 35;
/// `36 | RETRO_ENVIRONMENT_EXPERIMENTAL(0x10000)`. Not load-bearing here the
/// way it is on the NES side: snes9x exposes all 128 KB of WRAM through
/// `RETRO_MEMORY_SYSTEM_RAM`, and every address this harness watches lives in
/// it. The map is still recorded, because it is the only route to anything
/// outside WRAM (cartridge SRAM, ROM) if a detector ever needs one.
const ENV_SET_MEMORY_MAPS: u32 = 36 | 0x10000;
/// snes9x raises this when the picture switches to hi-res or interlace.
/// `cb_video_refresh` already reads the real dimensions off every frame, so
/// answering it is a formality — but refusing makes the core log about it.
const ENV_SET_GEOMETRY: u32 = 37;
/// Accepted and ignored, which is a small lie: if a core ever changed its
/// framerate or sample rate mid-run the pacing in `main` would keep using the
/// values from startup. No SNES core does, and refusing outright is worse.
const ENV_SET_SYSTEM_AV_INFO: u32 = 32;
const ENV_SET_SUPPORT_ACHIEVEMENTS: u32 = 42;
const ENV_SET_CORE_OPTIONS_V2: u32 = 67;
const ENV_SET_CORE_OPTIONS_V2_INTL: u32 = 68;

// -------------------------------------------------------------- FFI structs

#[repr(C)]
pub struct GameInfo {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

#[repr(C)]
pub struct SystemInfo {
    pub library_name: *const c_char,
    pub library_version: *const c_char,
    pub valid_extensions: *const c_char,
    pub need_fullpath: bool,
    pub block_extract: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GameGeometry {
    pub base_width: u32,
    pub base_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub aspect_ratio: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SystemTiming {
    pub fps: f64,
    pub sample_rate: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SystemAvInfo {
    pub geometry: GameGeometry,
    pub timing: SystemTiming,
}

#[repr(C)]
struct Variable {
    key: *const c_char,
    value: *const c_char,
}

/// One region of the emulated CPU address space, as described by the core via
/// `SET_MEMORY_MAPS`. Field order and types are fixed by libretro.h.
#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryDescriptor {
    flags: u64,
    ptr: *mut c_void,
    offset: usize,
    start: usize,
    select: usize,
    disconnect: usize,
    len: usize,
    addrspace: *const c_char,
}

#[repr(C)]
struct MemoryMap {
    descriptors: *const MemoryDescriptor,
    num_descriptors: u32,
}

type EnvironmentFn = unsafe extern "C" fn(u32, *mut c_void) -> bool;
type VideoRefreshFn = unsafe extern "C" fn(*const c_void, u32, u32, usize);
type AudioSampleFn = unsafe extern "C" fn(i16, i16);
type AudioSampleBatchFn = unsafe extern "C" fn(*const i16, usize) -> usize;
type InputPollFn = unsafe extern "C" fn();
type InputStateFn = unsafe extern "C" fn(u32, u32, u32, u32) -> i16;

// ------------------------------------------------------------- global state

/// Latest completed frame. `seq` increments so the main loop can tell a fresh
/// frame from a duped one.
pub struct VideoFrame {
    pub buf: Vec<u32>,
    pub width: usize,
    pub height: usize,
    pub seq: u64,
}

pub static VIDEO: Mutex<VideoFrame> = Mutex::new(VideoFrame {
    buf: Vec::new(),
    width: 0,
    height: 0,
    seq: 0,
});

/// Stereo f32 already resampled to the output device rate. Produced by the
/// emulator thread, consumed by the cpal callback.
pub struct AudioRing {
    pub samples: VecDeque<f32>,
    /// Fractional read position into the core stream, for the resampler.
    pub frac: f64,
    pub last: (f32, f32),
}

pub static AUDIO: Mutex<AudioRing> = Mutex::new(AudioRing {
    samples: VecDeque::new(),
    frac: 0.0,
    last: (0.0, 0.0),
});

/// Core sample rate divided by device sample rate, stored as f64 bits.
pub static RESAMPLE_STEP: AtomicU64 = AtomicU64::new(0);
/// Zero when there is no audio device; the batch callback then drops audio.
pub static AUDIO_ENABLED: AtomicU32 = AtomicU32::new(0);

/// Current joypad state as a bitmask indexed by the `JOY_*` ids.
pub static JOYPAD: AtomicU32 = AtomicU32::new(0);

static PIXEL_FORMAT: AtomicU32 = AtomicU32::new(PIXEL_FORMAT_0RGB1555);
static SYSTEM_DIR: OnceLock<CString> = OnceLock::new();

/// Regions the core published via `SET_MEMORY_MAPS`, copied out of the
/// core-owned descriptor array at the moment it is offered.
///
/// Whether this is the main route to a byte or the fallback is per console and
/// is stated in the manifest — on the NES it is the only way to reach cartridge
/// WRAM, and on the SNES it is a fallback for the little that lies outside work
/// RAM. `memory::AddressSpace` is where that choice is applied.
#[derive(Clone)]
pub struct MappedRegion {
    pub start: usize,
    pub select: usize,
    pub disconnect: usize,
    pub len: usize,
    pub offset: usize,
    /// Base pointer of the core-owned buffer. Held as a `usize` so the
    /// static stays `Send`; only dereferenced on the emulator thread. Zero
    /// when the core published a descriptor with no backing buffer — such a
    /// region is unreadable, but it is still recorded so a dump can say so.
    pub ptr: usize,
    pub flags: u64,
    /// The core's name for the address space this region belongs to. Usually
    /// absent; when present it is the fastest way to tell VRAM from APU RAM,
    /// which are both 64 KB on the SNES.
    pub addrspace: Option<String>,
}

impl MappedRegion {
    /// A region this harness can actually read a byte out of.
    pub fn readable(&self) -> bool {
        self.ptr != 0 && self.len != 0 && self.select == 0 && self.disconnect == 0
    }

    /// Flag names as libretro.h spells them, for a dump. Empty string when the
    /// core set none, which is common and not an error.
    pub fn flag_names(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        for (bit, name) in [
            (MEMDESC_CONST, "CONST"),
            (MEMDESC_BIGENDIAN, "BIGENDIAN"),
            (MEMDESC_SYSTEM_RAM, "SYSTEM_RAM"),
            (MEMDESC_SAVE_RAM, "SAVE_RAM"),
            (MEMDESC_VIDEO_RAM, "VIDEO_RAM"),
        ] {
            if self.flags & bit != 0 {
                out.push(name.to_string());
            }
        }
        let align = (self.flags >> MEMDESC_ALIGN_SHIFT) & 0x3;
        if align != 0 {
            out.push(format!("ALIGN_{}", 1 << align));
        }
        let minsize = (self.flags >> MEMDESC_MINSIZE_SHIFT) & 0x3;
        if minsize != 0 {
            out.push(format!("MINSIZE_{}", 1 << minsize));
        }
        out.join("|")
    }
}

pub static MEMORY_MAP: Mutex<Vec<MappedRegion>> = Mutex::new(Vec::new());

/// True once the core has published a memory map.
pub fn has_memory_map() -> bool {
    !lock(&MEMORY_MAP).is_empty()
}

/// A copy of every descriptor the core published, in the order it published
/// them. For reporting; reading bytes goes through `read_addr`.
pub fn memory_map() -> Vec<MappedRegion> {
    lock(&MEMORY_MAP).clone()
}

/// Reads one byte of the emulated CPU address space through the map the core
/// published. `None` when no descriptor covers `addr`, including when the core
/// published no map at all.
///
/// Only descriptors with `select == 0` are resolved — a plain
/// `start..start+len` window. The `select` / `disconnect` bit-matching form in
/// libretro.h is deliberately not implemented, and on the SNES that matters
/// more than it did on the NES: bank mirroring is exactly what those fields
/// describe, so a mirrored address may not resolve here even though the core
/// published it. Every default address this harness watches is in WRAM and
/// never takes this path. Guessing at the bit-matching would silently return
/// the wrong byte rather than nothing, which is worse than not answering.
pub fn read_addr(addr: usize) -> Option<u8> {
    let map = lock(&MEMORY_MAP);
    for r in map.iter() {
        if !r.readable() {
            continue;
        }
        if addr >= r.start && addr < r.start + r.len {
            // Safe while the core is loaded: the pointer is core-owned and
            // stays live for the core's lifetime, the index is bounded by
            // `len` above, and this only ever runs on the emulator thread.
            return Some(unsafe { *(r.ptr as *const u8).add(r.offset + (addr - r.start)) });
        }
    }
    None
}

fn audio_enabled() -> bool {
    AUDIO_ENABLED.load(Ordering::Relaxed) != 0
}

/// Locking helper: a poisoned frontend mutex is not a reason to kill the run.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    }
}

// ----------------------------------------------------------------- callbacks

unsafe extern "C" fn cb_environment(cmd: u32, data: *mut c_void) -> bool {
    match cmd {
        ENV_GET_CAN_DUPE => {
            if data.is_null() {
                return false;
            }
            *(data as *mut bool) = true;
            true
        }
        ENV_SET_PIXEL_FORMAT => {
            if data.is_null() {
                return false;
            }
            let fmt = *(data as *const u32);
            match fmt {
                PIXEL_FORMAT_0RGB1555 | PIXEL_FORMAT_XRGB8888 | PIXEL_FORMAT_RGB565 => {
                    PIXEL_FORMAT.store(fmt, Ordering::Relaxed);
                    true
                }
                _ => false,
            }
        }
        ENV_GET_SYSTEM_DIRECTORY | ENV_GET_SAVE_DIRECTORY => {
            if data.is_null() {
                return false;
            }
            match SYSTEM_DIR.get() {
                Some(dir) => {
                    *(data as *mut *const c_char) = dir.as_ptr();
                    true
                }
                None => false,
            }
        }
        ENV_GET_VARIABLE => {
            // No core options are overridden; every core keeps its defaults.
            if !data.is_null() {
                (*(data as *mut Variable)).value = std::ptr::null();
            }
            false
        }
        ENV_GET_VARIABLE_UPDATE => {
            if data.is_null() {
                return false;
            }
            *(data as *mut bool) = false;
            true
        }
        ENV_SET_MEMORY_MAPS => {
            if data.is_null() {
                return false;
            }
            let map = unsafe { &*(data as *const MemoryMap) };
            if map.descriptors.is_null() {
                return false;
            }
            let descs =
                unsafe { std::slice::from_raw_parts(map.descriptors, map.num_descriptors as usize) };
            let mut out = lock(&MEMORY_MAP);
            out.clear();
            // Unusable descriptors (no buffer, zero length) are recorded
            // rather than dropped: `read_addr` skips them, and a dump that
            // silently omitted them would misreport what the core offered.
            for d in descs {
                out.push(MappedRegion {
                    start: d.start,
                    select: d.select,
                    disconnect: d.disconnect,
                    len: d.len,
                    offset: d.offset,
                    ptr: d.ptr as usize,
                    flags: d.flags,
                    addrspace: match unsafe { cstr(d.addrspace) } {
                        n if n.is_empty() => None,
                        n => Some(n),
                    },
                });
            }
            true
        }
        // Accepted and ignored. Refusing these makes some cores noisy.
        ENV_SET_PERFORMANCE_LEVEL
        | ENV_SET_INPUT_DESCRIPTORS
        | ENV_SET_VARIABLES
        | ENV_SET_SUPPORT_NO_GAME
        | ENV_SET_SUBSYSTEM_INFO
        | ENV_SET_CONTROLLER_INFO
        | ENV_SET_GEOMETRY
        | ENV_SET_SYSTEM_AV_INFO
        | ENV_SET_SUPPORT_ACHIEVEMENTS
        | ENV_SET_CORE_OPTIONS_V2
        | ENV_SET_CORE_OPTIONS_V2_INTL => true,
        // GET_LOG_INTERFACE (27) deliberately falls through to false: its
        // callback is C-variadic, and stable Rust cannot define one.
        _ => false,
    }
}

unsafe extern "C" fn cb_video_refresh(data: *const c_void, width: u32, height: u32, pitch: usize) {
    // A null frame means "repeat the last one" (we advertised CAN_DUPE).
    if data.is_null() || width == 0 || height == 0 {
        return;
    }
    let (w, h) = (width as usize, height as usize);
    let fmt = PIXEL_FORMAT.load(Ordering::Relaxed);

    let mut video = lock(&VIDEO);
    if video.buf.len() != w * h {
        video.buf.resize(w * h, 0);
    }
    video.width = w;
    video.height = h;

    for y in 0..h {
        let row = (data as *const u8).add(y * pitch);
        let out = &mut video.buf[y * w..y * w + w];
        match fmt {
            PIXEL_FORMAT_XRGB8888 => {
                let src = row as *const u32;
                for x in 0..w {
                    out[x] = src.add(x).read_unaligned() & 0x00FF_FFFF;
                }
            }
            PIXEL_FORMAT_RGB565 => {
                let src = row as *const u16;
                for x in 0..w {
                    let p = src.add(x).read_unaligned() as u32;
                    let r = (p >> 11) & 0x1F;
                    let g = (p >> 5) & 0x3F;
                    let b = p & 0x1F;
                    out[x] = ((r << 3 | r >> 2) << 16) | ((g << 2 | g >> 4) << 8) | (b << 3 | b >> 2);
                }
            }
            _ => {
                // 0RGB1555
                let src = row as *const u16;
                for x in 0..w {
                    let p = src.add(x).read_unaligned() as u32;
                    let r = (p >> 10) & 0x1F;
                    let g = (p >> 5) & 0x1F;
                    let b = p & 0x1F;
                    out[x] = ((r << 3 | r >> 2) << 16) | ((g << 3 | g >> 2) << 8) | (b << 3 | b >> 2);
                }
            }
        }
    }
    video.seq = video.seq.wrapping_add(1);
}

/// Push one core-rate stereo pair, resampling to the device rate on the way in.
/// Doing it here rather than in the cpal callback keeps that callback a dumb drain.
fn push_sample(ring: &mut AudioRing, left: f32, right: f32) {
    let step = f64::from_bits(RESAMPLE_STEP.load(Ordering::Relaxed));
    if !(step > 0.0) {
        return;
    }
    let (pl, pr) = ring.last;
    while ring.frac < 1.0 {
        let t = ring.frac as f32;
        ring.samples.push_back(pl + (left - pl) * t);
        ring.samples.push_back(pr + (right - pr) * t);
        ring.frac += step;
    }
    ring.frac -= 1.0;
    ring.last = (left, right);
}

unsafe extern "C" fn cb_audio_sample(left: i16, right: i16) {
    if !audio_enabled() {
        return;
    }
    let mut ring = lock(&AUDIO);
    push_sample(&mut ring, left as f32 / 32768.0, right as f32 / 32768.0);
}

unsafe extern "C" fn cb_audio_sample_batch(data: *const i16, frames: usize) -> usize {
    if !audio_enabled() || data.is_null() {
        return frames;
    }
    let mut ring = lock(&AUDIO);
    for i in 0..frames {
        let l = data.add(i * 2).read_unaligned() as f32 / 32768.0;
        let r = data.add(i * 2 + 1).read_unaligned() as f32 / 32768.0;
        push_sample(&mut ring, l, r);
    }
    frames
}

unsafe extern "C" fn cb_input_poll() {
    // The main loop polls window and gamepad before calling retro_run.
}

unsafe extern "C" fn cb_input_state(port: u32, device: u32, _index: u32, id: u32) -> i16 {
    if port != 0 || device != RETRO_DEVICE_JOYPAD {
        return 0;
    }
    let bits = JOYPAD.load(Ordering::Relaxed);
    if id == JOY_MASK {
        return bits as i16;
    }
    if id < 16 && bits & (1 << id) != 0 {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------- core

pub struct Core {
    fn_init: unsafe extern "C" fn(),
    fn_deinit: unsafe extern "C" fn(),
    fn_run: unsafe extern "C" fn(),
    fn_reset: unsafe extern "C" fn(),
    fn_load_game: unsafe extern "C" fn(*const GameInfo) -> bool,
    fn_unload_game: unsafe extern "C" fn(),
    fn_get_system_info: unsafe extern "C" fn(*mut SystemInfo),
    fn_get_system_av_info: unsafe extern "C" fn(*mut SystemAvInfo),
    fn_get_memory_data: unsafe extern "C" fn(u32) -> *mut c_void,
    fn_get_memory_size: unsafe extern "C" fn(u32) -> usize,
    fn_serialize_size: unsafe extern "C" fn() -> usize,
    fn_serialize: unsafe extern "C" fn(*mut c_void, usize) -> bool,
    fn_unserialize: unsafe extern "C" fn(*const c_void, usize) -> bool,
    fn_set_controller_port_device: unsafe extern "C" fn(u32, u32),
    // Held rather than called in `load`: only SET_ENVIRONMENT may run before
    // `retro_init`. Cores build the objects these callbacks feed inside
    // `retro_init`, so calling them earlier walks a null pointer.
    fn_set_video_refresh: unsafe extern "C" fn(VideoRefreshFn),
    fn_set_audio_sample: unsafe extern "C" fn(AudioSampleFn),
    fn_set_audio_sample_batch: unsafe extern "C" fn(AudioSampleBatchFn),
    fn_set_input_poll: unsafe extern "C" fn(InputPollFn),
    fn_set_input_state: unsafe extern "C" fn(InputStateFn),
    game_loaded: bool,
    initialised: bool,
    // Declared last so it is dropped after every symbol taken from it.
    _lib: Library,
}

macro_rules! sym {
    ($lib:expr, $name:literal, $t:ty) => {{
        let s: Symbol<$t> = unsafe { $lib.get(concat!($name, "\0").as_bytes()) }
            .map_err(|e| format!("core is missing {}: {e}", $name))?;
        *s
    }};
}

impl Core {
    /// Loads the core DLL and performs the libretro handshake up to `retro_init`.
    pub fn load(path: &Path) -> Result<Self, String> {
        // Several cores look for firmware in whatever we report as the system
        // directory. Point it at the folder the core itself lives in.
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let _ = SYSTEM_DIR
            .set(CString::new(dir.to_string_lossy().as_ref()).map_err(|e| e.to_string())?);

        let lib = unsafe { Library::new(path) }
            .map_err(|e| format!("cannot load core {}: {e}", path.display()))?;

        let api_version = sym!(lib, "retro_api_version", unsafe extern "C" fn() -> u32);
        let version = unsafe { api_version() };
        if version != 1 {
            return Err(format!("core reports libretro API {version}, expected 1"));
        }

        let set_environment = sym!(
            lib,
            "retro_set_environment",
            unsafe extern "C" fn(EnvironmentFn)
        );

        let core = Core {
            fn_init: sym!(lib, "retro_init", unsafe extern "C" fn()),
            fn_deinit: sym!(lib, "retro_deinit", unsafe extern "C" fn()),
            fn_run: sym!(lib, "retro_run", unsafe extern "C" fn()),
            fn_reset: sym!(lib, "retro_reset", unsafe extern "C" fn()),
            fn_load_game: sym!(
                lib,
                "retro_load_game",
                unsafe extern "C" fn(*const GameInfo) -> bool
            ),
            fn_unload_game: sym!(lib, "retro_unload_game", unsafe extern "C" fn()),
            fn_get_system_info: sym!(
                lib,
                "retro_get_system_info",
                unsafe extern "C" fn(*mut SystemInfo)
            ),
            fn_get_system_av_info: sym!(
                lib,
                "retro_get_system_av_info",
                unsafe extern "C" fn(*mut SystemAvInfo)
            ),
            fn_get_memory_data: sym!(
                lib,
                "retro_get_memory_data",
                unsafe extern "C" fn(u32) -> *mut c_void
            ),
            fn_get_memory_size: sym!(
                lib,
                "retro_get_memory_size",
                unsafe extern "C" fn(u32) -> usize
            ),
            fn_serialize_size: sym!(lib, "retro_serialize_size", unsafe extern "C" fn() -> usize),
            fn_serialize: sym!(
                lib,
                "retro_serialize",
                unsafe extern "C" fn(*mut c_void, usize) -> bool
            ),
            fn_unserialize: sym!(
                lib,
                "retro_unserialize",
                unsafe extern "C" fn(*const c_void, usize) -> bool
            ),
            fn_set_controller_port_device: sym!(
                lib,
                "retro_set_controller_port_device",
                unsafe extern "C" fn(u32, u32)
            ),
            fn_set_video_refresh: sym!(
                lib,
                "retro_set_video_refresh",
                unsafe extern "C" fn(VideoRefreshFn)
            ),
            fn_set_audio_sample: sym!(
                lib,
                "retro_set_audio_sample",
                unsafe extern "C" fn(AudioSampleFn)
            ),
            fn_set_audio_sample_batch: sym!(
                lib,
                "retro_set_audio_sample_batch",
                unsafe extern "C" fn(AudioSampleBatchFn)
            ),
            fn_set_input_poll: sym!(
                lib,
                "retro_set_input_poll",
                unsafe extern "C" fn(InputPollFn)
            ),
            fn_set_input_state: sym!(
                lib,
                "retro_set_input_state",
                unsafe extern "C" fn(InputStateFn)
            ),
            game_loaded: false,
            initialised: false,
            _lib: lib,
        };

        // SET_ENVIRONMENT must precede retro_init. The other five must not:
        // they are installed in `init`, after the core has built the objects
        // they feed.
        unsafe { set_environment(cb_environment) };
        Ok(core)
    }

    pub fn init(&mut self) {
        unsafe { (self.fn_init)() };
        self.initialised = true;
        // Now that retro_init has run, the core has somewhere to put these.
        unsafe {
            (self.fn_set_video_refresh)(cb_video_refresh);
            (self.fn_set_audio_sample)(cb_audio_sample);
            (self.fn_set_audio_sample_batch)(cb_audio_sample_batch);
            (self.fn_set_input_poll)(cb_input_poll);
            (self.fn_set_input_state)(cb_input_state);
        }
    }

    pub fn system_info(&self) -> (String, String) {
        let mut info = SystemInfo {
            library_name: std::ptr::null(),
            library_version: std::ptr::null(),
            valid_extensions: std::ptr::null(),
            need_fullpath: false,
            block_extract: false,
        };
        unsafe { (self.fn_get_system_info)(&mut info) };
        unsafe { (cstr(info.library_name), cstr(info.library_version)) }
    }

    /// The ROM is passed as both a path and a buffer, so cores that set
    /// `need_fullpath` and cores that want the bytes are both satisfied.
    pub fn load_game(&mut self, rom_path: &Path, rom: &[u8]) -> Result<(), String> {
        let path = CString::new(rom_path.to_string_lossy().as_ref()).map_err(|e| e.to_string())?;
        let info = GameInfo {
            path: path.as_ptr(),
            data: rom.as_ptr() as *const c_void,
            size: rom.len(),
            meta: std::ptr::null(),
        };
        if !unsafe { (self.fn_load_game)(&info) } {
            return Err("core refused the ROM (retro_load_game returned false)".into());
        }
        self.game_loaded = true;
        unsafe { (self.fn_set_controller_port_device)(0, RETRO_DEVICE_JOYPAD) };
        Ok(())
    }

    pub fn av_info(&self) -> SystemAvInfo {
        let mut info = SystemAvInfo {
            geometry: GameGeometry {
                base_width: 0,
                base_height: 0,
                max_width: 0,
                max_height: 0,
                aspect_ratio: 0.0,
            },
            timing: SystemTiming {
                fps: 60.0,
                sample_rate: 48000.0,
            },
        };
        unsafe { (self.fn_get_system_av_info)(&mut info) };
        info
    }

    pub fn run_frame(&mut self) {
        unsafe { (self.fn_run)() };
    }

    pub fn reset(&mut self) {
        unsafe { (self.fn_reset)() };
    }

    /// Borrow a memory region the core exposes. `None` if it has no such region.
    pub fn memory(&self, which: u32) -> Option<&[u8]> {
        let size = unsafe { (self.fn_get_memory_size)(which) };
        let ptr = unsafe { (self.fn_get_memory_data)(which) } as *const u8;
        if size == 0 || ptr.is_null() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(ptr, size) })
    }

    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        let size = unsafe { (self.fn_serialize_size)() };
        if size == 0 {
            return Err("core does not support savestates".into());
        }
        let mut buf = vec![0u8; size];
        if !unsafe { (self.fn_serialize)(buf.as_mut_ptr() as *mut c_void, size) } {
            return Err("retro_serialize failed".into());
        }
        Ok(buf)
    }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), String> {
        let size = unsafe { (self.fn_serialize_size)() };
        if data.len() != size {
            // Savestates are not portable across cores or core versions, and a
            // size mismatch is the only warning we get before the core eats
            // garbage. Refuse rather than hand it over.
            return Err(format!(
                "savestate is {} bytes but this core wants {size}: it was made by a different core or version",
                data.len()
            ));
        }
        if !unsafe { (self.fn_unserialize)(data.as_ptr() as *const c_void, data.len()) } {
            return Err("retro_unserialize failed".into());
        }
        Ok(())
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        unsafe {
            if self.game_loaded {
                (self.fn_unload_game)();
            }
            if self.initialised {
                (self.fn_deinit)();
            }
        }
    }
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
}
