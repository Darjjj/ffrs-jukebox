//! Playback: runs the queued commands, follows the mode, advances the
//! playlist. Lives on the game thread (see [`crate::game`]).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source as _};

use crate::check::SongCheck;
use crate::loudness;
use crate::config::{Playlist, Repeat};
use crate::game::{BgmMute, Game, refresh_game_tracks};
use crate::state::{self, Command, Source};

/// The output stream must stay on the thread that opened it, so it is opened
/// on a thread of its own that keeps it alive for the life of the process.
fn output() -> Option<&'static OutputStreamHandle> {
    static HANDLE: OnceLock<Option<OutputStreamHandle>> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || match OutputStream::try_default() {
                Ok((stream, handle)) => {
                    let _ = tx.send(Some(handle));
                    let _stream = stream;
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    log::error!("player: no audio output: {e}");
                    let _ = tx.send(None);
                }
            });
            rx.recv().ok().flatten()
        })
        .as_ref()
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
        sink: Sink,
        duration: Option<f32>,
        /// The file's key in `config.file_levels` and `file_peaks`.
        key: String,
    },
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
    /// The next start is a change of song, so it crossfades.
    crossfade_next: bool,
    /// When the current song started fading in, and over how long.
    fade_in: Option<(Instant, f32)>,
    /// Files still fading out under the next song: sink, start, its level
    /// then, and how long.
    fading_files: Vec<(Sink, Instant, f32, f32)>,
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
}

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
            battle_resume: None,
            crossfade_next: false,
            fade_in: None,
            fading_files: Vec::new(),
            resumed: false,
            saved_at: Instant::now(),
            duck: 1.0,
            duck_at: Instant::now(),
            after_fanfare: false,
        }
    }

    /// The list Next and Shuffle draw from right now: the battle music
    /// during a battle that has one, otherwise "Play from".
    fn list_now(&self, s: &state::Shared) -> Playlist {
        match (&s.config.battle_playlist, self.in_battle) {
            (Some(b), true) => b.clone(),
            _ => s.config.playlist.clone(),
        }
    }

    /// The current song and how far into it, if any.
    fn position(&self, game: &mut Game) -> Option<(usize, f32)> {
        match &self.now {
            Some((i, Playing::Cue { id, .. })) => Some((*i, game.time(*id))),
            Some((i, Playing::File { sink, .. })) => Some((*i, sink.get_pos().as_secs_f32())),
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
            log::info!("player: battle over, back to the exploring song at {at:.0}s");
            self.crossfade_next = true;
            self.start_at(i, at, game);
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
        for c in commands {
            // Choosing other music replaces a pending retry; pausing,
            // seeking or rescanning leave it to go ahead.
            if matches!(c, Command::Play(_) | Command::Next | Command::Previous | Command::Stop) {
                self.retry = None;
            }
            self.run(c, game);
        }
        if let Some((i, attempt, at)) = self.retry {
            if Instant::now() >= at {
                self.retry = None;
                log::info!("player: retrying (attempt {})", attempt + 1);
                self.start_attempt(i, 0.0, attempt, game);
            }
        }
        self.resume_last(game);
        // Boss battles can keep the game's own music, as the developers
        // scored them; the battle music isn't started for them either.
        let boss = scene == crate::config::Scene::Battle && game.boss_battle();
        let boss_held = boss && boss_setting;
        self.follow_battle(game, scene, play_in, boss_held);
        self.run_check(game);
        self.fade_files();

        // A game cue that ended on its own, or a file that ran out. A cue
        // only counts as ended once it has been seen playing; one that never
        // starts, or is cut off as it starts, is retried and then an error,
        // never a reason to skip ahead.
        let mut failed = None;
        let mut cut_off = None;
        let loop_battle = self.in_battle && state::lock().config.loop_battle_song;
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
                } else if cf > 0.0 && duration.is_some_and(|d| time >= d - cf) {
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
            Some((_, Playing::File { sink, duration, .. })) => {
                sink.empty()
                    || (cf > 0.0 && duration.is_some_and(|d| sink.get_pos().as_secs_f32() >= d - cf))
            }
            _ => false,
        };
        // A cue that has really played clears any earlier failure.
        if let Some((i, Playing::Cue { reached, .. })) = &self.now {
            if *reached >= 1.0 && state::lock().is_unplayable(*i) {
                state::lock().set_unplayable(*i, false);
            }
        }
        if let Some((i, attempt)) = cut_off {
            // Keep showing the song as current while it waits.
            self.now = None;
            self.retry = Some((i, attempt, Instant::now() + CUE_RETRY_DELAY));
        } else if let Some(i) = failed {
            let title = state::lock().tracks.get(i).map(|t| t.title.clone()).unwrap_or_default();
            log::error!("player: the game would not play {title}");
            self.stop(game);
            state::lock().set_unplayable(i, true);
            if self.auto {
                // Reached by Next, Shuffle or a playlist: go on to the song
                // after it.
                self.advance_from(game, false, Some(i));
            } else {
                let mut s = state::lock();
                s.playing = false;
                s.error = Some(format!(
                    "{title} has no audio in the game's files, so it was removed from the list."
                ));
            }
        } else if finished {
            self.advance(game, true);
        }

        let playing = state::lock().playing;
        // The game's victory fanfare plays out over a won battle's results;
        // the jukebox picks up again once the game moves on from it.
        let mut fanfare = fanfare_setting && game.current_bgm_cue().as_deref() == Some(crate::game::VICTORY_CUE);
        // The battle ends a moment after the fanfare does; the battle song
        // stays down until then instead of coming back for that moment.
        let in_battle_scene = scene == crate::config::Scene::Battle;
        self.after_fanfare = in_battle_scene && (self.after_fanfare || fanfare);
        fanfare |= self.after_fanfare;
        crate::log_change("fanfare", format!("player: victory fanfare playing={fanfare}"));
        let covered = play_in.covers(scene) && !fanfare && !boss_held;
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
            Some((_, Playing::File { sink, duration, .. })) => {
                s.position = sink.get_pos().as_secs_f32();
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
            Command::Play(i) => {
                self.auto = false;
                self.start(i, game);
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
                let mut s = state::lock();
                s.playing = false;
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
                    self.start_at(i, t, game);
                }
                Some((_, Playing::File { sink, .. })) => {
                    let _ = sink.try_seek(Duration::from_secs_f32(t.max(0.0)));
                }
                None => {}
            },
            Command::Rescan => {
                if matches!(self.now, Some((_, Playing::File { .. }))) {
                    self.stop(game);
                }
                self.history.clear();
                self.other_history.clear();
                let mut s = state::lock();
                let files = state::scan_folder(&s.music_dir);
                log::info!("player: {} files in {}", files.len(), s.music_dir.display());
                let old_files = s.tracks.iter().filter(|t| !t.is_game()).count();
                let game_tracks: Vec<_> = s.tracks.drain(..).filter(|t| t.is_game()).collect();
                // Game tracks keep their place after the files, so a playing
                // cue's index moves by the change in file count.
                let shift = files.len() as isize - old_files as isize;
                s.tracks = files;
                s.tracks.extend(game_tracks);
                s.apply_titles();
                let moved = |i: usize| (i as isize + shift).max(0) as usize;
                if let Some((i, _)) = &mut self.now {
                    *i = moved(*i);
                }
                if let Some((i, _, _)) = &mut self.retry {
                    *i = moved(*i);
                }
            }
        }
    }

    fn advance(&mut self, game: &mut Game, natural_end: bool) {
        let current = self.now.as_ref().map(|(i, _)| *i);
        self.advance_from(game, natural_end, current);
    }

    /// Starts the song after `current` (`None`: the playlist's first).
    fn advance_from(&mut self, game: &mut Game, natural_end: bool, current: Option<usize>) {
        let (list, repeat, shuffle) = {
            let s = state::lock();
            // A battle list has its own shuffle switch, and can hold the
            // song it started with for the whole fight.
            let battle_list = self.in_battle && s.config.battle_playlist.is_some();
            let shuffle = if battle_list { s.config.battle_shuffle } else { s.config.shuffle };
            let repeat = if battle_list && s.config.loop_battle_song { Repeat::One } else { s.config.repeat };
            (s.songs_of(&self.list_now(&s), current), repeat, shuffle)
        };
        let len = list.len();
        if len == 0 {
            return;
        }
        // Where the current song sits in the playlist. A song played from
        // outside it continues from the playlist's start.
        let at = current.and_then(|c| list.iter().position(|&i| i == c));
        let shuffled = if shuffle { self.draw(&list, current) } else { None };
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
            if let (Some(c), true) = (current, bag.len() > 1) {
                if let Some(p) = bag.iter().position(|&i| i == c) {
                    bag.remove(p);
                    bag.insert(0, c);
                }
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
        // A change of song crossfades; a seek or a retry starts cleanly.
        let cf = if std::mem::take(&mut self.crossfade_next) {
            state::lock().config.crossfade_secs.max(0.0)
        } else {
            0.0
        };
        self.retire(game, cf);
        self.fade_in = (cf > 0.0).then(|| (Instant::now(), cf));
        let Some(track) = state::lock().tracks.get(i).cloned() else { return };
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
                None => Err(format!("The game would not play {cue}")),
            },
            Source::File(path) => play_file(path),
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
            Err(e) => {
                log::error!("player: {e}");
                s.error = Some(e);
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

/// Starts a file silent; `tick` sets its levelled volume straight after.
fn play_file(path: &Path) -> Result<Playing, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let handle = output().ok_or("No audio output device")?;
    let file = File::open(path).map_err(|e| format!("Could not open {name}: {e}"))?;
    let decoder =
        Decoder::new(BufReader::new(file)).map_err(|e| format!("Could not decode {name}: {e}"))?;
    let duration = decoder.total_duration().map(|d| d.as_secs_f32());
    let sink = Sink::try_new(handle).map_err(|e| format!("Audio output failed: {e}"))?;
    let key = loudness::key(path, &state::lock().music_dir);
    loudness::ensure_measured(path, &key);
    sink.set_volume(0.0);
    sink.append(decoder);
    Ok(Playing::File { sink, duration, key })
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
}