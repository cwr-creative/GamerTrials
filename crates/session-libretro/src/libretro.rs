//! Raw libretro FFI plus a `Core` that loads a core DLL directly (ADR 0001).
//!
//! This is the merge of three frontends' copies. The two console harnesses
//! turned out to differ by four joypad button ids, three environment calls SNES
//! cores make, and a richer record of what `SET_MEMORY_MAPS` published. The DOS
//! spike (ADR 0018) then added the rest, and it is worth naming what it needed,
//! because none of it is per-game:
//!
//! * **Core options.** The console cores keep their defaults, so `GET_VARIABLE`
//!   used to answer null unconditionally. dosbox-pure's cycles setting decides
//!   whether the emulated CPU speed tracks the host's, which is the whole
//!   determinism question (invariant 4), so options have to be both readable
//!   and settable — and they are pinned by the manifest, not by a preference.
//! * **A keyboard.** There is no way to play a DOS game with a four-button
//!   diamond, and no way to type at a DOS prompt with one either.
//! * **`need_fullpath`.** Passing a path *and* a buffer is harmless for a
//!   cartridge; a core that mounts a filesystem out of the content wants
//!   exactly one, and picking for it invites it to choose differently between
//!   runs.
//! * **A great many `GET_*` answers.** A console core asks for a handful. A DOS
//!   core asks for two dozen, and a refused one is not an error — it is a core
//!   quietly doing something else, which is the failure mode that wastes a day.
//!   Hence [`unhandled_env`]: anything asked for and not answered is counted,
//!   so a new core's gaps can be read off rather than guessed at.
//!
//! Nothing in here is per-game or per-core: what differs between games is which
//! regions they read and which options they pin, and that lives in `memory.rs`
//! and the manifest.
//!
//! libretro callbacks are bare `extern "C"` function pointers with no user data,
//! so frontend state has to be global. It lives in the statics below. Everything
//! except the audio drain runs on the emulator thread.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::{c_char, c_uint, c_void, CString};
use std::path::Path;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use libloading::{Library, Symbol};

// ---------------------------------------------------------------- constants

pub const RETRO_DEVICE_JOYPAD: u32 = 1;
/// A relative mouse. Not a port device of its own here: a DOS core reads it
/// alongside the keyboard, the way a PC has both, so it is answered whatever
/// port 0 is set to and a core that never asks is never told.
pub const RETRO_DEVICE_MOUSE: u32 = 2;
/// Keyboard, as a controller device. A DOS game is driven through this rather
/// than through the pad (ADR 0018), and a core reads it out of `input_state`
/// exactly the way it reads a button.
pub const RETRO_DEVICE_KEYBOARD: u32 = 3;

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
/// A core may ask for a subclass of a device (`RETRO_DEVICE_SUBCLASS`); the
/// base device is the low byte.
const RETRO_DEVICE_MASK: u32 = 0xFF;

// Mouse ids, in libretro order. X and Y are motion since the last poll, not
// buttons; every other id is a button and is a bit in `MOUSE_BUTTONS`.
const MOUSE_X: u32 = 0;
const MOUSE_Y: u32 = 1;
pub const MOUSE_LEFT: u32 = 2;
pub const MOUSE_RIGHT: u32 = 3;
pub const MOUSE_WHEELUP: u32 = 4;
pub const MOUSE_WHEELDOWN: u32 = 5;
pub const MOUSE_MIDDLE: u32 = 6;

const PIXEL_FORMAT_0RGB1555: u32 = 0;
const PIXEL_FORMAT_XRGB8888: u32 = 1;
const PIXEL_FORMAT_RGB565: u32 = 2;

// Environment commands we answer. Anything not listed is counted by
// `unhandled_env` and gets `false`, which is the libretro way of saying
// "frontend does not support this" and is safe.
const EXPERIMENTAL: u32 = 0x10000;

const ENV_SET_ROTATION: u32 = 1;
const ENV_GET_OVERSCAN: u32 = 2;
const ENV_GET_CAN_DUPE: u32 = 3;
const ENV_SET_MESSAGE: u32 = 6;
const ENV_SHUTDOWN: u32 = 7;
const ENV_SET_PERFORMANCE_LEVEL: u32 = 8;
const ENV_GET_SYSTEM_DIRECTORY: u32 = 9;
const ENV_SET_PIXEL_FORMAT: u32 = 10;
const ENV_SET_INPUT_DESCRIPTORS: u32 = 11;
const ENV_SET_KEYBOARD_CALLBACK: u32 = 12;
const ENV_SET_DISK_CONTROL_INTERFACE: u32 = 13;
const ENV_SET_HW_RENDER: u32 = 14;
const ENV_GET_VARIABLE: u32 = 15;
const ENV_SET_VARIABLES: u32 = 16;
const ENV_GET_VARIABLE_UPDATE: u32 = 17;
const ENV_SET_SUPPORT_NO_GAME: u32 = 18;
const ENV_GET_LIBRETRO_PATH: u32 = 19;
const ENV_SET_FRAME_TIME_CALLBACK: u32 = 21;
const ENV_SET_AUDIO_CALLBACK: u32 = 22;
const ENV_GET_RUMBLE_INTERFACE: u32 = 23;
const ENV_GET_INPUT_DEVICE_CAPABILITIES: u32 = 24;
const ENV_GET_LOG_INTERFACE: u32 = 27;
const ENV_GET_PERF_INTERFACE: u32 = 28;
const ENV_GET_CORE_ASSETS_DIRECTORY: u32 = 30;
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
const ENV_GET_USERNAME: u32 = 38;
const ENV_GET_LANGUAGE: u32 = 39;
const ENV_SET_SUPPORT_ACHIEVEMENTS: u32 = 42;
const ENV_GET_VFS_INTERFACE: u32 = 45 | EXPERIMENTAL;
const ENV_GET_LED_INTERFACE: u32 = 46;
const ENV_GET_AUDIO_VIDEO_ENABLE: u32 = 47;
const ENV_GET_FASTFORWARDING: u32 = 49;
const ENV_GET_INPUT_BITMASKS: u32 = 51 | EXPERIMENTAL;
const ENV_GET_CORE_OPTIONS_VERSION: u32 = 52;
const ENV_SET_CORE_OPTIONS: u32 = 53;
const ENV_SET_CORE_OPTIONS_INTL: u32 = 54;
const ENV_SET_CORE_OPTIONS_DISPLAY: u32 = 55;
const ENV_GET_DISK_CONTROL_INTERFACE_VERSION: u32 = 57;
const ENV_SET_DISK_CONTROL_EXT_INTERFACE: u32 = 58;
const ENV_GET_MESSAGE_INTERFACE_VERSION: u32 = 59;
const ENV_SET_MESSAGE_EXT: u32 = 60;
const ENV_GET_INPUT_MAX_USERS: u32 = 61;
const ENV_SET_AUDIO_BUFFER_STATUS_CALLBACK: u32 = 62;
const ENV_SET_MINIMUM_AUDIO_LATENCY: u32 = 63;
const ENV_SET_FASTFORWARDING_OVERRIDE: u32 = 64;
const ENV_SET_CONTENT_INFO_OVERRIDE: u32 = 65;
const ENV_GET_GAME_INFO_EXT: u32 = 66;
const ENV_SET_CORE_OPTIONS_V2: u32 = 67;
const ENV_SET_CORE_OPTIONS_V2_INTL: u32 = 68;
const ENV_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK: u32 = 69;
const ENV_SET_VARIABLE: u32 = 70;
const ENV_GET_THROTTLE_STATE: u32 = 71 | EXPERIMENTAL;
const ENV_GET_SAVESTATE_CONTEXT: u32 = 72 | EXPERIMENTAL;

/// libretro.h caps a v2 option at this many values, and the array is inline in
/// the struct rather than a pointer, so the stride depends on it.
const NUM_CORE_OPTION_VALUES_MAX: usize = 128;

// -------------------------------------------------------------- FFI structs

#[repr(C)]
pub struct GameInfo {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

/// What a core says about itself, in owned form. `retro_get_system_info` may
/// be called before `retro_init`, and its strings are core-owned, so they are
/// copied out rather than borrowed.
#[derive(Clone, Debug)]
pub struct CoreInfo {
    pub name: String,
    pub version: String,
    pub valid_extensions: String,
    /// The core wants a path it can open itself, not a buffer. True for
    /// anything that mounts a filesystem out of the content.
    pub need_fullpath: bool,
    pub block_extract: bool,
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

#[repr(C)]
struct CoreOptionValue {
    value: *const c_char,
    label: *const c_char,
}

/// The `SET_CORE_OPTIONS_V2` (67) definition. The `values` array is inline
/// rather than a pointer, so the struct's stride depends on
/// `NUM_CORE_OPTION_VALUES_MAX` and getting it wrong walks the array at the
/// wrong pitch.
#[repr(C)]
struct CoreOptionV2Definition {
    key: *const c_char,
    desc: *const c_char,
    desc_categorized: *const c_char,
    info: *const c_char,
    info_categorized: *const c_char,
    category_key: *const c_char,
    values: [CoreOptionValue; NUM_CORE_OPTION_VALUES_MAX],
    default_value: *const c_char,
}

#[repr(C)]
struct CoreOptionsV2 {
    categories: *mut c_void,
    definitions: *const CoreOptionV2Definition,
}

#[repr(C)]
struct CoreOptionsV2Intl {
    us: *const CoreOptionsV2,
    local: *const CoreOptionsV2,
}

/// The v1 (`SET_CORE_OPTIONS`, 53) definition: the same shape minus the
/// categorised fields.
#[repr(C)]
struct CoreOptionDefinition {
    key: *const c_char,
    desc: *const c_char,
    info: *const c_char,
    values: [CoreOptionValue; NUM_CORE_OPTION_VALUES_MAX],
    default_value: *const c_char,
}

#[repr(C)]
struct CoreOptionsIntl {
    us: *const CoreOptionDefinition,
    local: *const CoreOptionDefinition,
}

/// What a core hands over when it wants to be told about key presses. Only the
/// pointer is used; the callback is invoked on every edge so a core that reads
/// its keyboard through events rather than through `input_state` still works.
#[repr(C)]
struct KeyboardCallback {
    callback: Option<unsafe extern "C" fn(bool, c_uint, u32, u16)>,
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

/// This frame's mouse motion, in host pixels, and its held buttons as a bitmask
/// indexed by the `MOUSE_*` ids. Latched once per frame by [`set_mouse`], so a
/// core that asks for X twice in one frame gets the same answer twice rather
/// than the motion counted twice.
static MOUSE_DX: AtomicI32 = AtomicI32::new(0);
static MOUSE_DY: AtomicI32 = AtomicI32::new(0);
static MOUSE_BUTTONS: AtomicU32 = AtomicU32::new(0);

/// Keycodes currently held, as `RETROK_*` values. A keyboard-driven core reads
/// these out of `input_state`; see [`key_down`].
static KEYS_DOWN: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

/// What the core published about its own options: key, legal values, default.
/// Printed by `harness --dump-options`, and the reason a manifest's
/// `[runner.libretro.options]` can be checked rather than trusted.
static OPTION_DEFS: Mutex<Vec<OptionDef>> = Mutex::new(Vec::new());

/// What we are telling the core its options are set to. Anything not in here
/// gets the core's own default.
static OPTION_VALUES: Mutex<BTreeMap<String, CString>> = Mutex::new(BTreeMap::new());

/// Environment commands the core asked for that we answered `false` to.
/// Counted rather than logged per call, because cores ask every frame — and
/// kept because "the core silently did something else" is the failure mode
/// that costs a day (ADR 0018's spike found three this way).
static UNHANDLED_ENV: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());

static PIXEL_FORMAT: AtomicU32 = AtomicU32::new(PIXEL_FORMAT_0RGB1555);
static SYSTEM_DIR: OnceLock<CString> = OnceLock::new();
static SAVE_DIR: OnceLock<CString> = OnceLock::new();
static CORE_PATH: OnceLock<CString> = OnceLock::new();
/// Set when an option changes, cleared when the core asks. Starts set so the
/// core reads the manifest's values on its first look.
static VARIABLES_DIRTY: AtomicU32 = AtomicU32::new(1);
/// The core's keyboard-event callback, if it registered one.
static KEYBOARD_CB: Mutex<Option<unsafe extern "C" fn(bool, c_uint, u32, u16)>> = Mutex::new(None);
/// Set when the core asks to shut down. `retro_run` after that is undefined,
/// and a DOS core reaches it the moment the guest program exits — quitting Doom
/// from its own menu is enough.
static SHUTDOWN: AtomicU32 = AtomicU32::new(0);

/// One core option, as the core itself described it.
#[derive(Clone, Debug)]
pub struct OptionDef {
    pub key: String,
    pub desc: String,
    /// Every legal value, in the order the core listed them. Empty when the
    /// core described the option without enumerating them.
    pub values: Vec<String>,
    pub default_value: String,
}

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

/// Pin one core option. Must be called before `retro_init`, because that is
/// when a core reads its options for the first time and sizes the machine it
/// builds from them.
///
/// Nothing is validated here: the core has not published its definitions yet.
/// [`check_options`] is what compares the two, once it has.
pub fn set_option(key: &str, value: &str) {
    if let Ok(v) = CString::new(value) {
        lock(&OPTION_VALUES).insert(key.to_string(), v);
        VARIABLES_DIRTY.store(1, Ordering::Relaxed);
    }
}

/// Every option the core published, for a dump.
pub fn option_defs() -> Vec<OptionDef> {
    lock(&OPTION_DEFS).clone()
}

/// Checks the options a manifest pinned against what the core says exist.
///
/// A misspelled key is not a no-op that quietly keeps a default — it is a
/// player running a differently configured machine from everyone else, which
/// for `dosbox_pure_cycles` means an unscoreable frame count (ADR 0018). So it
/// is an error, in the same spirit as the manifest loader's `deny_unknown_fields`.
///
/// Returns `Ok(())` when the core published no definitions at all: that is a
/// core we know nothing about rather than a manifest that is wrong.
pub fn check_options() -> Result<(), String> {
    let defs = lock(&OPTION_DEFS);
    if defs.is_empty() {
        return Ok(());
    }
    let mut problems = Vec::new();
    for (key, value) in lock(&OPTION_VALUES).iter() {
        let value = value.to_string_lossy();
        match defs.iter().find(|d| &d.key == key) {
            None => problems.push(format!(
                "`{key}` is not an option this core has (it publishes {})",
                defs.len()
            )),
            // A core may describe an option without enumerating its values —
            // a free-text path, say — and then anything is legal.
            Some(d) if !d.values.is_empty() && !d.values.iter().any(|v| v == &value) => {
                problems.push(format!(
                    "`{key} = \"{value}\"` is not one of this core's values: {}",
                    d.values.join(", ")
                ))
            }
            Some(_) => {}
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "runner.libretro.options does not match the core:
  {}",
            problems.join("
  ")
        ))
    }
}

/// Environment commands the core asked for and did not get, with how often.
/// Empty is the good answer; anything in it is a capability this frontend does
/// not have and the core has silently worked around.
pub fn unhandled_env() -> BTreeMap<u32, u64> {
    lock(&UNHANDLED_ENV).clone()
}

/// Press a key, as a `RETROK_*` code. Both channels are fed: the held-key set
/// `input_state` answers from, and the core's own callback if it registered one.
pub fn key_down(code: u32) {
    if !lock(&KEYS_DOWN).insert(code) {
        return;
    }
    let cb = *lock(&KEYBOARD_CB);
    if let Some(cb) = cb {
        unsafe { cb(true, code, 0, 0) };
    }
}

pub fn key_up(code: u32) {
    if !lock(&KEYS_DOWN).remove(&code) {
        return;
    }
    let cb = *lock(&KEYBOARD_CB);
    if let Some(cb) = cb {
        unsafe { cb(false, code, 0, 0) };
    }
}

/// Release everything. Used when focus is lost, and before a state is
/// restored — a key the core thinks is held across an entry is a stage that
/// starts with the player walking into a wall. The mouse is released with the
/// keys, for the same reason: a fire button held across an entry is a shot.
pub fn keys_clear() {
    let held: Vec<u32> = lock(&KEYS_DOWN).iter().copied().collect();
    for k in held {
        key_up(k);
    }
    set_mouse(0, 0, 0);
}

/// Latch the mouse for the next frame: motion since the last frame, and the
/// held buttons as `1 << MOUSE_*` bits. Call it once per frame, before
/// `retro_run`; a frame it is not called for repeats the last motion.
pub fn set_mouse(dx: i32, dy: i32, buttons: u32) {
    MOUSE_DX.store(dx, Ordering::Relaxed);
    MOUSE_DY.store(dy, Ordering::Relaxed);
    MOUSE_BUTTONS.store(buttons, Ordering::Relaxed);
}

/// Whether the core has asked to be shut down. Running frames past this drives
/// a core that has already torn itself down.
pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::Relaxed) != 0
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

/// Walks a null-key-terminated v2 definition array. Shared by
/// `SET_CORE_OPTIONS_V2` and the `us` half of its INTL form.
unsafe fn read_v2_defs(defs: *const CoreOptionV2Definition) -> Vec<OptionDef> {
    let mut out = Vec::new();
    if defs.is_null() {
        return out;
    }
    // Bounded rather than trusting the terminator: a struct-layout mistake
    // walks this array at the wrong stride, and a runaway loop is a much worse
    // way to find that out than a short list.
    for i in 0..MAX_OPTIONS {
        let d = &*defs.add(i);
        if d.key.is_null() {
            break;
        }
        out.push(OptionDef {
            key: cstr(d.key),
            desc: cstr(d.desc),
            values: read_values(&d.values),
            default_value: cstr(d.default_value),
        });
    }
    out
}

unsafe fn read_v1_defs(defs: *const CoreOptionDefinition) -> Vec<OptionDef> {
    let mut out = Vec::new();
    if defs.is_null() {
        return out;
    }
    for i in 0..MAX_OPTIONS {
        let d = &*defs.add(i);
        if d.key.is_null() {
            break;
        }
        out.push(OptionDef {
            key: cstr(d.key),
            desc: cstr(d.desc),
            values: read_values(&d.values),
            default_value: cstr(d.default_value),
        });
    }
    out
}

unsafe fn read_values(values: &[CoreOptionValue; NUM_CORE_OPTION_VALUES_MAX]) -> Vec<String> {
    let mut out = Vec::new();
    for v in values.iter() {
        if v.value.is_null() {
            break;
        }
        out.push(cstr(v.value));
    }
    out
}

/// The legacy `SET_VARIABLES` form: `{key, "description; a|b|c"}`, the first
/// value being the default.
unsafe fn read_legacy_vars(vars: *const Variable) -> Vec<OptionDef> {
    let mut out = Vec::new();
    if vars.is_null() {
        return out;
    }
    for i in 0..MAX_OPTIONS {
        let v = &*vars.add(i);
        if v.key.is_null() {
            break;
        }
        let blob = cstr(v.value);
        let (desc, rest) = match blob.split_once(';') {
            Some((d, r)) => (d.to_string(), r.trim().to_string()),
            None => (blob.clone(), String::new()),
        };
        let values: Vec<String> = if rest.is_empty() {
            Vec::new()
        } else {
            rest.split('|').map(|s| s.trim().to_string()).collect()
        };
        out.push(OptionDef {
            key: cstr(v.key),
            desc,
            default_value: values.first().cloned().unwrap_or_default(),
            values,
        });
    }
    out
}

/// A core describes its options once, in whichever of the four forms it
/// prefers. An empty list is not a description, so it never replaces one.
fn record_defs(defs: Vec<OptionDef>) {
    if defs.is_empty() {
        return;
    }
    *lock(&OPTION_DEFS) = defs;
}

const MAX_OPTIONS: usize = 4096;

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
        ENV_GET_SYSTEM_DIRECTORY | ENV_GET_CORE_ASSETS_DIRECTORY => {
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
        // Separate from the system directory, and it matters for DOS: this is
        // where dosbox-pure persists the guest's writes. A stale one is a boot
        // that is not cold, which is a boot that is not reproducible.
        ENV_GET_SAVE_DIRECTORY => {
            if data.is_null() {
                return false;
            }
            match SAVE_DIR.get().or_else(|| SYSTEM_DIR.get()) {
                Some(dir) => {
                    *(data as *mut *const c_char) = dir.as_ptr();
                    true
                }
                None => false,
            }
        }
        ENV_GET_LIBRETRO_PATH => {
            if data.is_null() {
                return false;
            }
            match CORE_PATH.get() {
                Some(p) => {
                    *(data as *mut *const c_char) = p.as_ptr();
                    true
                }
                None => false,
            }
        }
        ENV_GET_OVERSCAN => {
            if data.is_null() {
                return false;
            }
            *(data as *mut bool) = false;
            true
        }
        ENV_GET_LANGUAGE => {
            if data.is_null() {
                return false;
            }
            *(data as *mut c_uint) = 0; // RETRO_LANGUAGE_ENGLISH
            true
        }
        ENV_GET_INPUT_MAX_USERS => {
            if data.is_null() {
                return false;
            }
            *(data as *mut c_uint) = 1; // port 0 only; one local player
            true
        }
        ENV_GET_INPUT_DEVICE_CAPABILITIES => {
            if data.is_null() {
                return false;
            }
            *(data as *mut u64) =
                (1 << RETRO_DEVICE_JOYPAD) | (1 << RETRO_DEVICE_MOUSE) | (1 << RETRO_DEVICE_KEYBOARD);
            true
        }
        ENV_GET_AUDIO_VIDEO_ENABLE => {
            if data.is_null() {
                return false;
            }
            // Bit 0 video, bit 1 audio. A headless run still wants video: the
            // framebuffer is what a spectator gets (ADR 0003) and what a
            // screenshot is taken from.
            *(data as *mut c_uint) = 0b11;
            true
        }
        ENV_GET_FASTFORWARDING | ENV_GET_THROTTLE_STATE => {
            if data.is_null() {
                return false;
            }
            *(data as *mut bool) = false;
            true
        }
        ENV_GET_INPUT_BITMASKS => true,
        ENV_GET_CORE_OPTIONS_VERSION => {
            if data.is_null() {
                return false;
            }
            *(data as *mut c_uint) = 2;
            true
        }
        ENV_GET_MESSAGE_INTERFACE_VERSION => {
            if data.is_null() {
                return false;
            }
            *(data as *mut c_uint) = 1;
            true
        }
        ENV_GET_DISK_CONTROL_INTERFACE_VERSION => {
            if data.is_null() {
                return false;
            }
            *(data as *mut c_uint) = 1;
            true
        }
        ENV_GET_SAVESTATE_CONTEXT => {
            if data.is_null() {
                return false;
            }
            *(data as *mut c_uint) = 0; // RETRO_SAVESTATE_CONTEXT_NORMAL
            true
        }
        ENV_GET_USERNAME => {
            if !data.is_null() {
                *(data as *mut *const c_char) = std::ptr::null();
            }
            false
        }
        // The four forms a core may describe its options in. Recorded rather
        // than ignored, because that record is what `check_options` compares a
        // manifest's pinned values against.
        ENV_SET_CORE_OPTIONS_V2 => {
            if data.is_null() {
                return false;
            }
            record_defs(read_v2_defs((*(data as *const CoreOptionsV2)).definitions));
            true
        }
        ENV_SET_CORE_OPTIONS_V2_INTL => {
            if data.is_null() {
                return false;
            }
            let intl = &*(data as *const CoreOptionsV2Intl);
            if !intl.us.is_null() {
                record_defs(read_v2_defs((*intl.us).definitions));
            }
            true
        }
        ENV_SET_CORE_OPTIONS => {
            if data.is_null() {
                return false;
            }
            record_defs(read_v1_defs(data as *const CoreOptionDefinition));
            true
        }
        ENV_SET_CORE_OPTIONS_INTL => {
            if data.is_null() {
                return false;
            }
            record_defs(read_v1_defs((*(data as *const CoreOptionsIntl)).us));
            true
        }
        ENV_SET_VARIABLES => {
            if data.is_null() {
                return false;
            }
            record_defs(read_legacy_vars(data as *const Variable));
            true
        }
        ENV_GET_VARIABLE => {
            if data.is_null() {
                return false;
            }
            let v = &mut *(data as *mut Variable);
            let key = cstr(v.key);
            match lock(&OPTION_VALUES).get(&key) {
                Some(val) => {
                    v.value = val.as_ptr();
                    true
                }
                None => {
                    // Not pinned by the manifest: the core keeps its default.
                    v.value = std::ptr::null();
                    false
                }
            }
        }
        ENV_GET_VARIABLE_UPDATE => {
            if data.is_null() {
                return false;
            }
            *(data as *mut bool) = VARIABLES_DIRTY.swap(0, Ordering::Relaxed) != 0;
            true
        }
        ENV_SET_VARIABLE => {
            // The core asking the frontend to change a value - what a core does
            // after validating one of its own. Honoured so it is not fighting
            // us every frame.
            if data.is_null() {
                return false;
            }
            let v = &*(data as *const Variable);
            if v.key.is_null() || v.value.is_null() {
                return false;
            }
            if let Ok(val) = CString::new(cstr(v.value)) {
                lock(&OPTION_VALUES).insert(cstr(v.key), val);
            }
            true
        }
        ENV_SET_KEYBOARD_CALLBACK => {
            if data.is_null() {
                return false;
            }
            *lock(&KEYBOARD_CB) = (*(data as *const KeyboardCallback)).callback;
            true
        }
        // A core-supplied status line. Worth surfacing: it is where dosbox-pure
        // reports that it could not mount the content, which otherwise looks
        // like a game that boots to nothing.
        ENV_SET_MESSAGE => {
            if !data.is_null() {
                #[repr(C)]
                struct Message {
                    msg: *const c_char,
                    frames: c_uint,
                }
                let text = cstr((*(data as *const Message)).msg);
                if !text.is_empty() {
                    eprintln!("[core] {text}");
                }
            }
            true
        }
        ENV_SET_MESSAGE_EXT => {
            if !data.is_null() {
                // Only the leading `msg` pointer is read; the rest is
                // priorities and display targets this frontend has no use for.
                let text = cstr(*(data as *const *const c_char));
                if !text.is_empty() {
                    eprintln!("[core] {text}");
                }
            }
            true
        }
        ENV_SHUTDOWN => {
            SHUTDOWN.store(1, Ordering::Relaxed);
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
        ENV_SET_ROTATION
        | ENV_SET_PERFORMANCE_LEVEL
        | ENV_SET_INPUT_DESCRIPTORS
        | ENV_SET_SUPPORT_NO_GAME
        | ENV_SET_SUBSYSTEM_INFO
        | ENV_SET_CONTROLLER_INFO
        | ENV_SET_GEOMETRY
        | ENV_SET_SYSTEM_AV_INFO
        | ENV_SET_SUPPORT_ACHIEVEMENTS
        | ENV_SET_CORE_OPTIONS_DISPLAY
        | ENV_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK
        | ENV_SET_AUDIO_BUFFER_STATUS_CALLBACK
        | ENV_SET_MINIMUM_AUDIO_LATENCY
        | ENV_SET_FASTFORWARDING_OVERRIDE
        | ENV_SET_DISK_CONTROL_INTERFACE
        | ENV_SET_DISK_CONTROL_EXT_INTERFACE => true,

        // Deliberately refused, and listed so they are not counted as gaps.
        //
        // LOG_INTERFACE hands back a C-variadic function pointer, which stable
        // Rust cannot define. HW_RENDER would ask for a GL context this
        // frontend does not have - refusing is what keeps dosbox-pure on its
        // software renderer, which is what ADR 0018 wants anyway. The rest are
        // capabilities we genuinely lack.
        ENV_GET_LOG_INTERFACE
        | ENV_SET_HW_RENDER
        | ENV_GET_VFS_INTERFACE
        | ENV_GET_RUMBLE_INTERFACE
        | ENV_GET_LED_INTERFACE
        | ENV_GET_PERF_INTERFACE
        | ENV_SET_FRAME_TIME_CALLBACK
        | ENV_SET_AUDIO_CALLBACK
        | ENV_SET_CONTENT_INFO_OVERRIDE
        | ENV_GET_GAME_INFO_EXT => false,

        // Everything else: refused, and counted. `harness --dump-options`
        // prints the tally, which is how the next core's gaps get found
        // without reading its source.
        other => {
            *lock(&UNHANDLED_ENV).entry(other).or_insert(0) += 1;
            false
        }
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

fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

unsafe extern "C" fn cb_input_poll() {
    // The main loop polls window and gamepad before calling retro_run.
}

unsafe extern "C" fn cb_input_state(port: u32, device: u32, _index: u32, id: u32) -> i16 {
    if port != 0 {
        return 0;
    }
    match device & RETRO_DEVICE_MASK {
        RETRO_DEVICE_JOYPAD => {
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
        // `id` is a RETROK_* code here, not a button index. A core polls this
        // for every key it cares about, every frame.
        RETRO_DEVICE_KEYBOARD => i16::from(lock(&KEYS_DOWN).contains(&id)),
        RETRO_DEVICE_MOUSE => match id {
            MOUSE_X => clamp_i16(MOUSE_DX.load(Ordering::Relaxed)),
            MOUSE_Y => clamp_i16(MOUSE_DY.load(Ordering::Relaxed)),
            _ if id < 32 => i16::from(MOUSE_BUTTONS.load(Ordering::Relaxed) & (1 << id) != 0),
            _ => 0,
        },
        _ => 0,
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
    /// Loads the core DLL and performs the libretro handshake up to
    /// `retro_set_environment`.
    ///
    /// `save_dir` is where a core persists what the guest writes. It is
    /// separate from the system directory because for DOS it is load-bearing:
    /// dosbox-pure keeps the guest's writes there, so a stale one turns a cold
    /// boot into a warm one and takes reproducibility with it. Callers that do
    /// not care may pass the core's own directory.
    ///
    /// Both paths are made absolute: a core resolves them against its own
    /// working directory, which is not necessarily ours.
    pub fn load(path: &Path, save_dir: Option<&Path>) -> Result<Self, String> {
        // Several cores look for firmware in whatever we report as the system
        // directory. Point it at the folder the core itself lives in.
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let _ = SYSTEM_DIR.set(cpath(dir)?);
        let _ = SAVE_DIR.set(cpath(save_dir.unwrap_or(dir))?);
        let _ = CORE_PATH.set(cpath(path)?);

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

    pub fn system_info(&self) -> CoreInfo {
        let mut info = SystemInfo {
            library_name: std::ptr::null(),
            library_version: std::ptr::null(),
            valid_extensions: std::ptr::null(),
            need_fullpath: false,
            block_extract: false,
        };
        unsafe { (self.fn_get_system_info)(&mut info) };
        unsafe {
            CoreInfo {
                name: cstr(info.library_name),
                version: cstr(info.library_version),
                valid_extensions: cstr(info.valid_extensions),
                need_fullpath: info.need_fullpath,
                block_extract: info.block_extract,
            }
        }
    }

    /// Hands the content over, honouring `need_fullpath` rather than passing
    /// both a path and a buffer.
    ///
    /// Passing both is harmless for a cartridge and wrong for a core that
    /// mounts a filesystem out of the content: it leaves the core to pick, and
    /// a core that picks differently between runs is a core that boots
    /// differently between runs. `device` is which libretro device port 0 is
    /// driven through — a pad for a console, a keyboard for DOS.
    pub fn load_game(&mut self, rom_path: &Path, rom: &[u8], device: u32) -> Result<(), String> {
        let path = CString::new(rom_path.to_string_lossy().as_ref()).map_err(|e| e.to_string())?;
        let need_fullpath = self.system_info().need_fullpath;
        let info = GameInfo {
            path: path.as_ptr(),
            data: if need_fullpath {
                std::ptr::null()
            } else {
                rom.as_ptr() as *const c_void
            },
            size: if need_fullpath { 0 } else { rom.len() },
            meta: std::ptr::null(),
        };
        if !unsafe { (self.fn_load_game)(&info) } {
            return Err("core refused the content (retro_load_game returned false)".into());
        }
        self.game_loaded = true;
        unsafe { (self.fn_set_controller_port_device)(0, device) };
        Ok(())
    }

    /// Unload and reload the same content, giving a core the closest thing to a
    /// fresh process it can offer in-place.
    ///
    /// `retro_reset` is the usual way to start over, and for a console it is
    /// enough. A DOS core boots a machine out of the content when the content
    /// is loaded, so re-loading is what actually reproduces a cold boot.
    pub fn reload_game(&mut self, rom_path: &Path, rom: &[u8], device: u32) -> Result<(), String> {
        if self.game_loaded {
            unsafe { (self.fn_unload_game)() };
            self.game_loaded = false;
        }
        keys_clear();
        JOYPAD.store(0, Ordering::Relaxed);
        self.load_game(rom_path, rom, device)
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

    pub fn serialize_size(&self) -> usize {
        unsafe { (self.fn_serialize_size)() }
    }

    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        let size = self.serialize_size();
        if size == 0 {
            return Err("core does not support savestates".into());
        }
        let mut buf = vec![0u8; size];
        if !unsafe { (self.fn_serialize)(buf.as_mut_ptr() as *mut c_void, size) } {
            return Err("retro_serialize failed".into());
        }
        Ok(buf)
    }

    /// Restore a captured state. The core arbitrates whether it can take it.
    ///
    /// This used to pre-check `data.len()` against `retro_serialize_size` — a
    /// savestate is not portable across cores or core versions, and a size
    /// mismatch looked like the only warning before a core ate garbage. Two
    /// things about dosbox-pure retired that check, and both are worth
    /// recording because the reasoning generalises to the next core:
    ///
    /// * **The size is not a constant.** dosbox-pure reports 365 KB before the
    ///   guest has booted and 3.6 MB after, because what it serialises is a
    ///   machine that does not fully exist yet. An equality test against a
    ///   moving number refuses states that are perfectly good.
    /// * **Asking is not free.** `retro_serialize_size` opens a handshake with
    ///   the core's emulation thread that `retro_serialize` closes. Calling it
    ///   and then restoring instead leaves that handshake dangling, and the
    ///   core deadlocks — silently, on a call that has no timeout.
    ///
    /// libretro's contract already covers the case the check was guarding:
    /// `retro_unserialize` returns false for data it cannot take. So the core
    /// is asked directly, which is what it is for.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), String> {
        if data.is_empty() {
            return Err("savestate is empty".into());
        }
        if !unsafe { (self.fn_unserialize)(data.as_ptr() as *const c_void, data.len()) } {
            return Err(format!(
                "retro_unserialize refused {} bytes: the state was probably made by a \
                 different core or a different version of this one",
                data.len()
            ));
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

/// An absolute path as a C string, for handing to a core. Absolute because a
/// core resolves a relative one against its own working directory rather than
/// against ours.
fn cpath(p: &Path) -> Result<CString, String> {
    let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    // `canonicalize` returns a \\?\ prefixed path on Windows, which several
    // cores mishandle when they concatenate it with their own separators.
    let text = abs.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    CString::new(text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse(id: u32) -> i16 {
        unsafe { cb_input_state(0, RETRO_DEVICE_MOUSE, 0, id) }
    }

    /// The mouse is latched once per frame, so a core that asks twice gets the
    /// same motion twice; and it is released with the keys, so a held fire
    /// button does not survive a stage entry.
    #[test]
    fn the_mouse_is_latched_per_frame_and_released_with_the_keys() {
        set_mouse(5, -40_000, (1 << MOUSE_LEFT) | (1 << MOUSE_WHEELUP));
        assert_eq!((mouse(MOUSE_X), mouse(MOUSE_X)), (5, 5));
        assert_eq!(mouse(MOUSE_Y), i16::MIN, "motion saturates rather than wrapping");
        assert_eq!((mouse(MOUSE_LEFT), mouse(MOUSE_RIGHT), mouse(MOUSE_WHEELUP)), (1, 0, 1));
        // A subclassed device is still a mouse.
        assert_eq!(unsafe { cb_input_state(0, RETRO_DEVICE_MOUSE | (1 << 8), 0, MOUSE_X) }, 5);
        // Only port 0 has a player behind it.
        assert_eq!(unsafe { cb_input_state(1, RETRO_DEVICE_MOUSE, 0, MOUSE_X) }, 0);

        keys_clear();
        assert_eq!((mouse(MOUSE_X), mouse(MOUSE_Y), mouse(MOUSE_LEFT)), (0, 0, 0));
    }
}
