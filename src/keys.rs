//! Keyboard and controller polling for the jukebox's hotkeys and combos,
//! and readable key names.

use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyNameTextW, MAPVK_VK_TO_VSC, MapVirtualKeyW,
};
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleA, GetModuleHandleExW,
};
use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
use windows::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_BACK, XINPUT_GAMEPAD_DPAD_DOWN, XINPUT_GAMEPAD_DPAD_LEFT,
    XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_LEFT_THUMB,
    XINPUT_GAMEPAD_RIGHT_SHOULDER, XINPUT_GAMEPAD_RIGHT_THUMB, XINPUT_GAMEPAD_START, XINPUT_GAMEPAD_X,
    XINPUT_GAMEPAD_Y, XINPUT_STATE, XInputGetState,
};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible};

pub const ESCAPE: i32 = 0x1B;
pub const DELETE: i32 = 0x2E;
pub const BACKSPACE: i32 = 0x08;

/// Whether the game is the active window. Key state is read globally, so
/// without this a hotkey pressed in another program would reach the jukebox.
pub fn game_focused() -> bool {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
    pid == unsafe { GetCurrentProcessId() }
}

/// Whether the game has opened its window yet.
pub fn game_window_exists() -> bool {
    unsafe extern "system" fn found(hwnd: HWND, out: LPARAM) -> BOOL {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            unsafe { *(out.0 as *mut bool) = true };
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut exists = false;
    let _ = unsafe { EnumWindows(Some(found), LPARAM(&mut exists as *mut bool as isize)) };
    exists
}

/// Keys that may be bound: everything but the mouse buttons and the
/// left/right-agnostic Shift, Ctrl and Alt codes.
fn bindable(vk: i32) -> bool {
    (0x08..=0xFE).contains(&vk) && !(0x10..=0x12).contains(&vk)
}

/// Whether `vk` is down right now; never for an unbound key.
pub fn held(vk: i32) -> bool {
    vk != 0 && unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0
}

/// Remembers which keys were down last frame, to report fresh presses.
pub struct KeyPoll {
    down: [bool; 256],
}

impl KeyPoll {
    pub fn new() -> Self {
        Self { down: [false; 256] }
    }

    /// The keys pressed since the last call, in key-code order.
    pub fn pressed(&mut self) -> Vec<i32> {
        let mut fresh = Vec::new();
        for vk in 0..256i32 {
            let now = unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0;
            if now && !self.down[vk as usize] && bindable(vk) {
                fresh.push(vk);
            }
            self.down[vk as usize] = now;
        }
        fresh
    }
}

/// A controller button, as the combos and the settings name it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PadButton {
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    A,
    B,
    X,
    Y,
    LB,
    RB,
    LT,
    RT,
    View,
    Start,
    LeftStick,
    RightStick,
}

impl PadButton {
    pub const ALL: [PadButton; 16] = [
        PadButton::DpadUp,
        PadButton::DpadDown,
        PadButton::DpadLeft,
        PadButton::DpadRight,
        PadButton::A,
        PadButton::B,
        PadButton::X,
        PadButton::Y,
        PadButton::LB,
        PadButton::RB,
        PadButton::LT,
        PadButton::RT,
        PadButton::View,
        PadButton::Start,
        PadButton::LeftStick,
        PadButton::RightStick,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PadButton::DpadUp => "D-pad Up",
            PadButton::DpadDown => "D-pad Down",
            PadButton::DpadLeft => "D-pad Left",
            PadButton::DpadRight => "D-pad Right",
            PadButton::A => "A",
            PadButton::B => "B",
            PadButton::X => "X",
            PadButton::Y => "Y",
            PadButton::LB => "LB",
            PadButton::RB => "RB",
            PadButton::LT => "LT",
            PadButton::RT => "RT",
            PadButton::View => "View / Back",
            PadButton::Start => "Menu / Start",
            PadButton::LeftStick => "Left stick click",
            PadButton::RightStick => "Right stick click",
        }
    }

    /// The button's bit in `wButtons`; none for the triggers.
    fn bit(self) -> u16 {
        match self {
            PadButton::DpadUp => XINPUT_GAMEPAD_DPAD_UP.0,
            PadButton::DpadDown => XINPUT_GAMEPAD_DPAD_DOWN.0,
            PadButton::DpadLeft => XINPUT_GAMEPAD_DPAD_LEFT.0,
            PadButton::DpadRight => XINPUT_GAMEPAD_DPAD_RIGHT.0,
            PadButton::A => XINPUT_GAMEPAD_A.0,
            PadButton::B => XINPUT_GAMEPAD_B.0,
            PadButton::X => XINPUT_GAMEPAD_X.0,
            PadButton::Y => XINPUT_GAMEPAD_Y.0,
            PadButton::LB => XINPUT_GAMEPAD_LEFT_SHOULDER.0,
            PadButton::RB => XINPUT_GAMEPAD_RIGHT_SHOULDER.0,
            PadButton::View => XINPUT_GAMEPAD_BACK.0,
            PadButton::Start => XINPUT_GAMEPAD_START.0,
            PadButton::LeftStick => XINPUT_GAMEPAD_LEFT_THUMB.0,
            PadButton::RightStick => XINPUT_GAMEPAD_RIGHT_THUMB.0,
            PadButton::LT | PadButton::RT => 0,
        }
    }
}

/// How far a trigger must be held (of 255) to count as pressed.
const TRIGGER_HELD: u8 = 100;
/// How far a stick must lean (of 32767) to count as a D-pad direction in
/// the menu.
const STICK_LEAN: i16 = 16000;
/// How much of the right stick's travel is ignored, so a resting stick
/// doesn't drift the jukebox.
const STICK_DEAD: f32 = 0.25;
/// An empty controller slot is slow to ask about, so one is only asked
/// again this often.
const EMPTY_SLOT_RETRY: Duration = Duration::from_secs(2);

/// Every connected controller's buttons together, as one.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct PadState {
    buttons: u16,
    lt: u8,
    rt: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}

impl PadState {
    pub fn held(&self, b: PadButton) -> bool {
        match b {
            PadButton::LT => self.lt >= TRIGGER_HELD,
            PadButton::RT => self.rt >= TRIGGER_HELD,
            _ => self.buttons & b.bit() != 0,
        }
    }

    /// The right stick as -1.0 to 1.0 on each axis (up is negative, like
    /// the screen), with a dead zone in the middle and a gentle start, so
    /// small moves are fine and a full lean is fast.
    pub fn right_stick(&self) -> [f32; 2] {
        let axis = |v: i16| {
            let v = v as f32 / 32767.0;
            let past = (v.abs() - STICK_DEAD).max(0.0) / (1.0 - STICK_DEAD);
            past * past * v.signum()
        };
        [axis(self.rx), -axis(self.ry)]
    }

    /// The D-pad, or the left stick leaning that way, for moving around the
    /// menu: up, down, left, right.
    pub fn nav(&self) -> [bool; 4] {
        [
            self.held(PadButton::DpadUp) || self.ly > STICK_LEAN,
            self.held(PadButton::DpadDown) || self.ly < -STICK_LEAN,
            self.held(PadButton::DpadLeft) || self.lx < -STICK_LEAN,
            self.held(PadButton::DpadRight) || self.lx > STICK_LEAN,
        ]
    }
}

// What the game is shown of the controller, so the jukebox can keep buttons
// to itself: the combo buttons while the hold button is down, and everything
// while the menu is being driven with the controller.
//
// Only the game's own call is rerouted: its import-table entry for
// XInputGetState (delay-loaded from XINPUT1_4.dll) is pointed at a filter
// that calls on to whatever the entry held. XInputGetState itself is never
// patched. Steam Input makes a DualSense or any other pad look like an Xbox
// one by hooking that function in the game's process; patching it as well
// (as 1.6.0 did) left Steam's layer out, and those pads stopped working.

/// Buttons (`wButtons` bits) hidden from the game right now.
static HIDE_BUTTONS: AtomicU16 = AtomicU16::new(0);
/// The whole controller is hidden from the game.
static HIDE_ALL: AtomicBool = AtomicBool::new(false);
/// The right stick is hidden from the game (it's moving the jukebox).
static HIDE_RIGHT_STICK: AtomicBool = AtomicBool::new(false);
/// What the game's import entry held before the filter took its place:
/// XInputGetState, or Steam's hook of it.
static NEXT: AtomicUsize = AtomicUsize::new(0);
/// How many times the filter has (re)claimed the entry. Something that keeps
/// taking it back wins after a few tries; the filter then stays out of the way.
static CLAIMS: AtomicU32 = AtomicU32::new(0);
const MAX_CLAIMS: u32 = 4;

type GetState = unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32;

thread_local! {
    /// The filter is running on this thread, so a call that comes back round
    /// to it goes straight to XInput instead of looping.
    static IN_FILTER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

unsafe extern "system" fn get_state_filter(slot: u32, state: *mut XINPUT_STATE) -> u32 {
    let next = NEXT.load(Ordering::Relaxed);
    if next == 0 || IN_FILTER.get() {
        return unsafe { XInputGetState(slot, state) };
    }
    IN_FILTER.set(true);
    let result = unsafe { std::mem::transmute::<usize, GetState>(next)(slot, state) };
    IN_FILTER.set(false);
    if result == 0 {
        if let Some(s) = unsafe { state.as_mut() } {
            if HIDE_ALL.load(Ordering::Relaxed) {
                s.Gamepad = Default::default();
            } else {
                s.Gamepad.wButtons.0 &= !HIDE_BUTTONS.load(Ordering::Relaxed);
                if HIDE_RIGHT_STICK.load(Ordering::Relaxed) {
                    s.Gamepad.sThumbRX = 0;
                    s.Gamepad.sThumbRY = 0;
                }
            }
        }
    }
    result
}

/// The game's delay-load import entry for XINPUT1_4.dll's XInputGetState,
/// and the game exe's address range.
struct Entry {
    slot: *mut usize,
    exe: std::ops::Range<usize>,
}

unsafe impl Send for Entry {}
unsafe impl Sync for Entry {}

fn find_entry() -> Option<Entry> {
    let base = unsafe { GetModuleHandleA(None) }.ok()?.0 as usize;
    let u32_at = |off: usize| unsafe { std::ptr::read_unaligned((base + off) as *const u32) } as usize;
    let nt = u32_at(0x3C);
    let size = u32_at(nt + 0x18 + 0x38);
    // The delay-load directory is entry 13 of the data directories.
    let delay = u32_at(nt + 0x18 + 0x70 + 13 * 8);
    if delay == 0 {
        return None;
    }
    let c_str = |rva: usize| unsafe { std::ffi::CStr::from_ptr((base + rva) as *const std::ffi::c_char) };
    let mut d = base + delay;
    loop {
        // Attributes, DllName, ModuleHandle, IAT, INT (RVAs), 32 bytes each.
        let field = |i: usize| unsafe { std::ptr::read_unaligned((d + i * 4) as *const u32) } as usize;
        let name = field(1);
        if name == 0 {
            return None;
        }
        if c_str(name).to_bytes().eq_ignore_ascii_case(b"XINPUT1_4.dll") {
            let (iat, names) = (field(3), field(4));
            for i in 0.. {
                let thunk = unsafe { std::ptr::read_unaligned((base + names + i * 8) as *const u64) };
                if thunk == 0 {
                    return None;
                }
                // By ordinal (XInputGetState is ordinal 2) or by name.
                let hit = if thunk >> 63 == 1 {
                    thunk & 0xFFFF == 2
                } else {
                    c_str(thunk as usize + 2).to_bytes() == b"XInputGetState"
                };
                if hit {
                    return Some(Entry { slot: (base + iat + i * 8) as *mut usize, exe: base..base + size });
                }
            }
        }
        d += 32;
    }
}

/// Keeps the game's XInputGetState entry pointing at the filter. Called every
/// frame: the entry only holds the real function after the game's first call
/// (until then it points at the delay-load stub inside the exe), and Steam may
/// put its own hook there later, which the filter then calls on to.
pub fn keep_xinput_filtered() {
    static ENTRY: std::sync::OnceLock<Option<Entry>> = std::sync::OnceLock::new();
    let Some(entry) = ENTRY.get_or_init(|| {
        let e = find_entry();
        if e.is_none() {
            log::warn!("keys: no XInputGetState import in the game, the game will see the combo buttons too");
        }
        e
    }) else {
        return;
    };
    let filter = get_state_filter as *const () as usize;
    let now = unsafe { std::ptr::read_volatile(entry.slot) };
    // Not resolved yet, already ours, or given up on.
    if now == filter || entry.exe.contains(&now) || CLAIMS.load(Ordering::Relaxed) >= MAX_CLAIMS {
        return;
    }
    let claims = CLAIMS.fetch_add(1, Ordering::Relaxed) + 1;
    if claims >= MAX_CLAIMS {
        log::warn!("keys: something keeps taking the game's XInputGetState entry back; leaving it be");
        return;
    }
    NEXT.store(now, Ordering::Relaxed);
    let mut old = PAGE_PROTECTION_FLAGS(0);
    let slot = entry.slot as *const std::ffi::c_void;
    if unsafe { VirtualProtect(slot, 8, PAGE_READWRITE, &mut old) }.is_err() {
        log::warn!("keys: could not reroute the game's XInputGetState");
        return;
    }
    unsafe { std::ptr::write_volatile(entry.slot, filter) };
    let _ = unsafe { VirtualProtect(slot, 8, old, &mut old) };
    log::info!("keys: game's XInputGetState rerouted through the jukebox, calling on to {}", module_of(now));
}

/// Which module an address is in, for the log: "XINPUT1_4.dll" when the game
/// calls XInput directly, Steam's overlay when Steam Input sits in between.
fn module_of(addr: usize) -> String {
    let mut module = HMODULE::default();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, windows::core::PCWSTR(addr as *const u16), &mut module) }.is_err() {
        return format!("{addr:#x}");
    }
    let mut buf = [0u16; 260];
    let len = unsafe { GetModuleFileNameW(module, &mut buf) } as usize;
    let path = String::from_utf16_lossy(&buf[..len]);
    let name = path.rsplit('\\').next().unwrap_or(&path);
    format!("{name}+{:#x}", addr - module.0 as usize)
}

/// Reads one controller slot as the jukebox sees it: through XInput as is,
/// Steam Input included, never through the filter.
fn read_slot(slot: u32, s: &mut XINPUT_STATE) -> u32 {
    unsafe { XInputGetState(slot, s) }
}

/// What the controller asked of the jukebox this frame.
pub enum PadEvent {
    Previous,
    Next,
    PlayPause,
    Stop,
    /// Open the menu for the controller, or close it.
    Menu,
}

/// The combo buttons, from the settings.
pub struct PadBinds {
    pub hold: PadButton,
    pub previous: PadButton,
    pub next: PadButton,
    pub play_pause: PadButton,
    pub stop: PadButton,
    pub menu: PadButton,
}

/// Reads every connected controller for the jukebox's combos.
pub struct PadPoll {
    /// Everything as of the last call, so a press counts once.
    last: PadState,
    /// When each empty slot may next be asked about.
    retry: [Option<Instant>; 4],
}

impl PadPoll {
    pub fn new() -> Self {
        Self { last: PadState::default(), retry: [None; 4] }
    }

    /// Reads the controllers: their state now, and the buttons newly
    /// pressed since the last call.
    pub fn read(&mut self) -> (PadState, Vec<PadButton>) {
        let mut now = PadState::default();
        for slot in 0..4u32 {
            let i = slot as usize;
            if self.retry[i].is_some_and(|t| Instant::now() < t) {
                continue;
            }
            let mut s = XINPUT_STATE::default();
            if read_slot(slot, &mut s) == 0 {
                self.retry[i] = None;
                let g = s.Gamepad;
                now.buttons |= g.wButtons.0;
                now.lt = now.lt.max(g.bLeftTrigger);
                now.rt = now.rt.max(g.bRightTrigger);
                if g.sThumbLX.unsigned_abs() > now.lx.unsigned_abs() {
                    now.lx = g.sThumbLX;
                }
                if g.sThumbRX.unsigned_abs() > now.rx.unsigned_abs() {
                    now.rx = g.sThumbRX;
                }
                if g.sThumbRY.unsigned_abs() > now.ry.unsigned_abs() {
                    now.ry = g.sThumbRY;
                }
                if g.sThumbLY.unsigned_abs() > now.ly.unsigned_abs() {
                    now.ly = g.sThumbLY;
                }
            } else {
                self.retry[i] = Some(Instant::now() + EMPTY_SLOT_RETRY);
            }
        }
        let fresh = PadButton::ALL.into_iter().filter(|&b| now.held(b) && !self.last.held(b)).collect();
        self.last = now;
        (now, fresh)
    }
}

/// The combos among `fresh` presses: the hold button down, then an action
/// button. While the hold button is down, the action buttons are kept from
/// the game; `navigating` hides the whole controller from it.
pub fn combos(now: &PadState, fresh: &[PadButton], b: &PadBinds, on: bool, navigating: bool) -> Vec<PadEvent> {
    HIDE_ALL.store(navigating, Ordering::Relaxed);
    let actions = [
        (b.previous, PadEvent::Previous),
        (b.next, PadEvent::Next),
        (b.play_pause, PadEvent::PlayPause),
        (b.stop, PadEvent::Stop),
        (b.menu, PadEvent::Menu),
    ];
    let holding = on && now.held(b.hold);
    let hide = if holding { actions.iter().fold(0, |m, (btn, _)| m | btn.bit()) } else { 0 };
    HIDE_BUTTONS.store(hide, Ordering::Relaxed);
    // Hold + right stick moves the jukebox, not the camera.
    HIDE_RIGHT_STICK.store(holding, Ordering::Relaxed);
    if !holding {
        return Vec::new();
    }
    actions.into_iter().filter(|(btn, _)| *btn != b.hold && fresh.contains(btn)).map(|(_, e)| e).collect()
}

/// "F9", "7", "Num 4", "Page Up"... "-" for an unbound key.
pub fn name(vk: i32) -> String {
    match vk {
        0 => "-".into(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        _ => {
            let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) };
            // Keys whose scan code is "extended" share it with the numpad,
            // so the flag picks the right name (Insert, not Num 0).
            let extended = matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0xA3 | 0xA5 | 0x6F);
            let lparam = ((scan as i32) << 16) | if extended { 1 << 24 } else { 0 };
            let mut buf = [0u16; 64];
            let len = unsafe { GetKeyNameTextW(lparam, &mut buf) };
            if len > 0 {
                String::from_utf16_lossy(&buf[..len as usize])
            } else {
                format!("Key 0x{vk:X}")
            }
        }
    }
}
