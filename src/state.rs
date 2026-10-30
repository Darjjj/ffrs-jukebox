//! State shared between the menu (render thread) and the player (game thread).
//! The menu only reads it and queues [`Command`]s; the player is the only side
//! that touches audio or the game.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::config::{Config, CustomPlaylist, Playlist, Scene};

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

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Play(usize),
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
    pub error: Option<String>,
}

impl Shared {
    pub fn push(&mut self, command: Command) {
        self.commands.push_back(command);
    }

    pub fn save_config(&self) {
        self.config.save(&self.config_path);
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
    pub fn indices(&self, list: List) -> Vec<usize> {
        self.keys(list)
            .iter()
            .filter_map(|k| self.tracks.iter().position(|t| t.key == *k))
            .filter(|&i| !self.is_unplayable(i))
            .collect()
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
        let name = self.unique_name(name);
        self.config.playlists.push(CustomPlaylist { name, tracks: Vec::new() });
        self.save_config();
        self.config.playlists.len() - 1
    }

    pub fn rename_playlist(&mut self, p: usize, name: &str) {
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

    /// The folders inside the music folder that hold songs, by name, each
    /// one a playlist of its own.
    pub fn folders(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tracks
            .iter()
            .filter_map(|t| t.key.strip_prefix("file:")?.split_once('/').map(|(dir, _)| dir.to_string()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Every list songs can be played from: All songs, Favorites, your
    /// playlists, then the music folder's folders.
    pub fn play_from_choices(&self) -> Vec<Playlist> {
        [Playlist::AllSongs, Playlist::Favorites]
            .into_iter()
            .chain(self.config.playlists.iter().map(|p| Playlist::Custom(p.name.clone())))
            .chain(self.folders().into_iter().map(Playlist::Folder))
            .collect()
    }

    /// Gives song `i` your own name, or its default one back with `None`.
    pub fn rename_track(&mut self, i: usize, name: Option<&str>) {
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

    /// The songs of `which`, as [`playlist_keeping`](Self::playlist_keeping)
    /// gives them for "Play from"; also used for the battle music.
    pub fn songs_of(&self, which: &Playlist, keep: Option<usize>) -> Vec<usize> {
        let playable = |i: &usize| Some(*i) == keep || !self.is_unplayable(*i);
        let chosen: Option<Vec<usize>> = match which {
            Playlist::AllSongs => None,
            Playlist::Favorites => Some(self.indices(List::Favorites)),
            Playlist::Custom(name) => self.find_playlist(name).map(|p| self.indices(List::Custom(p))),
            Playlist::Folder(name) => {
                let prefix = format!("file:{name}/");
                Some((0..self.tracks.len()).filter(|&i| self.tracks[i].key.starts_with(&prefix)).collect())
            }
        };
        if let Some(songs) =
            chosen.map(|s| s.into_iter().filter(playable).collect::<Vec<_>>()).filter(|s| !s.is_empty())
        {
            return songs;
        }
        // Hidden songs stay out of "All songs", but a song you put in a
        // playlist yourself plays from it either way.
        let all = (0..self.tracks.len()).filter(playable);
        let shown: Vec<usize> = all.clone().filter(|&i| !self.contains(List::Hidden, i)).collect();
        if shown.is_empty() { all.collect() } else { shown }
    }

    pub fn is_unplayable_key(&self, key: &str) -> bool {
        self.config.unplayable.iter().any(|k| k == key)
    }

    pub fn is_unplayable(&self, i: usize) -> bool {
        self.tracks.get(i).is_some_and(|t| self.config.unplayable.contains(&t.key))
    }

    /// Records whether song `i` played, saving only on a change.
    pub fn set_unplayable(&mut self, i: usize, unplayable: bool) {
        let Some(key) = self.tracks.get(i).map(|t| t.key.clone()) else { return };
        let known = self.config.unplayable.iter().position(|k| *k == key);
        match (known, unplayable) {
            (None, true) => self.config.unplayable.push(key),
            (Some(p), false) => {
                self.config.unplayable.remove(p);
            }
            _ => return,
        }
        self.save_config();
    }

    pub fn unhide_all(&mut self) {
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

const EXTENSIONS: [&str; 7] = ["mp3", "ogg", "flac", "wav", "m4a", "aac", "opus"];

/// Every playable file under `dir`, sorted by path, titled by its path
/// relative to `dir` without the extension.
pub fn scan_folder(dir: &Path) -> Vec<Track> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
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
    walk(dir, &mut files);
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
        s.tracks.push(file("Chill/a.mp3"));
        s.tracks.push(file("Chill/b.mp3"));
        s.tracks.push(file("Rock/c.mp3"));
        s.tracks.push(file("loose.mp3"));
        assert_eq!(s.folders(), vec!["Chill".to_string(), "Rock".to_string()]);
        assert_eq!(s.songs_of(&Playlist::Folder("Chill".into()), None), vec![3, 4]);
        assert!(s.play_from_choices().contains(&Playlist::Folder("Rock".into())));
        // A game song and a file renamed, then one put back.
        s.rename_track(0, Some("Opening"));
        s.rename_track(5, Some("Rock out"));
        assert_eq!(s.tracks[0].title, "Opening");
        assert_eq!(s.tracks[5].title, "Rock out");
        assert!(s.is_renamed(5));
        s.rename_track(5, None);
        assert_eq!(s.tracks[5].title, "Rock/c.mp3");
        assert_eq!(s.config.titles.get("BGM_PP_T0").map(String::as_str), Some("Opening"));
    }

    #[test]
    fn everything_hidden_still_plays() {
        let mut s = shared();
        for i in 0..3 {
            s.toggle(List::Hidden, i);
        }
        assert_eq!(s.playlist(), vec![0, 1, 2]);
    }
}
