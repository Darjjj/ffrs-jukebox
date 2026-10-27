//! A silent pass over every game song, finding the ones with no audio in the
//! game's files. Each song is started at zero volume and counts as playable
//! once its position moves; one that is cut off as it starts, or never
//! starts, is tried once more and then left out of every list.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::game::Game;
use crate::state::{self, Source};

/// Play position that proves a song has audio.
const PLAYED: f32 = 0.3;
/// How long a song may take to start, or to reach [`PLAYED`].
const TIMEOUT: Duration = Duration::from_secs(4);
/// Pause between songs, so one is fully stopped before the next starts.
const GAP: Duration = Duration::from_millis(150);

struct Probe {
    track: usize,
    id: i32,
    started: Instant,
    seen: bool,
    reached: f32,
    attempt: u8,
}

pub struct SongCheck {
    queue: VecDeque<(usize, u8)>,
    total: usize,
    done: usize,
    probe: Option<Probe>,
    next_at: Instant,
    silent: Vec<String>,
}

enum Verdict {
    Playing,
    Silent,
    Waiting,
}

impl SongCheck {
    /// A check of every game song currently listed.
    pub fn new() -> Self {
        let queue: VecDeque<_> = state::lock()
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_game())
            .map(|(i, _)| (i, 0))
            .collect();
        log::info!("check: checking {} game songs for audio", queue.len());
        Self {
            total: queue.len(),
            queue,
            done: 0,
            probe: None,
            next_at: Instant::now(),
            silent: Vec::new(),
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

    /// Advances the check. `may_start` is false while the jukebox is playing
    /// a game song, so no new song is started next to it. Returns true once
    /// every song has been checked.
    pub fn step(&mut self, game: &mut Game, may_start: bool) -> bool {
        if let Some(p) = &mut self.probe {
            let exists = game.exists(p.id);
            if exists {
                p.seen = true;
                p.reached = p.reached.max(game.time(p.id));
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
            let (track, attempt) = (p.track, p.attempt);
            match verdict {
                Verdict::Waiting => return false,
                Verdict::Playing => {
                    game.stop(p.id);
                    state::lock().set_unplayable(track, false);
                    self.done += 1;
                }
                Verdict::Silent if attempt == 0 => {
                    game.stop(p.id);
                    self.queue.push_front((track, 1));
                }
                Verdict::Silent => {
                    game.stop(p.id);
                    let mut s = state::lock();
                    s.set_unplayable(track, true);
                    let title = s.tracks.get(track).map(|t| t.title.clone()).unwrap_or_default();
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
        let Some((track, attempt)) = self.queue.pop_front() else {
            log::info!(
                "check: done, {} of {} game songs have no audio{}",
                self.silent.len(),
                self.total,
                if self.silent.is_empty() { String::new() } else { format!(": {}", self.silent.join(", ")) }
            );
            return true;
        };
        let cue = match state::lock().tracks.get(track).map(|t| t.source.clone()) {
            Some(Source::Game { cue, .. }) => cue,
            _ => {
                self.done += 1;
                return false;
            }
        };
        match game.play_cue(&cue, 0.0) {
            Some(id) => {
                game.set_volume(id, 0.0);
                self.probe = Some(Probe { track, id, started: Instant::now(), seen: false, reached: 0.0, attempt });
            }
            None => {
                state::lock().set_unplayable(track, true);
                self.silent.push(cue);
                self.done += 1;
            }
        }
        false
    }
}
