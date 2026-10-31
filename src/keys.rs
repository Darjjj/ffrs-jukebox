//! Keyboard and controller polling for the jukebox's hotkeys and combos,
//! and readable key names.

use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyNameTextW, MAPVK_VK_TO_VSC, MapVirtualKeyW,
};
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
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

// What the game is shown of the controller. The game reads it through
// XInputGetState in XINPUT1_4.dll, delay-loaded; its import slot for that
// function is pointed here so the jukebox can keep buttons to itself: the
// combo buttons while the hold button is down, and everything while the menu
// is being driven with the controller. The function itself is left unpatched:
// patching it stopped the game getting any controller input.

/// Buttons (`wButtons` bits) hidden from the game right now.
static HIDE_BUTTONS: AtomicU16 = AtomicU16::new(0);
/// The whole controller is hidden from the game.
static HIDE_ALL: AtomicBool = AtomicBool::new(false);
/// The right stick is hidden from the game (it's moving the jukebox).
static HIDE_RIGHT_STICK: AtomicBool = AtomicBool::new(false);
/// XInputGetState as it was before the hook, for the jukebox's own reads.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The game's import slot for XInputGetState, once found.
static SLOT: AtomicUsize = AtomicUsize::new(0);
/// XInputGetState in XINPUT1_4.dll.
static REAL: AtomicUsize = AtomicUsize::new(0);
/// Where the game's exe sits in memory: start and end.
static EXE: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];

type GetState = unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32;

unsafe extern "system" fn get_state_hook(slot: u32, state: *mut XINPUT_STATE) -> u32 {
    let original: GetState = unsafe { std::mem::transmute(ORIGINAL.load(Ordering::Relaxed)) };
    let result = unsafe { original(slot, state) };
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

/// The game's delay-import slot for `function` in `dll`, read from its PE
/// headers, and the size of its image.
unsafe fn delay_import_slot(base: usize, dll: &str, function: &str) -> Option<(usize, usize)> {
    let u32_at = |at: usize| unsafe { std::ptr::read_unaligned((base + at) as *const u32) } as usize;
    let c_str = |at: usize| unsafe { std::ffi::CStr::from_ptr((base + at) as *const _) }.to_str().unwrap_or("");
    let optional = u32_at(0x3C) + 24;
    let image_size = u32_at(optional + 56);
    let mut descriptor = u32_at(optional + 112 + 13 * 8);
    if descriptor == 0 {
        return None;
    }
    while u32_at(descriptor + 4) != 0 {
        if c_str(u32_at(descriptor + 4)).eq_ignore_ascii_case(dll) {
            let (slots, names) = (u32_at(descriptor + 12), u32_at(descriptor + 16));
            for i in 0.. {
                let name = unsafe { std::ptr::read_unaligned((base + names + i * 8) as *const u64) };
                if name == 0 {
                    return None;
                }
                if name >> 63 == 0 && c_str(name as usize + 2) == function {
                    return Some((base + slots + i * 8, image_size));
                }
            }
        }
        descriptor += 32;
    }
    None
}

/// Points the game's import slot at the hook, unless it's there already. Until
/// the game's first call the slot holds its delay-load stub, which would write
/// the real function back over the hook, so that's never called. A real
/// function or another mod's hook found there is kept and called from the hook.
fn keep_hooked() {
    let slot = SLOT.load(Ordering::Relaxed) as *mut usize;
    if slot.is_null() {
        return;
    }
    let hook = get_state_hook as *const () as usize;
    let found = unsafe { std::ptr::read_volatile(slot) };
    if found == hook {
        return;
    }
    let in_exe = (EXE[0].load(Ordering::Relaxed)..EXE[1].load(Ordering::Relaxed)).contains(&found);
    let next = if in_exe { REAL.load(Ordering::Relaxed) } else { found };
    ORIGINAL.store(next, Ordering::Relaxed);
    let mut old = PAGE_PROTECTION_FLAGS(0);
    let size = size_of::<usize>();
    if unsafe { VirtualProtect(slot as *const _, size, PAGE_READWRITE, &mut old) }.is_err() {
        log::warn!("keys: couldn't write the game's XInputGetState import, the game will see the combo buttons too");
        SLOT.store(0, Ordering::Relaxed);
        return;
    }
    unsafe {
        std::ptr::write_volatile(slot, hook);
        let _ = VirtualProtect(slot as *const _, size, old, &mut old);
    }
}

/// Hooks the game's calls to XInputGetState. Without it the combos still work,
/// the game just sees their buttons too.
pub fn hook_xinput() {
    let Ok(module) = (unsafe { GetModuleHandleA(windows::core::s!("XINPUT1_4.dll")) }) else {
        log::warn!("keys: XINPUT1_4.dll isn't loaded, the game will see the combo buttons too");
        return;
    };
    let Some(target) = (unsafe { GetProcAddress(module, windows::core::s!("XInputGetState")) }) else {
        log::warn!("keys: no XInputGetState, the game will see the combo buttons too");
        return;
    };
    let Ok(exe) = (unsafe { GetModuleHandleA(None) }) else {
        log::warn!("keys: no game module, the game will see the combo buttons too");
        return;
    };
    let base = exe.0 as usize;
    let Some((slot, size)) = (unsafe { delay_import_slot(base, "XINPUT1_4.dll", "XInputGetState") }) else {
        log::warn!("keys: the game has no delay import of XInputGetState, it will see the combo buttons too");
        return;
    };
    REAL.store(target as usize, Ordering::Relaxed);
    EXE[0].store(base, Ordering::Relaxed);
    EXE[1].store(base + size, Ordering::Relaxed);
    SLOT.store(slot, Ordering::Relaxed);
    keep_hooked();
    log::info!("keys: game's XInputGetState import hooked");
}

/// Reads one controller slot past the hook, as it really is.
fn read_slot(slot: u32, s: &mut XINPUT_STATE) -> u32 {
    match ORIGINAL.load(Ordering::Relaxed) {
        0 => unsafe { XInputGetState(slot, s) },
        f => unsafe { std::mem::transmute::<usize, GetState>(f)(slot, s) },
    }
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
    keep_hooked();
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
