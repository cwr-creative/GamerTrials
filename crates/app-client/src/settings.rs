//! The player's settings: what drives the RetroPad, and how a picture is
//! shown. Set once, and overridable per core and per game (ADR 0028).
//!
//! Nothing here reaches the emulated machine. A binding decides which host
//! inputs press a RetroPad button, and video decides where the core's picture
//! lands in the window; neither changes a core option, the frame clock, or
//! what a frame means (invariant 4). Settings never go on the wire either:
//! they are this machine's, like the window it draws in.
//!
//! Each layer — game, core, global — holds only what it changes. A button a
//! layer does not mention is the next layer's, and under global is the
//! built-in default, which is the one place a manifest's `pad_layout` still
//! matters: it reproduces the mapping the client had before there were
//! settings, so a player who never opens this page plays as before.
//!
//! The keyboard-and-mouse half and the gamepad half of a binding inherit
//! apart. A player who moves a key has said nothing about their pad, so an
//! NES game keeps its two-button pad layout, and a keyboard game keeps its
//! pad binds, however the keys are set.
//!
//! Pure: no Win32, no Tauri. `keys` turns a [`ResolvedInput`] into presses.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use session_libretro::keymap::{self, RetroButton};
use session_libretro::manifest::{Controller, PadLayout};

/// How many inputs of one family one button may be bound to: a primary and
/// alternates. The built-in NES A already uses two pad buttons.
pub const MAX_SLOTS: usize = 3;
/// How many inputs one chord may hold down together.
pub const MAX_CHORD: usize = 4;
/// The spectator picture's bitrate when none is set, and the range a setting
/// is held to, in kbit/s: under 100 a retro picture smears; over 2000 buys
/// nothing at these resolutions that a home uplink would notice.
pub const DEFAULT_STREAM_KBPS: u32 = 500;
pub const STREAM_KBPS: (u32, u32) = (100, 2000);
/// The sidebar thumbnail's (ADR 0038), likewise. Lower than the full
/// picture's, since it is seen small, and allowed lower still: a thumbnail
/// that smears is still a glance at who is where.
pub const DEFAULT_THUMB_KBPS: u32 = 250;
pub const THUMB_KBPS: (u32, u32) = (50, 2000);

pub const SETTINGS_VERSION: u32 = 1;

/// The bout's UDP port unless Settings says otherwise: one above the
/// server's 7878, so a player hosting the server and playing on one machine
/// forwards the pair.
pub const DEFAULT_BOUT_PORT: u16 = 7879;

// ------------------------------------------------------------------ inputs

/// A physical pad button, by position. Same order as
/// `session_libretro::input::PAD_BUTTONS`, so its index is its bit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PadButton {
    South,
    East,
    West,
    North,
    Lb,
    Rb,
    Lt,
    Rt,
    L3,
    R3,
    Start,
    Select,
    Guide,
    DUp,
    DDown,
    DLeft,
    DRight,
}

impl PadButton {
    pub const ALL: [PadButton; 17] = [
        PadButton::South,
        PadButton::East,
        PadButton::West,
        PadButton::North,
        PadButton::Lb,
        PadButton::Rb,
        PadButton::Lt,
        PadButton::Rt,
        PadButton::L3,
        PadButton::R3,
        PadButton::Start,
        PadButton::Select,
        PadButton::Guide,
        PadButton::DUp,
        PadButton::DDown,
        PadButton::DLeft,
        PadButton::DRight,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            PadButton::South => "south",
            PadButton::East => "east",
            PadButton::West => "west",
            PadButton::North => "north",
            PadButton::Lb => "lb",
            PadButton::Rb => "rb",
            PadButton::Lt => "lt",
            PadButton::Rt => "rt",
            PadButton::L3 => "l3",
            PadButton::R3 => "r3",
            PadButton::Start => "start",
            PadButton::Select => "select",
            PadButton::Guide => "guide",
            PadButton::DUp => "dup",
            PadButton::DDown => "ddown",
            PadButton::DLeft => "dleft",
            PadButton::DRight => "dright",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Stick {
    Left,
    Right,
}

/// A stick pushed one way past the deadzone.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

impl MouseButton {
    const ALL: [MouseButton; 5] =
        [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::X1, MouseButton::X2];

    fn name(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
            MouseButton::X1 => "x1",
            MouseButton::X2 => "x2",
        }
    }

    /// Its Windows virtual key, which `GetAsyncKeyState` reads like a key.
    pub fn vk(self) -> i32 {
        match self {
            MouseButton::Left => 0x01,
            MouseButton::Right => 0x02,
            MouseButton::Middle => 0x04,
            MouseButton::X1 => 0x05,
            MouseButton::X2 => 0x06,
        }
    }
}

/// One host input a binding can name. Written as `key:z`, `mouse:left`,
/// `pad:south` or `stick:left-up`, so a settings file reads like what it is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Input {
    /// A Windows virtual-key code.
    Key(i32),
    Mouse(MouseButton),
    Pad(PadButton),
    Stick(Stick, Dir),
}

/// A host key's name: a guest key's name when it is one, the F-keys by
/// number, anything else by its code.
pub fn vk_name(vk: i32) -> String {
    if let Some(g) = keymap::guest_key_for_vk(vk) {
        return g.name.to_string();
    }
    if (0x70..=0x87).contains(&vk) {
        return format!("f{}", vk - 0x6F);
    }
    format!("vk{vk:02x}")
}

pub fn vk_parse(name: &str) -> Option<i32> {
    if let Some(g) = keymap::guest_key(name) {
        return Some(g.vk);
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<i32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x6F + n);
        }
    }
    let code = i32::from_str_radix(name.strip_prefix("vk")?, 16).ok()?;
    (0x07..=0xFE).contains(&code).then_some(code)
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Input::Key(vk) => write!(f, "key:{}", vk_name(*vk)),
            Input::Mouse(b) => write!(f, "mouse:{}", b.name()),
            Input::Pad(b) => write!(f, "pad:{}", b.name()),
            Input::Stick(s, d) => write!(
                f,
                "stick:{}-{}",
                match s {
                    Stick::Left => "left",
                    Stick::Right => "right",
                },
                match d {
                    Dir::Up => "up",
                    Dir::Down => "down",
                    Dir::Left => "left",
                    Dir::Right => "right",
                }
            ),
        }
    }
}

impl std::str::FromStr for Input {
    type Err = String;

    fn from_str(s: &str) -> Result<Input, String> {
        let bad = || format!("`{s}` is not an input; try key:z, mouse:left, pad:south or stick:left-up");
        let (kind, rest) = s.split_once(':').ok_or_else(bad)?;
        match kind {
            "key" => vk_parse(rest).map(Input::Key).ok_or_else(bad),
            "mouse" => MouseButton::ALL.into_iter().find(|b| b.name() == rest).map(Input::Mouse).ok_or_else(bad),
            "pad" => PadButton::ALL.into_iter().find(|b| b.name() == rest).map(Input::Pad).ok_or_else(bad),
            "stick" => {
                let (stick, dir) = rest.split_once('-').ok_or_else(bad)?;
                let stick = match stick {
                    "left" => Stick::Left,
                    "right" => Stick::Right,
                    _ => return Err(bad()),
                };
                let dir = match dir {
                    "up" => Dir::Up,
                    "down" => Dir::Down,
                    "left" => Dir::Left,
                    "right" => Dir::Right,
                    _ => return Err(bad()),
                };
                Ok(Input::Stick(stick, dir))
            }
            _ => Err(bad()),
        }
    }
}

impl Serialize for Input {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Input {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Input, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// The two halves of a binding, which inherit apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    /// The keyboard and the mouse's buttons.
    Keys,
    Pad,
}

impl Input {
    /// Whether this input comes from a gamepad rather than the keyboard or
    /// mouse. A keyboard game passes those two straight through, so only a
    /// gamepad's inputs go through its RetroPad and on to `pad_keys`.
    pub fn is_pad(&self) -> bool {
        matches!(self, Input::Pad(_) | Input::Stick(..))
    }

    pub fn family(&self) -> Family {
        if self.is_pad() {
            Family::Pad
        } else {
            Family::Keys
        }
    }
}

/// What each RetroPad button is bound to, in one layer. A button a layer does
/// not mention is inherited; an empty list is unbound on purpose.
pub type Binds = BTreeMap<RetroButton, Vec<Input>>;

/// Inputs held together. One input is a chord of one.
pub type Chord = Vec<Input>;

// ---------------------------------------------------------------- settings

/// The keys the client owns during play. Global only: they are the
/// client's, not a game's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Actions {
    pub pause: Vec<Chord>,
    pub give_up: Vec<Chord>,
}

impl Default for Actions {
    fn default() -> Self {
        Actions {
            pause: vec![vec![Input::Key(0x79)], vec![Input::Pad(PadButton::Select), Input::Pad(PadButton::Start)]],
            give_up: vec![vec![Input::Key(0x78)]],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputSettings {
    /// Global changes to the built-in keyboard and mouse mapping.
    pub keys: Binds,
    /// Global changes to the built-in gamepad mapping.
    pub pad: Binds,
    /// How far a stick must move to count as a press, 0..1.
    pub deadzone: f32,
    /// Multiplies mouse motion in a keyboard game.
    pub mouse_sensitivity: f32,
    pub actions: Actions,
}

impl Default for InputSettings {
    fn default() -> Self {
        InputSettings {
            keys: Binds::new(),
            pad: Binds::new(),
            deadzone: 0.5,
            mouse_sensitivity: 1.0,
            actions: Actions::default(),
        }
    }
}

/// One core's or one game's changes to the layer under it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: Binds,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub pad: Binds,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse_sensitivity: Option<f32>,
    /// A keyboard game: the guest key a RetroPad button presses, over the
    /// manifest's suggestion. An empty name presses nothing.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub pad_keys: BTreeMap<RetroButton, String>,
    /// A keyboard game: the guest key a stick direction sends of its own,
    /// rather than through whichever RetroPad button it is bound to (ADR
    /// 0031). An empty name sends nothing. A direction not listed goes
    /// through the RetroPad, as on a pad game.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub stick_keys: BTreeMap<Input, String>,
    /// A keyboard game: host key name to the guest key it sends instead. An
    /// empty name sends nothing. A key not listed sends itself.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub key_remap: BTreeMap<String, String>,
}

impl Layer {
    fn is_empty(&self) -> bool {
        self == &Layer::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Display {
    #[default]
    Windowed,
    /// The game window and the overlay over it, borderless at the monitor's
    /// size (ADR 0027).
    Borderless,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scaling {
    /// As large as fits, at the picture's aspect.
    #[default]
    Fit,
    /// The largest whole multiple of the picture's height that fits.
    Integer,
    /// Fill the window, aspect be damned.
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aspect {
    /// The shape the core says its picture is: a TV's 4:3 for the consoles.
    #[default]
    Core,
    /// Every pixel square, as the frame buffer is.
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoPrefs {
    /// Windowed size, as a multiple of the core's picture height.
    pub window_scale: u32,
    pub scaling: Scaling,
    pub aspect: Aspect,
    pub crop: Crop,
}

impl Default for VideoPrefs {
    fn default() -> Self {
        VideoPrefs { window_scale: 3, scaling: Scaling::Fit, aspect: Aspect::Core, crop: Crop::default() }
    }
}

/// A core's changes to the global video settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoLayer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_scale: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scaling: Option<Scaling>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect: Option<Aspect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop: Option<Crop>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    pub display: Display,
    pub global: VideoPrefs,
    pub cores: BTreeMap<String, VideoLayer>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub settings_version: u32,
    /// The display name the menu offers.
    pub name: String,
    /// `host[:port]` of the `gamertrials-server` Host game opens a lobby on
    /// and a bare invite code joins on (ADR 0030). Empty: Host plays on this
    /// machine alone.
    pub server: String,
    /// The picture this player sends a watcher, in kbit/s (ADR 0034). Only
    /// this player's uplink and their watchers' downlinks feel it; a link
    /// that cannot carry it drops pictures rather than falling behind.
    pub stream_kbps: u32,
    /// The thumbnail this player sends the other players' sidebars, in
    /// kbit/s (ADR 0038): sent for the whole run once anyone shows it, at
    /// the full frame rate.
    pub thumb_kbps: u32,
    /// In a bout (ADR 0036): frames this player's own input is delayed so the
    /// other machine's has time to arrive and fewer frames are guessed. Felt
    /// on one's own buttons; fewer rollbacks for the other side.
    pub bout_input_delay: u8,
    /// The UDP port a bout's inputs arrive on (ADR 0036): fixed, so a player
    /// at home can forward it on their router and be reached by a player
    /// whose own network cannot be (a VPN, a carrier's NAT). 0 takes any free port.
    /// Taken already — a second client on this machine — means any port.
    pub bout_port: u16,
    pub input: InputSettings,
    /// By core id: `mesen`, `snes9x`, `dosbox_pure`.
    pub cores: BTreeMap<String, Layer>,
    /// By game id. A game that is not installed keeps its entry.
    pub games: BTreeMap<String, Layer>,
    pub video: VideoSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            settings_version: SETTINGS_VERSION,
            name: "Player".into(),
            server: String::new(),
            stream_kbps: DEFAULT_STREAM_KBPS,
            thumb_kbps: DEFAULT_THUMB_KBPS,
            bout_input_delay: 2,
            bout_port: DEFAULT_BOUT_PORT,
            input: InputSettings::default(),
            cores: BTreeMap::new(),
            games: BTreeMap::new(),
            video: VideoSettings::default(),
        }
    }
}

// ---------------------------------------------------------------- defaults

/// The mapping the client had before it had settings, which is what every
/// layer finally inherits. Z/X are B/A and A/S are Y/X on the keyboard; a
/// pad's diamond is positional on a four-button console, and on a two-button
/// one South *or* East is A so both habits work (`session_libretro::input`).
pub fn builtin(layout: PadLayout, family: Family) -> Binds {
    let mut b = builtin_all(layout);
    for v in b.values_mut() {
        v.retain(|i| i.family() == family);
    }
    b.retain(|_, v| !v.is_empty());
    b
}

fn builtin_all(layout: PadLayout) -> Binds {
    use Input::{Key, Pad, Stick as S};
    use PadButton as P;
    let k = |c: u8| Key(c as i32);
    let mut b = Binds::new();
    b.insert(RetroButton::Up, vec![Key(0x26), Pad(P::DUp), S(Stick::Left, Dir::Up)]);
    b.insert(RetroButton::Down, vec![Key(0x28), Pad(P::DDown), S(Stick::Left, Dir::Down)]);
    b.insert(RetroButton::Left, vec![Key(0x25), Pad(P::DLeft), S(Stick::Left, Dir::Left)]);
    b.insert(RetroButton::Right, vec![Key(0x27), Pad(P::DRight), S(Stick::Left, Dir::Right)]);
    b.insert(RetroButton::Start, vec![Key(0x0D), Pad(P::Start)]);
    b.insert(RetroButton::Select, vec![Key(0xA1), Pad(P::Select)]);
    match layout {
        PadLayout::Nes => {
            b.insert(RetroButton::B, vec![k(b'Z'), Pad(P::West), Pad(P::North)]);
            b.insert(RetroButton::A, vec![k(b'X'), Pad(P::South), Pad(P::East)]);
            b.insert(RetroButton::Y, vec![k(b'A')]);
            b.insert(RetroButton::X, vec![k(b'S')]);
            b.insert(RetroButton::L, vec![k(b'Q')]);
            b.insert(RetroButton::R, vec![k(b'W')]);
        }
        PadLayout::Snes => {
            b.insert(RetroButton::B, vec![k(b'Z'), Pad(P::South)]);
            b.insert(RetroButton::A, vec![k(b'X'), Pad(P::East)]);
            b.insert(RetroButton::Y, vec![k(b'A'), Pad(P::West)]);
            b.insert(RetroButton::X, vec![k(b'S'), Pad(P::North)]);
            b.insert(RetroButton::L, vec![k(b'Q'), Pad(P::Lb)]);
            b.insert(RetroButton::R, vec![k(b'W'), Pad(P::Rb)]);
        }
    }
    // Neither console has these; a keyboard game's `pad_keys` may use them.
    b.insert(RetroButton::L2, vec![Pad(P::Lt)]);
    b.insert(RetroButton::R2, vec![Pad(P::Rt)]);
    b.insert(RetroButton::L3, vec![Pad(P::L3)]);
    b.insert(RetroButton::R3, vec![Pad(P::R3)]);
    b
}

/// The layout a game's built-in mapping is drawn from. A keyboard game takes
/// the positional one: its pad buttons are named for the RetroPad, and what
/// they press is `pad_keys`'.
pub fn layout_of(controller: Controller) -> PadLayout {
    match controller {
        Controller::Pad(l) => l,
        Controller::Keyboard => PadLayout::Snes,
    }
}

/// Which RetroPad buttons a console has, for the page to dim the rest. A
/// keyboard game "has" every one: any of them can press a key.
pub fn buttons_of(controller: Controller) -> Vec<RetroButton> {
    use RetroButton::*;
    match controller {
        Controller::Pad(PadLayout::Nes) => vec![B, A, Select, Start, Up, Down, Left, Right],
        Controller::Pad(PadLayout::Snes) => vec![B, Y, A, X, L, R, Select, Start, Up, Down, Left, Right],
        Controller::Keyboard => RetroButton::ALL.to_vec(),
    }
}

// --------------------------------------------------------------- resolving

/// Everything the game thread needs to read one game's input.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedInput {
    /// Indexed by RetroPad id. For a keyboard game, gamepad inputs only.
    pub binds: [Vec<Input>; 16],
    pub deadzone: f32,
    pub actions: Actions,
    /// Present for a keyboard game.
    pub keyboard: Option<KeyboardGame>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardGame {
    /// RetroPad id to the `RETROK_*` it presses.
    pub pad_keys: Vec<(u32, u32)>,
    /// A stick direction to the `RETROK_*` it presses of its own. Such a
    /// direction is in no RetroPad button's `binds`.
    pub stick_keys: Vec<(Input, u32)>,
    /// Every host key the game is passed: its virtual key and the `RETROK_*`
    /// it sends, or none.
    pub keys: Vec<(i32, Option<u32>)>,
    pub mouse_sensitivity: f32,
}

impl Default for ResolvedInput {
    /// The built-in SNES mapping with nothing changed: what a game gets when
    /// the page has never been opened.
    fn default() -> Self {
        Settings::default().resolve_input("", "", Controller::Pad(PadLayout::Snes), &BTreeMap::new())
    }
}

impl Settings {
    /// One half of one button's binding as a game sees it, and which layer
    /// it came from.
    pub fn bind_for(
        &self,
        core: &str,
        game: &str,
        layout: PadLayout,
        b: RetroButton,
        family: Family,
    ) -> (Vec<Input>, &'static str) {
        let half = |l: &Layer| match family {
            Family::Keys => l.keys.get(&b).cloned(),
            Family::Pad => l.pad.get(&b).cloned(),
        };
        if let Some(v) = self.games.get(game).and_then(half) {
            return (v, "game");
        }
        if let Some(v) = self.cores.get(core).and_then(half) {
            return (v, "core");
        }
        let global = match family {
            Family::Keys => &self.input.keys,
            Family::Pad => &self.input.pad,
        };
        if let Some(v) = global.get(&b) {
            return (v.clone(), "global");
        }
        (builtin(layout, family).remove(&b).unwrap_or_default(), "default")
    }

    pub fn resolve_input(
        &self,
        core: &str,
        game: &str,
        controller: Controller,
        suggested_pad_keys: &BTreeMap<RetroButton, String>,
    ) -> ResolvedInput {
        let layout = layout_of(controller);
        let keyboard_game = controller == Controller::Keyboard;
        let mut binds: [Vec<Input>; 16] = Default::default();
        for b in RetroButton::ALL {
            // A keyboard game's keys pass straight through instead.
            let mut inputs =
                if keyboard_game { Vec::new() } else { self.bind_for(core, game, layout, b, Family::Keys).0 };
            inputs.extend(self.bind_for(core, game, layout, b, Family::Pad).0);
            binds[b.id() as usize] = inputs;
        }
        let g = self.games.get(game);
        let c = self.cores.get(core);
        let keyboard = keyboard_game.then(|| {
            // A direction with a key of its own leaves the RetroPad, so the
            // button it was bound to does not press as well.
            let mut stick_keys = Vec::new();
            for s in [Stick::Left, Stick::Right] {
                for d in [Dir::Up, Dir::Down, Dir::Left, Dir::Right] {
                    let i = Input::Stick(s, d);
                    let Some(name) =
                        g.and_then(|l| l.stick_keys.get(&i)).or_else(|| c.and_then(|l| l.stick_keys.get(&i)))
                    else {
                        continue;
                    };
                    for v in binds.iter_mut() {
                        v.retain(|x| *x != i);
                    }
                    if let Some(key) = keymap::guest_key(name) {
                        stick_keys.push((i, key.retrok));
                    }
                }
            }
            let mut pad_keys = Vec::new();
            for b in RetroButton::ALL {
                let name = g
                    .and_then(|l| l.pad_keys.get(&b))
                    .or_else(|| c.and_then(|l| l.pad_keys.get(&b)))
                    .or_else(|| suggested_pad_keys.get(&b));
                if let Some(key) = name.and_then(|n| keymap::guest_key(n)) {
                    pad_keys.push((b.id(), key.retrok));
                }
            }
            let keys = keymap::GUEST_KEYS
                .iter()
                .map(|host| {
                    let to = g
                        .and_then(|l| l.key_remap.get(host.name))
                        .or_else(|| c.and_then(|l| l.key_remap.get(host.name)));
                    let sent = match to {
                        None => Some(host.retrok),
                        Some(name) => keymap::guest_key(name).map(|k| k.retrok),
                    };
                    (host.vk, sent)
                })
                .collect();
            let mouse_sensitivity = g
                .and_then(|l| l.mouse_sensitivity)
                .or_else(|| c.and_then(|l| l.mouse_sensitivity))
                .unwrap_or(self.input.mouse_sensitivity);
            KeyboardGame { pad_keys, stick_keys, keys, mouse_sensitivity }
        });
        ResolvedInput { binds, deadzone: self.input.deadzone, actions: self.input.actions.clone(), keyboard }
    }

    pub fn resolve_video(&self, core: &str) -> VideoPrefs {
        let mut v = self.video.global;
        if let Some(l) = self.video.cores.get(core) {
            if let Some(x) = l.window_scale {
                v.window_scale = x;
            }
            if let Some(x) = l.scaling {
                v.scaling = x;
            }
            if let Some(x) = l.aspect {
                v.aspect = x;
            }
            if let Some(x) = l.crop {
                v.crop = x;
            }
        }
        v
    }

    /// Bring whatever the page sent inside the bounds the client honours,
    /// rather than refusing it: every field has a sensible nearest value.
    pub fn sanitize(&mut self) {
        self.settings_version = SETTINGS_VERSION;
        self.name = self.name.trim().chars().take(24).collect();
        if self.name.is_empty() {
            self.name = "Player".into();
        }
        self.server = self.server.trim().chars().take(255).collect();
        self.stream_kbps = self.stream_kbps.clamp(STREAM_KBPS.0, STREAM_KBPS.1);
        self.thumb_kbps = self.thumb_kbps.clamp(THUMB_KBPS.0, THUMB_KBPS.1);
        self.bout_input_delay = self.bout_input_delay.min(10);
        let clamp =|v: f32, lo: f32, hi: f32, default: f32| if v.is_finite() { v.clamp(lo, hi) } else { default };
        self.input.deadzone = clamp(self.input.deadzone, 0.1, 0.95, 0.5);
        self.input.mouse_sensitivity = clamp(self.input.mouse_sensitivity, 0.1, 10.0, 1.0);
        let trim_binds = |b: &mut Binds, family: Family| {
            for v in b.values_mut() {
                v.retain(|i| i.family() == family);
                dedup(v);
                v.truncate(MAX_SLOTS);
            }
        };
        trim_binds(&mut self.input.keys, Family::Keys);
        trim_binds(&mut self.input.pad, Family::Pad);
        for chords in [&mut self.input.actions.pause, &mut self.input.actions.give_up] {
            chords.retain(|c| !c.is_empty());
            for c in chords.iter_mut() {
                dedup(c);
                c.truncate(MAX_CHORD);
            }
            chords.truncate(MAX_SLOTS);
        }
        for layer in self.cores.values_mut().chain(self.games.values_mut()) {
            trim_binds(&mut layer.keys, Family::Keys);
            trim_binds(&mut layer.pad, Family::Pad);
            layer.mouse_sensitivity = layer.mouse_sensitivity.map(|s| clamp(s, 0.1, 10.0, 1.0));
            layer.pad_keys.retain(|_, k| k.is_empty() || keymap::guest_key(k).is_some());
            layer
                .stick_keys
                .retain(|i, k| matches!(i, Input::Stick(..)) && (k.is_empty() || keymap::guest_key(k).is_some()));
            layer.key_remap.retain(|from, to| {
                keymap::guest_key(from).is_some() && (to.is_empty() || keymap::guest_key(to).is_some())
            });
        }
        self.cores.retain(|_, l| !l.is_empty());
        self.games.retain(|_, l| !l.is_empty());
        let fix = |v: &mut VideoPrefs| {
            v.window_scale = v.window_scale.clamp(1, 6);
            for c in [&mut v.crop.top, &mut v.crop.bottom, &mut v.crop.left, &mut v.crop.right] {
                *c = (*c).min(64);
            }
        };
        fix(&mut self.video.global);
        for l in self.video.cores.values_mut() {
            l.window_scale = l.window_scale.map(|s| s.clamp(1, 6));
            if let Some(c) = l.crop.as_mut() {
                for x in [&mut c.top, &mut c.bottom, &mut c.left, &mut c.right] {
                    *x = (*x).min(64);
                }
            }
        }
        self.video.cores.retain(|_, l| l != &VideoLayer::default());
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("settings always serialize")
    }

    pub fn from_toml(text: &str) -> Result<Settings, String> {
        let mut s: Settings = toml::from_str(text).map_err(|e| e.to_string())?;
        if s.settings_version > SETTINGS_VERSION {
            return Err(format!(
                "settings_version {} is newer than this client's {SETTINGS_VERSION}",
                s.settings_version
            ));
        }
        s.sanitize();
        Ok(s)
    }

    /// Read the file, or the defaults. A file that will not parse is moved
    /// aside to `.bad` rather than overwritten by the next save, and the
    /// reason is returned so the page can say so.
    pub fn load(path: &Path) -> (Settings, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), None),
            Err(e) => return (Settings::default(), Some(format!("{}: {e}", path.display()))),
        };
        match Settings::from_toml(&text) {
            Ok(s) => (s, None),
            Err(e) => {
                let aside = path.with_extension("toml.bad");
                let _ = std::fs::rename(path, &aside);
                (Settings::default(), Some(format!("{} could not be read ({e}); it was moved to {}", path.display(), aside.display())))
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn dedup(v: &mut Vec<Input>) {
    let mut seen = Vec::new();
    v.retain(|i| {
        if seen.contains(i) {
            false
        } else {
            seen.push(*i);
            true
        }
    });
}

/// Whether every input of a chord is held. An empty chord never is.
pub fn chord_held(chord: &[Input], down: impl Fn(&Input) -> bool) -> bool {
    !chord.is_empty() && chord.iter().all(down)
}

/// A chord's press, as an edge: true on the one poll where any of an
/// action's chords goes from not held to held.
#[derive(Default)]
pub struct Edge {
    was: bool,
}

impl Edge {
    pub fn poll(&mut self, chords: &[Chord], down: impl Fn(&Input) -> bool) -> bool {
        let now = chords.iter().any(|c| chord_held(c, &down));
        let fired = now && !self.was;
        self.was = now;
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_libretro::libretro::{JOY_A, JOY_B, JOY_L, JOY_LEFT, JOY_R, JOY_START, JOY_X, JOY_Y};

    fn nes() -> Controller {
        Controller::Pad(PadLayout::Nes)
    }

    fn snes() -> Controller {
        Controller::Pad(PadLayout::Snes)
    }

    fn ids(r: &ResolvedInput, id: u32) -> &[Input] {
        &r.binds[id as usize]
    }

    /// The built-in mapping is the one the client had before settings: the
    /// keyboard of `keys::joypad`, and `Pads::state` for each layout.
    #[test]
    fn untouched_settings_play_as_before() {
        let s = Settings::default();
        let n = s.resolve_input("mesen", "smb3", nes(), &BTreeMap::new());
        assert_eq!(ids(&n, JOY_A), [Input::Key(b'X' as i32), Input::Pad(PadButton::South), Input::Pad(PadButton::East)]);
        assert_eq!(ids(&n, JOY_B), [Input::Key(b'Z' as i32), Input::Pad(PadButton::West), Input::Pad(PadButton::North)]);
        assert_eq!(ids(&n, JOY_L), [Input::Key(b'Q' as i32)], "the NES layout never mapped the shoulders");
        let sn = s.resolve_input("snes9x", "dkc", snes(), &BTreeMap::new());
        assert_eq!(ids(&sn, JOY_B), [Input::Key(b'Z' as i32), Input::Pad(PadButton::South)]);
        assert_eq!(ids(&sn, JOY_Y), [Input::Key(b'A' as i32), Input::Pad(PadButton::West)]);
        assert_eq!(ids(&sn, JOY_X), [Input::Key(b'S' as i32), Input::Pad(PadButton::North)]);
        assert_eq!(ids(&sn, JOY_L), [Input::Key(b'Q' as i32), Input::Pad(PadButton::Lb)]);
        assert_eq!(ids(&sn, JOY_START), [Input::Key(0x0D), Input::Pad(PadButton::Start)]);
        assert!(n.keyboard.is_none());
    }

    #[test]
    fn pad_button_order_matches_the_adapters() {
        use session_libretro::input::{Button, PAD_BUTTONS};
        assert_eq!(PAD_BUTTONS.len(), PadButton::ALL.len());
        assert_eq!(PAD_BUTTONS[PadButton::South.index()], Button::South);
        assert_eq!(PAD_BUTTONS[PadButton::Lt.index()], Button::LeftTrigger2);
        assert_eq!(PAD_BUTTONS[PadButton::Guide.index()], Button::Mode);
        assert_eq!(PAD_BUTTONS[PadButton::DRight.index()], Button::DPadRight);
    }

    #[test]
    fn a_game_beats_its_core_which_beats_global() {
        let k = Input::Key(b'K' as i32);
        let mut s = Settings::default();
        s.input.keys.insert(RetroButton::A, vec![k]);
        let r = s.resolve_input("snes9x", "dkc", snes(), &BTreeMap::new());
        assert_eq!(ids(&r, JOY_A), [k, Input::Pad(PadButton::East)], "global replaces the default key only");

        s.cores.entry("snes9x".into()).or_default().pad.insert(RetroButton::A, vec![Input::Pad(PadButton::Rt)]);
        let r = s.resolve_input("snes9x", "dkc", snes(), &BTreeMap::new());
        assert_eq!(ids(&r, JOY_A), [k, Input::Pad(PadButton::Rt)]);
        let other = s.resolve_input("mesen", "smb3", nes(), &BTreeMap::new());
        assert_eq!(
            ids(&other, JOY_A),
            [k, Input::Pad(PadButton::South), Input::Pad(PadButton::East)],
            "another core still has global keys, and its own pad layout"
        );

        s.games.entry("dkc".into()).or_default().keys.insert(RetroButton::A, vec![Input::Mouse(MouseButton::Left)]);
        let r = s.resolve_input("snes9x", "dkc", snes(), &BTreeMap::new());
        assert_eq!(ids(&r, JOY_A), [Input::Mouse(MouseButton::Left), Input::Pad(PadButton::Rt)]);
        assert_eq!(s.bind_for("snes9x", "dkc", PadLayout::Snes, RetroButton::A, Family::Keys).1, "game");
        assert_eq!(s.bind_for("snes9x", "dkc", PadLayout::Snes, RetroButton::A, Family::Pad).1, "core");
        assert_eq!(s.bind_for("snes9x", "dkc", PadLayout::Snes, RetroButton::B, Family::Pad).1, "default");
    }

    #[test]
    fn an_empty_binding_is_unbound_not_inherited() {
        let mut s = Settings::default();
        let core = s.cores.entry("mesen".into()).or_default();
        core.keys.insert(RetroButton::Select, vec![]);
        core.pad.insert(RetroButton::Select, vec![]);
        let r = s.resolve_input("mesen", "smb3", nes(), &BTreeMap::new());
        assert!(ids(&r, session_libretro::libretro::JOY_SELECT).is_empty());
    }

    #[test]
    fn a_keyboard_game_takes_only_pad_inputs_and_presses_keys() {
        let mut suggested = BTreeMap::new();
        suggested.insert(RetroButton::B, "lctrl".to_string());
        suggested.insert(RetroButton::Start, "escape".to_string());
        let mut s = Settings::default();
        let doom = |s: &Settings| s.resolve_input("dosbox_pure", "doom", Controller::Keyboard, &suggested);
        let r = doom(&s);
        assert_eq!(ids(&r, JOY_B), [Input::Pad(PadButton::South)], "Z passes through as itself instead");
        s.input.keys.insert(RetroButton::B, vec![Input::Key(b'J' as i32)]);
        assert_eq!(ids(&doom(&s), JOY_B), [Input::Pad(PadButton::South)], "a global key bind leaves its pad alone");
        let kb = r.keyboard.as_ref().unwrap();
        assert!(kb.pad_keys.contains(&(JOY_B, 306)));
        assert!(kb.pad_keys.contains(&(JOY_START, 27)));
        assert!(kb.keys.contains(&(0x57, Some(119))), "w sends w");

        let g = s.games.entry("doom".into()).or_default();
        g.pad_keys.insert(RetroButton::B, "space".into());
        g.pad_keys.insert(RetroButton::Start, String::new());
        g.key_remap.insert("w".into(), "up".into());
        g.key_remap.insert("q".into(), String::new());
        g.mouse_sensitivity = Some(2.0);
        let r = doom(&s);
        let kb = r.keyboard.unwrap();
        assert!(kb.pad_keys.contains(&(JOY_B, 32)));
        assert!(!kb.pad_keys.iter().any(|(b, _)| *b == JOY_START), "unbound on purpose");
        assert!(kb.keys.contains(&(0x57, Some(273))), "w sends up");
        assert!(kb.keys.contains(&(0x51, None)), "q sends nothing");
        assert_eq!(kb.mouse_sensitivity, 2.0);
    }

    #[test]
    fn a_stick_direction_keeps_its_own_key_when_a_button_changes() {
        let mut suggested = BTreeMap::new();
        suggested.insert(RetroButton::R, "period".to_string());
        suggested.insert(RetroButton::Left, "left".to_string());
        let mut s = Settings::default();
        let doom = |s: &Settings| s.resolve_input("dosbox_pure", "doom", Controller::Keyboard, &suggested);
        let left = Input::Stick(Stick::Left, Dir::Left);
        let right = Input::Stick(Stick::Right, Dir::Right);
        assert!(ids(&doom(&s), JOY_LEFT).contains(&left), "until it has a key, a stick goes through the RetroPad");

        let g = s.games.entry("doom".into()).or_default();
        g.pad_keys.insert(RetroButton::R, "space".into());
        g.stick_keys.insert(right, "period".into());
        g.stick_keys.insert(left, "comma".into());
        let r = doom(&s);
        let kb = r.keyboard.as_ref().unwrap();
        assert!(kb.pad_keys.contains(&(JOY_R, 32)), "R sends space");
        assert!(kb.stick_keys.contains(&(right, 46)), "the right stick still sends period");
        assert!(kb.stick_keys.contains(&(left, 44)));
        assert!(!ids(&r, JOY_LEFT).contains(&left), "nor does it press LEFT as well");

        s.games.get_mut("doom").unwrap().stick_keys.insert(left, String::new());
        let r = doom(&s);
        assert!(!r.keyboard.as_ref().unwrap().stick_keys.iter().any(|(i, _)| *i == left), "nothing, on purpose");
        assert!(!ids(&r, JOY_LEFT).contains(&left));
        let dkc = s.resolve_input("snes9x", "dkc", snes(), &BTreeMap::new());
        assert!(ids(&dkc, JOY_LEFT).contains(&left), "another game is untouched");
    }

    #[test]
    fn inputs_read_back_as_written() {
        for s in ["key:z", "key:lctrl", "key:f9", "key:vk60", "mouse:x2", "pad:south", "pad:dup", "stick:right-left"] {
            let i: Input = s.parse().unwrap();
            assert_eq!(i.to_string(), s);
        }
        assert!("pad:triangle".parse::<Input>().is_err());
        assert!("key:".parse::<Input>().is_err());
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let mut s = Settings { name: "Ada".into(), ..Default::default() };
        s.input.keys.insert(RetroButton::A, vec![Input::Key(0x20)]);
        s.input.pad.insert(RetroButton::A, vec![Input::Pad(PadButton::Rt)]);
        s.games.entry("doom".into()).or_default().key_remap.insert("w".into(), "up".into());
        s.games.get_mut("doom").unwrap().stick_keys.insert(Input::Stick(Stick::Right, Dir::Right), "period".into());
        s.video.display = Display::Borderless;
        s.video.cores.insert("mesen".into(), VideoLayer { scaling: Some(Scaling::Integer), ..Default::default() });
        let text = s.to_toml();
        assert_eq!(Settings::from_toml(&text).unwrap(), s, "{text}");
        // And as the page sends it.
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["input"]["pad"]["a"][0], "pad:rt");
        assert_eq!(serde_json::from_value::<Settings>(json).unwrap(), s);
        assert_eq!(Settings::from_toml("").unwrap(), Settings::default());
    }

    #[test]
    fn a_bad_file_is_moved_aside_and_defaults_load() {
        let dir = std::env::temp_dir().join(format!("gt-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(&path, "input = 3").unwrap();
        let (s, warning) = Settings::load(&path);
        assert_eq!(s, Settings::default());
        assert!(warning.unwrap().contains("moved"));
        assert!(!path.exists());
        assert!(dir.join("settings.toml.bad").exists());
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), (Settings::default(), None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_brings_values_into_range() {
        let mut s = Settings::default();
        s.input.deadzone = 3.0;
        s.input.keys.insert(RetroButton::A, vec![Input::Key(1); 9].into_iter().chain((2..9).map(Input::Key)).collect());
        s.input.pad.insert(RetroButton::B, vec![Input::Key(1), Input::Pad(PadButton::South)]);
        s.video.global.window_scale = 40;
        s.stream_kbps = 9;
        s.thumb_kbps = 9000;
        s.games.insert("gone".into(), Layer::default());
        s.games.entry("doom".into()).or_default().pad_keys.insert(RetroButton::B, "f2".into());
        s.sanitize();
        assert_eq!(s.input.deadzone, 0.95);
        assert_eq!(s.input.keys[&RetroButton::A].len(), MAX_SLOTS);
        assert_eq!(s.input.keys[&RetroButton::A][0], Input::Key(1), "duplicates dropped first");
        assert_eq!(s.input.pad[&RetroButton::B], [Input::Pad(PadButton::South)], "a key is not a pad input");
        assert_eq!(s.video.global.window_scale, 6);
        assert_eq!(s.stream_kbps, STREAM_KBPS.0);
        assert_eq!(s.thumb_kbps, THUMB_KBPS.1);
        assert!(s.games.is_empty(), "an empty layer and an unknown key are both dropped");
    }

    #[test]
    fn a_chord_fires_once_when_all_of_it_is_held() {
        let actions = Actions::default();
        let mut edge = Edge::default();
        let held = |set: &'static [Input]| move |i: &Input| set.contains(i);
        assert!(!edge.poll(&actions.pause, held(&[Input::Pad(PadButton::Select)])));
        assert!(edge.poll(&actions.pause, held(&[Input::Pad(PadButton::Select), Input::Pad(PadButton::Start)])));
        assert!(!edge.poll(&actions.pause, held(&[Input::Pad(PadButton::Select), Input::Pad(PadButton::Start)])));
        assert!(!edge.poll(&actions.pause, held(&[])));
        assert!(edge.poll(&actions.pause, held(&[Input::Key(0x79)])));
        assert!(!chord_held(&[], |_| true));
    }

    #[test]
    fn a_core_overrides_global_video() {
        let mut s = Settings::default();
        s.video.global.scaling = Scaling::Integer;
        s.video.cores.insert("dosbox_pure".into(), VideoLayer { window_scale: Some(2), ..Default::default() });
        let v = s.resolve_video("dosbox_pure");
        assert_eq!((v.window_scale, v.scaling), (2, Scaling::Integer));
        assert_eq!(s.resolve_video("mesen").window_scale, 3);
    }
}
