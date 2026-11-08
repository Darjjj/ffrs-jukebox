//! `ffrs_jukebox.toml`, next to the .asi. Written back whenever a setting is
//! changed from the menu, so a choice survives a restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::keys::PadButton;

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
    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "Repeat off",
            Repeat::All => "Repeat playlist",
            Repeat::One => "Repeat song",
        }
    }

    /// The repeat button's next setting: off, playlist, song, then off again.
    pub fn next(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

/// Which songs Next and Shuffle pick from.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, Default)]
pub enum Playlist {
    #[default]
    AllSongs,
    Favorites,
    /// The game's full-length songs, without its short jingles and stingers.
    GameThemes,
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
            Playlist::GameThemes => "Game themes".into(),
            Playlist::Custom(name) => name.clone(),
            Playlist::Folder(name) => format!("Folder: {name}"),
        }
    }
}

/// The kind of place the player is exploring, as the game files its maps.
/// Underground areas and Vision shrines count as dungeons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Area {
    Field,
    Town,
    Dungeon,
}

impl Area {
    pub const ALL: [Area; 3] = [Area::Field, Area::Town, Area::Dungeon];

    pub fn label(self) -> &'static str {
        match self {
            Area::Field => "Field",
            Area::Town => "Town",
            Area::Dungeon => "Dungeon",
        }
    }
}

/// What plays while exploring one kind of place.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum AreaMusic {
    /// Whatever "Play from" is set to.
    #[default]
    PlayFrom,
    /// A list of its own.
    List(Playlist),
    /// The game's own music; the jukebox waits.
    Game,
}

impl AreaMusic {
    pub fn is_play_from(&self) -> bool {
        *self == AreaMusic::PlayFrom
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
    /// What plays while exploring the field, towns and dungeons.
    #[serde(skip_serializing_if = "AreaMusic::is_play_from")]
    pub field_music: AreaMusic,
    #[serde(skip_serializing_if = "AreaMusic::is_play_from")]
    pub town_music: AreaMusic,
    #[serde(skip_serializing_if = "AreaMusic::is_play_from")]
    pub dungeon_music: AreaMusic,
    /// Shuffle for each area's own list, apart from the main Shuffle.
    pub field_shuffle: bool,
    pub town_shuffle: bool,
    pub dungeon_shuffle: bool,
    /// Seconds one song blends into the next; 0 for a hard cut.
    pub crossfade_secs: f32,
    /// Start the last song again, where it was, when the game starts.
    pub resume_on_start: bool,
    /// Step aside for the game's victory fanfare after a won battle.
    pub victory_fanfare: bool,
    /// Pop the now-playing toast when a battle starts or ends.
    pub battle_toast: bool,
    /// Leave boss battles to the game's own music.
    pub boss_game_music: bool,
    /// Shuffle for the battle music list, apart from the main Shuffle.
    pub battle_shuffle: bool,
    /// Keep the battle song picked as a fight starts on a loop until it ends.
    pub loop_battle_song: bool,
    /// After a battle, pick the exploring song up where it was; off plays
    /// the next song instead.
    pub resume_after_battle: bool,
    /// Move on to the next song on entering another map (a town, a field).
    pub new_song_on_new_map: bool,
    /// Leave Vision shrines to the game's own music.
    pub shrine_game_music: bool,
    /// Tuck the mini player away during cutscenes; it comes back after.
    pub mini_hide_in_cutscenes: bool,
    /// Tuck the mini player away while talking to someone or reading a sign.
    pub mini_hide_while_talking: bool,
    /// Hide the reminder at the foot of the menu of which keys switch it.
    pub hide_key_hint: bool,
    /// Run on a game build this jukebox wasn't made for, as long as the
    /// game's code still looks the same where the jukebox hooks it. Off by
    /// default: a build the jukebox hasn't been checked against can crash
    /// the game. For testing whether a game update needs a jukebox update;
    /// only in the settings file, not in the menu.
    pub allow_untested_build: bool,
    /// Hold Left Trigger and press the D-pad: left previous, right next, up
    /// play/pause, down stop.
    pub controller: bool,
    /// The controller combos: hold `pad_hold`, then press an action button.
    pub pad_hold: PadButton,
    pub pad_previous: PadButton,
    pub pad_next: PadButton,
    pub pad_play_pause: PadButton,
    pub pad_stop: PadButton,
    /// Opens the menu to be driven with the controller, and closes it.
    pub pad_menu: PadButton,
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
    /// The settings window's size, once it has been resized.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_size: Option<[f32; 2]>,
    /// Where the mini player was last left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mini_pos: Option<[f32; 2]>,
}

impl Config {
    pub fn area_music(&self, area: Area) -> &AreaMusic {
        match area {
            Area::Field => &self.field_music,
            Area::Town => &self.town_music,
            Area::Dungeon => &self.dungeon_music,
        }
    }

    pub fn area_shuffle(&self, area: Area) -> bool {
        match area {
            Area::Field => self.field_shuffle,
            Area::Town => self.town_shuffle,
            Area::Dungeon => self.dungeon_shuffle,
        }
    }

    pub fn area_shuffle_mut(&mut self, area: Area) -> &mut bool {
        match area {
            Area::Field => &mut self.field_shuffle,
            Area::Town => &mut self.town_shuffle,
            Area::Dungeon => &mut self.dungeon_shuffle,
        }
    }

    pub fn area_music_mut(&mut self, area: Area) -> &mut AreaMusic {
        match area {
            Area::Field => &mut self.field_music,
            Area::Town => &mut self.town_music,
            Area::Dungeon => &mut self.dungeon_music,
        }
    }
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
            field_music: AreaMusic::PlayFrom,
            town_music: AreaMusic::PlayFrom,
            dungeon_music: AreaMusic::PlayFrom,
            field_shuffle: true,
            town_shuffle: true,
            dungeon_shuffle: true,
            crossfade_secs: 2.0,
            resume_on_start: true,
            victory_fanfare: true,
            battle_toast: true,
            boss_game_music: true,
            battle_shuffle: true,
            loop_battle_song: false,
            resume_after_battle: true,
            new_song_on_new_map: false,
            shrine_game_music: false,
            mini_hide_in_cutscenes: true,
            mini_hide_while_talking: false,
            hide_key_hint: false,
            allow_untested_build: false,
            controller: true,
            pad_hold: PadButton::LT,
            pad_previous: PadButton::DpadLeft,
            pad_next: PadButton::DpadRight,
            pad_play_pause: PadButton::DpadUp,
            pad_stop: PadButton::DpadDown,
            pad_menu: PadButton::Y,
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
            settings_size: None,
            mini_pos: None,
        }
    }
}

/// The controller combo buttons, in the order the settings list them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PadBind {
    Hold,
    Previous,
    Next,
    PlayPause,
    Stop,
    Menu,
}

impl PadBind {
    pub const ALL: [PadBind; 6] =
        [PadBind::Hold, PadBind::Previous, PadBind::Next, PadBind::PlayPause, PadBind::Stop, PadBind::Menu];

    pub fn label(self) -> &'static str {
        match self {
            PadBind::Hold => "Hold",
            PadBind::Previous => "Previous song",
            PadBind::Next => "Next song",
            PadBind::PlayPause => "Play / pause",
            PadBind::Stop => "Stop",
            PadBind::Menu => "Open the menu",
        }
    }
}

impl Config {
    pub fn pad(&self, bind: PadBind) -> PadButton {
        match bind {
            PadBind::Hold => self.pad_hold,
            PadBind::Previous => self.pad_previous,
            PadBind::Next => self.pad_next,
            PadBind::PlayPause => self.pad_play_pause,
            PadBind::Stop => self.pad_stop,
            PadBind::Menu => self.pad_menu,
        }
    }

    fn pad_mut(&mut self, bind: PadBind) -> &mut PadButton {
        match bind {
            PadBind::Hold => &mut self.pad_hold,
            PadBind::Previous => &mut self.pad_previous,
            PadBind::Next => &mut self.pad_next,
            PadBind::PlayPause => &mut self.pad_play_pause,
            PadBind::Stop => &mut self.pad_stop,
            PadBind::Menu => &mut self.pad_menu,
        }
    }

    /// Sets a combo button; one already used by another combo swaps over.
    pub fn set_pad(&mut self, bind: PadBind, button: PadButton) {
        let old = self.pad(bind);
        if let Some(other) = PadBind::ALL.into_iter().find(|&b| b != bind && self.pad(b) == button) {
            *self.pad_mut(other) = old;
        }
        *self.pad_mut(bind) = button;
    }

    pub fn pad_binds(&self) -> crate::keys::PadBinds {
        crate::keys::PadBinds {
            hold: self.pad_hold,
            previous: self.pad_previous,
            next: self.pad_next,
            play_pause: self.pad_play_pause,
            stop: self.pad_stop,
            menu: self.pad_menu,
        }
    }

    pub fn reset_pad(&mut self) {
        let d = Config::default();
        for b in PadBind::ALL {
            *self.pad_mut(b) = d.pad(b);
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

/// Whether settings are written back this session. Off when the file
/// couldn't be read at start: it's probably fine and only locked, and
/// saving defaults over it would lose everything in it.
static PERSIST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether changes to the settings are being saved this session.
pub fn persisting() -> bool {
    PERSIST.load(std::sync::atomic::Ordering::SeqCst)
}

/// Work for the writer thread.
enum Write {
    Save(PathBuf, Config),
    /// Write whatever is waiting now, and say when it's done.
    Flush(std::sync::mpsc::Sender<()>),
}

/// The one thread that writes the settings file. A save waits 300 ms for
/// more to arrive (a slider drag, the position saved every few seconds) and
/// writes the latest, so a burst of changes is one write, and neither the
/// menu nor the player waits on the disk.
fn writer() -> &'static std::sync::mpsc::Sender<Write> {
    static WRITER: std::sync::OnceLock<std::sync::mpsc::Sender<Write>> = std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        use std::sync::mpsc::RecvTimeoutError::{Disconnected, Timeout};
        let (tx, rx) = std::sync::mpsc::channel::<Write>();
        std::thread::spawn(move || {
            let mut pending: Option<(PathBuf, Config)> = None;
            loop {
                let next = match &pending {
                    Some(_) => rx.recv_timeout(std::time::Duration::from_millis(300)),
                    None => rx.recv().map_err(|_| Disconnected),
                };
                match next {
                    Ok(Write::Save(path, config)) => pending = Some((path, config)),
                    Ok(Write::Flush(done)) => {
                        if let Some((path, config)) = pending.take() {
                            config.save(&path);
                        }
                        let _ = done.send(());
                    }
                    Err(Timeout) => {
                        if let Some((path, config)) = pending.take() {
                            config.save(&path);
                        }
                    }
                    Err(Disconnected) => return,
                }
            }
        });
        tx
    })
}

/// Writes any save still waiting, now, and waits up to `timeout` for it:
/// for the game closing, when the writer thread would be cut off.
pub fn flush(timeout: std::time::Duration) {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    if writer().send(Write::Flush(done_tx)).is_ok() {
        let _ = done_rx.recv_timeout(timeout);
    }
}

/// `list` with each entry once, in its first place.
fn dedupe(list: &mut Vec<String>) {
    let mut seen = std::collections::BTreeSet::new();
    list.retain(|k| seen.insert(k.clone()));
}

impl Config {
    /// Where the jukebox plays. A settings file from before the toggles has
    /// its single choice moved into them when it's loaded (`sanitize`).
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
        if vk != 0
            && let Some(other) = Bind::ALL.into_iter().find(|&b| b != bind && self.key(b) == vk)
        {
            *self.key_mut(other) = old;
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
        // Read with a few retries: an antivirus or a sync client can hold
        // the file for a moment, and that must not look like a damaged file.
        let mut bytes = Err(std::io::Error::other("not read"));
        for attempt in 0..5 {
            bytes = std::fs::read(path);
            match &bytes {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let config = Self::default();
                    config.save(path);
                    return config;
                }
                Err(_) if attempt < 4 => std::thread::sleep(std::time::Duration::from_millis(100)),
                _ => break,
            }
        }
        let bytes = match bytes {
            Ok(bytes) => bytes,
            // Probably a fine file that can't be read right now: saving
            // defaults over it would lose everything in it, so nothing is
            // saved this session (the menu says so).
            Err(e) => {
                log::error!(
                    "config: {} could not be read ({e}); using defaults, and not saving over it this session",
                    path.display()
                );
                PERSIST.store(false, std::sync::atomic::Ordering::SeqCst);
                return Self::default();
            }
        };
        // Notepad's "Unicode" saves UTF-16, which isn't TOML.
        let Ok(text) = String::from_utf8(bytes) else {
            return Self::damaged(path, "isn't saved as UTF-8");
        };
        let table: toml::Table = match text.parse() {
            Ok(table) => table,
            Err(e) => return Self::damaged(path, &format!("is invalid: {e}")),
        };
        // Keys the jukebox doesn't know (a typo in a hand edit) are dropped
        // by the next save, so they're at least named here.
        let unknown = |path: serde_ignored::Path| {
            log::warn!("config: `{path}` isn't a setting the jukebox knows, and won't be kept");
        };
        let mut config = match serde_ignored::deserialize(toml::Value::Table(table.clone()), unknown) {
            Ok(config) => config,
            Err(_) => Self::salvage(table),
        };
        config.sanitize();
        config
    }

    /// A file with a wrong value somewhere (`repeat = "all"`, a key code
    /// too big for a number): every setting that does read is kept, and
    /// only the wrong ones go back to their defaults.
    fn salvage(table: toml::Table) -> Self {
        let mut kept = toml::Table::new();
        for (key, value) in table {
            let mut one = toml::Table::new();
            one.insert(key.clone(), value.clone());
            match one.try_into::<Self>() {
                Ok(_) => {
                    kept.insert(key, value);
                }
                Err(e) => log::error!("config: `{key}` is invalid and goes back to its default: {}", e.message()),
            }
        }
        kept.try_into().unwrap_or_default()
    }

    /// A file that isn't TOML at all: kept aside before anything is saved
    /// over it, so favorites, playlists and keys in it can be recovered. An
    /// earlier `.bad` is never overwritten.
    fn damaged(path: &Path, why: &str) -> Self {
        let backup = (0..10)
            .map(|n| path.with_extension(if n == 0 { "toml.bad".to_string() } else { format!("toml.bad{n}") }))
            .find(|p| !p.exists())
            .unwrap_or_else(|| path.with_extension("toml.bad9"));
        match std::fs::copy(path, &backup) {
            Ok(_) => log::error!(
                "config: {} {why}; using defaults, the old file is kept as {}",
                path.display(),
                backup.display()
            ),
            Err(ce) => log::error!("config: {} {why}, and could not be backed up: {ce}", path.display()),
        }
        Self::default()
    }

    /// Puts a loaded file right: hand-edited numbers back in range (a NaN or
    /// an infinity would stall a fade or break a seek), and an older file's
    /// single play mode turned into `play_in`, so it survives the next save
    /// (the old `mode` field isn't written back).
    fn sanitize(&mut self) {
        let d = Self::default();
        let fix = |v: &mut f32, lo: f32, hi: f32, default: f32| {
            *v = if v.is_finite() { v.clamp(lo, hi) } else { default };
        };
        fix(&mut self.volume, 0.0, 1.5, d.volume);
        fix(&mut self.ui_scale, 0.5, 4.0, d.ui_scale);
        fix(&mut self.crossfade_secs, 0.0, 8.0, d.crossfade_secs);
        fix(&mut self.loop_minutes, 1.0, 15.0, d.loop_minutes);
        fix(&mut self.my_music_level_db, -12.0, 6.0, d.my_music_level_db);
        if let Some((_, at)) = &mut self.last_song
            && !(at.is_finite() && (0.0..=86_400.0).contains(at))
        {
            *at = 0.0;
        }
        self.file_levels.retain(|_, gain| gain.is_finite() && *gain > 0.0);
        self.file_peaks.retain(|_, peak| peak.is_finite() && *peak > 0.0);
        for spot in [&mut self.menu_pos, &mut self.menu_size, &mut self.settings_size, &mut self.mini_pos] {
            if spot.is_some_and(|p| !p.iter().all(|v| v.is_finite())) {
                *spot = None;
            }
        }
        if self.play_in.is_none() {
            self.play_in = Some(self.mode.into());
        }
        // A song listed twice in a hand-edited list would show twice and
        // only half-remove; a playlist with no name or a name used twice
        // can't be told apart in the menu.
        dedupe(&mut self.favorites);
        dedupe(&mut self.hidden);
        let mut names: Vec<String> = Vec::new();
        for (n, p) in self.playlists.iter_mut().enumerate() {
            dedupe(&mut p.tracks);
            if p.name.trim().is_empty() {
                p.name = format!("Playlist {}", n + 1);
            }
            while names.contains(&p.name) {
                p.name.push_str(" 2");
            }
            names.push(p.name.clone());
        }
    }

    /// Hands a copy to the writer thread, which writes it after a short
    /// pause; copies that arrive meanwhile replace it, so a burst of changes
    /// (a slider drag, the position saved every few seconds) is one write.
    /// Neither the menu nor the player waits on the disk this way.
    pub fn save_in_background(&self, path: &Path) {
        if writer().send(Write::Save(path.to_path_buf(), self.clone())).is_err() {
            // The writer thread is gone; write here rather than lose it.
            self.save(path);
        }
    }

    /// Written to a temporary file and swapped in, so a crash mid-save can't
    /// leave a half-written config behind. Each save has a temporary file of
    /// its own, so two saves in flight can't swap in each other's halves.
    pub fn save(&self, path: &Path) {
        if !PERSIST.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let text = match toml::to_string_pretty(self) {
            Ok(text) => text,
            Err(e) => return log::error!("config: could not serialize: {e}"),
        };
        static SAVES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("toml.tmp{n}"));
        if let Err(e) = std::fs::write(&tmp, text) {
            log::error!("config: could not write {}: {e}", tmp.display());
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        // The swap is retried: a scanner may hold the file for a moment.
        let mut swapped = std::fs::rename(&tmp, path);
        for _ in 0..3 {
            if swapped.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            swapped = std::fs::rename(&tmp, path);
        }
        if let Err(e) = swapped {
            log::error!("config: could not write {}: {e}", path.display());
            let _ = std::fs::remove_file(&tmp);
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
    fn area_music_round_trips_and_defaults_to_play_from() {
        let mut c = Config::default();
        c.field_music = AreaMusic::List(Playlist::Custom("Overworld".into()));
        c.town_music = AreaMusic::Game;
        let text = toml::to_string(&c).unwrap();
        assert!(!text.contains("dungeon_music"));
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.field_music, c.field_music);
        assert_eq!(back.town_music, AreaMusic::Game);
        assert_eq!(back.dungeon_music, AreaMusic::PlayFrom);
        assert!(back.field_shuffle && back.town_shuffle && back.dungeon_shuffle && !back.shrine_game_music);
    }

    #[test]
    fn pad_buttons_swap_instead_of_doubling_up_and_round_trip() {
        let mut c = Config::default();
        c.set_pad(PadBind::Hold, PadButton::DpadLeft);
        assert_eq!((c.pad_hold, c.pad_previous), (PadButton::DpadLeft, PadButton::LT));
        c.set_pad(PadBind::Menu, PadButton::View);
        let back: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!((back.pad_hold, back.pad_previous, back.pad_menu), (PadButton::DpadLeft, PadButton::LT, PadButton::View));
        let old: Config = toml::from_str("controller = true").unwrap();
        assert_eq!((old.pad_hold, old.pad_menu), (PadButton::LT, PadButton::Y));
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

    #[test]
    fn a_1_3_0_config_gets_the_new_settings_at_their_defaults() {
        let old: Config = toml::from_str("shuffle = false\nbattle_toast = false\nvictory_fanfare = true").unwrap();
        assert!(!old.shuffle && !old.battle_toast);
        assert!(old.boss_game_music && old.battle_shuffle && old.mini_hide_in_cutscenes);
        assert!(!old.loop_battle_song && old.resume_after_battle && !old.new_song_on_new_map);
        assert!(!old.mini_hide_while_talking);
        assert_eq!(old.settings_size, None);
        // An unset window size isn't written out.
        assert!(!toml::to_string(&old).unwrap().contains("settings_size"));
    }

    #[test]
    fn a_settings_file_that_isnt_utf8_is_kept_aside_not_lost() {
        let dir = std::env::temp_dir().join(format!("ffrs_jukebox_utf16_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ffrs_jukebox.toml");
        // "volume = 0.5" as Notepad's UTF-16, with its byte order mark.
        let bytes: Vec<u8> =
            [0xFF, 0xFE].into_iter().chain("volume = 0.5".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        std::fs::write(&path, &bytes).unwrap();
        let config = Config::load(&path);
        assert_eq!(config.volume, Config::default().volume);
        assert_eq!(std::fs::read(path.with_extension("toml.bad")).unwrap(), bytes);
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "the file itself isn't saved over at load");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_play_mode_survives_a_save_and_reload() {
        let dir = std::env::temp_dir().join(format!("ffrs_jukebox_mode_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ffrs_jukebox.toml");
        std::fs::write(&path, "mode = \"ExploringAndBattles\"").unwrap();
        let want = PlayIn { exploring: true, battles: true, cutscenes: false };
        let config = Config::load(&path);
        assert_eq!(config.play_in(), want);
        config.save(&path);
        assert_eq!(Config::load(&path).play_in(), want);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hand_edited_numbers_are_put_back_in_range() {
        let mut c: Config =
            toml::from_str("crossfade_secs = inf
volume = nan
loop_minutes = 900.0
last_song = [\"game:X\", inf]").unwrap();
        c.sanitize();
        assert_eq!(c.crossfade_secs, Config::default().crossfade_secs);
        assert_eq!(c.volume, Config::default().volume);
        assert_eq!(c.loop_minutes, 15.0);
        assert_eq!(c.last_song, Some(("game:X".to_string(), 0.0)));
    }
}
