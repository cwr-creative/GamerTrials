//! DKC's level-code table: what the byte at `$7E003E` means.
//!
//! # Where this came from
//!
//! Transcribed from the RainbowZ Editor (`RainbowZ-Editor-DKC_Editor_0.557`),
//! a DKC1 ROM editor that targets the same US 1.0 ROM this harness does. The
//! editor carries the table twice — `Global.cs` as `levelStringRaw`, and
//! `ROM.Level.cs` as `nameByLevelCode` — and in both the *array index is the
//! level code*, i.e. the value the game keeps at `$7E003E`.
//!
//! That the index is the `$3E` value is not an inference. `DKCLevel.cs`
//! transcribes the game's own bound-lookup routine with its CPU trace intact:
//!
//! ```text
//! //$BC/B052 A5 3E       LDA $3E[$00:003E]
//! int tempX = levelCode << 1;
//! tempX = rom.Read16(0xbc8000 + tempX) - 4;
//! ```
//!
//! so `levelCode` and `$7E003E` are the same number. This is also what upgrades
//! the level-id address itself from "a wiki said so" to "read off working code".
//!
//! # Which of the two tables this is, and why
//!
//! `ROM.Level.cs`. The two disagree on exactly 33 entries, and only on those:
//! `Global.cs` labels them `(level exit)` where `ROM.Level.cs` labels them
//! `(level start)`. Every bonus-room entry is byte-identical in both.
//!
//! `(level start)` is taken as correct because every sibling entry in the table
//! is an *entrance* variant — `(from Save)`, `(from Bonus 1)`, `(from Winky
//! Room)`, `(from treehouse)`. The code says which level you are in and how you
//! got there; it does not say the level was finished. Nothing here depends on
//! that reading being right, because the completion rule keys on recognising
//! the rooms you can come *back* from, not on believing an "exit" label.
//!
//! # What it is used for
//!
//! `$7E051B` rising to `0x82` says the stage is being left, and never fires on
//! a death — but it fires identically for the exit and for a bonus barrel. The
//! destination level code is what tells those apart, and unlike counting fades
//! it does not care which route the player took. See `dkc::Watcher::tick`.

/// Level name by code, indexed by the value at `$7E003E`. A code past the end
/// of this table is a real possibility (the overworld map is not in here), and
/// is treated as "not part of any stage" — which is exactly right for it.
pub const LEVEL_NAMES: [&str; 230] = [
    "Jungle Hijinx (Reptile Rumble object map?)", // 0x00
    "Reptile Rumble (level start)", // 0x01
    "Reptile Rumble - Bonus 1", // 0x02
    "Bouncy Bonanza - Winky Room", // 0x03
    "Reptile Rumble - Bonus 3", // 0x04
    "Manic Mincers - Bonus 1", // 0x05
    "Jungle Hijinx - Bonus 1", // 0x06
    "Bouncy Bonanza (level start)", // 0x07
    "Jungle Hijinx (from Bonus 1)", // 0x08
    "Reptile Rumble (from Bonus 1)", // 0x09
    "Misty Mine (level start)", // 0x0A
    "Reptile Rumble (from Bonus 3)", // 0x0B
    "Ropey Rampage (level start)", // 0x0C
    "Orang-utan Gang (level start)", // 0x0D
    "Jungle Hijinx (from treehouse)", // 0x0E
    "Ropey Rampage (from Save)", // 0x0F
    "Bouncy Bonanza (from Winky Room)", // 0x10
    "Bouncy Bonanza - Bonus 2", // 0x11
    "Manic Mincers (level start)", // 0x12
    "Torchlight Trouble (from Save)", // 0x13
    "Torchlight Trouble (level start)", // 0x14
    "Bouncy Bonanza (from Save)", // 0x15
    "Jungle Hijinx (level start)", // 0x16
    "Barrel Cannon Canyon (level start)", // 0x17
    "Elevator Antics (level start)", // 0x18
    "Barrel Cannon Canyon (from Save)", // 0x19
    "Jungle Hijinx - Bonus 2", // 0x1A
    "Ropey Rampage - Bonus 2", // 0x1B
    "Ropey Rampage - Bonus 1", // 0x1C
    "Orang-utan Gang (from Save)", // 0x1D
    "Orang-utan Gang - Bonus 3", // 0x1E
    "Orang-utan Gang - Bonus 2", // 0x1F
    "Orang-utan Gang - Bonus 1", // 0x20
    "empty jungle room + boss music???", // 0x21
    "Poison Pond (level start)", // 0x22
    "Elevator Antics (from Save)", // 0x23
    "Snow Barrel Blast (level start)", // 0x24
    "Jungle Hijinx (from Save)", // 0x25
    "Reptile Rumble (from Save)", // 0x26
    "Mine Cart Madness (level start)", // 0x27
    "Snow Barrel Blast (from Save)", // 0x28
    "Manic Mincers (from Save)", // 0x29
    "Poison Pond (from Save)", // 0x2A
    "Platform Perils (level start)", // 0x2B
    "Platform Perils (from Save)", // 0x2C
    "Misty Mine (from Save)", // 0x2D
    "Mine Cart Carnage (level start)", // 0x2E
    "Trick Track Trek (level start)", // 0x2F
    "Tanked Up Trouble (level start)", // 0x30
    "Stop & Go Station (level start)", // 0x31
    "Misty Mine - Bonus 2", // 0x32
    "Misty Mine - Bonus 1", // 0x33
    "Animal Token Room", // 0x34
    "Millstone Mayhem (from warp)", // 0x35
    "Loopy Lights (level start)", // 0x36
    "Loopy Lights - Bonus 2", // 0x37
    "Mine Cart Carnage (from Save)", // 0x38
    "Trick Track Trek (from Save)", // 0x39
    "Tanked Up Trouble (from Save)", // 0x3A
    "Mine Cart Madness (from Save)", // 0x3B
    "Stop & Go Station (from Save)", // 0x3C
    "Loopy Lights (from Save)", // 0x3D
    "Croctopus Chase (level start)", // 0x3E
    "Croctopus Chase (from Save)", // 0x3F
    "Oil Drum Alley (level start)", // 0x40
    "Blackout Basement (level start)", // 0x41
    "Millstone Mayhem (level start)", // 0x42
    "Temple Tempest (level start)", // 0x43
    "Oil Drum Alley (from Save)", // 0x44
    "Blackout Basement (from Save)", // 0x45
    "Barrel Cannon Canyon - Bonus 1", // 0x46
    "Jungle Hijinx - Kong's Banana Hoard (empty)", // 0x47
    "Reptile Rumble - Bonus 2", // 0x48
    "Loopy Lights - Bonus 1", // 0x49
    "Stop & Go Station - Bonus 2", // 0x4A
    "Stop & Go Station - Bonus 1", // 0x4B
    "Jungle Hijinx - Kong's Banana Hoard (full)", // 0x4C
    "Mine Cart Madness - Bonus 1", // 0x4D
    "Platform Perils - Bonus 1", // 0x4E
    "Winky's Walkway - Bonus 1", // 0x4F
    "Platform Perils - Bonus 2", // 0x50
    "Winky's Walkway (from Bonus 1)", // 0x51
    "Temple Tempest - Bonus 1", // 0x52
    "Temple Tempest - Bonus 2", // 0x53
    "Tree Top Town (warp)", // 0x54
    "Millstone Mayhem - Bonus 1", // 0x55
    "Millstone Mayhem - Bonus 2", // 0x56
    "Millstone Mayhem - Bonus 3", // 0x57
    "Millstone Mayhem (from Save)", // 0x58
    "Temple Tempest (from Save)", // 0x59
    "Orang-utan Gang - Bonus 5", // 0x5A
    "Orang-utan Gang - Bonus 4", // 0x5B
    "Jungle Hijinx - Kong's Cabin", // 0x5C
    "Barrel Cannon Canyon - Bonus 2", // 0x5D
    "Credits", // 0x5E
    "Jungle Hijinx (from Hoard)", // 0x5F
    "Oil Drum Alley - Bonus 4", // 0x60
    "Oil Drum Alley - Bonus 2", // 0x61
    "Slipslide Ride (warp)", // 0x62
    "Oil Drum Alley - Bonus 1", // 0x63
    "Blackout Basement - Bonus 1", // 0x64
    "Vulture Culture (warp)", // 0x65
    "Snow Barrel Blast - Bonus 3", // 0x66
    "Snow Barrel Blast - Bonus 1", // 0x67
    "Gangplank Galleon", // 0x68
    "Snow Barrel Blast - Bonus 2", // 0x69
    "Ice Age Alley - Bonus 1", // 0x6A
    "Ice Age Alley - Bonus 2", // 0x6B
    "Expresso Bonus", // 0x6C
    "Slipslide Ride (level start)", // 0x6D
    "Jungle Hijinx (from Bonus 2)", // 0x6E
    "Ropey Rampage (from Bonus 1)", // 0x6F
    "Ropey Rampage (from Bonus 2)", // 0x70
    "Orang-utan Gang (from Bonus 4)", // 0x71
    "Orang-utan Gang (from Bonus 2)", // 0x72
    "Orang-utan Gang (from Bonus 1)", // 0x73
    "Orang-utan Gang (from Bonus 3)", // 0x74
    "Orang-utan Gang (from Bonus 5)", // 0x75
    "Barrel Cannon Canyon (from Bonus 1)", // 0x76
    "Barrel Cannon Canyon (from Bonus 2)", // 0x77
    "Bouncy Bonanza (from Bonus 1)", // 0x78
    "Bouncy Bonanza (from Bonus 2)", // 0x79
    "Manic Mincers (from Bonus 1)", // 0x7A
    "Manic Mincers (from Ledge Room)", // 0x7B
    "Manic Mincers (from Bonus 2)", // 0x7C
    "Elevator Antics (from Bonus 1)", // 0x7D
    "Elevator Antics (from Bonus 2)", // 0x7E
    "Elevator Antics (from Bonus 3)", // 0x7F
    "Misty Mine (from Bonus 1)", // 0x80
    "Misty Mine (from Bonus 2)", // 0x81
    "Stop & Go Station (from Bonus 1)", // 0x82
    "Stop & Go Station (from Bonus 2)", // 0x83
    "Loopy Lights (from Bonus 1)", // 0x84
    "Loopy Lights (from Bonus 2)", // 0x85
    "Platform Perils (from Bonus 1)", // 0x86
    "Platform Perils (from Bonus 2)", // 0x87
    "Trick Track Trek (from Bonus 1)", // 0x88
    "Trick Track Trek (from Bonus 3)", // 0x89
    "Trick Track Trek (from Bonus 2)", // 0x8A
    "Tanked Up Trouble (from Bonus 1)", // 0x8B
    "Mine Cart Madness (from Bonus 1)", // 0x8C
    "Mine Cart Madness (from Bonus 2)", // 0x8D
    "Mine Cart Madness (from Bonus 3)", // 0x8E
    "Oil Drum Alley (from Bonus 1)", // 0x8F
    "Oil Drum Alley (from Bonus 2/3)", // 0x90
    "Oil Drum Alley (from Bonus 4)", // 0x91
    "Blackout Basement (from Bonus 1)", // 0x92
    "Blackout Basement (from Bonus 2)", // 0x93
    "Snow Barrel Blast (from Bonus 1)", // 0x94
    "Snow Barrel Blast (from Bonus 2)", // 0x95
    "Snow Barrel Blast (from Bonus 3)", // 0x96
    "Bouncy Bonanza - Bonus 1", // 0x97
    "Manic Mincers - Bonus 2", // 0x98
    "Manic Mincers - Ledge Room", // 0x99
    "Elevator Antics - Bonus 1", // 0x9A
    "Elevator Antics - Bonus 2", // 0x9B
    "Elevator Antics - Bonus 3", // 0x9C
    "Trick Track Trek - Bonus 3", // 0x9D
    "Trick Track Trek - Bonus 2", // 0x9E
    "Tanked Up Trouble - Bonus 1", // 0x9F
    "Mine Cart Madness - Bonus 2", // 0xA0
    "Trick Track Trek - Bonus 1", // 0xA1
    "Mine Cart Madness - Bonus 3", // 0xA2
    "Blackout Basement - Bonus 2", // 0xA3
    "Tree Top Town (level start)", // 0xA4
    "Vulture Culture (level start)", // 0xA5
    "Enguarde Bonus", // 0xA6
    "Ice Age Alley (level start)", // 0xA7
    "Ice Age Alley (from Save)", // 0xA8
    "Tree Top Town (from Save)", // 0xA9
    "Vulture Culture (from Save)", // 0xAA
    "Slipslide Ride (from Save)", // 0xAB
    "Ice Age Alley (from Bonus 1)", // 0xAC
    "Ice Age Alley (from Bonus 2)", // 0xAD
    "Millstone Mayhem (from Bonus 1)", // 0xAE
    "Millstone Mayhem (from Bonus 2)", // 0xAF
    "Millstone Mayhem (from Bonus 3)", // 0xB0
    "Temple Tempest (from Bonus 1)", // 0xB1
    "Temple Tempest (from Bonus 2)", // 0xB2
    "Tree Top Town - Bonus 2", // 0xB3
    "Tree Top Town - Bonus 1", // 0xB4
    "Tree Top Town (from Bonus 1)", // 0xB5
    "Tree Top Town (from Bonus 2)", // 0xB6
    "Vulture Culture - Bonus 1", // 0xB7
    "Vulture Culture - Bonus 2", // 0xB8
    "Vulture Culture - Bonus 3", // 0xB9
    "Vulture Culture (from Bonus 1)", // 0xBA
    "Vulture Culture (from Bonus 2)", // 0xBB
    "Vulture Culture (from Bonus 3)", // 0xBC
    "Trick Track Trek (warp)", // 0xBD
    "Oil Drum Alley - Bonus 3", // 0xBE
    "Coral Capers (level start)", // 0xBF
    "Coral Capers (from Save)", // 0xC0
    "Torchlight Trouble - Bonus 1", // 0xC1
    "Torchlight Trouble (from Bonus 1)", // 0xC2
    "Torchlight Trouble - Bonus 2", // 0xC3
    "Torchlight Trouble (from Bonus 2)", // 0xC4
    "Slipslide Ride - Bonus 2", // 0xC5
    "Slipslide Ride - Bonus 3", // 0xC6
    "Slipslide Ride (from Bonus 1)", // 0xC7
    "Slipslide Ride (from Bonus 2)", // 0xC8
    "Reptile Rumble (from Bonus 2)", // 0xC9
    "Slipslide Ride - Bonus 1", // 0xCA
    "Slipslide Ride (from Bonus 3)", // 0xCB
    "Mine Cart Carnage (warp)", // 0xCC
    "Stop & Go Station (warp)", // 0xCD
    "Rope Bridge Rumble (level start)", // 0xCE
    "Rope Bridge Rumble (from Save)", // 0xCF
    "Forest Frenzy (level start)", // 0xD0
    "Forest Frenzy (from Save)", // 0xD1
    "Rambi Bonus", // 0xD2
    "Winky Bonus", // 0xD3
    "Forest Frenzy - Bonus 2", // 0xD4
    "Rope Bridge Rumble - Bonus 1", // 0xD5
    "Rope Bridge Rumble (from Bonus 2)", // 0xD6
    "Rope Bridge Rumble - Bonus 2", // 0xD7
    "Rope Bridge Rumble (from Bonus 1)", // 0xD8
    "Winky's Walkway (level start)", // 0xD9
    "Winky's Walkway (from Save)", // 0xDA
    "Forest Frenzy (from Bonus 2)", // 0xDB
    "Forest Frenzy - Bonus 1", // 0xDC
    "Forest Frenzy (from Bonus 1)", // 0xDD
    "Clam City (level start)", // 0xDE
    "Clam City (from Save)", // 0xDF
    "Very Gnawty's Lair", // 0xE0
    "Necky's Nuts", // 0xE1
    "Really Gnawty Rampage", // 0xE2
    "Boss Dumb Drum", // 0xE3
    "Necky's Revenge", // 0xE4
    "Bumble B Rumble", // 0xE5
];

/// The name for a level code, or `None` for a code outside the table.
pub fn name(code: u16) -> Option<&'static str> {
    LEVEL_NAMES.get(usize::from(code)).copied()
}

/// The stage a level code belongs to: the name with its entrance or sub-room
/// qualifier stripped, so every way of being in Jungle Hijinx — the start, the
/// checkpoint, its two bonus rooms, the Banana Hoard — answers `Jungle Hijinx`.
///
/// This is what makes leaving a stage detectable without knowing the route.
/// The player wanders between codes inside one family for the whole attempt;
/// the family changing is the stage actually ending.
pub fn family(code: u16) -> Option<&'static str> {
    let n = name(code)?;
    // " - " introduces a sub-room ("Jungle Hijinx - Bonus 1"), " (" an
    // entrance ("Jungle Hijinx (from Save)"). Whichever comes first ends the
    // stage name.
    let end = match (n.find(" - "), n.find(" (")) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => n.len(),
    };
    Some(n[..end].trim())
}

/// Whether a code is a room the player can enter mid-stage and come back from.
///
/// Family alone very nearly settles this — a level's own bonus rooms are named
/// after it and so share its family. The exception is the shared animal-token
/// rooms (`Winky Bonus`, `Rambi Bonus`, `Expresso Bonus`, `Enguarde Bonus`,
/// `Animal Token Room`), which belong to no level because several levels lead
/// to them. Without this they would read as a family change, and a token room
/// would be scored as a clear — the same class of false positive the fade
/// counting had, just rarer and harder to notice.
pub fn is_side_room(code: u16) -> bool {
    match name(code) {
        Some(n) => {
            n.contains("Bonus")
                || n.contains("Animal Token Room")
                || n.contains("Winky Room")
                || n.contains("Banana Hoard")
                || n.contains("Kong's Cabin")
        }
        None => false,
    }
}

/// Renders a code for logging: `0x16 Jungle Hijinx (level start)`, or just the
/// number when the table does not cover it.
pub fn show(code: u16) -> String {
    match name(code) {
        Some(n) => format!("0x{code:02X} {n}"),
        None => format!("0x{code:02X} <not in the level table>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spot-checks against the editor's table. These indices are the whole
    /// reason this file exists, so a transcription slip has to fail loudly.
    #[test]
    fn known_codes_have_their_documented_names() {
        assert_eq!(name(0x16), Some("Jungle Hijinx (level start)"));
        assert_eq!(name(0x06), Some("Jungle Hijinx - Bonus 1"));
        assert_eq!(name(0x08), Some("Jungle Hijinx (from Bonus 1)"));
        assert_eq!(name(0x25), Some("Jungle Hijinx (from Save)"));
        assert_eq!(name(0x01), Some("Reptile Rumble (level start)"));
        assert_eq!(LEVEL_NAMES.len(), 230);
    }

    #[test]
    fn a_code_past_the_table_is_not_a_stage() {
        assert_eq!(name(230), None);
        assert_eq!(family(230), None);
        assert!(!is_side_room(230));
    }

    /// Every way of being inside Jungle Hijinx answers the same family. This
    /// is the property the completion rule rests on.
    #[test]
    fn every_entrance_and_sub_room_shares_one_family() {
        for code in [0x06, 0x08, 0x0E, 0x16, 0x1A, 0x25, 0x47, 0x4C, 0x5C, 0x5F, 0x6E] {
            assert_eq!(family(code), Some("Jungle Hijinx"), "code 0x{code:02X}");
        }
        assert_eq!(family(0x01), Some("Reptile Rumble"));
        assert_ne!(family(0x16), family(0x01));
    }

    /// A level's own bonus rooms share its family, so family alone would carry
    /// them — but the shared animal rooms do not, and they are the reason
    /// `is_side_room` exists at all.
    #[test]
    fn shared_animal_rooms_are_side_rooms_despite_their_own_family() {
        for code in [0x34, 0x6C, 0xA6, 0xD2, 0xD3] {
            assert!(is_side_room(code), "{} should be a side room", show(code));
        }
        // ...and a real stage is not one.
        assert!(!is_side_room(0x16));
        assert!(!is_side_room(0x01));
    }
}
