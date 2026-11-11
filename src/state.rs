//! State shared between the menu (render thread) and the player (game thread).
//! The menu only reads it and queues [`Command`]s; the player is the only side
//! that touches audio or the game.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::config::{Config, CustomPlaylist, Playlist, Scene};

/// Game songs known to have no audio in the game's files (16 of the 212
/// named cues, as of the 1.0 release). They seed `config.unplayable` on a
/// fresh install or a game update, so no one has to wait for the song check
/// to find them; the check still runs and corrects the list either way.
pub const KNOWN_SILENT: [&str; 16] = [
    "BGM_LD_FF02_MAIN_THEME_FFPR",
    "BGM_LD_FF02_THE_REBEL_ARMY_FFPR",
    "BGM_LD_FF03_ETERNAL_WIND_FFPR",
    "BGM_LD_FF04_MAIN_THEME_FFPR",
    "BGM_LD_FF04_MAIN_THEME_SFC",
    "BGM_LD_FF04_THEME_OF_LOVE_FFPR",
    "BGM_LD_FF04_THEME_OF_LOVE_SFC",
    "BGM_LD_FF07_SECTOR_7_SLUMS",
    "BGM_LD_FF08_THE_OATH",
    "BGM_LD_FF09_SWORDS_OF_FURY",
    "BGM_LD_FF09_FESTIVAL_OF_THE_HUNT",
    "BGM_LD_FF09_NOT_ALONE",
    "BGM_LD_FF12_THE_DALMASCA_ESTERSAND",
    "BGM_LD_FF14_TOMORROW_AND_TOMORROW",
    "BGM_LD_FF14_THE_DARK_WHICH_ILLUMINATES_THE_WORLD",
    "BGM_LD_FF14_TOMORROW_AND_TOMORROW_REPRISE",
];

#[derive(Clone, Debug)]
pub enum Source {
    /// A cue in the game's BGM cue sheet. `looping` cues are the themes that
    /// play on until stopped; the rest are one-shot pieces such as fanfares.
    Game { cue: String, looping: bool },
    /// A file from the music folder.
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Track {
    /// The name shown: your own if you renamed it, else `base_title`.
    pub title: String,
    /// The name it gets when not renamed.
    pub base_title: String,
    pub source: Source,
    /// Stable name saved in the favorites list.
    pub key: String,
}

impl Track {
    pub fn game(cue: String, looping: bool) -> Self {
        let title = crate::titles::pretty(&cue);
        Self {
            base_title: title.clone(),
            title,
            key: format!("game:{cue}"),
            source: Source::Game { cue, looping },
        }
    }

    pub fn is_game_loop(&self) -> bool {
        matches!(self.source, Source::Game { looping: true, .. })
    }

    pub fn is_game(&self) -> bool {
        matches!(self.source, Source::Game { .. })
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    /// A song by its key, not its place in the list: a rescan can finish
    /// between the click and the player getting to it, and move every song.
    Play(String),
    TogglePause,
    Stop,
    Next,
    Previous,
    Seek(f32),
    Rescan,
    /// Check every game song for audio again.
    CheckSongs,
    /// A list choice changed (Play from, Battle music, an area's music).
    ListChanged,
}

#[derive(Default)]
pub struct Shared {
    pub config: Config,
    pub config_path: PathBuf,
    pub music_dir: PathBuf,
    pub tracks: Vec<Track>,
    /// Whether the game's songs have been listed yet. They can only be read
    /// once the game has loaded its BGM cue sheet.
    pub game_tracks_ready: bool,
    pub commands: VecDeque<Command>,
    /// Songs that would not play this session (a cue the game cut off, a
    /// file that would not decode), by track key: skipped like unplayable
    /// ones, but never written to the config. Only the song check decides
    /// what is unplayable for good. Files are tried again on a rescan. Each
    /// with why, shown on the song: it stays in the lists, greyed out, and
    /// a click tries it again.
    pub skipped: HashMap<String, String>,
    /// The last game song heard while the game had the music (the jukebox
    /// stopped, paused or stepping aside), by track key, so it can be
    /// favorited after the moment has passed.
    pub heard: Option<String>,

    // Written by the player, shown by the menu.
    pub current: Option<usize>,
    /// The player wants music playing (not paused or stopped by the user).
    pub playing: bool,
    /// Playing, but held because the current scene is outside the mode.
    pub held: bool,
    /// The current battle is a boss battle.
    pub boss: bool,
    /// Talking to someone or reading a sign (not a story scene).
    pub talking: bool,
    pub scene: Scene,
    /// The kind of place being explored, once the map is known.
    pub area: Option<crate::config::Area>,
    /// That place is a Vision shrine (counted as a dungeon).
    pub shrine: bool,
    pub position: f32,
    pub duration: Option<f32>,
    /// Bumped on every track change, so the menu can pop a now-playing toast.
    pub track_serial: u64,
    /// Songs checked so far, and in all, while the song check runs.
    pub check_progress: Option<(usize, usize)>,
    /// The song check is waiting for the jukebox's game song to stop.
    pub check_waiting: bool,
    pub error: Option<String>,
    /// A standing warning shown under the controls, kept for the session
    /// (running on a game build the jukebox wasn't checked against).
    pub notice: Option<String>,
    /// Bumped whenever the song lists change (songs, favorites, hidden,
    /// playlists, titles, skips): the menu keeps its rows until it moves.
    pub lists_rev: u64,
    /// The song lined up with Play next, by key: it plays at the next change
    /// of song while exploring, then the list carries on where it was.
    pub up_next: Option<String>,
    /// The music folder's folders and how many songs each holds, worked out
    /// when the files change rather than every frame the menu draws.
    pub folders: Vec<(String, usize)>,
    /// Each song's place by key, for the number of songs it was built for:
    /// looking songs up by key (playlists, every tick) without searching the
    /// whole list each time. Rebuilt when the list changes.
    pub(crate) index: RefCell<(usize, HashMap<String, usize>)>,
}

impl Shared {
    /// Puts `files` in place of your own files (they come first, the game's
    /// songs after) and works out the folders again.
    /// Notes that the song lists changed (see `lists_rev`).
    pub fn changed(&mut self) {
        self.lists_rev = self.lists_rev.wrapping_add(1);
    }

    /// Whether `list` has nothing in it at all.
    pub fn list_is_empty(&self, list: List) -> bool {
        self.keys(list).is_empty()
    }

    pub fn set_files(&mut self, files: Vec<Track>) {
        self.changed();
        // The same number of songs can still be different songs.
        self.index.borrow_mut().0 = usize::MAX;
        let game: Vec<_> = self.tracks.drain(..).filter(|t| t.is_game()).collect();
        self.tracks = files;
        self.tracks.extend(game);
        let mut folders: Vec<(String, usize)> = Vec::new();
        let mut names: Vec<&str> =
            self.tracks.iter().filter_map(|t| t.key.strip_prefix("file:")?.split_once('/').map(|(dir, _)| dir)).collect();
        names.sort_unstable();
        for name in names {
            match folders.last_mut() {
                Some((last, n)) if last == name => *n += 1,
                _ => folders.push((name.to_string(), 1)),
            }
        }
        self.folders = folders;
        self.apply_titles();
    }

    /// Queues song `i` of the list as shown now.
    pub fn play(&mut self, i: usize) {
        if let Some(t) = self.tracks.get(i) {
            let key = t.key.clone();
            self.push(Command::Play(key));
        }
    }

    /// Queues a command for the player. On a game build the jukebox doesn't
    /// support, the player never runs, so nothing is queued.
    pub fn push(&mut self, command: Command) {
        if !crate::UNSUPPORTED.load(std::sync::atomic::Ordering::Relaxed) {
            self.commands.push_back(command);
        }
    }

    /// Queues the config for writing on the writer thread: the menu and the
    /// player call this often, and neither should wait on the disk.
    pub fn save_config(&self) {
        self.config.save_in_background(&self.config_path);
    }

    fn keys(&self, list: List) -> &[String] {
        match list {
            List::Favorites => &self.config.favorites,
            List::Hidden => &self.config.hidden,
            List::Custom(p) => self.config.playlists.get(p).map_or(&[], |p| &p.tracks),
        }
    }

    fn keys_mut(&mut self, list: List) -> Option<&mut Vec<String>> {
        match list {
            List::Favorites => Some(&mut self.config.favorites),
            List::Hidden => Some(&mut self.config.hidden),
            List::Custom(p) => self.config.playlists.get_mut(p).map(|p| &mut p.tracks),
        }
    }

    pub fn contains(&self, list: List, i: usize) -> bool {
        self.tracks.get(i).is_some_and(|t| self.keys(list).contains(&t.key))
    }

    /// Adds song `i` to `list`, or takes it out if it is already there.
    pub fn toggle(&mut self, list: List, i: usize) {
        self.changed();
        let Some(key) = self.tracks.get(i).map(|t| t.key.clone()) else { return };
        let Some(keys) = self.keys_mut(list) else { return };
        match keys.iter().position(|k| *k == key) {
            Some(p) => {
                keys.remove(p);
            }
            None => keys.push(key),
        }
        self.save_config();
    }

    /// Moves song `from` to where song `to` is in `list`, for drag and drop;
    /// the songs between shift along by one.
    pub fn move_to(&mut self, list: List, from: usize, to: usize) {
        self.changed();
        let (Some(a), Some(b)) = (self.tracks.get(from), self.tracks.get(to)) else { return };
        let (a, b) = (a.key.clone(), b.key.clone());
        let Some(keys) = self.keys_mut(list) else { return };
        let (Some(pa), Some(pb)) = (keys.iter().position(|k| *k == a), keys.iter().position(|k| *k == b)) else {
            return;
        };
        if pa != pb {
            let key = keys.remove(pa);
            keys.insert(pb, key);
            self.save_config();
        }
    }

    /// `list`'s songs in play order, as track indices. A song that is
    /// missing (a deleted file, game songs not listed yet, or a game song
    /// with no audio) is skipped.
    #[cfg(test)]
    pub fn indices(&self, list: List) -> Vec<usize> {
        self.indices_all(list).into_iter().filter(|&i| !self.is_unplayable(i)).collect()
    }

    /// [`indices`](Self::indices), keeping songs that will not play too:
    /// the Hidden tab lists those as well, so they can still be unhidden.
    pub fn indices_all(&self, list: List) -> Vec<usize> {
        self.keys(list).iter().filter_map(|k| self.index_of(k)).collect()
    }

    pub fn find_playlist(&self, name: &str) -> Option<usize> {
        self.config.playlists.iter().position(|p| p.name == name)
    }

    /// `name`, or `name 2`, `name 3`... if a playlist already has it.
    fn unique_name(&self, name: &str) -> String {
        let base = name.trim();
        let base = if base.is_empty() { "New playlist" } else { base };
        let mut n = 1;
        loop {
            let candidate = if n == 1 { base.to_string() } else { format!("{base} {n}") };
            if self.find_playlist(&candidate).is_none() {
                return candidate;
            }
            n += 1;
        }
    }

    /// Makes an empty playlist and returns its index.
    pub fn create_playlist(&mut self, name: &str) -> usize {
        self.changed();
        let name = self.unique_name(name);
        self.config.playlists.push(CustomPlaylist { name, tracks: Vec::new() });
        self.save_config();
        self.config.playlists.len() - 1
    }

    pub fn rename_playlist(&mut self, p: usize, name: &str) {
        self.changed();
        let Some(old) = self.config.playlists.get(p).map(|p| p.name.clone()) else { return };
        if old == name.trim() {
            return;
        }
        let name = self.unique_name(name);
        let (old, new) = (Playlist::Custom(old), Playlist::Custom(name.clone()));
        if self.config.playlist == old {
            self.config.playlist = new.clone();
        }
        if self.config.battle_playlist.as_ref() == Some(&old) {
            self.config.battle_playlist = Some(new.clone());
        }
        for area in crate::config::Area::ALL {
            let music = self.config.area_music_mut(area);
            if *music == crate::config::AreaMusic::List(old.clone()) {
                *music = crate::config::AreaMusic::List(new.clone());
            }
        }
        self.config.playlists[p].name = name;
        self.save_config();
    }

    pub fn delete_playlist(&mut self, p: usize) {
        self.changed();
        if p >= self.config.playlists.len() {
            return;
        }
        let removed = Playlist::Custom(self.config.playlists.remove(p).name);
        if self.config.playlist == removed {
            self.config.playlist = Playlist::AllSongs;
        }
        if self.config.battle_playlist.as_ref() == Some(&removed) {
            self.config.battle_playlist = None;
        }
        for area in crate::config::Area::ALL {
            let music = self.config.area_music_mut(area);
            if *music == crate::config::AreaMusic::List(removed.clone()) {
                *music = crate::config::AreaMusic::PlayFrom;
            }
        }
        self.save_config();
    }

    /// The folders inside the music folder that hold songs, each one a
    /// playlist of its own: by name, with how many songs each holds.
    pub fn folders(&self) -> &[(String, usize)] {
        &self.folders
    }

    /// Every list songs can be played from: All songs, Favorites, Game
    /// themes, your playlists, then the music folder's folders.
    pub fn play_from_choices(&self) -> Vec<Playlist> {
        [Playlist::AllSongs, Playlist::Favorites, Playlist::GameThemes]
            .into_iter()
            .chain(self.config.playlists.iter().map(|p| Playlist::Custom(p.name.clone())))
            .chain(self.folders.iter().map(|(name, _)| Playlist::Folder(name.clone())))
            .collect()
    }

    /// Gives song `i` your own name, or its default one back with `None`.
    pub fn rename_track(&mut self, i: usize, name: Option<&str>) {
        self.changed();
        let Some(track) = self.tracks.get(i) else { return };
        let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
        match &track.source {
            Source::Game { cue, .. } => {
                let cue = cue.clone();
                match &name {
                    Some(n) => self.config.titles.insert(cue, n.clone()),
                    None => self.config.titles.remove(&cue),
                };
            }
            Source::File(_) => {
                let key = track.key.clone();
                match &name {
                    Some(n) => self.config.file_titles.insert(key, n.clone()),
                    None => self.config.file_titles.remove(&key),
                };
            }
        }
        self.apply_titles();
        self.save_config();
    }

    /// The track with `key`, by its place in the song list now.
    pub fn index_of(&self, key: &str) -> Option<usize> {
        let mut index = self.index.borrow_mut();
        let found = |map: &HashMap<String, usize>| {
            map.get(key).copied().filter(|&i| self.tracks.get(i).is_some_and(|t| t.key == key))
        };
        if index.0 == self.tracks.len() {
            match (found(&index.1), index.1.contains_key(key)) {
                (Some(i), _) => return Some(i),
                (None, false) => return None,
                // Listed at a place that holds another song now: rebuilt.
                (None, true) => {}
            }
        }
        index.1 = self.tracks.iter().enumerate().map(|(i, t)| (t.key.clone(), i)).collect();
        index.0 = self.tracks.len();
        found(&index.1)
    }

    pub fn is_renamed(&self, i: usize) -> bool {
        self.tracks.get(i).is_some_and(|t| t.title != t.base_title)
    }

    /// Puts your own names on the songs, and their default names on the rest.
    pub fn apply_titles(&mut self) {
        for t in &mut self.tracks {
            let own = match &t.source {
                Source::Game { cue, .. } => self.config.titles.get(cue),
                Source::File(_) => self.config.file_titles.get(&t.key),
            };
            t.title = own.cloned().unwrap_or_else(|| t.base_title.clone());
        }
    }

    /// The songs Next and Shuffle pick from, leaving out songs that will not
    /// play. Falls back to every song while the chosen playlist has nothing
    /// playable, so Play never does nothing.
    #[cfg(test)]
    pub fn playlist(&self) -> Vec<usize> {
        self.playlist_keeping(None)
    }

    /// [`playlist`](Self::playlist), but keeping song `keep` in place even
    /// if it will not play, so the song after it can be found.
    #[cfg(test)]
    pub fn playlist_keeping(&self, keep: Option<usize>) -> Vec<usize> {
        self.songs_of(&self.config.playlist, keep)
    }

    /// The songs of `which` in play order, leaving out the ones that won't
    /// play except `keep` (the song playing or just failed, so the list goes
    /// on after it); also used for the battle and area music.
    pub fn songs_of(&self, which: &Playlist, keep: Option<usize>) -> Vec<usize> {
        if let Some(songs) = self.songs_of_only(which, keep) {
            return songs;
        }
        // Hidden songs stay out of "All songs", but a song you put in a
        // playlist yourself plays from it either way.
        let playable = |i: &usize| Some(*i) == keep || !self.is_unplayable(*i);
        let all = (0..self.tracks.len()).filter(playable);
        let shown: Vec<usize> = all.clone().filter(|&i| !self.contains(List::Hidden, i)).collect();
        if shown.is_empty() { all.collect() } else { shown }
    }

    /// The playable songs of `which` itself: `None` for "All songs", and for
    /// a list with nothing playable in it, so a battle or area list that has
    /// run empty can fall back to "Play from" rather than to every song.
    pub fn songs_of_only(&self, which: &Playlist, keep: Option<usize>) -> Option<Vec<usize>> {
        let playable = |i: &usize| Some(*i) == keep || !self.is_unplayable(*i);
        // Skipped songs are left out once, below, so `keep` (the song that
        // just failed) keeps its place and the list goes on after it.
        let chosen: Vec<usize> = match which {
            Playlist::AllSongs => return None,
            Playlist::Favorites => self.indices_all(List::Favorites),
            // Like All songs, hidden songs stay out.
            Playlist::GameThemes => (0..self.tracks.len())
                .filter(|&i| self.tracks[i].is_game_loop() && !self.contains(List::Hidden, i))
                .collect(),
            Playlist::Custom(name) => self.indices_all(List::Custom(self.find_playlist(name)?)),
            Playlist::Folder(name) => {
                let prefix = format!("file:{name}/");
                (0..self.tracks.len()).filter(|&i| self.tracks[i].key.starts_with(&prefix)).collect()
            }
        };
        let songs: Vec<usize> = chosen.into_iter().filter(playable).collect();
        (!songs.is_empty()).then_some(songs)
    }

    /// Whether `which` has any playable song, as `songs_of_only(which,
    /// None).is_some()` but without building the list: asked every tick.
    pub fn has_songs(&self, which: &Playlist) -> bool {
        let playable = |i: usize| !self.is_unplayable(i);
        match which {
            Playlist::AllSongs => false,
            Playlist::Favorites => self.keys(List::Favorites).iter().filter_map(|k| self.index_of(k)).any(playable),
            Playlist::GameThemes => (0..self.tracks.len())
                .any(|i| self.tracks[i].is_game_loop() && !self.contains(List::Hidden, i) && playable(i)),
            Playlist::Custom(name) => self.find_playlist(name).is_some_and(|p| {
                self.keys(List::Custom(p)).iter().filter_map(|k| self.index_of(k)).any(playable)
            }),
            Playlist::Folder(name) => self.tracks.iter().enumerate().any(|(i, t)| {
                t.key.strip_prefix("file:").and_then(|k| k.strip_prefix(name.as_str())).is_some_and(|k| k.starts_with('/'))
                    && playable(i)
            }),
        }
    }

    pub fn is_unplayable_key(&self, key: &str) -> bool {
        self.is_silent_key(key) || self.skipped.contains_key(key)
    }

    /// Found by the song check to have no audio at all (for good, on this
    /// game build), unlike a song only skipped this session.
    fn is_silent_key(&self, key: &str) -> bool {
        self.config.unplayable.iter().any(|k| k == key)
    }

    pub fn is_silent(&self, i: usize) -> bool {
        self.tracks.get(i).is_some_and(|t| self.is_silent_key(&t.key))
    }

    /// Why song `i` was skipped this session, if it was.
    pub fn skip_reason(&self, i: usize) -> Option<&str> {
        self.tracks.get(i).and_then(|t| self.skipped.get(&t.key)).map(String::as_str)
    }

    /// `list`'s songs as the menu lists them: songs skipped this session stay
    /// (greyed out, to try again), songs with no audio at all don't.
    pub fn indices_shown(&self, list: List) -> Vec<usize> {
        self.indices_all(list).into_iter().filter(|&i| !self.is_silent(i)).collect()
    }

    pub fn is_unplayable(&self, i: usize) -> bool {
        self.tracks.get(i).is_some_and(|t| self.is_unplayable_key(&t.key))
    }

    /// Skips song `i` for the rest of the session (it would not play), with
    /// why. Nothing is written to the config.
    pub fn skip(&mut self, i: usize, why: impl Into<String>) {
        self.changed();
        let Some(key) = self.tracks.get(i).map(|t| t.key.clone()) else { return };
        self.skipped.insert(key, why.into());
    }

    /// Lets a skipped song play again.
    pub fn unskip(&mut self, i: usize) {
        self.changed();
        if let Some(t) = self.tracks.get(i) {
            self.skipped.remove(&t.key);
        }
    }

    /// Records whether song `i` has audio, as the song check found; saved
    /// only on a change. A song found playing is no longer skipped either.
    pub fn set_unplayable(&mut self, i: usize, unplayable: bool) {
        let Some(key) = self.tracks.get(i).map(|t| t.key.clone()) else { return };
        self.set_unplayable_key(&key, unplayable);
    }

    /// [`set_unplayable`](Self::set_unplayable) by track key.
    pub fn set_unplayable_key(&mut self, key: &str, unplayable: bool) {
        self.changed();
        if !unplayable {
            self.skipped.remove(key);
        }
        let known = self.config.unplayable.iter().position(|k| k == key);
        match (known, unplayable) {
            (None, true) => self.config.unplayable.push(key.to_string()),
            (Some(p), false) => {
                self.config.unplayable.remove(p);
            }
            _ => return,
        }
        self.save_config();
    }

    pub fn unhide_all(&mut self) {
        self.changed();
        self.config.hidden.clear();
        self.save_config();
    }
}

/// A list of songs you can add to: Favorites, Hidden, or one of your own
/// playlists by its index in `config.playlists`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum List {
    Favorites,
    Hidden,
    Custom(usize),
}

static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();

pub fn init(shared: Shared) {
    let _ = SHARED.set(Mutex::new(shared));
}

pub fn lock() -> MutexGuard<'static, Shared> {
    let m = SHARED.get().expect("state used before init");
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the decoder can play: no Opus (rodio has no decoder for it), so an
/// .opus file is left out rather than listed and then failing.
const EXTENSIONS: [&str; 6] = ["mp3", "ogg", "flac", "wav", "m4a", "aac"];

/// Every playable file under `dir`, sorted by path, titled by its path
/// relative to `dir` without the extension.
pub fn scan_folder(dir: &Path) -> Vec<Track> {
    // Deep enough for any music library; a linked folder that leads back up
    // the tree stops here instead of going round for ever.
    const MAX_DEPTH: u32 = 12;
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < MAX_DEPTH {
                    walk(&path, out, depth + 1);
                }
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            {
                out.push(path);
            }
        }
    }
    let _ = std::fs::create_dir_all(dir);
    let mut files = Vec::new();
    walk(dir, &mut files, 0);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let rel = path.strip_prefix(dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let title = Path::new(&rel).with_extension("").to_string_lossy().replace('/', " / ");
            Track { base_title: title.clone(), title, key: format!("file:{rel}"), source: Source::File(path) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        let mut s = Shared::default();
        s.config_path = std::env::temp_dir().join("ffrs_jukebox_test.toml");
        s.tracks = (0..3).map(|i| Track::game(format!("BGM_PP_T{i}"), true)).collect();
        s
    }

    #[test]
    fn hidden_songs_leave_all_songs_only() {
        let mut s = shared();
        s.toggle(List::Hidden, 1);
        assert_eq!(s.playlist(), vec![0, 2]);
        // In a playlist you made, a hidden song still plays.
        let p = s.create_playlist("Mine");
        s.toggle(List::Custom(p), 1);
        s.config.playlist = Playlist::Custom("Mine".into());
        assert_eq!(s.playlist(), vec![1]);
        // Unhiding puts it back.
        s.config.playlist = Playlist::AllSongs;
        s.toggle(List::Hidden, 1);
        assert_eq!(s.playlist(), vec![0, 1, 2]);
    }

    #[test]
    fn songs_without_audio_leave_every_list() {
        let mut s = shared();
        s.toggle(List::Favorites, 1);
        s.toggle(List::Favorites, 2);
        s.set_unplayable(1, true);
        assert_eq!(s.indices(List::Favorites), vec![2]);
        assert_eq!(s.playlist(), vec![0, 2]);
        // The failed song is kept only to find the one after it.
        assert_eq!(s.playlist_keeping(Some(1)), vec![0, 1, 2]);
        s.set_unplayable(1, false);
        assert_eq!(s.indices(List::Favorites), vec![1, 2]);
    }

    #[test]
    fn folders_are_playlists_and_renames_come_and_go() {
        let mut s = shared();
        let file = |rel: &str| Track {
            title: rel.into(),
            base_title: rel.into(),
            key: format!("file:{rel}"),
            source: Source::File(rel.into()),
        };
        s.set_files(vec![file("Chill/a.mp3"), file("Chill/b.mp3"), file("Rock/c.mp3"), file("loose.mp3")]);
        assert_eq!(s.folders(), [("Chill".to_string(), 2), ("Rock".to_string(), 1)]);
        assert_eq!(s.songs_of(&Playlist::Folder("Chill".into()), None), vec![0, 1]);
        assert!(s.play_from_choices().contains(&Playlist::Folder("Rock".into())));
        // A game song and a file renamed, then one put back.
        s.rename_track(4, Some("Opening"));
        s.rename_track(2, Some("Rock out"));
        assert_eq!(s.tracks[4].title, "Opening");
        assert_eq!(s.tracks[2].title, "Rock out");
        assert!(s.is_renamed(2));
        s.rename_track(2, None);
        assert_eq!(s.tracks[2].title, "Rock/c.mp3");
        assert_eq!(s.config.titles.get("BGM_PP_T0").map(String::as_str), Some("Opening"));
    }

    #[test]
    fn a_skipped_song_leaves_the_lists_for_the_session_only() {
        let mut s = shared();
        s.toggle(List::Favorites, 1);
        s.skip(1, "Could not decode");
        assert!(s.is_unplayable(1));
        assert_eq!(s.indices(List::Favorites), Vec::<usize>::new());
        // The menu still lists it, with why, so it can be tried again.
        assert_eq!(s.indices_shown(List::Favorites), vec![1]);
        assert_eq!(s.skip_reason(1), Some("Could not decode"));
        assert!(s.config.unplayable.is_empty(), "skipping must not touch the config");
        // The check finding it playing clears the skip.
        s.set_unplayable(1, false);
        assert!(!s.is_unplayable(1));
        // A list that has nothing playable is empty, not everything.
        s.skip(1, "Could not decode");
        assert_eq!(s.songs_of_only(&Playlist::Favorites, None), None);
        assert_eq!(s.songs_of(&Playlist::Favorites, None), vec![0, 2]);
        // The Hidden tab still lists a skipped song, so it can be unhidden.
        s.toggle(List::Hidden, 1);
        assert_eq!(s.indices_all(List::Hidden), vec![1]);
    }

    #[test]
    fn everything_hidden_still_plays() {
        let mut s = shared();
        for i in 0..3 {
            s.toggle(List::Hidden, i);
        }
        assert_eq!(s.playlist(), vec![0, 1, 2]);
    }

    #[test]
    fn a_click_plays_the_song_clicked_even_after_a_rescan() {
        let mut s = shared();
        s.play(1);
        let Some(Command::Play(key)) = s.commands.pop_front() else { panic!("no Play queued") };
        // A rescan lands a song in front of it before the player gets there.
        s.tracks.insert(0, Track::game("BGM_PP_NEW".into(), true));
        assert_eq!(s.index_of(&key), Some(2));
        assert_eq!(s.tracks[2].key, key);
    }

    #[test]
    fn game_themes_leave_out_jingles_hidden_songs_and_files() {
        let mut s = shared();
        s.tracks.push(Track::game("BGM_PP_FANFARE".into(), false));
        s.set_files(vec![Track {
            title: "a".into(),
            base_title: "a".into(),
            key: "file:a.mp3".into(),
            source: Source::File("a.mp3".into()),
        }]);
        // The file first, then the three looping themes, then the jingle.
        s.toggle(List::Hidden, 2);
        assert_eq!(s.songs_of(&Playlist::GameThemes, None), vec![1, 3]);
        assert!(s.play_from_choices().contains(&Playlist::GameThemes));
    }

    #[test]
    fn song_lookup_follows_every_change_to_the_list() {
        let file = |rel: &str| Track {
            title: rel.into(),
            base_title: rel.into(),
            key: format!("file:{rel}"),
            source: Source::File(rel.into()),
        };
        let mut s = shared();
        s.set_files(vec![file("a.mp3"), file("b.mp3")]);
        assert_eq!(s.index_of("file:b.mp3"), Some(1));
        // The same number of files, one renamed on disk.
        s.set_files(vec![file("a.mp3"), file("c.mp3")]);
        assert_eq!(s.index_of("file:b.mp3"), None);
        assert_eq!(s.index_of("file:c.mp3"), Some(1));
        // Songs added straight to the list, in front.
        s.tracks.insert(0, file("first.mp3"));
        assert_eq!(s.index_of("file:c.mp3"), Some(2));
        assert_eq!(s.index_of("file:first.mp3"), Some(0));
    }

    #[test]
    fn a_failed_song_in_a_playlist_hands_on_to_the_one_after_it() {
        let mut s = shared();
        let p = s.create_playlist("Mine");
        for i in 0..3 {
            s.toggle(List::Favorites, i);
            s.toggle(List::Custom(p), i);
        }
        s.skip(1, "Could not decode");
        // The failed song keeps its place, so the list goes on after it...
        for list in [Playlist::Favorites, Playlist::Custom("Mine".into())] {
            assert_eq!(s.songs_of(&list, Some(1)), vec![0, 1, 2]);
            // ...and is passed over otherwise.
            assert_eq!(s.songs_of(&list, None), vec![0, 2]);
        }
    }

    #[test]
    fn the_quick_check_for_songs_agrees_with_the_full_list() {
        let mut s = shared();
        let file = |rel: &str| Track {
            title: rel.into(),
            base_title: rel.into(),
            key: format!("file:{rel}"),
            source: Source::File(rel.into()),
        };
        s.set_files(vec![file("Chill/a.mp3"), file("Chillout/b.mp3")]);
        let p = s.create_playlist("Mine");
        s.toggle(List::Custom(p), 3);
        s.toggle(List::Favorites, 0);
        let lists = [
            Playlist::Favorites,
            Playlist::GameThemes,
            Playlist::Custom("Mine".into()),
            Playlist::Custom("Gone".into()),
            Playlist::Folder("Chill".into()),
            Playlist::Folder("Chillout".into()),
            Playlist::Folder("Nope".into()),
        ];
        let agree = |s: &Shared| {
            for list in &lists {
                assert_eq!(s.has_songs(list), s.songs_of_only(list, None).is_some(), "{list:?}");
            }
        };
        agree(&s);
        // A folder's only song skipped, a playlist's only song with no audio.
        s.skip(0, "Could not decode");
        s.set_unplayable(3, true);
        agree(&s);
        assert!(!s.has_songs(&Playlist::Folder("Chill".into())));
        assert!(s.has_songs(&Playlist::Folder("Chillout".into())));
    }
}
