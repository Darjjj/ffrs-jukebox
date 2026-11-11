//! A silent pass over every game song, finding the ones with no audio in the
//! game's files. Each song is started at zero volume and counts as playable
//! once its position moves; one that is cut off as it starts, or never
//! starts, is tried once more, a while later and after the rest, and then
//! left out of every list. One the game won't start at all (it refuses while
//! a map loads) proves nothing: it is tried again later, and its verdict is
//! left alone if it never starts. The player only runs the check while
//! exploring, where the game isn't stopping its music for a scene or a load.
//!
//! Songs are kept by track key, never by their place in the song list: a
//! rescan of the music folder moves every game song along, and a verdict
//! must land on the song that was actually played.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::game::Game;
use crate::state::{self, Source};

/// Play position that proves a song has audio.
const PLAYED: f32 = 0.3;
/// How long a song may take to start, or to reach [`PLAYED`].
const TIMEOUT: Duration = Duration::from_secs(4);
/// Pause between songs, so one is fully stopped before the next starts.
const GAP: Duration = Duration::from_millis(150);
/// Times a song the game won't start is tried, and the wait after each.
const REFUSALS: u8 = 3;
const REFUSED_WAIT: Duration = Duration::from_secs(2);

struct Probe {
    key: String,
    id: i32,
    started: Instant,
    seen: bool,
    reached: f32,
    attempt: u8,
}

pub struct SongCheck {
    queue: VecDeque<(String, u8)>,
    total: usize,
    done: usize,
    probe: Option<Probe>,
    next_at: Instant,
    silent: Vec<String>,
    /// Songs the game wouldn't start, and how often.
    refused: HashMap<String, u8>,
    /// The play id of the last song checked, kept after it's stopped: the
    /// game can go on naming it as its current music for a moment.
    last_id: Option<i32>,
}

enum Verdict {
    Playing,
    Silent,
    Waiting,
}

impl SongCheck {
    /// A check of every game song currently listed.
    pub fn new() -> Self {
        let queue: VecDeque<_> =
            state::lock().tracks.iter().filter(|t| t.is_game()).map(|t| (t.key.clone(), 0)).collect();
        log::info!("check: checking {} game songs for audio", queue.len());
        Self {
            total: queue.len(),
            queue,
            done: 0,
            probe: None,
            next_at: Instant::now(),
            silent: Vec::new(),
            refused: HashMap::new(),
            last_id: None,
        }
    }

    /// Stops the song being checked, for a check that is being replaced.
    pub fn cancel(&mut self, game: &mut Game) {
        if let Some(p) = self.probe.take() {
            game.stop(p.id);
        }
    }

    pub fn progress(&self) -> (usize, usize) {
        (self.done, self.total)
    }

    /// The play id of the song being checked right now, if one is.
    pub fn probe_id(&self) -> Option<i32> {
        self.probe.as_ref().map(|p| p.id)
    }

    /// Whether `id` is the song being checked, or the last one that was.
    pub fn is_probe(&self, id: i32) -> bool {
        self.last_id == Some(id)
    }

    /// Advances the check. `may_start` is false while the jukebox is playing
    /// a game song, so no new song is started next to it. `exploring` is
    /// whether the game is plainly exploring: a song that goes quiet as a
    /// battle, a scene or a movie begins was stopped by the game, which says
    /// nothing about its audio, so it's tried again without that counting.
    /// Returns true once every song has been checked.
    pub fn step(&mut self, game: &mut Game, may_start: bool, exploring: bool) -> bool {
        if let Some(p) = &mut self.probe {
            let exists = game.exists(p.id);
            if exists {
                p.seen = true;
                p.reached = p.reached.max(game.time(p.id));
                // Kept silent: the game treats the probe as its current BGM
                // for the moment, and nothing may turn it up.
                game.set_volume(p.id, 0.0);
            }
            let verdict = if p.reached >= PLAYED {
                Verdict::Playing
            } else if p.seen && !exists {
                Verdict::Silent
            } else if p.started.elapsed() > TIMEOUT {
                // Running but not moving is not proof of silence.
                if p.seen { Verdict::Playing } else { Verdict::Silent }
            } else {
                Verdict::Waiting
            };
            let (key, attempt) = (p.key.clone(), p.attempt);
            match verdict {
                Verdict::Waiting => return false,
                Verdict::Silent if !exploring => {
                    game.stop(p.id);
                    self.queue.push_back((key, attempt));
                    self.probe = None;
                    self.next_at = Instant::now() + REFUSED_WAIT;
                    return false;
                }
                Verdict::Playing => {
                    game.stop(p.id);
                    state::lock().set_unplayable_key(&key, false);
                    self.done += 1;
                }
                // Cut off once: tried again after the others, and not right
                // away, so a load or a scene change that stopped it has
                // passed by then.
                Verdict::Silent if attempt == 0 => {
                    game.stop(p.id);
                    self.queue.push_back((key, 1));
                    self.probe = None;
                    self.next_at = Instant::now() + REFUSED_WAIT;
                    return false;
                }
                Verdict::Silent => {
                    game.stop(p.id);
                    let mut s = state::lock();
                    s.set_unplayable_key(&key, true);
                    let title = s.tracks.iter().find(|t| t.key == key).map_or(key.clone(), |t| t.title.clone());
                    log::info!("check: no audio: {title}");
                    self.silent.push(title);
                    self.done += 1;
                }
            }
            self.probe = None;
            self.next_at = Instant::now() + GAP;
        }

        if !may_start || Instant::now() < self.next_at {
            return false;
        }
        let Some((key, attempt)) = self.queue.pop_front() else {
            log::info!(
                "check: done, {} of {} game songs have no audio{}",
                self.silent.len(),
                self.total,
                if self.silent.is_empty() { String::new() } else { format!(": {}", self.silent.join(", ")) }
            );
            return true;
        };
        let cue = match state::lock().tracks.iter().find(|t| t.key == key).map(|t| t.source.clone()) {
            Some(Source::Game { cue, .. }) => cue,
            _ => {
                self.done += 1;
                return false;
            }
        };
        match game.play_cue(&cue, 0.0) {
            Some((id, _)) => {
                game.set_volume(id, 0.0);
                self.last_id = Some(id);
                self.probe = Some(Probe { key, id, started: Instant::now(), seen: false, reached: 0.0, attempt });
            }
            None => {
                let refused = self.refused.entry(key.clone()).or_default();
                *refused += 1;
                if *refused < REFUSALS {
                    self.queue.push_back((key, attempt));
                } else {
                    log::info!("check: the game would not start {cue}, left as it was");
                    self.done += 1;
                }
                self.next_at = Instant::now() + REFUSED_WAIT;
            }
        }
        false
    }
}
