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
use std::sync::atomic::Ordering;

use grapnel::MidHook;
use windows::Win32::Foundation::{HMODULE, SYSTEMTIME};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleA};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::System::Threading::GetCurrentThreadId;

fn module_dir(module: HMODULE) -> PathBuf {
    let mut buf = [0u16; 260];
    let len = unsafe { GetModuleFileNameW(module, &mut buf) } as usize;
    PathBuf::from(String::from_utf16_lossy(&buf[..len]))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default()
}

/// The game exe's size and modified time, which change with every update.
fn game_build() -> String {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .map(|m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            format!("{}-{modified}", m.len())
        })
        .unwrap_or_default()
}

fn init_logging(path: &std::path::Path) {
    let Ok(file) = File::create(path) else { return };
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
        .chain(file)
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
    if config.game_build != build {
        log::info!("main: new game build, starting from the {} songs known to have no audio", state::KNOWN_SILENT.len());
        config.unplayable = state::KNOWN_SILENT.iter().map(|cue| format!("game:{cue}")).collect();
        config.game_build = build;
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
            }
        }
    }
}

/// The start of the game's executable image and its size in memory.
fn game_image() -> Option<(usize, usize)> {
    let base = unsafe { GetModuleHandleA(None) }.ok()?.0 as usize;
    // The DOS header's e_lfanew, then the optional header's SizeOfImage.
    let nt = unsafe { std::ptr::read_unaligned((base + 0x3C) as *const u32) } as usize;
    let size = unsafe { std::ptr::read_unaligned((base + nt + 0x50) as *const u32) } as usize;
    Some((base, size))
}

/// Game-thread entry for the player: inside UObject::ProcessEvent, not at
/// its entry. The ultrawide fix finds ProcessEvent by its first 0x55 bytes,
/// so a hook there hides it. +0x8C is the `je` both paths reach (native via
/// the jmp at +0x87, script by falling through +0x89), and nothing jumps into
/// its middle. Checked against the bytes first. On a game update that moved
/// ProcessEvent, nothing is hooked at all: hooking whatever lies there now
/// would crash the game.
fn hook_process_event() {
    let unsupported = |why: &str| {
        log::error!("main: {why}; the jukebox stays off on this game version");
        state::lock().error =
            Some("This version of the game isn't supported yet. Check for a jukebox update.".into());
    };
    let Some((base, size)) = game_image() else {
        return unsupported("could not find the game's image");
    };
    if sdk::config::PROCESS_EVENT + 0x90 > size {
        return unsupported("ProcessEvent lies outside the game's image");
    }
    let entry = (base + sdk::config::PROCESS_EVENT) as *mut u8;
    let at = |offset: usize, bytes: &[u8]| unsafe { std::slice::from_raw_parts(entry.add(offset), bytes.len()) } == bytes;
    if !at(0x00, &[0x40, 0x55, 0x56, 0x57]) {
        return unsupported("ProcessEvent isn't where this jukebox expects it");
    }
    let process_event = if at(0x87, &[0xEB, 0x03]) && at(0x8C, &[0x0F, 0x84]) {
        unsafe { entry.add(0x8C) }
    } else {
        log::warn!("main: ProcessEvent+0x8C not as expected, hooking its entry instead");
        entry
    };
    match MidHook::install(process_event, |_ctx| game::on_process_event()) {
        Ok(hook) => {
            // The hook stays for the life of the game, whatever a future
            // grapnel does on drop.
            std::mem::forget(hook);
            log::info!("main: ProcessEvent hooked @ {process_event:p}");
        }
        Err(e) => log::error!("main: could not hook ProcessEvent: {e}"),
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
