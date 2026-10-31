# Changelog

## 1.6.1 - 2026-10-31

- Fixed DualSense and other controllers that go through Steam Input not working in game since 1.6.0.
  The jukebox no longer patches XInput itself; it only reroutes the game's own calls, so Steam's
  controller support stays in the chain. Thanks to SenpaiOppai3 for the heads up on the bug.

## 1.6.0 - 2026-10-30 - Improved controller support

- Controller combos are rebindable in the Controls tab: the button to hold (LT by default) and each
  action. The game no longer sees the combo buttons while the hold button is down, so LT + D-pad no
  longer also does whatever the D-pad does in the game.
- Hold + Y (rebindable) steps through the views like F9. The full menu opens ready for the controller:
  D-pad or left stick to move, A to choose, B to back out (it presses Back where the menu shows one).
  The game doesn't see the controller meanwhile, and the menu remembers where you were for next time.
  A click or the mouse wheel hands the menu back to the mouse.
- Hold + right stick moves the menu or the mini player.
- Music by place: under "Music for", Battles, Field, Towns and Dungeons can each have a list of their
  own (with its own Shuffle), the game's own music, or follow "Play from". Thanks to NetalyaTaeris for
  the extra customization suggestion.
- New setting, "Game music in Vision shrines" (General tab, off by default).
- Changing the list that's playing now crossfades to a song from it, unless the current song is in it.
- Leaving a place left to the game's music carries on with the song that was waiting, if it belongs.
- The status line says which kind of place you're exploring.
- The key reminder at the foot of the menu is cleaner, includes the controller combo, and can be hidden
  ("Hide the key reminder", General tab).
- Fixed an ear-splitting buzz that could happen as the game started: the overlay now hooks in before
  the game's sound starts.
- Help text in the settings wraps instead of running off the window.

## 1.5.0 - 2026-10-29

- New setting, "New song on a new map" (General tab, off by default): entering another map, such as
  from the field into a town, moves on to the next song once the map has loaded.
- New setting, "Pick up where it left off after battles" (Battles tab, on by default). Without a
  battle music list, a song the game cut off during a battle now starts again where it was rather than
  skipping ahead; turned off, the next song plays after each battle.
- Save points no longer hand the music back to the game.
- A song the game won't start in one place is skipped there instead of being removed from your lists
  for good.
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
