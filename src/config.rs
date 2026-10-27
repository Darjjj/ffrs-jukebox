//! `ffrs_jukebox.toml`, next to the .asi. Written back whenever a setting is
//! changed from the menu, so a choice survives a restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the jukebox replaces the game's own music. Each place is switched
/// on or off on its own from the menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayIn {
    /// Field, towns and dungeons.
    pub exploring: bool,
    pub battles: bool,
    /// Story events played in the engine.
    pub cutscenes: bool,
}

impl Default for PlayIn {
    fn default() -> Self {
        PlayIn { exploring: true, battles: false, cutscenes: false }
    }
}

impl PlayIn {
    /// Whether the jukebox plays in `scene`.
    pub fn covers(self, scene: Scene) -> bool {
        match scene {
            Scene::Exploring => self.exploring,
            Scene::Battle => self.battles,
            Scene::Event => self.cutscenes,
            // A movie's own soundtrack can't be muted, so nothing plays over it.
            Scene::Movie => false,
        }
    }
}

/// The single choice that came before `PlayIn`, still read from older
/// config files so their setting carries over.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum Mode {
    #[default]
    JustExploring,
    ExploringAndBattles,
    All,
}

impl From<Mode> for PlayIn {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::JustExploring => PlayIn::default(),
            Mode::ExploringAndBattles => PlayIn { exploring: true, battles: true, cutscenes: false },
            Mode::All => PlayIn { exploring: true, battles: true, cutscenes: true },
        }
    }
}

/// What the game is doing right now, as far as the jukebox cares.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scene {
    #[default]
    Exploring,
    Battle,
    Event,
    /// A pre-rendered movie that has its own soundtrack.
    Movie,
}

impl Scene {
    pub fn label(self) -> &'static str {
        match self {
            Scene::Exploring => "Exploring",
            Scene::Battle => "Battle",
            Scene::Event => "Story event",
            Scene::Movie => "Movie",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum Repeat {
    /// Play through the playlist and stop at the end.
    Off,
    /// Loop the whole playlist.
    #[default]
    All,
    /// Loop the current track.
    One,
}

impl Repeat {
    pub const ALL: [Repeat; 3] = [Repeat::Off, Repeat::All, Repeat::One];

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "Off",
            Repeat::All => "Playlist",
            Repeat::One => "Track",
        }
    }
}

/// Which songs Next and Shuffle pick from.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum Playlist {
    #[default]
    AllSongs,
    Favorites,
    /// One of your own playlists, by name.
    Custom(String),
    /// A folder inside the music folder, by its name: every song in it.
    Folder(String),
}

impl Playlist {
    pub fn label(&self) -> String {
        match self {
            Playlist::AllSongs => "All songs".into(),
            Playlist::Favorites => "Favorites".into(),
            Playlist::Custom(name) => name.clone(),
            Playlist::Folder(name) => format!("Folder: {name}"),
        }
    }
}

/// A playlist you made, separate from Favorites.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CustomPlaylist {
    pub name: String,
    /// Track keys in play order, like `favorites`.
    pub tracks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Virtual-key code that steps through full menu, mini player and
    /// hidden. 0x78 is F9. Every key here is changeable in the menu's
    /// settings; 0 means unbound.
    pub menu_key: i32,
    /// Transport hotkeys: the 7, 8, 9 and 0 keys above the letters.
    pub key_previous: i32,
    pub key_play_pause: i32,
    pub key_next: i32,
    pub key_stop: i32,
    /// Volume down and up in 5% steps, repeating while held: the - and =
    /// keys.
    pub key_volume_down: i32,
    pub key_volume_up: i32,
    /// Held to show the mouse cursor over the game, for reaching the mini
    /// player while exploring. Left Alt.
    pub key_show_cursor: i32,
    /// Only read from older files; `play_in` replaces it.
    #[serde(skip_serializing)]
    pub mode: Mode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_in: Option<PlayIn>,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// 0.0 - 1.5 (0 - 150%), applied to both game songs and your own files.
    pub volume: f32,
    /// Size of the menu text; raise it on high-resolution screens.
    pub ui_scale: f32,
    /// Folder scanned for your own music. Relative paths are relative to the
    /// .asi's folder.
    pub music_folder: String,
    pub playlist: Playlist,
    /// Where battle music comes from, when the jukebox plays in battles.
    /// `None` keeps playing from `playlist`. With one set, a battle starts a
    /// song from it, and the exploring song picks up where it left off after.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battle_playlist: Option<Playlist>,
    /// Seconds one song blends into the next; 0 for a hard cut.
    pub crossfade_secs: f32,
    /// Start the last song again, where it was, when the game starts.
    pub resume_on_start: bool,
    /// Hold Left Trigger and press the D-pad: left previous, right next, up
    /// play/pause, down stop.
    pub controller: bool,
    /// How long each looping game theme plays before the next song.
    pub loop_minutes: f32,
    /// Your own names for game songs, by cue name, e.g.
    /// `BGM_LD_FF09_NOT_ALONE = "Not Alone"`. Applied when the game's songs
    /// are listed.
    pub titles: BTreeMap<String, String>,
    /// Your own names for songs in the music folder, by track key
    /// (`file:<path>`).
    pub file_titles: BTreeMap<String, String>,
    /// The song playing when the game last closed, by track key, and how far
    /// into it, for `resume_on_start`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_song: Option<(String, f32)>,
    /// The Favorites playlist, in play order. Each entry is a track key:
    /// `game:<cue name>` or `file:<path inside the music folder>`.
    pub favorites: Vec<String>,
    pub playlists: Vec<CustomPlaylist>,
    /// Your own songs' level next to the game's, in dB (-12 to +6), on top of
    /// the automatic levelling.
    pub my_music_level_db: f32,
    /// Measured gains that level your own songs to the game's music, by file
    /// (path inside the music folder and size). Filled in as songs are
    /// first played.
    pub file_levels: BTreeMap<String, f32>,
    /// Each file's loudest sample (0.0 - 1.0), by the same key, so no
    /// volume setting can push it past full scale.
    pub file_peaks: BTreeMap<String, f32>,
    /// Songs hidden from the song tabs and from "All songs", by track key.
    pub hidden: Vec<String>,
    /// Game songs the game would not play (their audio seems to be missing
    /// from the game's files), learned as they fail and left out of every
    /// list. Cleared when the game updates, so each gets another try.
    pub unplayable: Vec<String>,
    /// The game build `unplayable` was learned on: the exe's size and
    /// modified time.
    pub game_build: String,
    /// The game build every song was last checked for audio on.
    pub checked_build: String,
    /// Where the menu was last left on screen, in pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu_pos: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu_size: Option<[f32; 2]>,
    /// Where the mini player was last left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mini_pos: Option<[f32; 2]>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            menu_key: 0x78,
            key_previous: 0x37,
            key_play_pause: 0x38,
            key_next: 0x39,
            key_stop: 0x30,
            key_volume_down: 0xBD,
            key_volume_up: 0xBB,
            key_show_cursor: 0xA4,
            mode: Mode::default(),
            play_in: None,
            repeat: Repeat::default(),
            shuffle: false,
            volume: 0.8,
            ui_scale: 1.5,
            music_folder: "jukebox".into(),
            playlist: Playlist::default(),
            battle_playlist: None,
            crossfade_secs: 2.0,
            resume_on_start: true,
            controller: true,
            loop_minutes: 3.0,
            titles: BTreeMap::new(),
            file_titles: BTreeMap::new(),
            last_song: None,
            favorites: Vec::new(),
            playlists: Vec::new(),
            my_music_level_db: 0.0,
            file_levels: BTreeMap::new(),
            file_peaks: BTreeMap::new(),
            hidden: Vec::new(),
            unplayable: Vec::new(),
            game_build: String::new(),
            checked_build: String::new(),
            menu_pos: None,
            menu_size: None,
            mini_pos: None,
        }
    }
}

/// The actions that have a key, in the order the settings list them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bind {
    Menu,
    Previous,
    PlayPause,
    Next,
    Stop,
    VolumeDown,
    VolumeUp,
    /// Held rather than pressed.
    ShowCursor,
}

impl Bind {
    pub const ALL: [Bind; 8] = [
        Bind::Menu,
        Bind::Previous,
        Bind::PlayPause,
        Bind::Next,
        Bind::Stop,
        Bind::VolumeDown,
        Bind::VolumeUp,
        Bind::ShowCursor,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Bind::Menu => "Open / mini / hide",
            Bind::Previous => "Previous song",
            Bind::PlayPause => "Play / pause",
            Bind::Next => "Next song",
            Bind::Stop => "Stop",
            Bind::VolumeDown => "Volume down",
            Bind::VolumeUp => "Volume up",
            Bind::ShowCursor => "Show cursor (hold)",
        }
    }
}

impl Config {
    /// Where the jukebox plays, carried over from the older single choice
    /// until the toggles are first changed.
    pub fn play_in(&self) -> PlayIn {
        self.play_in.unwrap_or_else(|| self.mode.into())
    }

    pub fn key(&self, bind: Bind) -> i32 {
        match bind {
            Bind::Menu => self.menu_key,
            Bind::Previous => self.key_previous,
            Bind::PlayPause => self.key_play_pause,
            Bind::Next => self.key_next,
            Bind::Stop => self.key_stop,
            Bind::VolumeDown => self.key_volume_down,
            Bind::VolumeUp => self.key_volume_up,
            Bind::ShowCursor => self.key_show_cursor,
        }
    }

    fn key_mut(&mut self, bind: Bind) -> &mut i32 {
        match bind {
            Bind::Menu => &mut self.menu_key,
            Bind::Previous => &mut self.key_previous,
            Bind::PlayPause => &mut self.key_play_pause,
            Bind::Next => &mut self.key_next,
            Bind::Stop => &mut self.key_stop,
            Bind::VolumeDown => &mut self.key_volume_down,
            Bind::VolumeUp => &mut self.key_volume_up,
            Bind::ShowCursor => &mut self.key_show_cursor,
        }
    }

    /// Binds `vk` to `bind`. An action that already had `vk` takes the key
    /// `bind` had, so no key ever does two things.
    pub fn set_key(&mut self, bind: Bind, vk: i32) {
        let old = self.key(bind);
        if vk != 0 {
            if let Some(other) = Bind::ALL.into_iter().find(|&b| b != bind && self.key(b) == vk) {
                *self.key_mut(other) = old;
            }
        }
        *self.key_mut(bind) = vk;
    }

    /// Moves the volume by `delta`, kept on whole 5% steps within 0-150%.
    pub fn step_volume(&mut self, delta: f32) {
        let stepped = ((self.volume + delta) * 20.0).round() / 20.0;
        self.volume = stepped.clamp(0.0, 1.5);
    }

    pub fn reset_keys(&mut self) {
        let d = Config::default();
        for b in Bind::ALL {
            *self.key_mut(b) = d.key(b);
        }
    }

    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                // Kept aside before anything is saved over it, so favorites,
                // playlists and keys in a damaged file can still be recovered.
                let backup = path.with_extension("toml.bad");
                match std::fs::copy(path, &backup) {
                    Ok(_) => log::error!(
                        "config: {} is invalid, using defaults; the old file is kept as {}: {e}",
                        path.display(),
                        backup.display()
                    ),
                    Err(ce) => log::error!(
                        "config: {} is invalid ({e}) and could not be backed up: {ce}",
                        path.display()
                    ),
                }
                Self::default()
            }),
            Err(_) => {
                let config = Self::default();
                config.save(path);
                config
            }
        }
    }

    /// Written to a temporary file and swapped in, so a crash mid-save can't
    /// leave a half-written config behind.
    pub fn save(&self, path: &Path) {
        let text = match toml::to_string_pretty(self) {
            Ok(text) => text,
            Err(e) => return log::error!("config: could not serialize: {e}"),
        };
        let tmp = path.with_extension("toml.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)) {
            log::error!("config: could not write {}: {e}", path.display());
        }
    }

    pub fn music_dir(&self, mod_dir: &Path) -> PathBuf {
        let p = PathBuf::from(&self.music_folder);
        if p.is_absolute() { p } else { mod_dir.join(p) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_key_swaps_instead_of_doing_two_things() {
        let mut c = Config::default();
        // F9 is the menu key; giving it to Next hands Next's 9 to the menu.
        c.set_key(Bind::Next, 0x78);
        assert_eq!(c.key(Bind::Next), 0x78);
        assert_eq!(c.key(Bind::Menu), 0x39);
        c.set_key(Bind::Stop, 0);
        assert_eq!(c.key(Bind::Stop), 0);
        c.reset_keys();
        assert_eq!(c.key(Bind::Menu), 0x78);
        assert_eq!(c.key(Bind::Stop), 0x30);
    }
    #[test]
    fn volume_keys_step_by_five_percent_within_limits() {
        let mut c = Config::default();
        c.volume = 0.8;
        c.step_volume(0.05);
        assert!((c.volume - 0.85).abs() < 1e-6);
        // An odd volume from the slider snaps onto the 5% steps.
        c.volume = 0.83;
        c.step_volume(-0.05);
        assert!((c.volume - 0.8).abs() < 1e-6);
        c.volume = 1.48;
        c.step_volume(0.05);
        assert!((c.volume - 1.5).abs() < 1e-6);
        c.volume = 0.02;
        c.step_volume(-0.05);
        assert_eq!(c.volume, 0.0);
    }

    #[test]
    fn play_in_carries_over_the_old_mode_and_saves_as_toggles() {
        // An older file with only the single choice keeps its setting.
        let old: Config = toml::from_str("mode = \"ExploringAndBattles\"").unwrap();
        assert_eq!(old.play_in(), PlayIn { exploring: true, battles: true, cutscenes: false });
        assert_eq!(Config::default().play_in(), PlayIn::default());

        // Battles alone: only battles are covered, and movies never are.
        let mut c = Config::default();
        c.play_in = Some(PlayIn { exploring: false, battles: true, cutscenes: false });
        let p = c.play_in();
        assert!(p.covers(Scene::Battle));
        assert!(!p.covers(Scene::Exploring) && !p.covers(Scene::Event) && !p.covers(Scene::Movie));

        // Saved as toggles; the old field isn't written back.
        let text = toml::to_string(&c).unwrap();
        assert!(!text.contains("mode ="));
        assert_eq!(toml::from_str::<Config>(&text).unwrap().play_in(), p);
    }

    #[test]
    fn a_damaged_config_is_kept_aside_not_lost() {
        let dir = std::env::temp_dir().join(format!("ffrs_jukebox_cfg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ffrs_jukebox.toml");
        let damaged = "favorites = [\"game:BGM_PP_PRELUDE\"\nthis is not toml";
        std::fs::write(&path, damaged).unwrap();
        let c = Config::load(&path);
        assert!(c.favorites.is_empty());
        assert_eq!(std::fs::read_to_string(path.with_extension("toml.bad")).unwrap(), damaged);
        // Saving over it afterwards leaves a valid file and no temp file.
        c.save(&path);
        assert!(toml::from_str::<Config>(&std::fs::read_to_string(&path).unwrap()).is_ok());
        assert!(!path.with_extension("toml.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playlists_round_trip() {
        let mut c = Config::default();
        c.favorites = vec!["game:BGM_LD_FF09_NOT_ALONE".into()];
        c.playlists = vec![CustomPlaylist { name: "Road trip".into(), tracks: vec!["file:a.mp3".into()] }];
        c.playlist = Playlist::Custom("Road trip".into());
        c.titles.insert("BGM_LD_FF15".into(), "FF15 Medley".into());
        c.battle_playlist = Some(Playlist::Folder("Battle".into()));
        c.last_song = Some(("game:BGM_PP_PRELUDE".into(), 42.5));
        let text = toml::to_string_pretty(&c).unwrap();
        println!("{text}");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.playlist, Playlist::Custom("Road trip".into()));
        assert_eq!(back.playlists[0].tracks, vec!["file:a.mp3".to_string()]);
        assert_eq!(back.battle_playlist, Some(Playlist::Folder("Battle".into())));
        assert_eq!(back.last_song, Some(("game:BGM_PP_PRELUDE".to_string(), 42.5)));
        // Configs saved before playlists existed still load.
        let old: Config = toml::from_str("playlist = \"Favorites\"\nmenu_key = 120").unwrap();
        assert_eq!(old.playlist, Playlist::Favorites);
    }
}
