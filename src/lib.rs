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
    // Songs found to have no audio get another try once the game updates.
    let build = game_build();
    if config.game_build != build {
        if !config.unplayable.is_empty() {
            log::info!("main: game updated, retrying {} songs that had no audio", config.unplayable.len());
            config.unplayable.clear();
        }
        config.game_build = build;
        config.save(&config_path);
    }
    let music_dir = config.music_dir(&dir);
    let tracks = state::scan_folder(&music_dir);
    log::info!("main: {} files in {}", tracks.len(), music_dir.display());
    state::init(state::Shared { config, config_path, music_dir, tracks, ..Default::default() });
    state::lock().apply_titles();

    // Game-thread entry for the player: UObject::ProcessEvent.
    let base = unsafe { GetModuleHandleA(None) }.map(|h| h.0 as usize).unwrap_or(0);
    let process_event = (base + sdk::config::PROCESS_EVENT) as *mut u8;
    match MidHook::install(process_event, |_ctx| game::on_process_event()) {
        Ok(_) => log::info!("main: ProcessEvent hooked @ {process_event:p}"),
        Err(e) => {
            log::error!("main: could not hook ProcessEvent: {e}");
            return;
        }
    }

    // Let the game bring up its renderer before hooking it.
    std::thread::sleep(std::time::Duration::from_secs(5));
    match hudhook::Hudhook::builder()
        .with::<hudhook::hooks::dx12::ImguiDx12Hooks>(menu::Menu::new())
        .build()
        .apply()
    {
        Ok(()) => log::info!("main: overlay hooked (DX12)"),
        Err(e) => log::error!("main: overlay hook failed: {e:?}"),
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
