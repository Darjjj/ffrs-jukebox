//! Playback: runs the queued commands, follows the mode, advances the
//! playlist. Lives on the game thread (see [`crate::game`]).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
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
/// The window came back after the output was closed: the file that was
/// playing is tied to the closed output and must be started again.
static REOPENED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// An output is being opened on a thread of its own.
static OPENING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Windows' default sound device is no longer the one the output was opened
/// on (speakers switched to a headset, say): the output stays on the old
/// device by itself, so the player opens it again on the new one.
static DEVICE_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// How often the output's thread asks which device is the default.
const DEVICE_CHECK: Duration = Duration::from_secs(2);

/// The name of Windows' default sound device right now, if it has one.
fn default_device_name() -> Option<String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    rodio::cpal::default_host().default_output_device()?.name().ok()
}

/// The open audio output, if there is one right now. Never opens one: that
/// takes a moment on the device, and the game thread doesn't wait for it
/// (see [`open_output`]).
fn output_ready() -> Option<OutputStreamHandle> {
    if CLOSING.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    OUTPUT.lock().unwrap_or_else(|e| e.into_inner()).0.clone()
}

/// The last try to open the output failed a moment ago, so there's no
/// point trying again yet: the device really isn't there.
fn output_failed() -> bool {
    OUTPUT.lock().unwrap_or_else(|e| e.into_inner()).1.is_some_and(|failed| failed.elapsed() < OUTPUT_RETRY)
}

/// Opens the audio output on a thread of its own, unless one is open, being
/// opened, or just failed. The player starts the song that waits for it
/// once it's there (`Player::start_waiting`).
fn open_output() {
    if CLOSING.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    {
        let out = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
        if out.0.is_some() || out.1.is_some_and(|failed| failed.elapsed() < OUTPUT_RETRY) {
            return;
        }
    }
    if OPENING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let mut device = default_device_name();
        let opened = OutputStream::try_default();
        let mut out = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
        match opened {
            Ok((stream, handle)) => {
                // Checked under the same lock `silence_for_exit` takes: an
                // output that finished opening as the game closed is closed
                // straight away (dropping its sender ends its thread), never
                // left playing.
                let mut stop = STOP_OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
                if !CLOSING.load(std::sync::atomic::Ordering::SeqCst) {
                    if out.1.take().is_some() {
                        log::info!("player: audio output is back");
                    }
                    *stop = Some(stop_tx);
                    out.0 = Some(handle);
                    drop(stop);
                    drop(out);
                    // Kept until the game closes or the output is reset:
                    // dropping the stream silences every file at once.
                    // Meanwhile the default device is watched: the stream
                    // stays on the device it was opened on, so a change is
                    // handed to the player, which resets the output.
                    while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stop_rx.recv_timeout(DEVICE_CHECK) {
                        let now = default_device_name();
                        if now.is_some() && now != device {
                            if device.is_some() {
                                log::info!("player: the default sound device is now {}", now.as_deref().unwrap_or_default());
                                DEVICE_CHANGED.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            device = now;
                        }
                    }
                    drop(stream);
                }
            }
            Err(e) => {
                log::error!("player: no audio output: {e}");
                out.1 = Some(Instant::now());
            }
        }
        OPENING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// Closes the output (a stream that has stopped playing: its device is
/// gone or changed), so the next start opens a fresh one.
fn reset_output() {
    if let Some(stop) = STOP_OUTPUT.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = stop.send(());
    }
    let mut out = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
    out.0 = None;
    out.1 = None;
}

/// Tells the audio output's thread to close it.
static STOP_OUTPUT: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

/// Silences your own music as the game closes. The game takes a
/// moment to shut down after its window goes, and a file kept playing
/// through it; the game's own songs stop with the game.
pub fn silence_for_exit() {
    CLOSING.store(true, std::sync::atomic::Ordering::SeqCst);
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
    CLOSING.store(false, std::sync::atomic::Ordering::SeqCst);
    REOPENED.store(true, std::sync::atomic::Ordering::SeqCst);
    log::info!("player: the game's window is back, own music can play again");
    open_output();
}

/// Opens the audio output ahead of time, off the game thread, so the first
/// file to play doesn't wait for the device.
pub fn warm_up() {
    open_output();
}

/// How long a file's position may stand still, unpaused, before its output
/// counts as dead (its device unplugged or changed) and is opened again.
const OUTPUT_STALL: Duration = Duration::from_secs(3);

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

/// How long a jump within one of your own songs fades in, over the moment a
/// freshly opened MP3 takes to settle.
const SEEK_FADE: f32 = 0.15;

/// How long a file may take to open before it's skipped as broken.
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the opener may be stuck on one file before a new request gets a
/// thread of its own instead of waiting behind it.
const OPENER_STUCK: Duration = Duration::from_secs(2);

enum Playing {
    Cue {
        id: i32,
        duration: Option<f32>,
        /// Added to the game's time for the cue: a looping theme started
        /// past the end of its audio is wound back inside it (see
        /// `Game::play_cue`), and its position must read as before.
        offset: f32,
        paused: bool,
        started: Instant,
        /// The game has reported this cue as playing at least once.
        seen: bool,
        /// Furthest play position seen, to tell a cut-off start from an end.
        reached: f32,
        /// Where it was started: a song picked up after a battle, at the
        /// game's start or by a seek begins well past zero, and "cut off as
        /// it started" is measured from there.
        from: f32,
        attempt: u8,
        /// A looping theme, faded out over its last [`THEME_FADE`] seconds.
        fades: bool,
        fading: bool,
    },
    File {
        /// Shared with the list of songs fading out.
        sink: std::sync::Arc<Sink>,
        duration: Option<f32>,
        /// The file's key in `config.file_levels` and `file_peaks`.
        key: String,
        /// Where the file was started: the sink counts from there.
        offset: f32,
        /// The file is still being opened by the opener thread (a slow drive
        /// or a big file mustn't hold up the game); `tick` takes it.
        opening: Option<Receiver<Result<Opened, String>>>,
        /// When it was asked to open, for [`OPEN_TIMEOUT`].
        asked: Instant,
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
    /// A cut-off cue waiting to be started again: track, attempt, when, and
    /// the place in the song it was started at.
    retry: Option<(usize, u8, Instant, f32)>,
    /// The current song was picked by Next, Shuffle or a playlist rather
    /// than clicked, so if it will not play the next one is tried.
    auto: bool,
    /// The silent pass over every game song, while it runs.
    check: Option<SongCheck>,
    /// The volume actually applied, catching up with the setting.
    ramped: f32,
    /// Shuffle's deals, one per side (battle or not) and playlist: every
    /// song once, in random order, before any comes round again.
    bags: std::collections::HashMap<(bool, Playlist), Deal>,
    /// Playing from the battle music, and the exploring song (track, position)
    /// to pick up again once the battle ends.
    in_battle: bool,
    battle_resume: Option<(usize, f32)>,
    /// In a battle the exploring song carries on or waits through (no
    /// battle music list, or a boss keeping its music), and where that song
    /// was as it began.
    fight: bool,
    /// A battle is on this tick (with or without its own battle music).
    fighting: bool,
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
    /// Game songs still fading out under the next, and when their fade ends:
    /// a pause, a movie or Stop silences them at once.
    fading_cues: Vec<(i32, Instant)>,
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
    /// A file to start (track, position) once the audio output is open: the
    /// output is opened off the game thread, and the game's window may be
    /// away (see `CLOSING`).
    waiting: Option<(usize, f32)>,
    /// The playing file's position last tick and when it last moved, to
    /// notice an output that has stopped playing.
    stall: Option<(f32, Instant)>,
    /// The level the playing file is at, catching up with what it should
    /// be: a measured gain landing mid-song steps up over a few ticks.
    file_level: f32,
}

/// One shuffle deal: the songs still to come this round, and the ones
/// drawn. A song added to the list joins the round; one removed leaves it.
#[derive(Default)]
struct Deal {
    left: Vec<usize>,
    drawn: Vec<usize>,
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
            fighting: false,
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
            fading_cues: Vec::new(),
            resumed: false,
            saved_at: Instant::now(),
            duck: 1.0,
            duck_at: Instant::now(),
            after_fanfare: false,
            skip_from: None,
            scan: None,
            waiting: None,
            stall: None,
            file_level: 0.0,
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
            Some(list) if *list == Playlist::AllSongs || s.has_songs(list) => list.clone(),
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
            self.deal_exhausted(&list, &songs)
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

    /// The current song's length, when known.
    fn now_duration(&self) -> Option<f32> {
        match &self.now {
            Some((_, Playing::Cue { duration, .. } | Playing::File { duration, .. })) => *duration,
            None => None,
        }
    }

    /// The current song and how far into it, if any.
    fn position(&self, game: &mut Game) -> Option<(usize, f32)> {
        match &self.now {
            Some((i, Playing::Cue { id, offset, .. })) => Some((*i, game.time(*id) + *offset)),
            Some((i, Playing::File { sink, offset, .. })) => Some((*i, Playing::file_pos(sink, *offset))),
            // Between attempts at a cut-off song, or waiting for the output,
            // that song is still the current one.
            None => self.retry.map(|(i, _, _, at)| (i, at)).or(self.waiting),
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
        // Another song chosen during the battle stays, and a paused jukebox
        // keeps its song.
        if self.now.as_ref().map(|(c, _)| *c) != Some(i) || !state::lock().playing {
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
        // A song you started meanwhile (the game's songs can take a while
        // to be listed) is yours to keep.
        if self.now.is_some() || self.retry.is_some() || self.waiting.is_some() {
            self.resumed = true;
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
        let Some((i, at)) = self.position(game) else { return };
        let mut s = state::lock();
        if !s.playing {
            return;
        }
        // The place is kept up to date every tick, so the save made as the
        // game closes has the real one; the disk only sees it now and then.
        let at = (at * 10.0).round() / 10.0;
        if s.config.last_song.as_ref().is_some_and(|(k, t)| *t == at && s.tracks.get(i).is_some_and(|tr| tr.key == *k)) {
            return;
        }
        let Some(key) = s.tracks.get(i).map(|t| t.key.clone()) else { return };
        s.config.last_song = Some((key, at));
        if self.saved_at.elapsed() >= RESUME_SAVE_EVERY {
            self.saved_at = Instant::now();
            s.save_config();
        }
    }

    /// Lets the current song go to make way for the next: faded out over
    /// `fade` seconds, or stopped at once with 0.
    fn retire(&mut self, game: &mut Game, fade: f32) {
        match self.now.take() {
            Some((_, Playing::Cue { id, .. })) if fade > 0.0 => {
                game.fade_out(id, fade);
                self.fading_cues.push((id, Instant::now() + Duration::from_secs_f32(fade)));
            }
            Some((_, Playing::Cue { id, .. })) => game.stop(id),
            Some((_, Playing::File { sink, .. })) if fade > 0.0 => {
                let level = sink.volume();
                self.fading_files.push((sink, Instant::now(), level, fade));
            }
            Some((_, Playing::File { sink, .. })) => sink.stop(),
            None => {}
        }
    }

    /// Turns fading-out files down, and drops them once silent; game songs
    /// fade out by themselves, and are let go once their fade is over.
    fn fade_files(&mut self) {
        let now = Instant::now();
        self.fading_cues.retain(|(_, end)| *end > now);
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
    fn run_check(&mut self, game: &mut Game, scene: crate::config::Scene) {
        let due = {
            let s = state::lock();
            s.game_tracks_ready && s.config.checked_build != s.config.game_build
        };
        if self.check.is_none() && due {
            self.check = Some(SongCheck::new());
        }
        let Some(check) = &mut self.check else { return };
        // No song is started for checking next to a game song the jukebox
        // is playing, nor outside plain exploring: a battle, a scene or a
        // movie starting stops the game's sounds, which would read as a
        // song with no audio.
        let exploring = scene == crate::config::Scene::Exploring;
        let playing_cue = matches!(self.now, Some((_, Playing::Cue { .. }))) || self.retry.is_some();
        let finished = check.step(game, exploring && !playing_cue, exploring);
        let progress = check.progress();
        let mut s = state::lock();
        if finished {
            self.check = None;
            s.config.checked_build = s.config.game_build.clone();
            s.save_config();
            s.check_progress = None;
            s.check_waiting = false;
        } else {
            s.check_progress = Some(progress);
            // The menu says why the count has stopped moving.
            s.check_waiting = playing_cue;
        }
    }

    pub fn tick(&mut self, game: &mut Game) {
        refresh_game_tracks(game);
        let scene = game.scene();
        self.fighting = scene == crate::config::Scene::Battle;

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
        if let Some((i, attempt, when, from)) = self.retry
            && Instant::now() >= when
        {
            self.retry = None;
            log::info!("player: retrying (attempt {})", attempt + 1);
            self.keeping_intent(game, |p, game| p.start_attempt(i, from, attempt, game));
        }
        if let Some(i) = self.skip_from.take() {
            // A pause made while the failed song was starting holds.
            self.keeping_intent(game, |p, game| p.advance_from(game, false, Some(i)));
        }
        self.resume_last(game);
        // Boss battles can keep the game's own music, as the developers
        // scored them; the battle music isn't started for them either.
        let boss = scene == crate::config::Scene::Battle && game.boss_battle();
        let boss_held = boss && boss_setting;
        self.follow_battle(game, scene, play_in, boss_held);
        self.follow_fight(game, scene == crate::config::Scene::Battle);
        self.follow_map(game, scene, play_in);
        self.run_check(game, scene);
        self.fade_files();
        self.finish_opening();
        self.after_reopen(game);
        self.after_device_change(game);
        self.start_waiting(game);
        self.watch_stall(game);

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
            Some((i, Playing::Cue { id, duration, offset, paused: false, started, seen, reached, from, attempt, fades, fading })) => {
                // A looping theme held for the whole fight just keeps looping
                // in the game, with no fade or cut at the end of its length.
                let duration = if loop_battle && *fades { None } else { *duration };
                let exists = game.exists(*id);
                let time = game.time(*id) + *offset;
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
                } else if !exists && cut_off_at_start(*reached, *from, duration) {
                    log::debug!("player: cue id {id} was cut off as it started (attempt {})", *attempt + 1);
                    if *attempt + 1 < CUE_RETRIES {
                        cut_off = Some((*i, *attempt + 1, *from));
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
        if let Some((i, Playing::Cue { reached, from, .. })) = &self.now
            && *reached - *from >= CUT_OFF_WITHIN
        {
            let mut s = state::lock();
            if s.is_unplayable(*i) {
                s.set_unplayable(*i, false);
            }
        }
        if let Some((i, attempt, from)) = cut_off {
            // Keep showing the song as current while it waits.
            self.now = None;
            self.retry = Some((i, attempt, Instant::now() + CUE_RETRY_DELAY, from));
        } else if let Some(i) = failed {
            // A cue the game would not play: skipped for this session, never
            // removed for good. A cue is cut off while a map loads, or held
            // back in some places; only the song check, which plays every
            // song in peace, decides what has no audio at all.
            let title = state::lock().tracks.get(i).map(|t| t.title.clone()).unwrap_or_default();
            self.stop(game);
            log::warn!("player: the game would not play {title} here, skipping it");
            state::lock().skip(i, "The game wouldn't play it then");
            self.skip_on(i, format!("The game won't play {title} here. Try it again later."));
        } else if finished {
            self.advance(game, true);
        }

        let playing = state::lock().playing;
        // The song check's silent probe counts as the game's current BGM
        // while it runs: it is never muted and restored, read as the
        // fanfare, or kept as the song heard.
        let probe = self.check.as_ref().and_then(|c| c.probe_id());
        // The last song checked too: the game can go on naming it as its
        // current music for a moment after it's stopped.
        let bgm_is_probe = game.current_bgm().is_some_and(|id| self.check.as_ref().is_some_and(|c| c.is_probe(id)));
        // The game's victory fanfare plays out over a won battle's results;
        // the jukebox picks up again once the game moves on from it.
        let bgm_cue = if bgm_is_probe { None } else { game.current_bgm_cue() };
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
        // A pending retry, a skip to the next song and a song waiting for the
        // output keep the game's music muted, so the gap is quiet.
        let between = self.retry.is_some() || self.skip_from.is_some() || self.waiting.is_some();
        let active = playing && covered && (self.now.is_some() || between);
        self.set_paused(game, !(active || (playing && fading_out)));
        if active {
            let own = match &self.now {
                Some((_, Playing::Cue { id, .. })) => Some(*id),
                _ => None,
            };
            self.mute.hold(game, [own, probe]);
        } else {
            self.mute.release(game);
        }
        // The game's own song, heard while the game has the music, is kept
        // so it can be favorited afterwards. Only one the song list knows,
        // never a song the song check started itself (`bgm_cue` leaves those
        // out; a check that is only waiting its turn doesn't stop this), and
        // not the victory fanfare, which would replace the song you meant
        // after every won battle.
        if !active
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
        // A song crossfading in rises from silence. A file still opening
        // stays silent, and its fade waits for it (`finish_opening` starts it).
        let opening = matches!(self.now, Some((_, Playing::File { opening: Some(_), .. })));
        let fade = if opening { 0.0 } else { self.fade_in_level() };
        let volume = self.ramped * fade * self.duck;
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
                // Up in steps, down at once, like the volume: the measured
                // gain landing a moment into a quiet song is a step up.
                self.file_level =
                    if want > self.file_level { (self.file_level + VOLUME_RISE).min(want) } else { want };
                if (sink.volume() - self.file_level).abs() > 1e-4 {
                    sink.set_volume(self.file_level);
                }
            }
            _ => {}
        }

        self.last_list = self.list_now(&state::lock());
        self.save_for_resume(game);
        let mut s = state::lock();
        s.held = playing && !covered;
        s.boss = boss;
        s.current = self
            .now
            .as_ref()
            .map(|(i, _)| *i)
            .or(self.retry.map(|(i, ..)| i))
            .or(self.skip_from)
            .or(self.waiting.map(|(i, _)| i));
        match &self.now {
            Some((_, Playing::Cue { id, duration, offset, fades, .. })) => {
                s.position = game.time(*id) + *offset;
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
                    // A pause holds at once: on the tick a song ends, the end
                    // check runs before the pause would otherwise land, and
                    // would start the next song.
                    if !s.playing {
                        drop(s);
                        self.set_paused(game, true);
                    }
                } else if !s.tracks.is_empty() {
                    drop(s);
                    self.advance(game, false);
                }
            }
            Command::Stop => {
                self.stop(game);
                self.silence_fading(game);
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
                // A file is opened again at the new place rather than seeked
                // where it plays: an MP3 decoder seeked mid-song puts out a
                // burst of full-scale noise before it settles, where a fresh
                // one starts from silence. The jump fades in briefly, and the
                // song keeps its length and level while it opens.
                Some((i, Playing::File { duration, key, .. })) => {
                    let (i, length, level_key) = (*i, *duration, key.clone());
                    self.keeping_intent(game, |p, game| p.start_at(i, t.max(0.0), game));
                    if let Some((j, Playing::File { duration, key, opening: Some(_), .. })) = &mut self.now
                        && *j == i
                    {
                        *duration = length;
                        *key = level_key;
                        self.fade_in = Some((Instant::now(), SEEK_FADE));
                    }
                }
                None => {}
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
        let waiting = self.waiting.and_then(|(i, _)| key_of(&s, i));
        let battle = self.battle_resume.and_then(|(i, _)| key_of(&s, i));
        let fight = self.fight_resume.and_then(|(i, _)| key_of(&s, i));
        s.set_files(files);
        // A file that would not play gets another try after a rescan; it may
        // have been fixed or replaced.
        s.skipped.retain(|k, _| !k.starts_with("file:"));
        let find = |key: Option<String>| key.and_then(|k| s.index_of(&k));
        let (now_at, retry_at, skip_at, waiting_at) = (find(now), find(retry), find(skip), find(waiting));
        let (battle_at, fight_at) = (find(battle), find(fight));
        drop(s);
        self.retry = self.retry.zip(retry_at).map(|((_, attempt, when, from), at)| (at, attempt, when, from));
        self.skip_from = skip_at;
        self.waiting = self.waiting.zip(waiting_at).map(|((_, pos), at)| (at, pos));
        self.battle_resume = self.battle_resume.zip(battle_at).map(|((_, pos), at)| (at, pos));
        self.fight_resume = self.fight_resume.zip(fight_at).map(|((_, pos), at)| (at, pos));
        // History and shuffle deals hold places in the old list.
        self.history.clear();
        self.other_history.clear();
        self.bags.clear();
        match (now_at, &mut self.now) {
            (Some(at), Some((i, _))) => *i = at,
            (None, Some(_)) => {
                // Its file is gone: the list goes on, or a paused jukebox
                // stays paused on nothing.
                log::info!("player: the playing song's file is gone");
                self.stop(game);
                if state::lock().playing {
                    self.advance_from(game, false, None);
                }
            }
            _ => {}
        }
    }

    fn advance(&mut self, game: &mut Game, natural_end: bool) {
        let current = self.now.as_ref().map(|(i, _)| *i);
        self.advance_from(game, natural_end, current);
    }

    /// Starts the song after `current` (`None`: the playlist's first).
    fn advance_from(&mut self, game: &mut Game, natural_end: bool, mut current: Option<usize>) {
        // A song lined up with Play next goes first, while exploring; in a
        // battle it waits for the fight to end.
        let up_next = if self.in_battle || self.fighting { None } else { take_up_next(&mut state::lock()) };
        if let Some((i, key)) = up_next {
            let before = current.and_then(|c| state::lock().tracks.get(c).map(|t| t.key.clone()));
            log::info!("player: playing the song lined up next");
            self.after_up_next = Some((key, before));
            self.auto = true;
            self.start(i, game);
            return;
        }
        current = self.continue_from(&state::lock(), current);
        let (which, list, repeat, shuffle) = {
            let s = state::lock();
            let (repeat, shuffle) = self.repeat_and_shuffle(&s);
            let which = self.list_now(&s);
            (which.clone(), s.songs_of(&which, current), repeat, shuffle)
        };
        let len = list.len();
        if len == 0 {
            // Nothing playable anywhere: the jukebox stops rather than
            // reporting the finished song as finished every tick.
            self.stop(game);
            state::lock().playing = false;
            return;
        }
        // Where the current song sits in the playlist. A song played from
        // outside it continues from the playlist's start.
        let at = current.and_then(|c| list.iter().position(|&i| i == c));
        let next = self.choose(&which, &list, at, current, natural_end, repeat, shuffle);
        // A song that will not play is never picked again straight away;
        // with nothing else left, playback stops.
        let next = next.filter(|&i| !(state::lock().is_unplayable(i) && Some(i) == current));
        match next {
            Some(i) => {
                self.auto = true;
                self.start(i, game);
            }
            None => {
                // The playlist has played out (Repeat off). Like Stop, that
                // leaves nothing to pick up at the next start: the place
                // saved is the end of its last song, which would start,
                // end at once and move on.
                self.stop(game);
                let mut s = state::lock();
                s.playing = false;
                if s.config.last_song.take().is_some() {
                    s.save_config();
                }
            }
        }
    }

    /// Where the list goes on from. After a Play next song, the exploring
    /// list carries on from the song it cut in after, not from the start; a
    /// battle list leaves that for the exploring list to use later.
    fn continue_from(&mut self, s: &state::Shared, current: Option<usize>) -> Option<usize> {
        if self.in_battle {
            return current;
        }
        match self.after_up_next.take() {
            Some(after) => after_up_next(s, after, current),
            None => current,
        }
    }

    /// The song after `current` (at `at` in `list`, if it's in it), or none
    /// to stop.
    #[allow(clippy::too_many_arguments)]
    fn choose(
        &mut self,
        which: &Playlist,
        list: &[usize],
        at: Option<usize>,
        current: Option<usize>,
        natural_end: bool,
        repeat: Repeat,
        shuffle: bool,
    ) -> Option<usize> {
        let len = list.len();
        // With Repeat off, Shuffle stops once every song has had its turn.
        let deal_over = shuffle && natural_end && repeat == Repeat::Off && self.deal_exhausted(which, list);
        // Repeat One plays the song again without drawing from the shuffle
        // deal, so no song is used up unheard.
        let repeat_one = natural_end && repeat == Repeat::One && current.is_some();
        let shuffled = if shuffle && !deal_over && !repeat_one { self.draw(which, list, current) } else { None };
        match (current, at, natural_end, repeat) {
            (Some(c), _, true, Repeat::One) => Some(c),
            _ if shuffle => shuffled,
            (_, None, _, _) => list.first().copied(),
            (_, Some(p), true, Repeat::Off) if p + 1 >= len => None,
            (_, Some(p), _, _) => Some(list[(p + 1) % len]),
        }
    }

    /// The shuffle deal for `which` (on this side, battle or not) has had
    /// every song of `list` drawn this round.
    fn deal_exhausted(&self, which: &Playlist, list: &[usize]) -> bool {
        self.bags.get(&(self.in_battle, which.clone())).is_some_and(|deal| {
            let drawn: std::collections::HashSet<usize> = deal.drawn.iter().copied().collect();
            list.iter().all(|i| drawn.contains(i))
        })
    }

    /// Shuffle without repeats: every song of `list` once, in random order,
    /// before any comes round again. A new deal never opens with `current`.
    /// The deal follows the list: a song added joins the round at a random
    /// place, one removed leaves it, and the round isn't started over.
    fn draw(&mut self, which: &Playlist, list: &[usize], current: Option<usize>) -> Option<usize> {
        use rand::Rng;
        use rand::seq::SliceRandom;
        use std::collections::HashSet;
        // Battle and exploring each keep their own deal per list, even when
        // both play the same list, so neither disturbs the other.
        let deal = self.bags.entry((self.in_battle, which.clone())).or_default();
        let in_list: HashSet<usize> = list.iter().copied().collect();
        deal.left.retain(|i| in_list.contains(i));
        deal.drawn.retain(|i| in_list.contains(i));
        if !deal.drawn.is_empty() {
            let known: HashSet<usize> = deal.left.iter().chain(&deal.drawn).copied().collect();
            let mut rng = rand::thread_rng();
            for &i in list.iter().filter(|i| !known.contains(i)) {
                let at = rng.gen_range(0..=deal.left.len());
                deal.left.insert(at, i);
            }
        }
        if deal.left.is_empty() {
            deal.drawn.clear();
            deal.left = list.to_vec();
            deal.left.shuffle(&mut rand::thread_rng());
            // Drawn from the end, so the current song goes to the front.
            if let (Some(c), true) = (current, deal.left.len() > 1)
                && let Some(p) = deal.left.iter().position(|&i| i == c)
            {
                deal.left.remove(p);
                deal.left.insert(0, c);
            }
        }
        let next = deal.left.pop();
        if let Some(n) = next {
            deal.drawn.push(n);
        }
        next
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
        // A change of song crossfades; a seek or a retry starts cleanly. A
        // short outgoing song gets a shorter crossfade (see `crossfade_for`).
        let cf = if std::mem::take(&mut self.crossfade_next) {
            let cf = state::lock().config.crossfade_secs.max(0.0);
            self.now_duration().map_or(cf, |d| crossfade_for(cf, d))
        } else {
            0.0
        };
        // A seek or a retry restarts the same song: no now-playing popup.
        let same_song = attempt > 0 || self.now.as_ref().is_some_and(|(c, _)| *c == i);
        // Whatever this start replaces, it replaces for good: a cut-off cue
        // waiting to be tried again, or a failed song's successor, must not
        // cut in later over the song started now (a battle's, say).
        self.retry = None;
        self.skip_from = None;
        self.waiting = None;
        self.retire(game, cf);
        self.fade_in = (cf > 0.0).then(|| (Instant::now(), cf));
        let volume = if cf > 0.0 { 0.0 } else { self.ramped };
        let loop_secs = state::lock().config.loop_minutes * 60.0;
        let result = match &track.source {
            Source::Game { cue, looping } => match game.play_cue(cue, at) {
                Some((id, offset)) => {
                    game.set_volume(id, volume);
                    let duration = game.cue_duration(cue, *looping, loop_secs);
                    log::debug!("player: {cue} -> id {id}, length {duration:?}");
                    Ok(Some(Playing::Cue {
                        id,
                        duration,
                        offset,
                        paused: false,
                        started: Instant::now(),
                        seen: false,
                        reached: 0.0,
                        from: if at.is_finite() { at.max(0.0) } else { 0.0 },
                        attempt,
                        fades: *looping,
                        fading: false,
                    }))
                }
                // The game's sound system isn't ready: not the song's fault.
                None => Err(format!("The game would not play {cue}")),
            },
            Source::File(path) => match output_ready() {
                Some(handle) => play_file(&handle, path, at).map(Some),
                // The output is opened off the game thread (or the game's
                // window is away): the song starts once it's there, unless
                // the device really isn't there.
                None if output_failed() && !CLOSING.load(std::sync::atomic::Ordering::SeqCst) => {
                    Err(NO_OUTPUT.to_string())
                }
                None => {
                    open_output();
                    Ok(None)
                }
            },
        };
        let mut s = state::lock();
        match result {
            Ok(None) => {
                log::info!("player: {} waits for the sound output", track.title);
                self.waiting = Some((i, at));
                s.playing = true;
                s.error = None;
            }
            Ok(Some(p)) => {
                log::info!("player: playing {}", track.title);
                self.file_level = 0.0;
                // A skipped song chosen by hand gets another try; if it fails
                // again, it's skipped again.
                s.unskip(i);
                // A short game song fades in over a shorter time too (a file's
                // length is known once it opens, in `finish_opening`).
                if let (Some((t, secs)), Playing::Cue { duration: Some(d), .. }) = (self.fade_in, &p) {
                    self.fade_in = Some((t, crossfade_for(secs, *d)));
                }
                self.now = Some((i, p));
                if self.history.last() != Some(&i) {
                    self.history.push(i);
                    if self.history.len() > 200 {
                        self.history.remove(0);
                    }
                }
                s.playing = true;
                s.error = None;
                if !same_song {
                    s.track_serial += 1;
                }
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
    /// frame of the game. A skipped song is passed over by Next and Shuffle
    /// for the session (the menu still shows it, greyed out, to try again),
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
        let Some((i, Playing::File { sink, duration, key, offset, opening: opening @ Some(_), asked })) = &mut self.now
        else {
            return;
        };
        let result = match opening.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(result)) => result,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => Err("The song could not be opened".into()),
            // A drive that has stopped answering mustn't leave silence (and
            // the game's music muted) for ever.
            _ if asked.elapsed() > OPEN_TIMEOUT => Err("The song took too long to open".into()),
            _ => return,
        };
        *opening = None;
        match result {
            Ok(o) => {
                // Silent as it goes in: the audio thread plays it from the
                // moment it's appended, before `tick` sets the level.
                sink.set_volume(0.0);
                sink.append(o.decoder);
                *duration = o.duration;
                *key = o.key;
                *offset = o.at;
                // The fade-in starts now that there's sound to fade in, and
                // a short song gets a shorter one.
                if let Some((_, secs)) = self.fade_in {
                    let secs = o.duration.map_or(secs, |d| crossfade_for(secs, d));
                    self.fade_in = Some((Instant::now(), secs));
                }
            }
            Err(e) => {
                let i = *i;
                sink.stop();
                self.now = None;
                log::error!("player: {e}");
                state::lock().skip(i, e.clone());
                self.skip_on(i, e);
            }
        }
    }

    /// After the output was closed and opened again, the file that was
    /// playing starts again where it stopped, on the new output.
    fn after_reopen(&mut self, game: &mut Game) {
        if !REOPENED.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if let Some((i, at)) = self.position(game)
            && matches!(self.now, Some((_, Playing::File { .. })))
        {
            log::info!("player: the song starts again on the new output at {at:.0}s once it's open");
            self.stop(game);
            self.waiting = Some((i, at));
        }
    }

    /// Windows' default sound device changed: the output is opened again on
    /// the new one, and a file that was playing carries on there from where
    /// it was (paused, if it was paused).
    fn after_device_change(&mut self, game: &mut Game) {
        if !DEVICE_CHANGED.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if CLOSING.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if let Some((i, at)) = self.position(game)
            && matches!(self.now, Some((_, Playing::File { .. })))
        {
            log::info!("player: moving your music to the new sound device at {at:.0}s");
            self.stop(game);
            self.waiting = Some((i, at));
        }
        // Songs still fading out are on the old output, which is going.
        for (sink, ..) in self.fading_files.drain(..) {
            sink.stop();
        }
        reset_output();
        open_output();
    }

    /// Starts the song waiting for the audio output once the output is
    /// open; gives up with the no-device message if it couldn't be opened.
    /// While the game's window is away it just waits.
    fn start_waiting(&mut self, game: &mut Game) {
        let Some((i, at)) = self.waiting else { return };
        if CLOSING.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if output_ready().is_some() {
            self.waiting = None;
            self.keeping_intent(game, |p, game| p.start_at(i, at, game));
        } else if output_failed() {
            self.waiting = None;
            log::error!("player: {NO_OUTPUT}");
            let mut s = state::lock();
            s.playing = false;
            s.error = Some(NO_OUTPUT.to_string());
        } else {
            open_output();
        }
    }

    /// A playing file whose position hasn't moved for a while, unpaused, is
    /// on an output that has died (its device unplugged, or no longer the
    /// default): the output is opened again and the song started where it
    /// stopped, instead of playing on in silence with the game's music muted.
    fn watch_stall(&mut self, game: &mut Game) {
        let Some((i, Playing::File { sink, offset, opening: None, .. })) = &self.now else {
            self.stall = None;
            return;
        };
        if sink.is_paused() || sink.empty() {
            self.stall = None;
            return;
        }
        let pos = Playing::file_pos(sink, *offset);
        match self.stall {
            Some((last, since)) if last == pos => {
                if since.elapsed() > OUTPUT_STALL {
                    let i = *i;
                    log::warn!("player: own music stopped moving at {pos:.1}s, opening the sound output again");
                    self.stall = None;
                    self.stop(game);
                    reset_output();
                    self.waiting = Some((i, pos));
                    open_output();
                }
            }
            _ => self.stall = Some((pos, Instant::now())),
        }
    }

    fn stop(&mut self, game: &mut Game) {
        match self.now.take() {
            Some((_, Playing::Cue { id, .. })) => game.stop(id),
            Some((_, Playing::File { sink, .. })) => sink.stop(),
            None => {}
        }
    }

    /// Stops every song still fading out under the current one.
    fn silence_fading(&mut self, game: &mut Game) {
        for (sink, ..) in self.fading_files.drain(..) {
            sink.stop();
        }
        for (id, _) in self.fading_cues.drain(..) {
            game.stop(id);
        }
    }

    fn set_paused(&mut self, game: &mut Game, pause: bool) {
        // A song fading out under the next stops with it, so a pause or a
        // movie is quiet straight away.
        if pause {
            self.silence_fading(game);
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
/// list and its key. One that's gone or has no audio at all is dropped; one
/// only skipped this session gets another try, as a click on it would.
fn take_up_next(s: &mut state::Shared) -> Option<(usize, String)> {
    let key = s.up_next.take()?;
    let i = s.index_of(&key).filter(|&i| !s.is_silent(i))?;
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

/// A game song that goes quiet within this many seconds of where it was
/// started was cut off by the game, not played.
const CUT_OFF_WITHIN: f32 = 1.0;

/// Whether a game song that has gone quiet was cut off as it started, rather
/// than having ended: it got under a second on from `from`, where it was
/// started (the game's time for a song counts from the song's top, so one
/// picked up two minutes in reads two minutes at once), and it isn't at its
/// own end, which a seek into a song's last moments reaches as quickly.
fn cut_off_at_start(reached: f32, from: f32, duration: Option<f32>) -> bool {
    reached - from < CUT_OFF_WITHIN && !duration.is_some_and(|d| reached >= d - 1.5)
}

/// How long before its end a song of `duration` seconds crossfades into
/// the next: the crossfade setting, but never more than a third of the
/// song, so a short one is heard before it fades rather than skipped at once.
fn crossfade_for(cf: f32, duration: f32) -> f32 {
    cf.min(duration / 3.0)
}

const NO_OUTPUT: &str = "No sound device for your own music. Check your speakers or headphones, then press Play.";

/// Starts a file silent, `at` seconds in; `tick` sets its levelled volume
/// straight after. Opening, decoding and seeking happen on the opener thread,
/// and `tick` hands the file to its sink once it's ready.
fn play_file(handle: &OutputStreamHandle, path: &Path, at: f32) -> Result<Playing, String> {
    let sink = Sink::try_new(handle).map_err(|e| format!("{NO_OUTPUT} ({e})"))?;
    sink.set_volume(0.0);
    let (tx, rx) = std::sync::mpsc::channel();
    let request = OPENS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let music_dir = state::lock().music_dir.clone();
    let req = OpenRequest { path: path.to_path_buf(), music_dir, at, request, reply: tx };
    if opener_stuck() {
        // The opener is stuck on a file (a drive that stopped answering):
        // this one gets a thread of its own rather than waiting behind it.
        std::thread::spawn(move || run_open(req));
    } else if let Err(std::sync::mpsc::SendError(req)) = opener().send(req) {
        // The opener thread is gone (a decoder panic): this file gets its
        // own thread rather than silence.
        std::thread::spawn(move || run_open(req));
    }
    let offset = if at.is_finite() { at.max(0.0) } else { 0.0 };
    Ok(Playing::File {
        sink: std::sync::Arc::new(sink),
        duration: None,
        key: String::new(),
        offset,
        opening: Some(rx),
        asked: Instant::now(),
    })
}

/// A file for the opener thread to open, and where its answer goes.
struct OpenRequest {
    path: std::path::PathBuf,
    /// The music folder, for the file's levelling key.
    music_dir: std::path::PathBuf,
    at: f32,
    /// Its number in [`OPENS`]: a later request makes it outdated.
    request: u64,
    reply: std::sync::mpsc::Sender<Result<Opened, String>>,
}

/// Requests made so far. Only the latest is ever wanted: one song plays at a
/// time, and a song replaced before it opened is dropped by the player.
static OPENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// When the opener started on the file it's working on, as milliseconds
/// since [`opener_clock`] began; 0 while it's idle.
static OPENER_BUSY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn opener_clock() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// Whether the opener has been on one file for longer than [`OPENER_STUCK`].
fn opener_stuck() -> bool {
    let since = OPENER_BUSY.load(std::sync::atomic::Ordering::SeqCst);
    since != 0 && opener_clock().saturating_sub(since) > OPENER_STUCK.as_millis() as u64
}

/// Opens `req`'s file and answers, unless it's outdated by then. A decoder
/// that panics on a broken file answers with an error instead of taking
/// the thread down.
fn run_open(req: OpenRequest) {
    let current = || OPENS.load(std::sync::atomic::Ordering::SeqCst) == req.request;
    let name = req.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        open_file(&req.path, &req.music_dir, req.at, &current)
    }));
    match opened {
        Ok(Some(opened)) => {
            let _ = req.reply.send(opened);
        }
        Ok(None) => {}
        Err(_) => {
            let _ = req.reply.send(Err(format!("Could not decode {name}: the decoder failed")));
        }
    }
}

/// The one thread that opens files, so skipping fast through songs on a slow
/// drive queues them instead of starting a thread each. Waiting requests are
/// skipped for the latest, and an outdated one stops between steps.
fn opener() -> std::sync::mpsc::Sender<OpenRequest> {
    static OPENER: OnceLock<Mutex<std::sync::mpsc::Sender<OpenRequest>>> = OnceLock::new();
    OPENER
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<OpenRequest>();
            std::thread::spawn(move || {
                while let Ok(mut req) = rx.recv() {
                    while let Ok(later) = rx.try_recv() {
                        req = later;
                    }
                    // Marked idle again however the open ends.
                    struct Busy;
                    impl Drop for Busy {
                        fn drop(&mut self) {
                            OPENER_BUSY.store(0, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    OPENER_BUSY.store(opener_clock(), std::sync::atomic::Ordering::SeqCst);
                    let _busy = Busy;
                    run_open(req);
                }
            });
            Mutex::new(tx)
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Opens and decodes a file and moves it `at` seconds in. A file that can't
/// seek starts from the top. `None` once the request is outdated: nobody waits for it.
fn open_file(path: &Path, music_dir: &Path, at: f32, current: &dyn Fn() -> bool) -> Option<Result<Opened, String>> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) => return Some(Err(format!("Could not open {name}: {e}"))),
    };
    if !current() {
        return None;
    }
    let mut decoder = match Decoder::new(BufReader::new(file)) {
        Ok(decoder) => decoder,
        Err(e) => return Some(Err(format!("Could not decode {name}: {e}"))),
    };
    if !current() {
        return None;
    }
    // A length of 0 (a damaged header) would end the song the moment it
    // starts; unknown is better.
    let duration = decoder.total_duration().map(|d| d.as_secs_f32()).filter(|d| *d > 0.0);
    let mut start = 0.0;
    if at > 0.5 && at.is_finite() && duration.is_none_or(|d| at < d) {
        match decoder.try_seek(Duration::from_secs_f32(at)) {
            Ok(()) => start = at,
            Err(e) => log::warn!("player: can't seek in {name} ({e}), starting from the top"),
        }
    }
    let key = loudness::key(path, music_dir);
    loudness::ensure_measured(path, &key);
    Some(Ok(Opened { decoder, duration, key, at: start }))
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

    const ALL: Playlist = Playlist::AllSongs;

    #[test]
    fn a_song_added_to_the_list_joins_the_round_and_one_removed_leaves_it() {
        let mut p = Player::new();
        let first = p.draw(&ALL, &[1, 2, 3], None).unwrap();
        // Song 4 is added mid-round: the rest of the round is the two left
        // plus 4, each once, and the round doesn't start over.
        let mut rest: Vec<usize> = (0..3).map(|_| p.draw(&ALL, &[1, 2, 3, 4], None).unwrap()).collect();
        rest.sort_unstable();
        let mut expect: Vec<usize> = [1, 2, 3, 4].into_iter().filter(|&i| i != first).collect();
        expect.sort_unstable();
        assert_eq!(rest, expect);
        assert!(p.deal_exhausted(&ALL, &[1, 2, 3, 4]));
        // A song removed leaves the round; one still to come stays.
        let second = p.draw(&ALL, &[5, 6, 7], None).unwrap();
        assert!(!p.deal_exhausted(&ALL, &[5, 6, 7]));
        let next = p.draw(&ALL, &[6, 7], None).unwrap();
        assert!(next != second && (next == 6 || next == 7));
    }

    #[test]
    fn shuffle_plays_every_song_once_before_repeating() {
        let mut p = Player::new();
        let list = vec![3, 5, 7, 9, 11];
        for _ in 0..20 {
            let mut seen: Vec<usize> = (0..list.len()).map(|_| p.draw(&ALL, &list, None).unwrap()).collect();
            seen.sort_unstable();
            assert_eq!(seen, list);
        }
    }

    #[test]
    fn a_new_deal_never_opens_with_the_current_song() {
        for _ in 0..50 {
            let mut p = Player::new();
            assert_ne!(p.draw(&ALL, &[1, 2, 3], Some(2)), Some(2));
        }
    }

    #[test]
    fn a_different_playlist_has_a_deal_of_its_own() {
        let mut p = Player::new();
        p.draw(&ALL, &[1, 2, 3], None);
        let next = p.draw(&Playlist::Favorites, &[8, 9], None).unwrap();
        assert!(next == 8 || next == 9);
        // The first playlist's round is where it was.
        assert!(!p.deal_exhausted(&ALL, &[1, 2, 3]));
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
            last = p.draw(&ALL, &songs, None).unwrap();
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
        let first = p.draw(&ALL, &list, None).unwrap();
        // A whole battle deal of the same list, unaffected by the exploring one.
        p.in_battle = true;
        let mut battle: Vec<usize> = (0..list.len()).map(|_| p.draw(&ALL, &list, None).unwrap()).collect();
        battle.sort_unstable();
        assert_eq!(battle, list);
        // Back out of battle, the exploring deal carries on without `first`.
        p.in_battle = false;
        let rest: Vec<usize> = (0..list.len() - 1).map(|_| p.draw(&ALL, &list, None).unwrap()).collect();
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
    fn a_song_cut_off_where_it_was_picked_up_is_not_taken_for_finished() {
        // From the top, quiet after a moment: cut off.
        assert!(cut_off_at_start(0.0, 0.0, Some(180.0)));
        assert!(cut_off_at_start(0.4, 0.0, None));
        // Picked up at 109 s after a battle and quiet at once: cut off too,
        // though the game's clock already reads 109 s.
        assert!(cut_off_at_start(109.2, 109.2, Some(180.0)));
        // It played on for a while from there: it ended.
        assert!(!cut_off_at_start(125.0, 109.2, Some(180.0)));
        assert!(!cut_off_at_start(3.0, 0.0, None));
        // A seek into a jingle's last moments runs out that quickly by itself.
        assert!(!cut_off_at_start(24.6, 24.2, Some(24.8)));
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
        // A song skipped this session gets another try, like a click on it.
        s.up_next = Some(s.tracks[1].key.clone());
        s.skip(1, "Could not decode");
        assert_eq!(take_up_next(&mut s), Some((1, s.tracks[1].key.clone())));
        // One with no audio at all is dropped from the queue.
        s.up_next = Some(s.tracks[1].key.clone());
        s.set_unplayable(1, true);
        assert_eq!(take_up_next(&mut s), None);
        assert_eq!(s.up_next, None);
    }

    #[test]
    fn repeat_one_uses_up_no_shuffle_picks() {
        let mut p = Player::new();
        let list = vec![1, 2, 3, 4];
        for _ in 0..10 {
            assert_eq!(p.choose(&ALL, &list, Some(1), Some(2), true, Repeat::One, true), Some(2));
        }
        // Repeat off again: the whole deal is still there, every song once.
        let mut seen: Vec<usize> = (0..list.len()).map(|_| p.draw(&ALL, &list, None).unwrap()).collect();
        seen.sort_unstable();
        assert_eq!(seen, list);
    }

    #[test]
    fn a_battle_list_leaves_the_play_next_place_for_later() {
        let s = three_songs();
        let mut p = Player::new();
        p.after_up_next = Some((s.tracks[2].key.clone(), Some(s.tracks[0].key.clone())));
        // The battle list starts from its own place and leaves it be.
        p.in_battle = true;
        assert_eq!(p.continue_from(&s, Some(2)), Some(2));
        assert!(p.after_up_next.is_some());
        // Back exploring, the queued song ends: the list goes on after song 0.
        p.in_battle = false;
        assert_eq!(p.continue_from(&s, Some(2)), Some(0));
        assert!(p.after_up_next.is_none());
    }
}
