//! Playback: runs the queued commands, follows the mode, advances the
//! playlist. Lives on the game thread (see [`crate::game`]).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source as _};

use crate::check::SongCheck;
use crate::loudness;
use crate::config::{Area, AreaMusic, Playlist, Repeat};
use crate::game::{BgmMute, Game, refresh_game_tracks};
use crate::state::{self, Command, Source, Track};

/// How long after the audio output failed to open it is tried again: a
/// player who plugs in headphones (or whose device was busy) gets their own
/// music back without restarting the game.
const OUTPUT_RETRY: Duration = Duration::from_secs(3);

/// The audio output for your own files. The stream must stay on the thread
/// that opened it, so it is opened on a thread of its own that keeps it alive
/// for the life of the process. A failure is not kept for good: the next file
/// to play after [`OUTPUT_RETRY`] tries again.
static OUTPUT: Mutex<(Option<OutputStreamHandle>, Option<Instant>)> = Mutex::new((None, None));
/// The game's window is gone: no output is opened until it comes back.
static CLOSING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn output() -> Option<OutputStreamHandle> {
    if CLOSING.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let mut out = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(handle) = &out.0 {
        return Some(handle.clone());
    }
    if out.1.is_some_and(|failed| failed.elapsed() < OUTPUT_RETRY) {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || match OutputStream::try_default() {
        Ok((stream, handle)) => {
            let _ = tx.send(Some(handle));
            // Kept until the game closes (see `silence_for_exit`): dropping
            // the stream silences every file at once.
            let _ = stop_rx.recv();
            drop(stream);
        }
        Err(e) => {
            log::error!("player: no audio output: {e}");
            let _ = tx.send(None);
        }
    });
    let handle = rx.recv().ok().flatten();
    match &handle {
        Some(_) if out.1.is_some() => log::info!("player: audio output is back"),
        Some(_) => {}
        None => out.1 = Some(Instant::now()),
    }
    if handle.is_some() {
        *STOP_OUTPUT.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop_tx);
    }
    out.0 = handle.clone();
    handle
}

/// Tells the audio output's thread to close it.
static STOP_OUTPUT: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

/// Silences your own music as the game closes. The game takes a
/// moment to shut down after its window goes, and a file kept playing
/// through it; the game's own songs stop with the game.
pub fn silence_for_exit() {
    CLOSING.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(stop) = STOP_OUTPUT.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = stop.send(());
        log::info!("player: the game is closing, own music silenced");
    }
    // The handle to the closed stream is no use to anything after it.
    OUTPUT.lock().unwrap_or_else(|e| e.into_inner()).0 = None;
}

/// The game's window came back after all (a display mode change, not the
/// game closing): your own music can open its output again.
pub fn reopen_output() {
    CLOSING.store(false, std::sync::atomic::Ordering::Relaxed);
    log::info!("player: the game's window is back, own music can play again");
}

/// Opens the audio output ahead of time, off the game thread, so the first
/// file to play doesn't wait for the device.
pub fn warm_up() {
    std::thread::spawn(|| {
        output();
    });
}

/// How long a game cue may take to start before it counts as failed. Some
/// cues are not reported as playing for a moment after PlaySound2D, while
/// the game loads them.
const CUE_START_GRACE: Duration = Duration::from_secs(4);

/// Some cues are cut off the moment they start, apparently while the game is
/// still releasing the song stopped just before. Such a cue is started again
/// this many times, this long apart, before it counts as failed.
const CUE_RETRIES: u8 = 3;
const CUE_RETRY_DELAY: Duration = Duration::from_millis(400);

/// How long a looping theme takes to fade out at the end of its time.
const THEME_FADE: f32 = 4.0;

/// The most the volume rises per tick (50 ms): 0 to 150% in 0.75 s.
const VOLUME_RISE: f32 = 0.1;

/// How long the jukebox fades out under the victory fanfare, and back in
/// once the game moves on from it.
const FANFARE_FADE: f32 = 1.5;

enum Playing {
    Cue {
        id: i32,
        duration: Option<f32>,
        paused: bool,
        started: Instant,
        /// The game has reported this cue as playing at least once.
        seen: bool,
        /// Furthest play position seen, to tell a cut-off start from an end.
        reached: f32,
        attempt: u8,
        /// A looping theme, faded out over its last [`THEME_FADE`] seconds.
        fades: bool,
        fading: bool,
    },
    File {
        /// Shared with a seek in progress (see `Command::Seek`).
        sink: std::sync::Arc<Sink>,
        duration: Option<f32>,
        /// The file's key in `config.file_levels` and `file_peaks`.
        key: String,
        /// Where the file was started: the sink counts from there.
        offset: f32,
        /// The file is still being opened on a thread of its own (a slow
        /// drive or a big file mustn't hold up the game); `tick` takes it.
        opening: Option<Receiver<Result<Opened, String>>>,
    },
}

impl Playing {
    /// How far into a file it is.
    fn file_pos(sink: &Sink, offset: f32) -> f32 {
        offset + sink.get_pos().as_secs_f32()
    }
}

/// A file opened, decoded and moved to its start, ready for its sink.
struct Opened {
    decoder: Decoder<BufReader<File>>,
    duration: Option<f32>,
    key: String,
    /// Where it starts: 0 if it couldn't seek.
    at: f32,
}

pub struct Player {
    now: Option<(usize, Playing)>,
    mute: BgmMute,
    /// Tracks played, for Previous in shuffle.
    history: Vec<usize>,
    /// The other side's history, swapped in as a battle starts or ends, so
    /// Previous stays within the battle music or the exploring music.
    other_history: Vec<usize>,
    volume: f32,
    /// A cut-off cue waiting to be started again: track, attempt, when.
    retry: Option<(usize, u8, Instant)>,
    /// The current song was picked by Next, Shuffle or a playlist rather
    /// than clicked, so if it will not play the next one is tried.
    auto: bool,
    /// The silent pass over every game song, while it runs.
    check: Option<SongCheck>,
    /// The volume actually applied, catching up with the setting.
    ramped: f32,
    /// Shuffle's remaining songs, drawn without repeats until each runs out:
    /// one deal per side (battle or not) and playlist.
    bags: std::collections::HashMap<(bool, Vec<usize>), Vec<usize>>,
    /// Playing from the battle music, and the exploring song (track, position)
    /// to pick up again once the battle ends.
    in_battle: bool,
    battle_resume: Option<(usize, f32)>,
    /// In a battle the exploring song carries on or waits through (no
    /// battle music list, or a boss keeping its music), and where that song
    /// was as it began.
    fight: bool,
    fight_resume: Option<(usize, f32)>,
    /// The Play next song that's playing, and the song it cut in after (by
    /// key), so the list carries on from that one afterwards.
    after_up_next: Option<(String, Option<String>)>,
    /// The map last settled on while exploring, and a different one seen
    /// since and when it was first seen.
    map_id: i32,
    map_new: Option<(i32, Instant)>,
    /// The kind of place that map is (field, town, dungeon).
    area: Option<Area>,
    shrine: bool,
    /// The list songs came from as of the last tick, to tell whether a
    /// changed list choice is the one playing.
    last_list: Playlist,
    /// The next start is a change of song, so it crossfades.
    crossfade_next: bool,
    /// When the current song started fading in, and over how long.
    fade_in: Option<(Instant, f32)>,
    /// Files still fading out under the next song: sink, start, its level
    /// then, and how long.
    fading_files: Vec<(std::sync::Arc<Sink>, Instant, f32, f32)>,
    /// The last song from the previous session has been dealt with.
    resumed: bool,
    /// When the current song and position were last saved for resuming.
    saved_at: Instant,
    /// 1.0 normally, falling to 0.0 under the victory fanfare, and when it
    /// was last stepped.
    duck: f32,
    duck_at: Instant,
    /// This battle's fanfare has played; cleared once the battle is over.
    after_fanfare: bool,
    /// A song that would not start, whose next song is tried on the next
    /// tick (see [`Player::skip_on`]).
    skip_from: Option<usize>,
    /// A rescan of the music folder running on a thread of its own; its
    /// result is taken in on the tick it arrives.
    scan: Option<Receiver<Vec<Track>>>,
}

/// What plays while exploring a place of this kind; a Vision shrine can be
/// left to the game's own music whatever the dungeon setting.
fn area_source(s: &state::Shared, area: Option<Area>, shrine: bool) -> AreaMusic {
    if shrine && s.config.shrine_game_music {
        return AreaMusic::Game;
    }
    area.map_or(AreaMusic::PlayFrom, |a| s.config.area_music(a).clone())
}

/// How long a new map has to hold before it counts as entered.
const MAP_SETTLE: Duration = Duration::from_millis(1500);

/// How often the current song and position are saved for resuming.
const RESUME_SAVE_EVERY: Duration = Duration::from_secs(10);

impl Player {
    pub fn new() -> Self {
        Self {
            now: None,
            mute: BgmMute::default(),
            history: Vec::new(),
            other_history: Vec::new(),
            volume: -1.0,
            retry: None,
            auto: false,
            check: None,
            ramped: 0.0,
            bags: std::collections::HashMap::new(),
            in_battle: false,
            fight: false,
            fight_resume: None,
            after_up_next: None,
            map_id: 0,
            map_new: None,
            area: None,
            shrine: false,
            last_list: Playlist::AllSongs,
            battle_resume: None,
            crossfade_next: false,
            fade_in: None,
            fading_files: Vec::new(),
            resumed: false,
            saved_at: Instant::now(),
            duck: 1.0,
            duck_at: Instant::now(),
            after_fanfare: false,
            skip_from: None,
            scan: None,
        }
    }

    /// The list Next and Shuffle draw from right now: the battle music
    /// during a battle that has one, else this kind of place's own list,
    /// else "Play from". A battle or area list with nothing playable in it
    /// (a folder emptied, every song hidden) falls back to "Play from" too,
    /// rather than to every song.
    fn list_now(&self, s: &state::Shared) -> Playlist {
        let own = match (&s.config.battle_playlist, self.in_battle) {
            (Some(b), true) => Some(b),
            _ => match self.area.map(|a| s.config.area_music(a)) {
                Some(AreaMusic::List(p)) => Some(p),
                _ => None,
            },
        };
        match own {
            // "All songs" chosen for a battle or a place means all songs,
            // whatever "Play from" is; it is never empty.
            Some(list) if *list == Playlist::AllSongs || s.songs_of_only(list, None).is_some() => list.clone(),
            _ => s.config.playlist.clone(),
        }
    }

    /// Whether a natural end of `current` ends the playlist too: Repeat is
    /// off and it is the last song, or with Shuffle, the last of the deal.
    /// Such a song plays out to its end instead of crossfading into nothing.
    fn ends_playlist(&self, s: &state::Shared, current: usize) -> bool {
        let list = self.list_now(s);
        let (repeat, shuffle) = self.repeat_and_shuffle(s);
        if repeat != Repeat::Off {
            return false;
        }
        let songs = s.songs_of(&list, Some(current));
        if shuffle {
            self.deal_exhausted(&songs)
        } else {
            songs.last() == Some(&current)
        }
    }

    /// The repeat and shuffle settings in force: a battle list has its own
    /// shuffle switch and can hold its song for the whole fight; an area's
    /// own list has its own shuffle switch too.
    fn repeat_and_shuffle(&self, s: &state::Shared) -> (Repeat, bool) {
        let battle_list = self.in_battle && s.config.battle_playlist.is_some();
        let area_list =
            self.area.filter(|&a| !self.in_battle && matches!(s.config.area_music(a), AreaMusic::List(_)));
        let shuffle = match (battle_list, area_list) {
            (true, _) => s.config.battle_shuffle,
            (false, Some(a)) => s.config.area_shuffle(a),
            (false, None) => s.config.shuffle,
        };
        let repeat = if battle_list && s.config.loop_battle_song { Repeat::One } else { s.config.repeat };
        (repeat, shuffle)
    }

    /// This kind of place is left to the game's own music.
    fn area_is_game(&self, s: &state::Shared) -> bool {
        area_source(s, self.area, self.shrine) == AreaMusic::Game
    }

    /// The current song and how far into it, if any.
    fn position(&self, game: &mut Game) -> Option<(usize, f32)> {
        match &self.now {
            Some((i, Playing::Cue { id, .. })) => Some((*i, game.time(*id))),
            Some((i, Playing::File { sink, offset, .. })) => Some((*i, Playing::file_pos(sink, *offset))),
            None => None,
        }
    }

    /// Switches to the battle music as a battle starts and back to the
    /// exploring song, where it left off, once it ends.
    fn follow_battle(&mut self, game: &mut Game, scene: crate::config::Scene, play_in: crate::config::PlayIn, boss_held: bool) {
        let (has_battle_list, playing) = {
            let s = state::lock();
            (s.config.battle_playlist.is_some(), s.playing)
        };
        let battle = scene == crate::config::Scene::Battle && play_in.covers(scene) && has_battle_list && !boss_held;
        if battle == self.in_battle {
            return;
        }
        self.in_battle = battle;
        std::mem::swap(&mut self.history, &mut self.other_history);
        if battle {
            if !playing {
                return;
            }
            self.battle_resume = self.position(game);
            log::info!("player: battle, switching to the battle music");
            self.advance_from(game, false, None);
        } else if let Some((i, at)) = self.battle_resume.take() {
            self.crossfade_next = true;
            // Paused during the battle: the exploring song comes back paused.
            self.keeping_intent(game, |p, game| {
                if state::lock().config.resume_after_battle {
                    log::info!("player: battle over, back to the exploring song at {at:.0}s");
                    p.start_at(i, at, game);
                } else {
                    log::info!("player: battle over, on to the next exploring song");
                    p.advance_from(game, false, Some(i));
                }
            });
        }
    }

    /// Runs `f`, which starts a song by itself (not by your choice: a battle
    /// ending, a seek, a retry), and keeps play or pause as it was: starting
    /// a song counts as playing, which would undo a pause. A paused song is
    /// paused at once, not a tick later.
    fn keeping_intent(&mut self, game: &mut Game, f: impl FnOnce(&mut Self, &mut Game)) {
        let was = state::lock().playing;
        f(self, game);
        if !was {
            state::lock().playing = false;
            self.set_paused(game, true);
        }
    }

    /// A battle without the battle music: the exploring song waits through
    /// it (or plays on), and picks up where it was after. One the game cut
    /// off meanwhile starts again there; with the setting off, the next
    /// song plays instead.
    fn follow_fight(&mut self, game: &mut Game, fight: bool) {
        if fight == self.fight {
            return;
        }
        self.fight = fight;
        if fight {
            self.fight_resume = if self.in_battle { None } else { self.position(game) };
            return;
        }
        let Some((i, at)) = self.fight_resume.take() else { return };
        // Another song chosen during the battle stays.
        if self.now.as_ref().map(|(c, _)| *c) != Some(i) {
            return;
        }
        let alive = match &self.now {
            Some((_, Playing::Cue { id, .. })) => game.exists(*id),
            Some((_, Playing::File { sink, opening, .. })) => opening.is_some() || !sink.empty(),
            None => false,
        };
        if !state::lock().config.resume_after_battle {
            log::info!("player: battle over, on to the next exploring song");
            self.crossfade_next = true;
            self.keeping_intent(game, |p, game| p.advance_from(game, false, Some(i)));
        } else if !alive {
            log::info!("player: battle over, the exploring song was cut off; again from {at:.0}s");
            self.keeping_intent(game, |p, game| p.start_at(i, at, game));
        }
    }

    /// Entering another map moves on to the next song, if that's the
    /// setting. Only maps seen while exploring count, so a battle or a
    /// story scene in between doesn't.
    fn follow_map(&mut self, game: &mut Game, scene: crate::config::Scene, play_in: crate::config::PlayIn) {
        if scene != crate::config::Scene::Exploring {
            return;
        }
        let id = game.map_id();
        if id <= 0 || id == self.map_id {
            self.map_new = None;
            return;
        }
        // A map change passes through loading maps for a moment; only one
        // that holds counts, and by then the game has finished loading it.
        match self.map_new {
            Some((new, since)) if new == id && since.elapsed() >= MAP_SETTLE => {}
            Some((new, _)) if new == id => return,
            _ => {
                self.map_new = Some((id, Instant::now()));
                return;
            }
        }
        // A loading map has no map data of its own; wait for a real one.
        let Some((area, shrine)) = game.map_area(id) else {
            self.map_new = Some((id, Instant::now()));
            return;
        };
        self.map_new = None;
        let old = std::mem::replace(&mut self.map_id, id);
        log::info!("player: map {old} -> {id}");
        let old_area = self.area.replace(area);
        let old_shrine = std::mem::replace(&mut self.shrine, shrine);
        let (on, playing, before, now) = {
            let mut s = state::lock();
            s.area = self.area;
            s.shrine = shrine;
            let (before, now) = (area_source(&s, old_area, old_shrine), area_source(&s, self.area, shrine));
            (s.config.new_song_on_new_map, s.playing, before, now)
        };
        if old <= 0 || !playing || !play_in.covers(scene) || now == AreaMusic::Game {
            return;
        }
        let current = self.now.as_ref().map(|(i, _)| *i);
        // Back out of a place left to the game's music: the song that was
        // waiting carries on, if it belongs here.
        let belongs = current.is_some_and(|c| {
            let s = state::lock();
            s.songs_of(&self.list_now(&s), current).contains(&c)
        });
        if before == AreaMusic::Game && belongs {
            log::info!("player: back from the game's music, the song carries on");
        } else if before != now {
            // Into a place with music of its own (or back out of one).
            log::info!("player: {} music", self.area.map_or("area", |a| a.label()));
            self.crossfade_next = true;
            self.advance_from(game, false, current);
        } else if on && self.now.is_some() {
            log::info!("player: new map, next song");
            self.crossfade_next = true;
            self.advance(game, false);
        }
    }

    /// Starts last session's song where it was, once its track is listed.
    fn resume_last(&mut self, game: &mut Game) {
        if self.resumed {
            return;
        }
        let (want, found, waiting) = {
            let s = state::lock();
            let want = s.config.resume_on_start.then(|| s.config.last_song.clone()).flatten();
            let found = want.as_ref().and_then(|(key, at)| {
                s.tracks.iter().position(|t| t.key == *key && !s.is_unplayable_key(key)).map(|i| (i, *at))
            });
            // A game song only appears once the game's songs are listed.
            let waiting = want.as_ref().is_some_and(|(key, _)| key.starts_with("game:")) && !s.game_tracks_ready;
            (want, found, waiting)
        };
        if want.is_none() {
            self.resumed = true;
            return;
        }
        if let Some((i, at)) = found {
            self.resumed = true;
            log::info!("player: resuming last session's song at {at:.0}s");
            self.crossfade_next = true;
            self.start_at(i, at, game);
        } else if !waiting {
            self.resumed = true;
        }
    }

    /// Saves the current song and position every so often, for resuming.
    fn save_for_resume(&mut self, game: &mut Game) {
        if self.saved_at.elapsed() < RESUME_SAVE_EVERY {
            return;
        }
        self.saved_at = Instant::now();
        let Some((i, at)) = self.position(game) else { return };
        let mut s = state::lock();
        if !s.playing {
            return;
        }
        let Some(key) = s.tracks.get(i).map(|t| t.key.clone()) else { return };
        s.config.last_song = Some((key, (at * 10.0).round() / 10.0));
        s.save_config();
    }

    /// Lets the current song go to make way for the next: faded out over
    /// `fade` seconds, or stopped at once with 0.
    fn retire(&mut self, game: &mut Game, fade: f32) {
        match self.now.take() {
            Some((_, Playing::Cue { id, .. })) if fade > 0.0 => game.fade_out(id, fade),
            Some((_, Playing::Cue { id, .. })) => game.stop(id),
            Some((_, Playing::File { sink, .. })) if fade > 0.0 => {
                let level = sink.volume();
                self.fading_files.push((sink, Instant::now(), level, fade));
            }
            Some((_, Playing::File { sink, .. })) => sink.stop(),
            None => {}
        }
    }

    /// Turns fading-out files down, and drops them once silent.
    fn fade_files(&mut self) {
        self.fading_files.retain(|(sink, start, level, fade)| {
            let t = start.elapsed().as_secs_f32() / fade;
            if t >= 1.0 || sink.empty() {
                sink.stop();
                false
            } else {
                sink.set_volume(level * (1.0 - t));
                true
            }
        });
    }

    /// How far the current song has faded in, 0.0 - 1.0.
    fn fade_in_level(&mut self) -> f32 {
        match self.fade_in {
            Some((start, secs)) => {
                let t = start.elapsed().as_secs_f32() / secs;
                if t >= 1.0 {
                    self.fade_in = None;
                    1.0
                } else {
                    t.max(0.0)
                }
            }
            None => 1.0,
        }
    }

    /// Runs the song check once per game build, and when asked to.
    fn run_check(&mut self, game: &mut Game) {
        let due = {
            let s = state::lock();
            s.game_tracks_ready && s.config.checked_build != s.config.game_build
        };
        if self.check.is_none() && due {
            self.check = Some(SongCheck::new());
        }
        let Some(check) = &mut self.check else { return };
        // No song is started for checking next to a game song the jukebox
        // is playing.
        let busy = matches!(self.now, Some((_, Playing::Cue { .. }))) || self.retry.is_some();
        let finished = check.step(game, !busy);
        let progress = check.progress();
        let mut s = state::lock();
        if finished {
            self.check = None;
            s.config.checked_build = s.config.game_build.clone();
            s.save_config();
            s.check_progress = None;
        } else {
            s.check_progress = Some(progress);
        }
    }

    pub fn tick(&mut self, game: &mut Game) {
        refresh_game_tracks(game);
        let scene = game.scene();

        let (commands, play_in, fanfare_setting, boss_setting, volume, level_db, cf) = {
            let mut s = state::lock();
            s.scene = scene;
            s.talking = crate::game::talking();
            (
                s.commands.drain(..).collect::<Vec<_>>(),
                s.config.play_in(),
                s.config.victory_fanfare,
                s.config.boss_game_music,
                s.config.volume,
                s.config.my_music_level_db,
                s.config.crossfade_secs.max(0.0),
            )
        };
        self.take_scan(game);
        for c in commands {
            // Choosing other music replaces a pending retry or skip; pausing,
            // seeking or rescanning leave it to go ahead.
            if matches!(c, Command::Play(_) | Command::Next | Command::Previous | Command::Stop) {
                self.retry = None;
                self.skip_from = None;
            }
            self.run(c, game);
        }
        if let Some((i, attempt, at)) = self.retry
            && Instant::now() >= at
        {
            self.retry = None;
            log::info!("player: retrying (attempt {})", attempt + 1);
            self.keeping_intent(game, |p, game| p.start_attempt(i, 0.0, attempt, game));
        }
        if let Some(i) = self.skip_from.take() {
            self.advance_from(game, false, Some(i));
        }
        self.resume_last(game);
        // Boss battles can keep the game's own music, as the developers
        // scored them; the battle music isn't started for them either.
        let boss = scene == crate::config::Scene::Battle && game.boss_battle();
        let boss_held = boss && boss_setting;
        self.follow_battle(game, scene, play_in, boss_held);
        self.follow_fight(game, scene == crate::config::Scene::Battle);
        self.follow_map(game, scene, play_in);
        self.run_check(game);
        self.fade_files();
        self.finish_opening();

        // A game cue that ended on its own, or a file that ran out. A cue
        // only counts as ended once it has been seen playing; one that never
        // starts, or is cut off as it starts, is retried and then an error,
        // never a reason to skip ahead.
        let mut failed = None;
        let mut cut_off = None;
        let (loop_battle, last) = {
            let s = state::lock();
            let current = self.now.as_ref().map(|(i, _)| *i);
            // Only asked near the crossfade point (last tick's position, with
            // a second to spare): working out the list every tick is wasted
            // on a song in the middle.
            let near_end = cf > 0.0 && s.duration.is_some_and(|d| s.position + 1.0 >= d - crossfade_for(cf, d));
            (
                self.in_battle && s.config.loop_battle_song,
                near_end && current.is_some_and(|c| self.ends_playlist(&s, c)),
            )
        };
        // The last song of a playlist (Repeat off) plays to its end: there is
        // nothing to crossfade into, and the crossfade point would cut it.
        let cf = if last { 0.0 } else { cf };
        let finished = match &mut self.now {
            Some((i, Playing::Cue { id, duration, paused: false, started, seen, reached, attempt, fades, fading })) => {
                // A looping theme held for the whole fight just keeps looping
                // in the game, with no fade or cut at the end of its length.
                let duration = if loop_battle && *fades { None } else { *duration };
                let exists = game.exists(*id);
                let time = game.time(*id);
                if exists && !*seen {
                    *seen = true;
                    log::debug!("player: cue id {id} started after {:.2}s", started.elapsed().as_secs_f32());
                }
                if exists {
                    *reached = reached.max(time);
                }
                if !*seen {
                    if started.elapsed() > CUE_START_GRACE {
                        failed = Some(*i);
                    }
                    false
                } else if !exists && *reached < 1.0 {
                    log::debug!("player: cue id {id} was cut off as it started (attempt {})", *attempt + 1);
                    if *attempt + 1 < CUE_RETRIES {
                        cut_off = Some((*i, *attempt + 1));
                    } else {
                        failed = Some(*i);
                    }
                    false
                } else if !exists {
                    log::debug!("player: cue id {id} ended at {:.1}s", *reached);
                    true
                } else if cf > 0.0 && duration.is_some_and(|d| time >= d - crossfade_for(cf, d)) {
                    // Over to the next song while this one fades out under it.
                    log::debug!("player: cue id {id} crossfading at {time:.1}s");
                    true
                } else if *fades && !*fading && duration.is_some_and(|d| time >= d - THEME_FADE) {
                    // A looping theme has no ending of its own, so it fades
                    // out over its last seconds rather than stopping dead.
                    log::debug!("player: cue id {id} fading out at {time:.1}s");
                    game.fade_out(*id, THEME_FADE);
                    *fading = true;
                    false
                } else if duration.is_some_and(|d| time >= d) {
                    log::debug!("player: cue id {id} reached its length ({time:.1}s of {duration:?})");
                    true
                } else {
                    false
                }
            }
            // A paused file stays put, even one seeked into its last seconds.
            Some((_, Playing::File { sink, duration, offset, opening: None, .. })) if !sink.is_paused() => {
                sink.empty()
                    || (cf > 0.0
                        && duration.is_some_and(|d| Playing::file_pos(sink, *offset) >= d - crossfade_for(cf, d)))
            }
            _ => false,
        };
        // A cue that has really played clears any earlier failure.
        if let Some((i, Playing::Cue { reached, .. })) = &self.now
            && *reached >= 1.0
            && state::lock().is_unplayable(*i)
        {
            state::lock().set_unplayable(*i, false);
        }
        if let Some((i, attempt)) = cut_off {
            // Keep showing the song as current while it waits.
            self.now = None;
            self.retry = Some((i, attempt, Instant::now() + CUE_RETRY_DELAY));
        } else if let Some(i) = failed {
            // A cue the game would not play: skipped for this session, never
            // removed for good. A cue is cut off while a map loads, or held
            // back in some places; only the song check, which plays every
            // song in peace, decides what has no audio at all.
            let title = state::lock().tracks.get(i).map(|t| t.title.clone()).unwrap_or_default();
            self.stop(game);
            log::warn!("player: the game would not play {title} here, skipping it");
            state::lock().set_skipped(i, true);
            self.skip_on(i, format!("The game won't play {title} here. Try it again later."));
        } else if finished {
            self.advance(game, true);
        }

        let playing = state::lock().playing;
        // The game's victory fanfare plays out over a won battle's results;
        // the jukebox picks up again once the game moves on from it.
        let bgm_cue = game.current_bgm_cue();
        let mut fanfare = fanfare_setting && bgm_cue.as_deref() == Some(crate::game::VICTORY_CUE);
        // The battle ends a moment after the fanfare does; the battle song
        // stays down until then instead of coming back for that moment.
        let in_battle_scene = scene == crate::config::Scene::Battle;
        self.after_fanfare = in_battle_scene && (self.after_fanfare || fanfare);
        fanfare |= self.after_fanfare;
        crate::log_change("fanfare", format!("player: victory fanfare playing={fanfare}"));
        // A kind of place left to the game's own music while exploring.
        let area_game = scene == crate::config::Scene::Exploring && self.area_is_game(&state::lock());
        let covered = play_in.covers(scene) && !fanfare && !boss_held && !area_game;
        // The song fades out under the fanfare rather than stopping dead,
        // and fades back in after it.
        let step = self.duck_at.elapsed().as_secs_f32() / FANFARE_FADE;
        self.duck_at = Instant::now();
        self.duck = if fanfare { (self.duck - step).max(0.0) } else { (self.duck + step).min(1.0) };
        let fading_out = fanfare && self.duck > 0.0 && play_in.covers(scene) && self.now.is_some();
        // A pending retry keeps the game's music muted, so the gap is quiet.
        let active = playing && covered && (self.now.is_some() || self.retry.is_some());
        self.set_paused(game, !(active || (playing && fading_out)));
        if active {
            let own = match &self.now {
                Some((_, Playing::Cue { id, .. })) => Some(*id),
                _ => None,
            };
            self.mute.hold(game, own);
        } else {
            self.mute.release(game);
        }
        // The game's own song, heard while the game has the music, is kept
        // so it can be favorited afterwards. Only one the song list knows,
        // never while the song check is starting songs of its own, and not
        // the victory fanfare, which would replace the song you meant after
        // every won battle.
        if !active
            && self.check.is_none()
            && scene != crate::config::Scene::Movie
            && let Some(cue) = &bgm_cue
            && cue != crate::game::VICTORY_CUE
        {
            let key = format!("game:{cue}");
            let mut s = state::lock();
            if s.heard.as_deref() != Some(key.as_str()) && s.index_of(&key).is_some() {
                log::info!("player: heard the game play {cue}");
                s.heard = Some(key);
            }
        }
        // Volume rises gradually and falls at once: one click at the far end
        // of the slider would otherwise jump straight to 150%.
        self.ramped = if volume > self.ramped { (self.ramped + VOLUME_RISE).min(volume) } else { volume };
        // A song crossfading in rises from silence.
        let volume = self.ramped * self.fade_in_level() * self.duck;
        match &mut self.now {
            Some((_, Playing::Cue { id, .. })) if (volume - self.volume).abs() > 1e-4 => {
                self.volume = volume;
                game.set_volume(*id, volume);
            }
            Some((_, Playing::File { sink, key, .. })) => {
                // The measured gain and peak land a moment after the song
                // starts; the peak caps the level so it can never clip.
                let (gain, peak) = loudness::saved(key);
                let want = loudness::safe_level(file_volume(volume, gain, level_db, game.music_volume()), peak);
                if (sink.volume() - want).abs() > 1e-4 {
                    sink.set_volume(want);
                }
            }
            _ => {}
        }

        self.last_list = self.list_now(&state::lock());
        self.save_for_resume(game);
        let mut s = state::lock();
        s.held = playing && !covered;
        s.boss = boss;
        s.current = self.now.as_ref().map(|(i, _)| *i).or(self.retry.map(|(i, _, _)| i));
        match &self.now {
            Some((_, Playing::Cue { id, duration, fades, .. })) => {
                s.position = game.time(*id);
                s.duration = if loop_battle && *fades { None } else { *duration };
            }
            Some((_, Playing::File { sink, duration, offset, .. })) => {
                s.position = Playing::file_pos(sink, *offset);
                s.duration = *duration;
            }
            None => {
                s.position = 0.0;
                s.duration = None;
            }
        }
    }

    fn run(&mut self, command: Command, game: &mut Game) {
        match command {
            Command::Play(key) => {
                // A song gone from the list in the meantime: the one playing
                // carries on.
                let found = state::lock().index_of(&key);
                if let Some(i) = found {
                    self.auto = false;
                    self.start(i, game);
                }
            }
            Command::TogglePause => {
                let mut s = state::lock();
                // A song waiting on a retry is still the current one.
                if self.now.is_some() || self.retry.is_some() {
                    s.playing = !s.playing;
                } else if !s.tracks.is_empty() {
                    drop(s);
                    self.advance(game, false);
                }
            }
            Command::Stop => {
                self.stop(game);
                for (sink, ..) in self.fading_files.drain(..) {
                    sink.stop();
                }
                self.mute.release(game);
                self.battle_resume = None;
                self.after_up_next = None;
                let mut s = state::lock();
                s.playing = false;
                s.up_next = None;
                // A stopped jukebox stays quiet at the next start too.
                s.config.last_song = None;
                s.save_config();
            }
            Command::Next => self.advance(game, false),
            Command::CheckSongs => {
                if let Some(old) = &mut self.check {
                    old.cancel(game);
                }
                self.check = Some(SongCheck::new());
            }
            Command::Previous => {
                let current = self.now.as_ref().map(|(i, _)| *i);
                // Pop the current track, then the one before it.
                if self.history.last() == current.as_ref() {
                    self.history.pop();
                }
                let list = {
                    let s = state::lock();
                    s.songs_of(&self.list_now(&s), None)
                };
                let before = current
                    .and_then(|c| list.iter().position(|&i| i == c))
                    .map(|p| list[(p + list.len() - 1) % list.len()]);
                if let Some(i) = self.history.pop().or(before) {
                    self.start(i, game);
                }
            }
            Command::Seek(t) => match &mut self.now {
                Some((i, Playing::Cue { .. })) => {
                    let i = *i;
                    // Seeking restarts the cue at the new place. Paused, it
                    // stays paused; while the game's music has the scene, it
                    // is held silent at once rather than at the next tick.
                    self.keeping_intent(game, |p, game| p.start_at(i, t, game));
                    if state::lock().held {
                        self.set_paused(game, true);
                    }
                }
                // Not before it's open: the seek would go nowhere.
                Some((_, Playing::File { sink, offset, opening: None, .. })) => {
                    // The sink counts from wherever it seeks to. A seek waits
                    // on the audio thread for a few milliseconds, so it's
                    // done on a thread of its own, not the game's.
                    *offset = 0.0;
                    let sink = sink.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = sink.try_seek(Duration::from_secs_f32(t.max(0.0))) {
                            log::warn!("player: can't seek ({e})");
                        }
                    });
                }
                _ => {}
            },
            Command::ListChanged => {
                // Only the list playing right now swaps the song, and only
                // when the song isn't in the new list too.
                let (list, songs, playing) = {
                    let s = state::lock();
                    let list = self.list_now(&s);
                    let current = self.now.as_ref().map(|(i, _)| *i);
                    (list.clone(), s.songs_of(&list, current), s.playing)
                };
                let current = self.now.as_ref().map(|(i, _)| *i);
                if list != self.last_list && playing && current.is_some_and(|c| !songs.contains(&c)) {
                    log::info!("player: now playing from {}", list.label());
                    self.crossfade_next = true;
                    self.advance_from(game, false, current);
                }
                self.last_list = list;
            }
            Command::Rescan => {
                // The folder is walked on a thread of its own: a big or slow
                // folder must not hold up the game, nor the menu waiting on
                // the shared state. The result is taken in by `take_scan`.
                if self.scan.is_none() {
                    let dir = state::lock().music_dir.clone();
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = tx.send(state::scan_folder(&dir));
                    });
                    self.scan = Some(rx);
                }
            }
        }
    }

    /// Takes in a finished rescan. Every song the player holds on to (the
    /// one playing, one waiting on a retry or a skip, the ones to come back
    /// after a battle) is found again by its key, since adding or removing
    /// files moves songs around the list; one whose file is gone is let go.
    fn take_scan(&mut self, game: &mut Game) {
        let Some(rx) = &self.scan else { return };
        let files = match rx.try_recv() {
            Ok(files) => files,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.scan = None;
                return;
            }
        };
        self.scan = None;
        let mut s = state::lock();
        log::info!("player: {} files in {}", files.len(), s.music_dir.display());
        let key_of = |s: &state::Shared, i: usize| s.tracks.get(i).map(|t| t.key.clone());
        let now = self.now.as_ref().and_then(|(i, _)| key_of(&s, *i));
        let retry = self.retry.and_then(|(i, ..)| key_of(&s, i));
        let skip = self.skip_from.and_then(|i| key_of(&s, i));
        let battle = self.battle_resume.and_then(|(i, _)| key_of(&s, i));
        let fight = self.fight_resume.and_then(|(i, _)| key_of(&s, i));
        s.set_files(files);
        // A file that would not play gets another try after a rescan; it may
        // have been fixed or replaced.
        s.skipped.retain(|k| !k.starts_with("file:"));
        let find = |key: Option<String>| key.and_then(|k| s.index_of(&k));
        let (now_at, retry_at, skip_at) = (find(now), find(retry), find(skip));
        let (battle_at, fight_at) = (find(battle), find(fight));
        drop(s);
        match (now_at, &mut self.now) {
            (Some(at), Some((i, _))) => *i = at,
            (None, Some(_)) => {
                log::info!("player: the playing song's file is gone");
                self.stop(game);
            }
            _ => {}
        }
        self.retry = self.retry.zip(retry_at).map(|((_, attempt, when), at)| (at, attempt, when));
        self.skip_from = skip_at;
        self.battle_resume = self.battle_resume.zip(battle_at).map(|((_, pos), at)| (at, pos));
        self.fight_resume = self.fight_resume.zip(fight_at).map(|((_, pos), at)| (at, pos));
        // History and shuffle deals hold places in the old list.
        self.history.clear();
        self.other_history.clear();
        self.bags.clear();
    }

    fn advance(&mut self, game: &mut Game, natural_end: bool) {
        let current = self.now.as_ref().map(|(i, _)| *i);
        self.advance_from(game, natural_end, current);
    }

    /// Starts the song after `current` (`None`: the playlist's first).
    fn advance_from(&mut self, game: &mut Game, natural_end: bool, mut current: Option<usize>) {
        // A song lined up with Play next goes first, while exploring; a battle
        // keeps its own music and the song waits for the fight to end.
        let up_next = if self.in_battle { None } else { take_up_next(&mut state::lock()) };
        if let Some((i, key)) = up_next {
            let before = current.and_then(|c| state::lock().tracks.get(c).map(|t| t.key.clone()));
            log::info!("player: playing the song lined up next");
            self.after_up_next = Some((key, before));
            self.auto = true;
            self.start(i, game);
            return;
        }
        // After a Play next song, the list goes on from the song it cut in
        // after, not from the start.
        if let Some(after) = self.after_up_next.take() {
            current = after_up_next(&state::lock(), after, current);
        }
        let (list, repeat, shuffle) = {
            let s = state::lock();
            let (repeat, shuffle) = self.repeat_and_shuffle(&s);
            (s.songs_of(&self.list_now(&s), current), repeat, shuffle)
        };
        let len = list.len();
        if len == 0 {
            return;
        }
        // Where the current song sits in the playlist. A song played from
        // outside it continues from the playlist's start.
        let at = current.and_then(|c| list.iter().position(|&i| i == c));
        // With Repeat off, Shuffle stops once every song has had its turn.
        let deal_over = shuffle && natural_end && repeat == Repeat::Off && self.deal_exhausted(&list);
        let shuffled = if shuffle && !deal_over { self.draw(&list, current) } else { None };
        let next = match (current, at, natural_end, repeat) {
            (Some(c), _, true, Repeat::One) => Some(c),
            _ if shuffle => shuffled,
            (_, None, _, _) => Some(list[0]),
            (_, Some(p), true, Repeat::Off) if p + 1 >= len => None,
            (_, Some(p), _, _) => Some(list[(p + 1) % len]),
        };
        // A song that will not play is never picked again straight away;
        // with nothing else left, playback stops.
        let next = next.filter(|&i| !(state::lock().is_unplayable(i) && Some(i) == current));
        match next {
            Some(i) => {
                self.auto = true;
                self.start(i, game);
            }
            None => {
                self.stop(game);
                state::lock().playing = false;
            }
        }
    }

    /// The shuffle deal for `list` (on this side, battle or not) has been
    /// dealt and every song drawn.
    fn deal_exhausted(&self, list: &[usize]) -> bool {
        let mut sorted = list.to_vec();
        sorted.sort_unstable();
        self.bags.get(&(self.in_battle, sorted)).is_some_and(|bag| bag.is_empty())
    }

    /// Shuffle without repeats: every song of `list` once, in random order,
    /// before any comes round again. A new deal never opens with `current`.
    fn draw(&mut self, list: &[usize], current: Option<usize>) -> Option<usize> {
        use rand::seq::SliceRandom;
        // Battle and exploring each keep their own deal per list, even when
        // both play the same list, so neither disturbs the other.
        let mut sorted = list.to_vec();
        sorted.sort_unstable();
        let bag = self.bags.entry((self.in_battle, sorted)).or_default();
        if bag.is_empty() {
            *bag = list.to_vec();
            bag.shuffle(&mut rand::thread_rng());
            // Drawn from the end, so the current song goes to the front.
            if let (Some(c), true) = (current, bag.len() > 1)
                && let Some(p) = bag.iter().position(|&i| i == c)
            {
                bag.remove(p);
                bag.insert(0, c);
            }
        }
        bag.pop()
    }

    /// Starts song `i` as a change of song: it crossfades with the last.
    fn start(&mut self, i: usize, game: &mut Game) {
        self.crossfade_next = true;
        self.start_at(i, 0.0, game);
    }

    fn start_at(&mut self, i: usize, at: f32, game: &mut Game) {
        self.start_attempt(i, at, 0, game);
    }

    fn start_attempt(&mut self, i: usize, at: f32, attempt: u8, game: &mut Game) {
        // Looked up before anything stops: a song that isn't there (an index
        // from before a rescan) must not leave silence behind it.
        let Some(track) = state::lock().tracks.get(i).cloned() else {
            log::warn!("player: no song at {i} to start");
            return;
        };
        // A change of song crossfades; a seek or a retry starts cleanly.
        let cf = if std::mem::take(&mut self.crossfade_next) {
            state::lock().config.crossfade_secs.max(0.0)
        } else {
            0.0
        };
        self.retire(game, cf);
        self.fade_in = (cf > 0.0).then(|| (Instant::now(), cf));
        let volume = if cf > 0.0 { 0.0 } else { self.ramped };
        let loop_secs = state::lock().config.loop_minutes * 60.0;
        let result = match &track.source {
            Source::Game { cue, looping } => match game.play_cue(cue, at) {
                Some(id) => {
                    game.set_volume(id, volume);
                    let duration = game.cue_duration(cue, *looping, loop_secs);
                    log::debug!("player: {cue} -> id {id}, length {duration:?}");
                    Ok(Playing::Cue {
                        id,
                        duration,
                        paused: false,
                        started: Instant::now(),
                        seen: false,
                        reached: 0.0,
                        attempt,
                        fades: *looping,
                        fading: false,
                    })
                }
                // The game's sound system isn't ready: not the song's fault.
                None => Err(format!("The game would not play {cue}")),
            },
            Source::File(path) => play_file(path, at),
        };
        let mut s = state::lock();
        match result {
            Ok(p) => {
                log::info!("player: playing {}", track.title);
                self.now = Some((i, p));
                if self.history.last() != Some(&i) {
                    self.history.push(i);
                    if self.history.len() > 200 {
                        self.history.remove(0);
                    }
                }
                s.playing = true;
                s.error = None;
                s.track_serial += 1;
                self.volume = volume;
            }
            // A file that won't open or decode turns up a moment later, in
            // `finish_opening`.
            Err(e) => {
                // No sound device (or the game's sound isn't up): every song
                // would fail the same way, so nothing is skipped and nothing
                // moves on. The next Play tries the device again.
                log::error!("player: {e}");
                s.playing = false;
                s.error = Some(e);
            }
        }
    }

    /// After song `i` would not play: on to the song after it if a list
    /// picked it (Next, Shuffle, a playlist), else stop with `error` shown.
    /// The next song is tried on the next tick, not now, so a run of files
    /// that won't open costs one attempt per tick instead of stalling one
    /// frame of the game. Each skipped song leaves the lists for the session,
    /// so a list of songs that all fail ends rather than going round for ever.
    fn skip_on(&mut self, i: usize, error: String) {
        if self.auto {
            self.skip_from = Some(i);
        } else {
            let mut s = state::lock();
            s.playing = false;
            s.error = Some(error);
        }
    }

    /// Hands a file that has finished opening to its sink. One that failed
    /// is skipped like any song that won't play.
    fn finish_opening(&mut self) {
        let Some((i, Playing::File { sink, duration, key, offset, opening: opening @ Some(_) })) = &mut self.now
        else {
            return;
        };
        let result = match opening.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(result)) => result,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => Err("The song could not be opened".into()),
            _ => return,
        };
        *opening = None;
        match result {
            Ok(o) => {
                sink.append(o.decoder);
                *duration = o.duration;
                *key = o.key;
                *offset = o.at;
            }
            Err(e) => {
                let i = *i;
                sink.stop();
                self.now = None;
                log::error!("player: {e}");
                state::lock().set_skipped(i, true);
                self.skip_on(i, e);
            }
        }
    }

    fn stop(&mut self, game: &mut Game) {
        match self.now.take() {
            Some((_, Playing::Cue { id, .. })) => game.stop(id),
            Some((_, Playing::File { sink, .. })) => sink.stop(),
            None => {}
        }
    }

    fn set_paused(&mut self, game: &mut Game, pause: bool) {
        // A song fading out under the next stops with it, so a pause or a
        // movie is quiet straight away.
        if pause {
            for (sink, ..) in self.fading_files.drain(..) {
                sink.stop();
            }
        }
        match &mut self.now {
            Some((_, Playing::Cue { id, paused, started, seen, .. })) if *paused != pause => {
                if pause { game.pause(*id) } else { game.resume(*id) }
                *paused = pause;
                // A cue held before it was seen starting gets its full grace
                // again once it resumes.
                if !pause && !*seen {
                    *started = Instant::now();
                }
            }
            Some((_, Playing::File { sink, .. })) if sink.is_paused() != pause => {
                if pause { sink.pause() } else { sink.play() }
            }
            _ => {}
        }
    }
}

/// The song lined up with Play next, taken off the queue: its place in the
/// list and its key. One that's gone or won't play is dropped.
fn take_up_next(s: &mut state::Shared) -> Option<(usize, String)> {
    let key = s.up_next.take()?;
    let i = s.index_of(&key).filter(|&i| !s.is_unplayable(i))?;
    Some((i, key))
}

/// Where the list goes on from: after the Play next song `played`, the song
/// it cut in after; any other song (one picked by hand since) is `current`.
fn after_up_next(s: &state::Shared, (played, before): (String, Option<String>), current: Option<usize>) -> Option<usize> {
    if current.is_some_and(|c| s.tracks.get(c).is_some_and(|t| t.key == played)) {
        before.and_then(|k| s.index_of(&k))
    } else {
        current
    }
}

/// How long before its end a song of `duration` seconds crossfades into
/// the next: the crossfade setting, but never more than a third of the
/// song, so a short one is heard before it fades rather than skipped at once.
fn crossfade_for(cf: f32, duration: f32) -> f32 {
    cf.min(duration / 3.0)
}

const NO_OUTPUT: &str = "No sound device for your own music. Check your speakers or headphones, then press Play.";

/// Starts a file silent, `at` seconds in; `tick` sets its levelled volume
/// straight after. Opening, decoding and seeking happen on a thread of their
/// own, and `tick` hands the file to its sink once it's ready.
fn play_file(path: &Path, at: f32) -> Result<Playing, String> {
    let handle = output().ok_or_else(|| NO_OUTPUT.to_string())?;
    let sink = Sink::try_new(&handle).map_err(|e| format!("{NO_OUTPUT} ({e})"))?;
    sink.set_volume(0.0);
    let (tx, rx) = std::sync::mpsc::channel();
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(open_file(&path, at));
    });
    let offset = if at.is_finite() { at.max(0.0) } else { 0.0 };
    Ok(Playing::File { sink: std::sync::Arc::new(sink), duration: None, key: String::new(), offset, opening: Some(rx) })
}

/// Opens and decodes a file and moves it `at` seconds in. A file that can't
/// seek starts from the top.
fn open_file(path: &Path, at: f32) -> Result<Opened, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let file = File::open(path).map_err(|e| format!("Could not open {name}: {e}"))?;
    let mut decoder = Decoder::new(BufReader::new(file)).map_err(|e| format!("Could not decode {name}: {e}"))?;
    let duration = decoder.total_duration().map(|d| d.as_secs_f32());
    let mut start = 0.0;
    if at > 0.5 && at.is_finite() {
        match decoder.try_seek(Duration::from_secs_f32(at)) {
            Ok(()) => start = at,
            Err(e) => log::warn!("player: can't seek in {name} ({e}), starting from the top"),
        }
    }
    let music_dir = state::lock().music_dir.clone();
    let key = loudness::key(path, &music_dir);
    loudness::ensure_measured(path, &key);
    Ok(Opened { decoder, duration, key, at: start })
}

/// What a file plays at: your volume, its levelling gain (a safe guess
/// until it is measured), the My Music level and the game's own Music
/// volume, which game songs get from the game.
fn file_volume(volume: f32, gain: Option<f32>, level_db: f32, music: f32) -> f32 {
    volume * gain.unwrap_or(loudness::PROVISIONAL) * loudness::db_gain(level_db) * music
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffle_plays_every_song_once_before_repeating() {
        let mut p = Player::new();
        let list = vec![3, 5, 7, 9, 11];
        for _ in 0..20 {
            let mut seen: Vec<usize> = (0..list.len()).map(|_| p.draw(&list, None).unwrap()).collect();
            seen.sort_unstable();
            assert_eq!(seen, list);
        }
    }

    #[test]
    fn a_new_deal_never_opens_with_the_current_song() {
        for _ in 0..50 {
            let mut p = Player::new();
            assert_ne!(p.draw(&[1, 2, 3], Some(2)), Some(2));
        }
    }

    #[test]
    fn a_changed_playlist_starts_a_fresh_deal() {
        let mut p = Player::new();
        p.draw(&[1, 2, 3], None);
        let next = p.draw(&[8, 9], None).unwrap();
        assert!(next == 8 || next == 9);
    }

    fn three_songs() -> state::Shared {
        let mut s = state::Shared::default();
        s.config_path = std::env::temp_dir().join("ffrs_jukebox_player_test.toml");
        s.tracks = (0..3).map(|i| state::Track::game(format!("BGM_PP_T{i}"), true)).collect();
        s
    }

    #[test]
    fn repeat_off_ends_with_the_last_song_or_the_last_of_the_deal() {
        let mut s = three_songs();
        s.config.repeat = Repeat::Off;
        let mut p = Player::new();
        assert!(!p.ends_playlist(&s, 1));
        assert!(p.ends_playlist(&s, 2));
        // Repeat on: the playlist never ends.
        s.config.repeat = Repeat::All;
        assert!(!p.ends_playlist(&s, 2));
        // Shuffle: the deal ends once every song has been drawn.
        s.config.repeat = Repeat::Off;
        s.config.shuffle = true;
        let songs = s.songs_of(&Playlist::AllSongs, None);
        assert!(!p.ends_playlist(&s, 0), "nothing dealt yet");
        let mut last = 0;
        for _ in 0..songs.len() {
            last = p.draw(&songs, None).unwrap();
        }
        assert!(p.ends_playlist(&s, last));
    }

    #[test]
    fn an_empty_battle_or_area_list_falls_back_to_play_from() {
        let mut s = three_songs();
        s.create_playlist("Empty");
        s.config.playlist = Playlist::Favorites;
        s.config.battle_playlist = Some(Playlist::Custom("Empty".into()));
        let mut p = Player::new();
        p.in_battle = true;
        assert_eq!(p.list_now(&s), Playlist::Favorites);
        // With a song in it, the battle list is used.
        s.toggle(state::List::Custom(0), 1);
        assert_eq!(p.list_now(&s), Playlist::Custom("Empty".into()));
        // The same for an area's list.
        p.in_battle = false;
        p.area = Some(Area::Town);
        s.config.town_music = AreaMusic::List(Playlist::Folder("Nope".into()));
        assert_eq!(p.list_now(&s), Playlist::Favorites);
        // "All songs" chosen on purpose is used, not taken for empty.
        s.config.town_music = AreaMusic::List(Playlist::AllSongs);
        assert_eq!(p.list_now(&s), Playlist::AllSongs);
        p.area = None;
        p.in_battle = true;
        s.config.battle_playlist = Some(Playlist::AllSongs);
        assert_eq!(p.list_now(&s), Playlist::AllSongs);
    }

    #[test]
    fn a_rescan_finds_held_songs_again_by_key() {
        let file = |rel: &str| state::Track {
            title: rel.into(),
            base_title: rel.into(),
            key: format!("file:{rel}"),
            source: Source::File(rel.into()),
        };
        // The only test that uses the shared state.
        state::init(state::Shared {
            config_path: std::env::temp_dir().join("ffrs_jukebox_rescan_test.toml"),
            tracks: vec![file("a.mp3"), file("c.mp3"), file("gone.mp3"), state::Track::game("BGM_PP_T0".into(), true)],
            ..Default::default()
        });
        let mut p = Player::new();
        p.battle_resume = Some((1, 30.0)); // c.mp3
        p.fight_resume = Some((2, 5.0)); // gone.mp3
        p.skip_from = Some(3); // the game song
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(vec![file("a.mp3"), file("b.mp3"), file("c.mp3")]).unwrap();
        p.scan = Some(rx);
        p.take_scan(&mut Game::none());
        let s = state::lock();
        assert_eq!(p.battle_resume, Some((2, 30.0)), "c.mp3 moved one along, not by the file count");
        assert_eq!(p.fight_resume, None, "a removed file is let go");
        assert_eq!(p.skip_from.map(|i| s.tracks[i].key.clone()), Some("game:BGM_PP_T0".into()));
        assert!(p.scan.is_none());
    }

    #[test]
    fn battle_and_exploring_shuffle_keep_separate_deals_of_one_list() {
        let mut p = Player::new();
        let list = vec![1, 2, 3, 4];
        let first = p.draw(&list, None).unwrap();
        // A whole battle deal of the same list, unaffected by the exploring one.
        p.in_battle = true;
        let mut battle: Vec<usize> = (0..list.len()).map(|_| p.draw(&list, None).unwrap()).collect();
        battle.sort_unstable();
        assert_eq!(battle, list);
        // Back out of battle, the exploring deal carries on without `first`.
        p.in_battle = false;
        let rest: Vec<usize> = (0..list.len() - 1).map(|_| p.draw(&list, None).unwrap()).collect();
        assert!(!rest.contains(&first));
    }

    #[test]
    fn a_short_song_is_heard_before_it_crossfades() {
        // Eight seconds of crossfade on a two-second jingle: it plays most of
        // its length, not none of it.
        assert!((crossfade_for(8.0, 2.0) - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(crossfade_for(8.0, 240.0), 8.0);
        assert_eq!(crossfade_for(0.0, 240.0), 0.0);
    }

    #[test]
    fn play_next_cuts_in_once_and_the_list_carries_on() {
        let mut s = three_songs();
        s.up_next = Some(s.tracks[2].key.clone());
        assert_eq!(take_up_next(&mut s), Some((2, s.tracks[2].key.clone())));
        assert_eq!(take_up_next(&mut s), None, "it plays once");
        // Song 0 was playing when song 2 cut in: the list goes on after 0.
        let after = (s.tracks[2].key.clone(), Some(s.tracks[0].key.clone()));
        assert_eq!(after_up_next(&s, after.clone(), Some(2)), Some(0));
        // A song picked by hand meanwhile keeps its own place.
        assert_eq!(after_up_next(&s, after, Some(1)), Some(1));
        // A song that won't play is dropped from the queue.
        s.up_next = Some(s.tracks[1].key.clone());
        s.set_skipped(1, true);
        assert_eq!(take_up_next(&mut s), None);
        assert_eq!(s.up_next, None);
    }
}
