# Changelog

## 1.5.0 - 2026-10-29

- New setting, "New song on a new map" (General tab, off by default): entering another map, such as
  from the field into a town, moves on to the next song once the map has loaded.
- New setting, "Pick up where it left off after battles" (Battles tab, on by default). Without a
  battle music list, a song the game cut off during a battle now starts again where it was rather than
  skipping ahead; turned off, the next song plays after each battle.
- Save points no longer hand the music back to the game.
- A song the game won't start in one place is skipped there instead of being removed from your lists
  for good. (Songs removed this way before can be brought back: delete their lines under `unplayable`
  in `ffrs_jukebox.toml` while the game is closed.)
- Thanks to Cloudjr5 for the suggestions.

## 1.4.2 - 2026-10-28

- Vision attacks no longer interrupt your battle song. Their movie was mistaken for the battle ending, so
  the game's battle theme came in and the next battle song started afterwards; now the song carries on
  straight through. Thanks to Obsidianpunchbowl for the catch.

## 1.4.1 - 2026-10-28

- New setting, "Loop the battle song" (Battles tab, off by default): the song picked as a battle starts
  keeps playing on a loop until the battle ends, instead of moving on to the next one.

## 1.4.0 - 2026-10-28

- Boss battles keep the game's own music, as the developers intended: a new setting, "Game music in
  boss battles" (on by default). The status line in the menu says "Boss battle" during one.
- Talking to townsfolk, shops, the inn, bulletin boards and the like no longer hands the music back to
  the game; only story scenes count as cutscenes now.
- The mini player can hide itself during cutscenes ("Hide during cutscenes", on by default) and while
  talking ("Hide while talking", off by default), and comes back when they end.
- Battle music has a Shuffle of its own, separate from the main Shuffle. Battle and exploring music
  also keep separate shuffle orders and Previous histories, so neither disturbs the other.
- The settings window is reworked into General, Battles and Controls tabs, with one On/Off switch per
  option. It can be resized and remembers its size.
- Works alongside PolarWizard's ultrawide fix again: the jukebox no longer hides the game function the
  fix looks for.
- Boss fights no longer start a battle song in the background, story scenes no longer flicker the music
  back on between their parts, and a key waiting to be rebound stops waiting when you leave its tab.

## 1.3.0 - 2026-10-27

- The now-playing popup also shows the current song when a battle starts and when it ends. A new
  setting, "Show the song on battle start and end" (on by default), turns it off.
- Thanks to Obsidianpunchbowl for suggesting the battle popup.
- Repeat is now a button next to Shuffle, in the full menu and the mini player: click it to switch
  between off, repeat playlist and repeat song (shown with a small 1).

## 1.2.0 - 2026-10-27

- New setting, "Let the victory fanfare play" (on by default): winning a battle fades the jukebox out
  for the game's own victory fanfare, then goes straight back to your exploring song once the
  battle is over.

## 1.1.0 - 2026-10-27

- "Play during" is now three toggles, Exploring, Battles and Cutscenes, so any mix works, including
  battles only. Your old choice carries over.
- Thanks to AlucardLockheart for suggesting a battles-only option.

## 1.0.0 - 2026-10-27

First release.

- In-game jukebox menu (F9) in FINAL FANTASY RESONANCE's own menu style, plus a mini player.
- Plays any of the game's 196 playable songs, split into Themes and Jingles, with readable names.
- Plays your own music (mp3, ogg, flac, wav, m4a, aac) from a `jukebox` folder, levelled to match the
  game's music and following the game's Music and Master volume. Subfolders become playlists.
- Modes: Just Exploring, Exploring and Battles, or Exploring, Battling, and Cutscenes.
- Separate battle music, with the exploring song picking up where it left off afterwards.
- Favorites, your own playlists, hide songs, rename songs. Playlists show as cards with Play and
  Shuffle; inside one, drag a song by its title to reorder (Favorites too).
- Shuffle without repeats, repeat modes, crossfade between songs, resume the last song at start.
- Rebindable hotkeys (F9 menu, 7/8/9/0 transport, -/= volume, hold Left Alt for the cursor) and
  controller combos (hold LT + D-pad).
- Steps aside for pre-rendered movies that have their own soundtrack.
- Finds and leaves out the game songs whose audio is missing from the game's files.
- Volume rises smoothly and your own files can never clip.
