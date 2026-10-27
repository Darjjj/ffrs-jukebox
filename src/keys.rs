//! Keyboard polling for the jukebox's hotkeys, and readable key names.

use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyNameTextW, MAPVK_VK_TO_VSC, MapVirtualKeyW,
};
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD_DPAD_DOWN, XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP,
    XINPUT_STATE, XInputGetState,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

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

/// What a controller combo asks for: hold Left Trigger and press the D-pad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PadAction {
    Previous,
    Next,
    PlayPause,
    Stop,
}

/// How far Left Trigger must be held (of 255) for the D-pad to count.
const TRIGGER_HELD: u8 = 100;
/// An empty controller slot is slow to ask about, so one is only asked
/// again this often.
const EMPTY_SLOT_RETRY: Duration = Duration::from_secs(2);

/// Reads every connected controller for the jukebox's combos.
pub struct PadPoll {
    /// D-pad buttons held last frame, so a press counts once.
    held: u16,
    /// When each empty slot may next be asked about.
    retry: [Option<Instant>; 4],
}

impl PadPoll {
    pub fn new() -> Self {
        Self { held: 0, retry: [None; 4] }
    }

    /// The combos pressed since the last call.
    pub fn pressed(&mut self) -> Vec<PadAction> {
        let (mut buttons, mut trigger) = (0u16, 0u8);
        for slot in 0..4u32 {
            let i = slot as usize;
            if self.retry[i].is_some_and(|t| Instant::now() < t) {
                continue;
            }
            let mut s = XINPUT_STATE::default();
            if unsafe { XInputGetState(slot, &mut s) } == 0 {
                self.retry[i] = None;
                buttons |= s.Gamepad.wButtons.0;
                trigger = trigger.max(s.Gamepad.bLeftTrigger);
            } else {
                self.retry[i] = Some(Instant::now() + EMPTY_SLOT_RETRY);
            }
        }
        let dpad = if trigger >= TRIGGER_HELD {
            buttons & (XINPUT_GAMEPAD_DPAD_LEFT.0 | XINPUT_GAMEPAD_DPAD_RIGHT.0 | XINPUT_GAMEPAD_DPAD_UP.0 | XINPUT_GAMEPAD_DPAD_DOWN.0)
        } else {
            0
        };
        let fresh = dpad & !self.held;
        self.held = dpad;
        [
            (XINPUT_GAMEPAD_DPAD_LEFT.0, PadAction::Previous),
            (XINPUT_GAMEPAD_DPAD_RIGHT.0, PadAction::Next),
            (XINPUT_GAMEPAD_DPAD_UP.0, PadAction::PlayPause),
            (XINPUT_GAMEPAD_DPAD_DOWN.0, PadAction::Stop),
        ]
        .into_iter()
        .filter(|(bit, _)| fresh & bit != 0)
        .map(|(_, action)| action)
        .collect()
    }
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
