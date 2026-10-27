//! Levels your own music to the game's. Commercial tracks are mastered far
//! louder than game soundtracks, so each file's loudness is measured once,
//! on a thread of its own, and turned into a gain that brings it to
//! [`TARGET_DB`]. Gains are kept in the config by file (path and size), so a
//! replaced file is measured again.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Mutex;

use rodio::{Decoder, Source};

use crate::state;

/// Loudness the files are brought to, in dB of the signal's gated RMS: about
/// where game soundtracks sit, well under a modern pop master (-8 to -12).
pub const TARGET_DB: f32 = -20.0;
/// The gain used while a file is still being measured: about what a typical
/// commercial track needs, so the first seconds aren't too loud.
pub const PROVISIONAL: f32 = 0.35;
/// 400 ms blocks quieter than this don't count, so silence and fades don't
/// make a track measure quieter than it sounds.
const GATE_DB: f32 = -60.0;

/// The config key for a file: its path inside `music_dir` and its size.
pub fn key(path: &Path, music_dir: &Path) -> String {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let rel = path.strip_prefix(music_dir).unwrap_or(path).to_string_lossy().replace('\\', "/");
    format!("{rel}|{size}")
}

/// A level offset in dB as a gain.
pub fn db_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Starts measuring the file at `path` (config key `key`) unless its gain
/// and peak are both saved already. Files measured before peaks were kept
/// are measured again, once.
pub fn ensure_measured(path: &Path, key: &str) {
    {
        let s = state::lock();
        if s.config.file_levels.contains_key(key) && s.config.file_peaks.contains_key(key) {
            return;
        }
    }
    // One measurement per file at a time, however often it is replayed.
    if MEASURING.lock().unwrap_or_else(|e| e.into_inner()).insert(key.to_string()) {
        let (path, key) = (path.to_path_buf(), key.to_string());
        std::thread::spawn(move || {
            measure_and_store(&path, &key);
            MEASURING.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
        });
    }
}

/// A file's saved levelling gain and peak, each `None` until measured.
pub fn saved(key: &str) -> (Option<f32>, Option<f32>) {
    let s = state::lock();
    (s.config.file_levels.get(key).copied(), s.config.file_peaks.get(key).copied())
}

static MEASURING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn measure_and_store(path: &Path, key: &str) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match measure(path) {
        Some((db, peak)) => {
            let gain = gain_for(db);
            log::info!("loudness: {name} measures {db:.1} dB, peak {peak:.2}, gain {gain:.2}");
            let mut s = state::lock();
            s.config.file_levels.insert(key.to_string(), gain);
            s.config.file_peaks.insert(key.to_string(), peak);
            s.save_config();
        }
        None => log::warn!("loudness: could not measure {name}"),
    }
}

/// Highest a file's final level may go, as a fraction of full scale.
const HEADROOM: f32 = 0.98;

/// The level a file plays at: `wanted` (volume, levelling, Level, the
/// game's own volumes) capped so its loudest sample stays under full scale.
/// Boosting past 100% or a big Level can never push a song into clipping;
/// until its peak is known, nothing over 1.0 is allowed.
pub fn safe_level(wanted: f32, peak: Option<f32>) -> f32 {
    let limit = match peak {
        Some(p) if p > 0.0 => HEADROOM / p,
        _ => 1.0,
    };
    wanted.min(limit).max(0.0)
}

/// The gain that brings a track measuring `db` to the target. Never boosts
/// more than 2x, so a quiet or near-silent file isn't blown up.
pub fn gain_for(db: f32) -> f32 {
    10f32.powf((TARGET_DB - db) / 20.0).clamp(0.02, 2.0)
}

/// Gated RMS loudness of the whole file in dB, and its peak sample level.
fn measure(path: &Path) -> Option<(f32, f32)> {
    let decoder = Decoder::new(BufReader::new(File::open(path).ok()?)).ok()?;
    let rate = decoder.sample_rate().max(1) as usize;
    let channels = decoder.channels().max(1) as usize;
    let block = rate * channels * 2 / 5;
    let mut peak = 0.0f32;
    let samples = decoder.convert_samples::<f32>().inspect(|s| peak = peak.max(s.abs()));
    let db = block_db(samples, block)?;
    Some((db, peak))
}

/// Gated RMS of `samples` in blocks of `block` samples.
pub fn block_db(samples: impl Iterator<Item = f32>, block: usize) -> Option<f32> {
    let gate = 10f32.powf(GATE_DB / 10.0);
    let (mut sum, mut n, mut kept, mut kept_blocks) = (0.0f64, 0usize, 0.0f64, 0usize);
    let mut flush = |sum: f64, n: usize| {
        let ms = sum / n.max(1) as f64;
        if ms as f32 > gate {
            kept += ms;
            kept_blocks += 1;
        }
    };
    for s in samples {
        sum += (s as f64) * (s as f64);
        n += 1;
        if n == block {
            flush(sum, n);
            sum = 0.0;
            n = 0;
        }
    }
    if n > block / 4 {
        flush(sum, n);
    }
    (kept_blocks > 0).then(|| 10.0 * ((kept / kept_blocks as f64) as f32).log10())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_scale_sine_measures_about_minus_three_db() {
        let sine = (0..48_000).map(|i| (i as f32 * 0.05).sin());
        let db = block_db(sine, 4800).unwrap();
        assert!((db + 3.0).abs() < 0.2, "{db}");
    }

    #[test]
    fn silence_is_gated_out() {
        let quiet = (0..48_000).map(|i| if i < 24_000 { 0.0 } else { (i as f32 * 0.05).sin() * 0.5 });
        let db = block_db(quiet, 4800).unwrap();
        // Only the half-scale half counts: -3 dB - 6 dB.
        assert!((db + 9.0).abs() < 0.3, "{db}");
        assert!(block_db(std::iter::repeat_n(0.0, 48_000), 4800).is_none());
    }

    #[test]
    fn loud_masters_are_turned_down_and_quiet_ones_capped() {
        assert!((gain_for(-10.0) - 0.316).abs() < 0.01);
        assert!((gain_for(TARGET_DB) - 1.0).abs() < 1e-6);
        assert_eq!(gain_for(-60.0), 2.0);
    }

    /// `JUKEBOX_DIR=<music folder> cargo test -- --nocapture measures_real_files`
    #[test]
    fn measures_real_files() {
        let Ok(dir) = std::env::var("JUKEBOX_DIR") else { return };
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let m = measure(&e.path());
            println!("{:40} {m:?} -> gain {:?}", e.file_name().to_string_lossy(), m.map(|(db, _)| gain_for(db)));
        }
    }

    #[test]
    fn levels_never_push_a_peak_past_full_scale() {
        // A quiet file (peak 0.5) asked for 6x: capped at just under 2x.
        assert!((safe_level(6.0, Some(0.5)) - 1.96).abs() < 1e-4);
        // Within headroom it is left alone.
        assert_eq!(safe_level(0.4, Some(1.0)), 0.4);
        // Unmeasured: never above 1.0.
        assert_eq!(safe_level(3.0, None), 1.0);
    }
}