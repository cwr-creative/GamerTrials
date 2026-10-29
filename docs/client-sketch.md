## Main Menu

On launch, there is a simple main menu featuring a large art piece that is random every time the user starts the program. There are three options on the main menu: 'Join game', 'Host game', and 'Settings'.

- **Join Game** — Reveals a text field for a player to paste an invite code. On connection, takes the user to the associated lobby.
- **Host Game** — Creates a lobby and takes the user there.
- **Settings** — Where the user sets up their keybinds, control surfaces, and video settings for all available emulators. They also choose their display name.

## Settings

Design TBD.

## Lobby

A **match** is a series of challenges. This lobby is where all the details of the match (types of challenges, types of games) will be decided. This menu is a series of toggles/customizable variables that populates/depopulates based on the host's selections. It's designed so that the host may 'drill down' and get as granular as they want, expanding collapsible menus and sub-menus to toggle on/off various categories, emulated systems, games, levels, and challenges -- but also functions at a high level if the host only wants to toggle a handful of high-level options.

### Options

- **Match Mode** — The overall style of the match. There are three options. Depending on which is selected, menus below it will change. For instance, if 'Time Trial' is selected, various fixed-scrolling games will be automatically removed from the lists below, as well as fixed-scrolling stages from other games. The default is survival.
	- **Time Trial** — Deals only time trial challenges.
	- **Survival** — Deals only survival challenges.
	- **Combo** — Deals both challenge types.
- **Match Settings** — Settings that determine finer details related to scoring and available challenges.
	- **Duration** — How the length of the match is determined. The host can select one of the following:
		- **Number of challenges** — Host enters a certain number of challenges. Players will play through that number untill it's completion.
			- In 'Time Trial' mode, players' ranks are determined by their total frames across the entire match -- fewer total frames means higher placement.
			- In 'Survival' mode, each challenge is pass/fail. At the end, players are ranked by how many challenges they completed (a survival challenge ends on death or on the `ends_attempt_on` criteria from the manifest).
			- In 'Combo' mode, time trial challenges award placement points. Each player receives a point for the number of players they beat (completed faster than) Survival challenges also award placement points -- each player who completed the challenge scores a number of points equal to the number of players who failed. Placement points are totaled at the end of the match to find the final ranking (more points means a higher rank).
		- **Number of strikes** — The host enters the number of consecutive failures that will eliminate a player from the match. When a player is eliminated, they are knocked out of the running and can only spectate. Last player standing wins.
			- In 'Time Trial' mode, finishing last is considered a strike (a failure).
			- In 'Survival' mode, a strike is failing to complete a challenge. Multiple players could be eliminated by one challenge.
			- 'Combo' mode combines the above. Finishing last is a strike for time trial challenges, and failure to complete is a strike for survival challenges. 
		- **Number of wins** — The host enters a number. The first player to 'succeed' that number of times wins.
			- In 'Time Trial' mode, a success means finishing first.
			- In 'Survival' mode, it means completing the challenge.
			- In 'Combo' mode, use the above definitions. 
		- **Stock** — Players have a set number of 'lives'. Running out of lives means being eliminated from the match.
			- In 'Time Trial' mode, finishing last loses a stock.
			- In 'Survival' mode, failing to complete a challenge loses a stock.
			- 'Combo' mode combines the above.
	- **Allow head-to-head challenges** — Enables PvP challenges via netplay (fighting games, 2P games, etc.). Currently not coded. Default would be toggled 'off'.
	- **Allow custom challenges** — Custom challenges refer to those that use a rom hack or generated content (anything that isn't vanilla). Toggling this off would remove them from the pool. By default, this is toggled 'on'.
- **Games** — Nested collapsible lists that show which categories/games/challenges are available. This should be populated via whatever is available from the manifests, minus those which aren't applicable due to the match mode/settings.
	- **Categorization Method** — Host can choose 'by era', 'by genre', or 'by system'.
		- **Category** — The highest level list item. For instance, if the method were 'by system', this might include 'SNES', 'NES', 'Sega Genesis', etc. Each item has a tri-state checkbox which shows wether all the children are on, off, or indeterminate (some on and some off). Clicking on these items will cycle between on/off and (if currently indeterminate) turn all children on.
			- **Title** — Alphabetical lists of games. Again these have tri-state checkboxes. Turning some of them off will turn the parent category to indeterminate.
				- **Challenges** — These are standard checboxes, as there are no children below this. These are the specific challenges and those that are toggled 'on' elligbile for dealing. Toggling some of these off would turn the parent item (the game title) to indeterminate. Each item here should list the challenge title and the completion criteria.

### Player list

Players who are currently in the lobby should be listed at the bottom right. A check should be continually run that each player has the correct roms, manifests, and savestates needed to play the **current** match. If they are missing a few items, the host should be able to toggle those titles/challenges off, allowing them to play. Missing items should be listed with their hash and filename. 

### Start button

No complicated 'ready' states are needed. If all players have the required files, the match begins as soon as the host presses start.

## Gameplay

There will be various full-screen animated graphics and sound effects that play over the emulation. These will require transparency effects, as they will animate on/off screen in intricate ways. Placeholders should be used for now, which will be crude but convey the necessary meaning. They should be designed such that they can be easily replaced with final animated assets.

### Challenge intros

Before each challenge, a full-screen intro graphic will animate into place. It will show the title of the game, the name of the system (SNES, NES, etc), and the 'goal text'. Goal text is the completion criteria, but in plain english (e.g. "Survive for thirty seconds!"). The manifest schema should include way to define this text per challenge. If a manifest fails to define this text, the client should be coded to translate the completion criteria into something understandable. For something like `complete_when = { enemies_defeated = 5 }`, this is pretty easy. For something like `complete_when = { counts = { score_award = 4 } }`, this will be vague (but better than nothing).

The fullscreen graphic will animate away, revealing the game session, which is paused. A countdown animation will then play, counting down from 3, and starting the gameplay when it hits 0.

### Death and outros

Whenever a player dies (regardless of whether it's during a time trial where gameplay continues or a survival match where they spectate), a brief full-screen animation should play (no longer than half a second).

If a player has completed an attempt and is transitioning to spectate the others, a garland should briefly be displayed. If a player has failed a survival challenge and is transitioning to spectate the others, it should be a skull instead.

When the results for all players are in, everyone's current score (remaining stock, total frames, current standing, etc) should be briefly displayed before moving to the next challenge (or the final score screen).

## Score screen

The results are displayed. There is a 'back to lobby' button and a 'play again' button. The latter runs the same match settings again (with different seeds, if applicable).