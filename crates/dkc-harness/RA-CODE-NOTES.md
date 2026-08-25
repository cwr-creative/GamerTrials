# DKC RAM map, from RetroAchievements code notes

Game 337, fetched 2026-08-24 from the RA connect API:

```sh
curl -s -X POST https://retroachievements.org/dorequest.php \
  --data-urlencode r=codenotes2 --data-urlencode g=337
```

No credentials required — `codenotes2` takes only a game id (`rc_api_editor.c:17`).
RA's SNES address space maps 1:1 onto WRAM (`consoleinfo.c:887`): RA address *N*
is `$7E0000 + N`, so every address below is already in the form `--addr` and
`--watch` accept.

These are notes written by achievement authors against running code, not wiki
transcriptions. They are evidence, not proof: the `?` marks are theirs.

## General map

| Address | Note | Author |
|---|---|---|
| `$7E0002` | 8-bit -> Overworld Screen ID | Bartis1989 |
| `$7E0004` | Music? | JAM |
| `$7E0008` | Music? | thatdude1286 |
| `$7E000A` | Music! | JAM |
| `$7E001C` | 16-bit -> Might be Color Pallette handler | Bartis1989 |
| `$7E0028` | [2 Byte] Frame Counter | SamuraiGoroh |
| `$7E002A` | [2 Byte] Stage Timer? (Continues when pausing, stops in world map) | SamuraiGoroh |
| `$7E002E` | Room ID to start from (changes after Star Barrel) | JAM |
| `$7E003E` | Room ID (last exit used) | SamuraiGoroh |
| `$7E0040` | Last Room ID inside a level (else 0) | SamuraiGoroh |
| `$7E0042` | Game Mode (00: One Player, 01: Two Player Team, 02: Two Player Contest) | SamuraiGoroh |
| `$7E0046` | [2 Byte] Frames (Up to 0x0E0F or 3599) | SamuraiGoroh |
| `$7E0048` | [2 Byte] Minutes (Up to 0x176F or 99:59) | SamuraiGoroh |
| `$7E005E` | [2 Byte] Character's X coord | SamuraiGoroh |
| `$7E0060` | [2 Byte] Character's Y coord | GalacticSpear |
| `$7E00BE` | [16-Bits] Camera - X coord | GalacticSpear |
| `$7E00C0` | [16-Bits] Camera - Y coord | GalacticSpear |
| `$7E00C4` | 0 when animal bous time ends | JAM |
| `$7E0501` | player inputs b0 = right b1 = left b2 = down b3 = up b4 = start b5 = select b6 = Y b7 = B | Thoreau |
| `$7E0514` | " | Lucoshi |
| `$7E0516` | Riding Buddy? No=0, Rambi=9, Expresso=0a, Winky=0b, Enguarde=0c, Bonus doesn't affect, Persists on Level Clear, resets on Start/Select or death | ShadwSonic |
| `$7E051A` | screen fade-in/fade-out 00 = dark 0f = full view | Thoreau |
| `$7E051B` | 82 = Screen is Fading-Out | GalacticSpear |
| `$7E0521` | Music track selected | JAM |
| `$7E0523` | Music track playing | JAM |
| `$7E0527` | 00=In a Stage 01=In Overworld | GalacticSpear |
| `$7E0529` | Bananas | SamuraiGoroh |
| `$7E052B` | Banana/Mini Animal Token Display Digit: 00X | SamuraiGoroh |
| `$7E052C` | Banana/Mini Animal Token Display Digit: 0X0 | SamuraiGoroh |
| `$7E052D` | Mini Animal Token Display Digit: X00 | SamuraiGoroh |
| `$7E0563` | Stage ID? | matheus2653 |
| `$7E0565` | Stage's Room ID: 16 = Universal main room of the stage (By entering the level) 00 = Universal main room of the stage (By returning from a bonus) | GalacticSpear |
| `$7E056B` | Cheat Mode flags: b0=Bad Buddy, b1=Dyddy, b2=Barral, b3=Darby Day | SamuraiGoroh |
| `$7E056F` | 1=DK, 2=Diddy | dude1286 |
| `$7E0571` | Winky - Frog Token (Up to 3) | SamuraiGoroh |
| `$7E0572` | Expresso - Ostrich Token | SamuraiGoroh |
| `$7E0573` | Rambi - Rhino Token | SamuraiGoroh |
| `$7E0574` | Engarde - Swordfish Token | SamuraiGoroh |
| `$7E0575` | [2 Byte] Lives | SamuraiGoroh |
| `$7E0577` | [2 Byte] Lives (Display) | SamuraiGoroh |
| `$7E0579` | State: b0=2 Kongs, b1=Invulnerable?, b2=?, b3=Inside Barrel/Bouncing?, b4=?, b5=?, b6=Pause, b7=Launched from barrel? | SamuraiGoroh |
| `$7E057B` | Bananas / Mini Animal Tokens (Up to 0x03E7 or 999) | SamuraiGoroh |
| `$7E057F` | KONG letters flag | SamuraiGoroh |
| `$7E0581` | 8-bit -> Cursor at the "Select a game" screen | Bartis1989 |
| `$7E0D4B` | Object ID _3 | GalacticSpear |
| `$7E0D4D` | Object ID _4 | GalacticSpear |
| `$7E0D4F` | Object ID _5 | GalacticSpear |
| `$7E0D51` | Object ID _6 | GalacticSpear |
| `$7E0D53` | Object ID _7 | GalacticSpear |
| `$7E0D55` | Object ID _8 | GalacticSpear |
| `$7E0D57` | Object ID _9 | GalacticSpear |
| `$7E0D59` | Object ID _10 | GalacticSpear |
| `$7E0D5B` | Object ID _11 | GalacticSpear |
| `$7E0D5D` | Object ID _12 | GalacticSpear |
| `$7E0D5F` | Object ID _13 | GalacticSpear |
| `$7E0D61` | Object ID _14 | GalacticSpear |
| `$7E0FC3` | [16-Bits] DK's Position X | GalacticSpear |
| `$7E0FC5` | [16-Bits] Diddy's Position X | GalacticSpear |
| `$7E0FF7` | [16-Bits] DK's Position Y | GalacticSpear |
| `$7E0FF9` | [16-Bits] Diddy's Position Y | GalacticSpear |
| `$7E102F` | 01 = Object _3 Defeated | GalacticSpear |
| `$7E1031` | 01 = Object _4 Defeated | GalacticSpear |
| `$7E1033` | 01 = Object _5 Defeated | GalacticSpear |
| `$7E1035` | 01 = Object _6 Defeated | GalacticSpear |
| `$7E1037` | 01 = Object _7 Defeated | GalacticSpear |
| `$7E1039` | 01 = Object _8 Defeated | GalacticSpear |
| `$7E103B` | 01 = Object _9 Defeated | GalacticSpear |
| `$7E103D` | 01 = Object _10 Defeated | GalacticSpear |
| `$7E103F` | 01 = Object _11 Defeated | GalacticSpear |
| `$7E1041` | 01 = Object _12 Defeated | GalacticSpear |
| `$7E1043` | 01 = Object _13 Defeated | GalacticSpear |
| `$7E1045` | 01 = Object _14 Defeated | GalacticSpear |
| `$7E1049` | 01 = Object _15 Defeated(?) | GalacticSpear |
| `$7E1175` | Queen Bee's Health | dude1286 |
| `$7E137F` | Inside Bonus Game - Timer | SamuraiGoroh |
| `$7E13F3` | Sprite - Bonus Stage Timer (Hex = Dec) | SamuraiGoroh |
| `$7E1501` | Master Necky's Health | dude1286 |
| `$7E1503` | Very Gnawty and Really Gnawty's Health | dude1286 |
| `$7E16D1` | [2 Byte] Timer: Donkey flashes (when taking hit) | SamuraiGoroh |
| `$7E16D3` | [2 Byte] Timer: Diddy flashes (when taking hit) | SamuraiGoroh |
| `$7E16D5` | [2 Byte] Timer: Donkey's Invulnerability | SamuraiGoroh |
| `$7E16D7` | [2 Byte] Timer: Diddy's Invulnerability | SamuraiGoroh |
| `$7E16E1` | [2 Byte] DK - Distance walked?! | SamuraiGoroh |
| `$7E16E3` | [2 Byte] Diddy - Distance walked?! | SamuraiGoroh |
| `$7E180F` | Character's State= 0=Idle, walking, running, 1=Jumping, 2=Grabbing/Throwing barrel, 4=Bouncing from unkillable enemy, 6=Rolling, 7=Riding Steel Barrel, 8=Ducking, 9=Climbing, A=Swimming, ... | SamuraiGoroh |
| `$7E191F` | Player's Animations: 0x03 = Walking | GalacticSpear |
| `$7E1AE9` | Related to Sprites on Screen (Used for the DK Barrel in the "Why Not Both?" cheevo) | GalacticSpear |
| `$7E1DF3` | Weather: 00=Sunny Day, 01=Rainy Day, 02=Night, 03=Rainy Night, 04=Lightning, 07=Rain and Lightning | SamuraiGoroh |
| `$7E1E13` | Blue balloon timer (4-1) | JAM |
| `$7E1E17` | Enemies stomped in a row | SamuraiGoroh |
| `$7E7A06` | Candy's Game Save - Seconds | SamuraiGoroh |
| `$7E7A07` | Candy's Game Save - Minutes | SamuraiGoroh |
| `$7E7A08` | Candy's Game Save - Hours | SamuraiGoroh |
| `$7E7A0A` | [16-Bit] Candy's Game Save - Percentage Note found by SamuraiGoroh | TheJediSonic |

## Per-stage flags

One address per named stage, contiguous from `$7E0584`. Authored piecemeal by
three people, so the block is probably complete even where a name is missing.

| Address | Stage |
|---|---|
| `$7E0584` | Reptile Rumble |
| `$7E058A` | Bouncy Bonanza |
| `$7E058D` | Misty Mine |
| `$7E058F` | Ropey Rampage |
| `$7E0590` | Orang-utan Gang |
| `$7E0595` | Manic Mincers |
| `$7E0597` | Torchlight Trouble |
| `$7E0599` | Jungle Hijinx |
| `$7E059A` | Barrel Cannon Canyon |
| `$7E059B` | Elevator Antics |
| `$7E05A5` | Poison Pond |
| `$7E05A7` | Snow Barrel Blast |
| `$7E05AA` | Mine Cart Madness |
| `$7E05AE` | Platform Perils |
| `$7E05B1` | Mine Cart Carnage |
| `$7E05B2` | Trick Track Trek |
| `$7E05B3` | Tanked Up Trouble |
| `$7E05B4` | Stop and Go Station |
| `$7E05B9` | Loopy Lights |
| `$7E05C1` | Croctopus Chase |
| `$7E05C3` | Oil Drum Alley |
| `$7E05C4` | Blackout Basement |
| `$7E05C5` | Millstone Mayhem |
| `$7E05C6` | Temple Tempest |
| `$7E05EB` | Gang-Plank Galleon |
| `$7E05F0` | Slipslide Ride |
| `$7E0627` | Tree Top Town |
| `$7E0628` | Vulture Culture |
| `$7E062A` | Ice Age Alley |
| `$7E0642` | Coral Capers? |
| `$7E0651` | Rope Bridge Rumble |
| `$7E0653` | Forest Frenzy |
| `$7E065C` | Winky's Walkway |
| `$7E0661` | Clam City |
| `$7E0663` | Really Gnawty |
| `$7E0664` | Necky's Nuts |
| `$7E0665` | Really Gnawty Rampage |
| `$7E0666` | Boss Dumb Drum |
| `$7E0667` | Necky's Revenge |
| `$7E0668` | Bumble B rumble |
