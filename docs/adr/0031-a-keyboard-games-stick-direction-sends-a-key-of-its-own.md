# 0031. A keyboard game's stick direction sends a key of its own

Date: 2026-10-31
Status: Accepted

## Context

ADR 0028 drives a keyboard game's gamepad in two hops: a pad input presses a
RetroPad button, and `pad_keys` says which guest key that button sends. For a
pad *button* the hop is invisible, because each one has a RetroPad button of
the same name and place. A stick direction has none. To send a key, it had to
join some button's list, and then it sent whatever that button sent.

The first hand test found the result. A player changed R (RB) from `.`
(strafe right) to space (use), then went to point the right stick at `.` and
found no button left that sent it. The page offered "R (space)". The stick
had no key of its own, only a share in a button's. In DOOM the RetroPad is
not a machine the game reads. It is sixteen slots between some twenty-four
pad inputs and the keyboard, and a stick direction is left with no slot.

## Decision

**In a keyboard game, a stick direction may send a guest key of its own:**
`stick_keys`, beside `pad_keys` in a game's or a core's layer, keyed by the
direction (`"stick:right-right" = "period"`). The game's beats the core's. An
empty name sends nothing.

**A direction with a key of its own leaves the RetroPad.** Resolution strips
it from every button's binds for that game, so a stick never sends a key and
presses a button too. A direction without one goes through its button as
before, so the left stick still drives DOOM's arrows through UP/DOWN/LEFT/
RIGHT until the player says otherwise. A player who never opens Settings
plays exactly as before.

The page offers keys, not buttons, for a keyboard game's stick directions,
plus "as its button" to go back through the RetroPad.

## Alternatives considered

**Map every pad input of a keyboard game straight to a key, with no RetroPad
at all.** This is the cleaner model for DOOM, but it reopens ADR 0028's layer
order: a global pad bind could no longer reach a keyboard game. It also
leaves a keyboard game with two tables where one does for every button that
has a RetroPad twin. Deferred until a game needs more than sticks.

**Keep sticks in the RetroPad and borrow an unused button.** Point the stick
at L3, say, and set L3 to `.`. This works today, but it is the coupling again
under another name: it uses up the slot a button needed, and nothing on the
page says why L3 sends `.`.

**A manifest suggestion for stick keys.** Not needed. DOOM's suggestion is
the d-pad's, which the left stick already reaches through its buttons.

## Consequences

A pad game is unchanged: its core reads the RetroPad, and a stick direction
there is only ever an input on a button.

If a core's layer sets a stick key, a game under it can no longer route that
direction through a button. "As its button" removes only the game's own
entry. The fix, if anyone needs one, is a "via its button" value in the game
layer.
