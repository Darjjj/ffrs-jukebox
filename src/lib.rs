//! FF Resonance Jukebox: an in-game menu (F9) that plays any of the game's
//! songs, or your own files, in place of the game's music.

mod check;
mod config;
mod game;
mod keys;
mod loudness;
mod menu;
mod player;
mod state;
mod theme;
mod titles;

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use grapnel::MidHook;
use windows::Win32::Foundation::{HMODULE, SYSTEMTIME};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleA};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::System::Threading::GetCurrentThreadId;

fn module_dir(module: HMODULE) -> PathBuf {
    // Long enough for any path Windows allows, not just the classic 260.
    let mut buf = vec![0u16; 32_768];
    let len = unsafe { GetModuleFileNameW(module, &mut buf) } as usize;
    PathBuf::from(String::from_utf16_lossy(&buf[..len]))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default()
}

/// The game build, for the song check and the songs known to have no audio:
/// the exe's link time stamp, which changes with every update and with
/// nothing else (a Steam file check that rewrites the exe doesn't change it).
fn game_build() -> String {
    match game_image() {
        Some((_, _, stamp)) => format!("{stamp:#010X}"),
        None => std::env::current_exe()
            .and_then(std::fs::metadata)
            .map(|m| format!("{}", m.len()))
            .unwrap_or_default(),
    }
}

/// Logs to the file from a thread of its own: a line costs the thread that
/// logs it a channel send, not a write to the disk (the game thread logs
/// too). Lines are written as they arrive and flushed when the queue is
/// empty, so the file is current within a moment.
fn init_logging(path: &std::path::Path) {
    use std::io::Write;
    // Last session's log is kept beside the new one: after a crash the game
    // is usually started again before anyone thinks to look at the log.
    let _ = std::fs::rename(path, path.with_extension("log.old"));
    let Ok(file) = File::create(path) else { return };
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut file = std::io::BufWriter::new(file);
        while let Ok(line) = rx.recv() {
            let _ = file.write_all(line.as_bytes());
            while let Ok(line) = rx.try_recv() {
                let _ = file.write_all(line.as_bytes());
            }
            let _ = file.flush();
        }
    });
    let _ = fern::Dispatch::new()
        .format(|out, message, record| {
            let t: SYSTEMTIME = unsafe { GetLocalTime() };
            out.finish(format_args!(
                "[{:02}:{:02}:{:02}.{:03}] [{}] {message}",
                t.wHour, t.wMinute, t.wSecond, t.wMilliseconds,
                record.level()
            ))
        })
        .level(log::LevelFilter::Debug)
        .level_for("hudhook", log::LevelFilter::Warn)
        // The decoder narrates every byte it skips in a damaged file; the
        // jukebox's own "could not decode" line says all that's needed.
        .filter(|meta| !meta.target().starts_with("symphonia") || meta.level() <= log::Level::Error)
        .chain(tx)
        .apply();
}

/// Logs `text` only when it differs from the last line logged under `key`.
pub fn log_change(key: &str, text: String) {
    static PREVIOUS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    let Ok(mut previous) = PREVIOUS.lock() else { return };
    if previous.get(key) != Some(&text) {
        log::info!("{text}");
        previous.insert(key.to_owned(), text);
    }
}

fn main(this_module: HMODULE) {
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
    let dir = module_dir(this_module);
    init_logging(&dir.join("ffrs_jukebox.log"));
    log::info!("FF Resonance Jukebox {}", env!("CARGO_PKG_VERSION"));

    let config_path = dir.join("ffrs_jukebox.toml");
    let mut config = config::Config::load(&config_path);
    // On a fresh install or a game update, the songs with no audio start
    // out as the known ones; the song check then goes over every song on
    // this build and corrects the list.
    let build = game_build();
    let mut changed = false;
    if config.game_build != build {
        log::info!("main: new game build, starting from the {} songs known to have no audio", state::KNOWN_SILENT.len());
        config.unplayable = state::KNOWN_SILENT.iter().map(|cue| format!("game:{cue}")).collect();
        config.game_build = build.clone();
        changed = true;
    }
    // On the build this jukebox was made for, that list is the answer: it
    // is what the song check finds there. The check can only run while no
    // game song is playing, so for someone who plays game songs from the
    // moment the game starts it would never finish; it's left for a build
    // the jukebox doesn't know, and for "Check songs again".
    if build == format!("{SUPPORTED_BUILD:#010X}") && config.checked_build != build {
        config.checked_build = build;
        changed = true;
    }
    if changed {
        config.save(&config_path);
    }
    let music_dir = config.music_dir(&dir);
    // A big music folder on a slow drive can take a while to go through, and
    // the renderer hook must not wait for it (see below).
    let scan = {
        let music_dir = music_dir.clone();
        std::thread::spawn(move || state::scan_folder(&music_dir))
    };
    state::init(state::Shared { config, config_path, music_dir: music_dir.clone(), ..Default::default() });
    if !config::persisting() {
        state::lock().notice = Some(
            "Your settings file couldn't be read, so nothing is saved this session. Close the game and check ffrs_jukebox.toml."
                .into(),
        );
    } else if config::restored() {
        state::lock().notice = Some(
            "Your settings file was damaged, so your settings were restored from the copy made at the last good start."
                .into(),
        );
    }
    // Opening the audio output takes a moment; done here rather than on the
    // game thread when the first file plays.
    player::warm_up();

    // Hooking the renderer freezes every thread of the game for a moment
    // (and builds a throwaway graphics device). Once the game's sound is
    // running, that froze its audio mid-buffer: an ear-splitting buzz at
    // start-up. So it's done as soon as the game has a window, well before
    // the sound starts (about 5 s in).
    let waited = std::time::Instant::now();
    while !keys::game_window_exists() && waited.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let window_seen = keys::game_window_exists();
    let waited = waited.elapsed().as_secs_f32();
    let hooking = std::time::Instant::now();
    match hudhook::Hudhook::builder()
        .with::<hudhook::hooks::dx12::ImguiDx12Hooks>(menu::Menu::new())
        .build()
        .apply()
    {
        Ok(()) => log::info!(
            "main: overlay hooked (DX12) {waited:.1}s in, took {} ms",
            hooking.elapsed().as_millis()
        ),
        Err(e) => log::error!("main: overlay hook failed: {e:?}"),
    }

    // Your own files are in before the player first runs, so last session's
    // song can pick up where it left off.
    let tracks = scan.join().unwrap_or_default();
    log::info!("main: {} files in {}", tracks.len(), music_dir.display());
    state::lock().set_files(tracks);

    hook_process_event();

    // Once the game's window is gone the game is closing: your own music
    // stops there and then, not a second later when the process ends.
    // Only a window that was there counts, and it must be gone twice in a
    // row; one that comes back (a display mode change) gets the music back.
    let mut seen = window_seen;
    let mut gone = 0;
    let mut silenced = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if keys::game_window_exists() {
            seen = true;
            gone = 0;
            if silenced {
                silenced = false;
                player::reopen_output();
            }
        } else if seen {
            gone += 1;
            if gone >= 2 && !silenced {
                silenced = true;
                player::silence_for_exit();
                // The game ends soon after its window: whatever the settings
                // writer still holds (the song to resume, a late change)
                // goes to the disk now, before the thread is cut off.
                state::lock().save_config();
                config::flush(std::time::Duration::from_millis(1500));
            }
        }
    }
}

/// The game build this jukebox was made for: the link time stamped in the
/// exe's PE header, the same on every install of one build. The SDK's
/// addresses and layouts (ProcessEvent, GObjects, GNames, every class) hold
/// for this build only, so on any other the jukebox stays off rather than
/// read the wrong memory. A game update means a jukebox update.
const SUPPORTED_BUILD: u32 = 0x6A58_6B2F;

/// The game isn't the build this jukebox was made for: the player never
/// runs, and the overlay only says so (no playback, no controller hooks).
pub static UNSUPPORTED: AtomicBool = AtomicBool::new(false);

/// The game's executable image: where it starts, its size in memory, and
/// its link time stamp.
fn game_image() -> Option<(usize, usize, u32)> {
    let base = unsafe { GetModuleHandleA(None) }.ok()?.0 as usize;
    // The DOS header's e_lfanew, then the file header's TimeDateStamp and
    // the optional header's SizeOfImage.
    let nt = unsafe { std::ptr::read_unaligned((base + 0x3C) as *const u32) } as usize;
    let stamp = unsafe { std::ptr::read_unaligned((base + nt + 0x08) as *const u32) };
    let size = unsafe { std::ptr::read_unaligned((base + nt + 0x50) as *const u32) } as usize;
    Some((base, size, stamp))
}

/// Game-thread entry for the player: inside UObject::ProcessEvent, not at
/// its entry. The ultrawide fix finds ProcessEvent by its first 0x55 bytes,
/// so a hook there hides it. +0x8C is the `je` both paths reach (native via
/// the jmp at +0x87, script by falling through +0x89), and nothing jumps into
/// its middle. Only on the supported game build, and checked against the
/// bytes too: on any other, the game itself is left alone (the overlay, hooked
/// earlier, only shows that it needs an update), since hooking whatever lies
/// there now would crash the game.
fn hook_process_event() {
    const NEEDS_UPDATE: &str = "This version of the game isn't supported yet. Check for a jukebox update.";
    const OTHER_MOD: &str =
        "Another mod has changed the part of the game the jukebox hooks into, so the jukebox is off. Try it without your other mods.";
    let unsupported = |why: &str, shown: &str| {
        log::error!("main: {why}; the jukebox stays off");
        UNSUPPORTED.store(true, Ordering::SeqCst);
        state::lock().error = Some(shown.into());
    };
    let Some((base, size, stamp)) = game_image() else {
        return unsupported("could not find the game's image", NEEDS_UPDATE);
    };
    let known_build = stamp == SUPPORTED_BUILD;
    if !known_build {
        let why = format!("the game build is {stamp:#010X}, this jukebox was made for {SUPPORTED_BUILD:#010X}");
        // Only by choice, in the settings file: the game's code is checked
        // where the jukebox hooks it, but the rest of what it relies on
        // (offsets into the game's objects) can't be, and may have moved.
        if !state::lock().config.allow_untested_build {
            return unsupported(&why, NEEDS_UPDATE);
        }
        log::warn!("main: {why}; running anyway (allow_untested_build), which may crash the game");
        state::lock().notice = Some("Running on an untested game build (allow_untested_build): it may crash.".into());
    }
    if sdk::config::PROCESS_EVENT + 0x90 > size {
        return unsupported("ProcessEvent lies outside the game's image", NEEDS_UPDATE);
    }
    let entry = (base + sdk::config::PROCESS_EVENT) as *mut u8;
    let at = |offset: usize, bytes: &[u8]| unsafe { std::slice::from_raw_parts(entry.add(offset), bytes.len()) } == bytes;
    let entry_ok = at(0x00, &[0x40, 0x55, 0x56, 0x57]);
    let inside_ok = at(0x87, &[0xEB, 0x03]) && at(0x8C, &[0x0F, 0x84]);
    // On the build the jukebox was made for, the build stamp already says
    // where ProcessEvent is. Its first bytes differing there means another
    // mod hooked its entry first, which leaves the spot inside it as it was.
    // On an untested build the first bytes are the only proof there is.
    let process_event = if inside_ok && (entry_ok || known_build) {
        if !entry_ok {
            log::info!("main: another mod has hooked ProcessEvent's entry; hooking inside it as usual");
        }
        unsafe { entry.add(0x8C) }
    } else if entry_ok {
        log::warn!("main: ProcessEvent+0x8C not as expected, hooking its entry instead");
        entry
    } else if known_build {
        return unsupported("something else has changed ProcessEvent at its entry and inside", OTHER_MOD);
    } else {
        return unsupported("ProcessEvent isn't where this jukebox expects it", NEEDS_UPDATE);
    };
    match MidHook::install(process_event, |_ctx| game::on_process_event()) {
        Ok(hook) => {
            // The hook stays for the life of the game, whatever a future
            // grapnel does on drop.
            std::mem::forget(hook);
            log::info!("main: ProcessEvent hooked @ {process_event:p}");
        }
        // Without the hook nothing would ever play; the menu says so rather
        // than looking like it works.
        Err(e) => unsupported(&format!("could not hook ProcessEvent: {e}"), NEEDS_UPDATE),
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(hinst: HMODULE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // The ASI loader loads scripts on the main thread, which is the
        // engine's game thread.
        game::GAME_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
        let module = hinst.0 as usize;
        std::thread::spawn(move || main(HMODULE(module as *mut c_void)));
    }
    1
}
