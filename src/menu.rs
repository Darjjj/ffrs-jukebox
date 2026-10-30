//! The in-game menu, drawn by hudhook over the game's DX12 swapchain.

use std::time::Instant;

use hudhook::imgui::{
    BackendFlags, Condition, ConfigFlags, Context, FontId, Io, ItemHoveredFlags, MouseButton, ProgressBar, DragDropFlags, Key, SelectableFlags, StyleColor, StyleVar, TabBarFlags, Ui, WindowHoveredFlags,
    WindowFlags,
};
use hudhook::{ImguiRenderLoop, MessageFilter, RenderContext};

use crate::config::{Area, AreaMusic, Bind, PadBind, Playlist, Repeat};
use crate::keys::{self, KeyPoll, PadButton, PadEvent, PadState};
use crate::state::{self, Command, List};
use crate::theme::{self, ACCENT, DIM, Icon};

const TOAST_SECS: f32 = 4.0;
/// How long after a battle starts or ends the now-playing toast waits.
const BATTLE_TOAST_DELAY: f32 = 0.4;
/// Track changes this soon after a battle starts or ends come from the
/// battle playlist swap.
const BATTLE_SWAP_SECS: f32 = 2.0;
/// How long a volume key is held before it repeats, and how fast then.
const VOLUME_REPEAT_DELAY: std::time::Duration = std::time::Duration::from_millis(400);
const VOLUME_REPEAT: std::time::Duration = std::time::Duration::from_millis(70);
/// How long the volume popup stays after the last step.
const VOLUME_POPUP_SECS: f32 = 1.5;

/// What a click in the menu asked for, applied after the window is drawn.
enum Action {
    Play(usize),
    Toggle(List, usize),
    /// A new playlist from a song's + menu, holding that song.
    Create(Option<usize>),
    CreateNamed(String),
    Rename(usize, String),
    Delete(usize),
    /// Play from this list: its first song, or shuffled.
    PlayFrom(Playlist, bool),
    /// Drag and drop: song `.1` moves to where song `.2` is in the list.
    MoveTo(List, usize, usize),
    UnhideAll,
    /// A song's new name, or `None` for its default one back.
    RenameTrack(usize, Option<String>),
}

/// What the jukebox shows. F9 steps through them in order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Hidden,
    Full,
    /// Just the song, its progress and the transport buttons.
    Mini,
}

impl View {
    fn next(self) -> Self {
        match self {
            View::Hidden => View::Full,
            View::Full => View::Mini,
            View::Mini => View::Hidden,
        }
    }
}

pub struct Menu {
    view: View,
    keys: KeyPoll,
    pad: keys::PadPoll,
    settings_open: bool,
    /// The place chosen under "Music for": battles, field, towns, dungeons.
    place_tab: usize,
    /// The keybind waiting for its new key in the settings.
    capturing: Option<Bind>,
    /// The controller combo button waiting for its new button.
    pad_capturing: Option<PadBind>,
    /// The menu is being driven with the controller (opened by its combo).
    pad_nav: bool,
    /// Give the menu the focus on its next frame, so the controller starts
    /// out in it.
    pad_focus: bool,
    /// The controller as last read, fed to the menu while navigating, and
    /// the menu's keys as last fed.
    pad_now: PadState,
    pad_fed: [bool; 10],
    /// B was pressed while navigating: close whatever is on top.
    pad_back: bool,
    /// The combo Hold button is down.
    pad_hold_down: bool,
    /// The B press that closed the menu (or the settings) is kept from it
    /// until let go: the menu's own "cancel" would forget the spot the
    /// controller was on, which it otherwise remembers for next time.
    pad_swallow_b: bool,
    /// A Back button was shown this frame (an open playlist's), which B
    /// presses.
    back_shown: bool,
    /// Hold + right stick: how far it leans, moving the menu or the mini
    /// player; and where each window was last frame.
    pad_move: [f32; 2],
    mini_at: Option<[f32; 2]>,
    full_at: Option<[f32; 2]>,
    search: String,
    /// The playlist or folder open in the Playlists tab; `None` shows the
    /// cards.
    open_list: Option<Playlist>,
    /// The new-playlist card's name box, while open.
    new_list: Option<String>,
    /// A playlist being renamed in its card: index and the name so far.
    renaming_list: Option<(usize, String)>,
    /// The playlist whose Delete is waiting to be confirmed.
    confirm_delete: Option<usize>,
    /// A name box just opened and should take the keyboard.
    focus_name: bool,
    toast_serial: u64,
    toast_since: Option<Instant>,
    /// Whether the last frame was in a battle, and when that last changed,
    /// for the battle start/end toast.
    was_battle: bool,
    battle_changed: Option<Instant>,
    battle_toast_due: bool,
    heading: Option<FontId>,
    song_font: Option<FontId>,
    small_font: Option<FontId>,
    scale: f32,
    /// The song being renamed: track, the name typed so far, and whether
    /// the name box still needs the keyboard.
    renaming: Option<(usize, String, bool)>,
    /// The song row the mouse was on last frame.
    hovered: Option<usize>,
    /// The mini player is up but tucked away for a cutscene.
    mini_away: bool,
    /// The mouse was over the mini player last frame.
    mini_hovered: bool,
    /// The show-cursor key is being held.
    cursor_held: bool,
    /// The mini player's volume was changed and not yet saved.
    mini_volume_dirty: bool,
    /// A held volume key, and when it next steps.
    volume_repeat: Option<(Bind, Instant)>,
    volume_unsaved: bool,
    /// When a volume key last changed the volume, for the popup.
    volume_shown: Option<Instant>,
}

// SAFETY: the menu is handed to hudhook once and only used on its render
// thread afterwards; `FontId` is a pointer into that thread's font atlas,
// which lives as long as the imgui context.
unsafe impl Send for Menu {}
unsafe impl Sync for Menu {}

impl Menu {
    pub fn new() -> Self {
        Self {
            view: View::Hidden,
            keys: KeyPoll::new(),
            pad: keys::PadPoll::new(),
            settings_open: false,
            capturing: None,
            pad_capturing: None,
            pad_nav: false,
            pad_focus: false,
            pad_now: PadState::default(),
            pad_fed: [false; 10],
            pad_back: false,
            pad_hold_down: false,
            pad_swallow_b: false,
            back_shown: false,
            pad_move: [0.0; 2],
            mini_at: None,
            full_at: None,
            search: String::new(),
            open_list: None,
            new_list: None,
            renaming_list: None,
            confirm_delete: None,
            focus_name: false,
            toast_serial: 0,
            toast_since: None,
            was_battle: false,
            place_tab: 1,
            battle_changed: None,
            battle_toast_due: false,
            heading: None,
            song_font: None,
            small_font: None,
            scale: 1.0,
            hovered: None,
            mini_away: false,
            renaming: None,
            mini_hovered: false,
            cursor_held: false,
            mini_volume_dirty: false,
            volume_repeat: None,
            volume_unsaved: false,
            volume_shown: None,
        }
    }

    /// Runs the hotkeys, or hands a fresh key press to the key being rebound.
    /// Only while the game is the active window, and not while a text box is
    /// being typed in.
    fn poll_keys(&mut self, typing: bool) {
        let pressed = self.keys.pressed();
        let focused = keys::game_focused();
        // The show-cursor key works by being held, not pressed.
        let hold = state::lock().config.key(Bind::ShowCursor);
        self.cursor_held = focused && self.capturing.is_none() && keys::held(hold);
        // A held volume key keeps stepping, after a pause like a key repeat.
        if let Some((bind, at)) = self.volume_repeat {
            let key = state::lock().config.key(bind);
            if !focused || typing || !keys::held(key) {
                self.volume_repeat = None;
            } else if Instant::now() >= at {
                self.step_volume(bind);
                self.volume_repeat = Some((bind, Instant::now() + VOLUME_REPEAT));
            }
        }
        // The volume is saved once the keys are let go.
        if self.volume_repeat.is_none() && self.volume_unsaved {
            self.volume_unsaved = false;
            state::lock().save_config();
        }
        // Controller combos: hold one button and press another. Read every
        // frame, so a press only counts once.
        let (now, fresh) = self.pad.read();
        self.pad_now = now;
        let (binds, on) = {
            let s = state::lock();
            (s.config.pad_binds(), s.config.controller)
        };
        self.pad_hold_down = on && now.held(binds.hold);
        self.pad_move = if self.pad_hold_down && focused { now.right_stick() } else { [0.0; 2] };
        if self.view != View::Full {
            self.pad_nav = false;
        }
        if let Some(bind) = self.pad_capturing {
            // A combo button being rebound takes the next press.
            keys::combos(&now, &[], &binds, false, false);
            if let Some(&button) = fresh.first() {
                let mut s = state::lock();
                s.config.set_pad(bind, button);
                s.save_config();
                self.pad_capturing = None;
            }
        } else {
            let navigating = self.pad_nav && focused;
            let events = keys::combos(&now, &fresh, &binds, on && focused && !typing, navigating);
            self.pad_back = navigating && fresh.contains(&PadButton::B) && !now.held(binds.hold);
            let mut s = state::lock();
            for event in events {
                match event {
                    PadEvent::Previous => s.push(Command::Previous),
                    PadEvent::Next => s.push(Command::Next),
                    PadEvent::PlayPause => s.push(Command::TogglePause),
                    PadEvent::Stop => s.push(Command::Stop),
                    // Steps through the views like the menu key; the full
                    // menu opens ready for the controller.
                    PadEvent::Menu => {
                        self.view = self.view.next();
                        self.pad_nav = self.view == View::Full;
                        self.pad_focus = self.pad_nav;
                    }
                }
            }
        }
        if pressed.is_empty() || !focused {
            return;
        }
        let mut s = state::lock();
        if let Some(bind) = self.capturing {
            let vk = pressed[0];
            match vk {
                keys::ESCAPE => {}
                keys::DELETE | keys::BACKSPACE => s.config.set_key(bind, 0),
                _ => s.config.set_key(bind, vk),
            }
            self.capturing = None;
            s.save_config();
            return;
        }
        if typing {
            return;
        }
        let mut volume = None;
        for vk in pressed {
            match Bind::ALL.into_iter().find(|&b| s.config.key(b) == vk) {
                Some(Bind::Menu) => {
                    // The menu key always opens the menu for the mouse.
                    self.view = self.view.next();
                    self.pad_nav = false;
                }
                Some(Bind::Previous) => s.push(Command::Previous),
                Some(Bind::PlayPause) => s.push(Command::TogglePause),
                Some(Bind::Next) => s.push(Command::Next),
                Some(Bind::Stop) => s.push(Command::Stop),
                Some(b @ (Bind::VolumeDown | Bind::VolumeUp)) => volume = Some(b),
                Some(Bind::ShowCursor) | None => {}
            }
        }
        drop(s);
        if let Some(bind) = volume {
            self.step_volume(bind);
            self.volume_repeat = Some((bind, Instant::now() + VOLUME_REPEAT_DELAY));
        }
    }

    /// One 5% volume step, shown in a popup while the jukebox is hidden.
    fn step_volume(&mut self, bind: Bind) {
        let delta = if bind == Bind::VolumeUp { 0.05 } else { -0.05 };
        state::lock().config.step_volume(delta);
        self.volume_unsaved = true;
        self.volume_shown = Some(Instant::now());
    }

    /// The Playlists tab's front page: a card for each of your playlists
    /// and each folder in the music folder, and one to make a new playlist.
    fn playlist_cards(&mut self, ui: &Ui, s: &state::Shared, actions: &mut Vec<Action>, size: [f32; 2]) {
        ui.child_window("playlist_cards").size(size).build(|| {
            ui.text_disabled("Your playlists");
            for (p, pl) in s.config.playlists.iter().enumerate() {
                let n = s.indices(List::Custom(p)).len();
                let sub = song_count(n);
                // Renaming happens in the card itself.
                if let Some((rp, buf)) = &mut self.renaming_list {
                    if *rp == p {
                        match name_box(ui, &format!("##renamepl{p}"), buf, "Playlist name", &mut self.focus_name) {
                            NameBox::Done => {
                                actions.push(Action::Rename(p, buf.clone()));
                                self.renaming_list = None;
                            }
                            NameBox::Cancelled => self.renaming_list = None,
                            NameBox::Typing => {}
                        }
                        continue;
                    }
                }
                let (open, button) =
                    card(ui, &format!("##pl{p}"), &pl.name, &sub, &[(theme::PLAY, "Play"), (theme::MORE, "More")]);
                if open {
                    self.open_list = Some(Playlist::Custom(pl.name.clone()));
                }
                let menu = format!("plmenu{p}");
                match button {
                    Some(0) if n > 0 => actions.push(Action::PlayFrom(Playlist::Custom(pl.name.clone()), false)),
                    Some(1) => ui.open_popup(&menu),
                    _ => {}
                }
                ui.popup(&menu, || {
                    let _hover = ui.push_style_color(StyleColor::HeaderHovered, [0.45, 0.8, 0.95, 0.35]);
                    if ui.selectable("Rename") {
                        self.renaming_list = Some((p, pl.name.clone()));
                        self.focus_name = true;
                    }
                    if ui.selectable_config("Delete...").flags(SelectableFlags::DONT_CLOSE_POPUPS).build() {
                        self.confirm_delete = Some(p);
                    }
                    if self.confirm_delete == Some(p) {
                        ui.text(format!("Delete \"{}\"?", pl.name));
                        if ui.small_button("Delete") {
                            actions.push(Action::Delete(p));
                            self.confirm_delete = None;
                            ui.close_current_popup();
                        }
                        ui.same_line();
                        if ui.small_button("Cancel") {
                            self.confirm_delete = None;
                        }
                    }
                });
            }
            // The new-playlist card turns into a name box when clicked.
            match &mut self.new_list {
                Some(buf) => match name_box(ui, "##newpl", buf, "Name the new playlist", &mut self.focus_name) {
                    NameBox::Done => {
                        actions.push(Action::CreateNamed(std::mem::take(buf)));
                        self.new_list = None;
                    }
                    NameBox::Cancelled => self.new_list = None,
                    NameBox::Typing => {}
                },
                None => {
                    if card(ui, "##newpl_card", "+  New playlist", "Then use + on any song to add it", &[]).0 {
                        self.new_list = Some(String::new());
                        self.focus_name = true;
                    }
                }
            }
            let folders = s.folders();
            if !folders.is_empty() {
                ui.spacing();
                ui.text_disabled("Folders in your music folder");
                for name in folders {
                    let prefix = format!("file:{name}/");
                    let n = s.tracks.iter().filter(|t| t.key.starts_with(&prefix)).count();
                    let (open, button) =
                        card(ui, &format!("##folder{name}"), &name, &format!("Folder - {}", song_count(n)), &[(theme::PLAY, "Play")]);
                    if open {
                        self.open_list = Some(Playlist::Folder(name.clone()));
                    }
                    if button == Some(0) {
                        actions.push(Action::PlayFrom(Playlist::Folder(name), false));
                    }
                }
            }
        });
    }

    /// One playlist's own page: a header with Play and Shuffle, then its
    /// songs in order, numbered. Your own playlists reorder by dragging a
    /// song's number onto another song.
    #[allow(clippy::too_many_arguments)]
    fn playlist_page<R: Fn(&state::Shared, usize, &mut Vec<Action>, Option<List>)>(
        &mut self,
        ui: &Ui,
        s: &state::Shared,
        open: &Playlist,
        actions: &mut Vec<Action>,
        size: [f32; 2],
        title_font: Option<FontId>,
        row: &R,
    ) {
        let (list, songs) = match open {
            Playlist::Custom(name) => {
                let Some(p) = s.find_playlist(name) else { return };
                (Some(List::Custom(p)), s.indices(List::Custom(p)))
            }
            Playlist::Folder(name) => {
                let prefix = format!("file:{name}/");
                (None, (0..s.tracks.len()).filter(|&i| s.tracks[i].key.starts_with(&prefix)).collect())
            }
            _ => return,
        };
        self.back_shown = true;
        if ui.small_button("\u{2190} Playlists") {
            self.open_list = None;
        }
        {
            let _font = title_font.map(|f| ui.push_font(f));
            let name = match open {
                Playlist::Custom(n) | Playlist::Folder(n) => n.as_str(),
                _ => "",
            };
            theme::shadow_text(ui, theme::WHITE, name);
        }
        let kind = if list.is_some() { "Playlist" } else { "Folder" };
        ui.text_disabled(format!("{kind} - {}", song_count(songs.len())));
        if !songs.is_empty() {
            if ui.button(format!("{} Play", theme::PLAY)) {
                actions.push(Action::PlayFrom(open.clone(), false));
            }
            ui.same_line();
            if ui.button(format!("{} Shuffle", theme::SHUFFLE)) {
                actions.push(Action::PlayFrom(open.clone(), true));
            }
        }
        ui.spacing();
        let rest = [size[0], size[1] - 0.0];
        ui.child_window("playlist_page").size(rest).build(|| {
            if songs.is_empty() {
                ui.text_disabled("Empty. Use + on any song, or right-click it, to add it here.");
            }
            for (n, &i) in songs.iter().enumerate() {
                ui.align_text_to_frame_padding();
                ui.text_disabled(format!("{:>2}", n + 1));
                if let Some(list) = list {
                    ui.same_line();
                    if ui.small_button(format!("{}##rm{i}", theme::CLOSE_SMALL)) {
                        actions.push(Action::Toggle(list, i));
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip_text("Remove from this playlist");
                    }
                }
                ui.same_line();
                row(s, i, actions, list);
            }
        });
    }

    /// The settings window, beside the full menu, in three tabs: General,
    /// Battles and Controls. Each option is one row, its name on the left and
    /// an On/Off switch on the right; the details are in its tooltip.
    fn settings(&mut self, ui: &Ui, s: &mut state::Shared, at: [f32; 2]) {
        let heading = self.song_font;
        let small_font = self.small_font;
        let k = self.scale;
        let mut close = false;
        let mut reset = false;
        let mut changed = false;
        let mut resized_to = None;
        // Resizable from its corner; the size is kept for next time.
        let size = s.config.settings_size.unwrap_or([340.0 * k, 430.0 * k]);
        ui.window("##jukebox_settings")
            .position(at, Condition::Appearing)
            .size(size, Condition::Appearing)
            .size_constraints([270.0 * k, 200.0 * k], [10_000.0, 10_000.0])
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_COLLAPSE)
            .build(|| {
                if !ui.is_mouse_down(MouseButton::Left) {
                    let z = ui.window_size();
                    if s.config.settings_size.is_none_or(|a| (a[0] - z[0]).abs() > 0.5 || (a[1] - z[1]).abs() > 0.5) {
                        resized_to = Some(z);
                    }
                }
                theme::backdrop(ui);
                {
                    let _font = heading.map(|f| ui.push_font(f));
                    theme::shadow_text(ui, theme::WHITE, "SETTINGS");
                }
                ui.same_line();
                if header_buttons(ui, &[(Icon::Close, "Close settings")]).is_some() {
                    close = true;
                }
                theme::heading_rule(ui);
                let tab = |name: &str| {
                    let _font = small_font.map(|f| ui.push_font(f));
                    ui.tab_item(name)
                };
                let Some(_bar) = ui.tab_bar("settings_tabs") else { return };
                if let Some(_tab) = tab("General") {
                    section(ui, "Playback");
                    // The crossfade slider takes the switch's place, wider.
                    ui.align_text_to_frame_padding();
                    ui.text("Crossfade");
                    let label_hovered = ui.is_item_hovered();
                    let slider_w = (ui.frame_height() * 6.0)
                        .min(ui.content_region_avail()[0] - ui.item_rect_size()[0] - ui.clone_style().item_spacing[0]);
                    ui.same_line_with_pos(ui.window_content_region_max()[0] - slider_w);
                    ui.set_next_item_width(slider_w);
                    let mut cf = s.config.crossfade_secs;
                    if ui.slider_config("##crossfade", 0.0, 8.0).display_format("%.0f s").build(&mut cf) {
                        s.config.crossfade_secs = cf.round();
                        changed = true;
                    }
                    if label_hovered || ui.is_item_hovered() {
                        ui.tooltip_text("How long one song blends into the next. 0 for a straight cut.");
                    }
                    changed |= option(
                        ui,
                        "Resume last song at start",
                        "When the game starts, carry on with the song you were\nplaying last time, where you left it. Stop clears it.",
                        &mut s.config.resume_on_start,
                    );
                    changed |= option(
                        ui,
                        "New song on a new map",
                        "Moves on to the next song when you enter another map,\nsuch as from the field into a town.",
                        &mut s.config.new_song_on_new_map,
                    );
                    changed |= option(
                        ui,
                        "Game music in Vision shrines",
                        "Vision shrines keep the game's own music; the jukebox
waits, and carries on once you leave.",
                        &mut s.config.shrine_game_music,
                    );
                    section(ui, "Mini player");
                    changed |= option(
                        ui,
                        "Hide during cutscenes",
                        "Tucks the mini player away for story events and movies,\nand brings it back when they end.",
                        &mut s.config.mini_hide_in_cutscenes,
                    );
                    changed |= option(
                        ui,
                        "Hide while talking",
                        "Tucks the mini player away while you talk to townsfolk,\nshops or read a sign, and brings it back after.",
                        &mut s.config.mini_hide_while_talking,
                    );
                    section(ui, "Menu");
                    changed |= option(
                        ui,
                        "Hide the key reminder",
                        "The line at the foot of the menu saying which keys\nswitch to the mini player.",
                        &mut s.config.hide_key_hint,
                    );
                }
                if let Some(_tab) = tab("Battles") {
                    section(ui, "Music");
                    changed |= option(
                        ui,
                        "Game music in boss battles",
                        "Boss battles keep the game's own music, as the\ndevelopers intended, even with Battles switched on.",
                        &mut s.config.boss_game_music,
                    );
                    changed |= option(
                        ui,
                        "Loop the battle song",
                        "Keeps the song picked as a battle starts playing on a
loop until the battle ends, instead of moving on.",
                        &mut s.config.loop_battle_song,
                    );
                    changed |= option(
                        ui,
                        "Pick up where it left off after battles",
                        "After a battle, your exploring song carries on from where it
was when the battle started. Off: the next song plays instead.",
                        &mut s.config.resume_after_battle,
                    );
                    changed |= option(
                        ui,
                        "Let the victory fanfare play",
                        "Winning a battle fades the jukebox out for the game's own\nvictory fanfare, then picks up again once it's over.",
                        &mut s.config.victory_fanfare,
                    );
                    section(ui, "Popup");
                    changed |= option(
                        ui,
                        "Show the song on battle start and end",
                        "Pops the now-playing note in the top right when a\nbattle starts or ends, while the jukebox is playing.",
                        &mut s.config.battle_toast,
                    );
                }
                let mut on_controls = false;
                if let Some(_tab) = tab("Controls") {
                    on_controls = true;
                    section(ui, "Keys");
                    let label_w = Bind::ALL.iter().map(|b| ui.calc_text_size(b.label())[0]).fold(0.0, f32::max)
                        + ui.frame_height();
                    let left = ui.cursor_pos()[0];
                    let key_w = ui.window_content_region_max()[0] - left - label_w;
                    for bind in Bind::ALL {
                        ui.align_text_to_frame_padding();
                        ui.text(bind.label());
                        ui.same_line_with_pos(left + label_w);
                        let waiting = self.capturing == Some(bind);
                        let text = if waiting { "Press a key...".to_string() } else { keys::name(s.config.key(bind)) };
                        let _lit = waiting.then(|| ui.push_style_color(StyleColor::Button, [0.35, 0.68, 0.84, 0.75]));
                        if ui.button_with_size(format!("{text}##bind{bind:?}"), [key_w, 0.0]) {
                            self.capturing = if waiting { None } else { Some(bind) };
                        }
                    }
                    note(ui, "Click a key, then press the new one. Esc cancels, Delete unbinds.");
                    if ui.button("Reset keys to defaults") {
                        reset = true;
                    }
                    section(ui, "Controller");
                    changed |= option(
                        ui,
                        "Controller combos",
                        "Hold the Hold button and press another to use the jukebox.\n\
                         The game doesn't see those presses while Hold is down.",
                        &mut s.config.controller,
                    );
                    const MOVE_LABEL: &str = "Move the menu";
                    let label_w = PadBind::ALL
                        .iter()
                        .map(|b| ui.calc_text_size(b.label())[0])
                        .fold(ui.calc_text_size(MOVE_LABEL)[0], f32::max)
                        + ui.frame_height();
                    let left = ui.cursor_pos()[0];
                    let button_w = ui.window_content_region_max()[0] - left - label_w;
                    {
                        let _off = ui.begin_disabled(!s.config.controller);
                        for bind in PadBind::ALL {
                            ui.align_text_to_frame_padding();
                            ui.text(bind.label());
                            ui.same_line_with_pos(left + label_w);
                            let waiting = self.pad_capturing == Some(bind);
                            let shown = s.config.pad(bind).label();
                            let text = match (waiting, bind) {
                                (true, _) => "Press a button...".to_string(),
                                (false, PadBind::Hold) => shown.to_string(),
                                (false, _) => format!("{} + {shown}", s.config.pad_hold.label()),
                            };
                            let _lit = waiting.then(|| ui.push_style_color(StyleColor::Button, [0.35, 0.68, 0.84, 0.75]));
                            if ui.button_with_size(format!("{text}##pad{bind:?}"), [button_w, 0.0]) {
                                self.pad_capturing = if waiting { None } else { Some(bind) };
                            }
                        }
                        // Moving the menu always uses the right stick; shown
                        // with the others, but it can't be changed.
                        ui.align_text_to_frame_padding();
                        ui.text(MOVE_LABEL);
                        ui.same_line_with_pos(left + label_w);
                        ui.button_with_size(
                            format!("{} + Right stick##padmove", s.config.pad_hold.label()),
                            [button_w, 0.0],
                        );
                        if ui.is_item_hovered() {
                            ui.tooltip_text("Always the right stick; only Hold can be changed.");
                        }
                    }
                    note(
                        ui,
                        "Click a combo, then press its new button on the controller. Open the menu with the \
                         controller to move around it with the D-pad or left stick: A chooses, B backs out. \
                         Hold and push the right stick to move the menu or the mini player.",
                    );
                    if ui.button("Reset controller to defaults") {
                        s.config.reset_pad();
                        self.pad_capturing = None;
                        changed = true;
                    }
                }
                // A key waiting to be rebound stops waiting once its tab is left.
                if !on_controls {
                    self.capturing = None;
                    self.pad_capturing = None;
                }
            });
        if let Some(z) = resized_to {
            s.config.settings_size = Some(z);
            changed = true;
        }
        if changed {
            s.save_config();
        }
        if reset {
            s.config.reset_keys();
            s.save_config();
            self.capturing = None;
        }
        if close {
            self.settings_open = false;
            self.capturing = None;
        }
    }

    /// The mini player: song, progress and transport buttons, in a small
    /// panel that keeps its own place on screen. The title wraps onto as
    /// many lines as it needs, so the whole of it can be read.
    fn mini(&mut self, ui: &Ui, s: &mut state::Shared, title: &Option<String>) {
        let w = 256.0 * self.scale;
        let [dw, dh] = ui.io().display_size;
        let pos = s.config.mini_pos.unwrap_or([dw - w - 40.0, 40.0]);
        let pos = [pos[0].clamp(0.0, (dw - 100.0).max(0.0)), pos[1].clamp(0.0, (dh - 60.0).max(0.0))];
        let small_font = self.small_font;
        let mut next_view = None;
        let mut moved_to = None;
        let mut hovered = false;
        let mut volume_changed = false;
        let k = self.scale;
        let _pad = ui.push_style_var(StyleVar::WindowPadding([9.0 * k, 7.0 * k]));
        let _gap = ui.push_style_var(StyleVar::ItemSpacing([5.0 * k, 4.0 * k]));
        let moving = self.pad_move != [0.0; 2];
        let (pos, cond) = match (moving, self.mini_at) {
            (true, Some(at)) => (stick_moved(ui, at, self.pad_move, [w, 60.0]), Condition::Always),
            _ => (pos, Condition::FirstUseEver),
        };
        ui.window("##jukebox_mini")
            .position(pos, cond)
            .size_constraints([w, 0.0], [w, 10_000.0])
            .flags(
                WindowFlags::NO_TITLE_BAR
                    | WindowFlags::NO_COLLAPSE
                    | WindowFlags::ALWAYS_AUTO_RESIZE
                    | WindowFlags::NO_FOCUS_ON_APPEARING,
            )
            .build(|| {
                theme::backdrop(ui);
                // The whole title, wrapped to the panel's width.
                {
                    let (text, color) = match title {
                        Some(t) => (t.as_str(), theme::WHITE),
                        None => ("Nothing playing", DIM),
                    };
                    let _color = ui.push_style_color(StyleColor::Text, color);
                    ui.text_wrapped(text);
                }
                let _font = small_font.map(|f| ui.push_font(f));
                progress(ui, s);
                let fh = ui.frame_height();
                let b = transport(ui, s, None, [fh * 1.05, fh * 1.05]);
                ui.same_line();
                if repeat(ui, s, b) {
                    s.save_config();
                }
                // Volume, expand and hide sit at the end of the transport row.
                ui.same_line();
                let volume = s.config.volume;
                let tip = format!("Volume {:.0}% (click for slider, or scroll)", volume * 100.0);
                let (clicked, over) = header_buttons_ex(
                    ui,
                    &[(Icon::Volume(volume), tip.as_str()), (Icon::Expand, "Full menu"), (Icon::Close, "Hide")],
                );
                match clicked {
                    Some(0) => ui.open_popup("##minivolume"),
                    Some(1) => next_view = Some(View::Full),
                    Some(_) => next_view = Some(View::Hidden),
                    None => {}
                }
                // The wheel over the speaker steps the volume without opening
                // the slider.
                let wheel = ui.io().mouse_wheel;
                if over == Some(0) && wheel != 0.0 {
                    s.config.step_volume(0.05 * wheel.signum());
                    volume_changed = true;
                }
                // The same volume as the full menu's, in a small slider that
                // opens from the speaker and drags either way.
                ui.popup("##minivolume", || {
                    let mut vol = s.config.volume * 100.0;
                    ui.set_next_item_width(150.0 * k);
                    if ui.slider_config("##minivol", 0.0, 150.0).display_format("%.0f%%").build(&mut vol) {
                        s.config.volume = vol / 100.0;
                        volume_changed = true;
                    }
                    let wheel = ui.io().mouse_wheel;
                    if ui.is_item_hovered() && wheel != 0.0 {
                        s.config.step_volume(0.05 * wheel.signum());
                        volume_changed = true;
                    }
                });
                hovered = ui.is_window_hovered_with_flags(WindowHoveredFlags::ROOT_AND_CHILD_WINDOWS);
                self.mini_at = Some(ui.window_pos());
                // Saved once a drag or a stick move is let go.
                if !ui.is_mouse_down(MouseButton::Left) && !moving {
                    let p = ui.window_pos();
                    if s.config.mini_pos.is_none_or(|a| (a[0] - p[0]).abs() > 0.5 || (a[1] - p[1]).abs() > 0.5) {
                        moved_to = Some(p);
                    }
                }
            });
        self.mini_hovered = hovered;
        if let Some(p) = moved_to {
            s.config.mini_pos = Some(p);
            s.save_config();
        }
        // The volume is saved once the slider is let go, not on every step
        // of a drag.
        self.mini_volume_dirty |= volume_changed;
        if self.mini_volume_dirty && !ui.is_mouse_down(MouseButton::Left) {
            self.mini_volume_dirty = false;
            s.save_config();
        }
        if let Some(v) = next_view {
            self.view = v;
        }
    }
}

fn song_count(n: usize) -> String {
    match n {
        0 => "Empty".into(),
        1 => "1 song".into(),
        n => format!("{n} songs"),
    }
}

/// A clickable card: a title over a dim line, with small buttons at its
/// right end. Returns whether the card itself was clicked, and which button.
fn card(ui: &Ui, id: &str, title: &str, sub: &str, buttons: &[(&str, &str)]) -> (bool, Option<usize>) {
    let w = ui.content_region_avail()[0];
    let line = ui.text_line_height();
    let h = line * 2.0 + ui.clone_style().frame_padding[1] * 3.0;
    let b = ui.frame_height();
    let gap = ui.clone_style().item_spacing[0];
    let buttons_w = buttons.len() as f32 * (b + gap);
    let start = ui.cursor_screen_pos();
    let opened = ui.invisible_button(id, [(w - buttons_w).max(b), h]);
    let hovered = ui.is_item_hovered();
    let end = [start[0] + w, start[1] + h];
    {
        let draw = ui.get_window_draw_list();
        draw.add_rect(start, end, [0.05, 0.2, 0.3, if hovered { 0.0 } else { 0.35 }]).filled(true).rounding(3.0).build();
        draw.add_rect(start, end, [0.75, 0.88, 0.97, if hovered { 0.0 } else { 0.18 }]).rounding(3.0).build();
    }
    if hovered {
        theme::row_bar(ui, start, end, true);
        theme::cursor(ui, [start[0] + line * 0.6, start[1] + h * 0.5], line * 0.38);
    }
    let pad = [line * 1.4, ui.clone_style().frame_padding[1] * 1.2];
    {
        let draw = ui.get_window_draw_list();
        draw.add_text([start[0] + pad[0], start[1] + pad[1]], theme::WHITE, title);
        draw.add_text([start[0] + pad[0], start[1] + pad[1] + line * 1.05], DIM, sub);
    }
    let mut clicked = None;
    for (n, (icon, tip)) in buttons.iter().enumerate() {
        ui.set_cursor_screen_pos([end[0] - (buttons.len() - n) as f32 * (b + gap) + gap, start[1] + (h - b) * 0.5]);
        if ui.button_with_size(format!("{icon}{id}b{n}"), [b, b]) {
            clicked = Some(n);
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(tip);
        }
    }
    ui.set_cursor_screen_pos([start[0], end[1] + ui.clone_style().item_spacing[1]]);
    ui.dummy([0.0, 0.0]);
    (opened, clicked)
}

enum NameBox {
    Typing,
    Done,
    Cancelled,
}

/// A name box in place of a card: Enter keeps the name, Esc cancels.
fn name_box(ui: &Ui, id: &str, buf: &mut String, hint: &str, focus: &mut bool) -> NameBox {
    // The keyboard goes to the box once, as it opens.
    if std::mem::take(focus) {
        ui.set_keyboard_focus_here();
    }
    ui.set_next_item_width(-1.0);
    let enter = ui.input_text(id, buf).hint(hint).enter_returns_true(true).build();
    let result = if enter && !buf.trim().is_empty() {
        NameBox::Done
    } else if ui.is_key_pressed(Key::Escape) {
        NameBox::Cancelled
    } else {
        NameBox::Typing
    };
    ui.text_disabled("Enter to keep, Esc to cancel");
    result
}

fn clock(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// `text`, cut short with an ellipsis if it is wider than `max_w`.
fn fit(ui: &Ui, text: &str, max_w: f32) -> String {
    if ui.calc_text_size(text)[0] <= max_w {
        return text.to_string();
    }
    let mut cut: String = text.to_string();
    while !cut.is_empty() && ui.calc_text_size(format!("{cut}\u{2026}"))[0] > max_w {
        cut.pop();
    }
    format!("{}\u{2026}", cut.trim_end())
}

/// The playing song's title in the song font, cut to `max_w` if given.
fn song_title(ui: &Ui, title: &Option<String>, font: Option<FontId>, max_w: Option<f32>) {
    let _font = font.map(|f| ui.push_font(f));
    let (text, color) = match title {
        Some(t) => (t.as_str(), theme::WHITE),
        None => ("Nothing playing", DIM),
    };
    let text = match max_w {
        Some(w) => fit(ui, text, w),
        None => text.to_string(),
    };
    theme::shadow_text(ui, color, &text);
}

/// The progress bar; clicking it seeks.
fn progress(ui: &Ui, s: &mut state::Shared) {
    let (pos, dur) = (s.position, s.duration);
    let frac = dur.map_or(0.0, |d| (pos / d).clamp(0.0, 1.0));
    let overlay = match dur {
        Some(d) => format!("{} / {}", clock(pos), clock(d)),
        None => clock(pos),
    };
    ProgressBar::new(frac).overlay_text(overlay).size([-1.0, ui.frame_height() * 0.8]).build(ui);
    if let (true, Some(d)) = (ui.is_item_clicked(), dur) {
        let [x0, _] = ui.item_rect_min();
        let [x1, _] = ui.item_rect_max();
        let f = ((ui.io().mouse_pos[0] - x0) / (x1 - x0)).clamp(0.0, 1.0);
        s.push(Command::Seek(f * d));
    }
}

/// Previous, play/pause, next and stop, all one size, with the icons in the
/// larger song font so they fill their buttons. Returns the button size.
fn transport(ui: &Ui, s: &mut state::Shared, font: Option<FontId>, b: [f32; 2]) -> [f32; 2] {
    let playing = s.playing && s.current.is_some();
    let buttons = [
        (theme::PREV, "Previous", Command::Previous, Bind::Previous),
        (if playing { theme::PAUSE } else { theme::PLAY }, if playing { "Pause" } else { "Play" }, Command::TogglePause, Bind::PlayPause),
        (theme::NEXT, "Next", Command::Next, Bind::Next),
        (theme::STOP, "Stop", Command::Stop, Bind::Stop),
    ];
    for (n, (icon, tip, command, bind)) in buttons.into_iter().enumerate() {
        let key = s.config.key(bind);
        let tip = if key == 0 { tip.to_string() } else { format!("{tip} ({})", keys::name(key)) };
        let tip = tip.as_str();
        if n > 0 {
            ui.same_line();
        }
        let clicked = {
            let _font = font.map(|f| ui.push_font(f));
            ui.button_with_size(format!("{icon}##transport{n}"), b)
        };
        if clicked {
            s.push(command);
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(tip);
        }
    }
    b
}

/// The repeat button, which steps through off, playlist and song. Returns
/// whether it was clicked.
fn repeat(ui: &Ui, s: &mut state::Shared, b: [f32; 2]) -> bool {
    let (on, one) = (s.config.repeat != Repeat::Off, s.config.repeat == Repeat::One);
    let clicked = theme::repeat_button(ui, "##repeat", on, one, b);
    if clicked {
        s.config.repeat = s.config.repeat.next();
    }
    if ui.is_item_hovered() {
        ui.tooltip_text(format!("{}\nClick to switch: off, playlist, song.", s.config.repeat.label()));
    }
    clicked
}

/// A small heading over a group of settings, with a little room above it.
fn section(ui: &Ui, title: &str) {
    ui.spacing();
    ui.text_disabled(title);
}

/// One settings row: the name on the left and an On/Off switch at the right
/// edge, with `tip` shown over either. Returns whether it was switched.
fn option(ui: &Ui, label: &str, tip: &str, on: &mut bool) -> bool {
    let w = ui.frame_height() * 2.4;
    let right = ui.window_content_region_max()[0];
    let gap = ui.clone_style().item_spacing[0];
    ui.align_text_to_frame_padding();
    {
        // A long name wraps onto a second line rather than running under
        // the switch when the window is narrow.
        let _wrap = ui.push_text_wrap_pos_with_pos(right - w - gap);
        ui.text(label);
    }
    let label_hovered = ui.is_item_hovered();
    ui.same_line_with_pos(right - w);
    let text = if *on { "On" } else { "Off" };
    let switched = theme::toggle(ui, &format!("{text}##{label}"), on, [w, 0.0]);
    if label_hovered || ui.is_item_hovered() {
        ui.tooltip_text(tip);
    }
    switched
}

fn header_button_size(ui: &Ui) -> f32 {
    ui.frame_height() * 1.05
}

fn header_buttons_width(ui: &Ui, buttons: &[(Icon, &str)]) -> f32 {
    let gap = ui.clone_style().item_spacing[0] * 0.5;
    buttons.len() as f32 * (header_button_size(ui) + gap)
}

/// Icon buttons at the right end of the current line; returns which was
/// clicked.
fn header_buttons(ui: &Ui, buttons: &[(Icon, &str)]) -> Option<usize> {
    header_buttons_ex(ui, buttons).0
}

/// [`header_buttons`], also returning which button the mouse is over.
fn header_buttons_ex(ui: &Ui, buttons: &[(Icon, &str)]) -> (Option<usize>, Option<usize>) {
    let gap = ui.clone_style().item_spacing[0] * 0.5;
    let x = ui.window_content_region_max()[0] - header_buttons_width(ui, buttons) + gap;
    ui.set_cursor_pos([x, ui.cursor_pos()[1]]);
    let (mut clicked, mut over) = (None, None);
    for (n, (icon, tip)) in buttons.iter().enumerate() {
        if n > 0 {
            ui.same_line_with_spacing(0.0, gap);
        }
        if theme::icon_button(ui, &format!("##hdr{n}"), *icon, header_button_size(ui)) {
            clicked = Some(n);
        }
        if ui.is_item_hovered() {
            over = Some(n);
            ui.tooltip_text(tip);
        }
    }
    (clicked, over)
}

/// Where a window at `at` goes this frame with the right stick leaning
/// `lean`: up to 1200 px a second at a full lean, kept on screen with at
/// least `keep` of it showing.
fn stick_moved(ui: &Ui, at: [f32; 2], lean: [f32; 2], keep: [f32; 2]) -> [f32; 2] {
    const SPEED: f32 = 1200.0;
    let io = ui.io();
    let step = SPEED * io.delta_time;
    let [dw, dh] = io.display_size;
    [
        (at[0] + lean[0] * step).clamp(0.0, (dw - keep[0]).max(0.0)),
        (at[1] + lean[1] * step).clamp(0.0, (dh - keep[1]).max(0.0)),
    ]
}

/// A dim note that wraps at the window's edge instead of running off it.
fn note(ui: &Ui, text: &str) {
    let _wrap = ui.push_text_wrap_pos();
    ui.text_disabled(text);
}

/// Whether any popup (a dropdown's list, a right-click menu) is open.
fn any_popup_open() -> bool {
    use hudhook::imgui::sys;
    let flags = (sys::ImGuiPopupFlags_AnyPopupId | sys::ImGuiPopupFlags_AnyPopupLevel) as i32;
    unsafe { sys::igIsPopupOpen(c"".as_ptr(), flags) }
}

impl ImguiRenderLoop for Menu {
    fn initialize<'a>(&'a mut self, ctx: &mut Context, _: &'a mut dyn RenderContext) {
        ctx.set_ini_filename(None);
        self.scale = state::lock().config.ui_scale;
        let fonts = theme::load_fonts(ctx, self.scale);
        self.heading = fonts.heading;
        self.song_font = fonts.song;
        self.small_font = fonts.small;
        theme::apply(ctx, self.scale);
    }

    fn before_render<'a>(&'a mut self, ctx: &mut Context, _: &'a mut dyn RenderContext) {
        // The mini player stays up while exploring, where the game hides its
        // cursor, so one is drawn only while the mouse is over the player or
        // the show-cursor key (Left Alt) is held.
        let io = ctx.io_mut();
        // Whichever was used last drives the menu: a click or the wheel hands
        // it back to the mouse (and the controller back to the game). Not
        // mouse movement: the game moves the cursor itself while exploring.
        let mouse_used = io.mouse_down.iter().any(|&d| d) || io.mouse_wheel != 0.0;
        if self.view != View::Full || mouse_used {
            self.pad_nav = false;
        }
        io.mouse_draw_cursor = (self.view == View::Full && !self.pad_nav)
            || (self.view == View::Mini && !self.mini_away && (self.mini_hovered || self.cursor_held));
        // While the menu is driven with the controller, its buttons move
        // around the menu: D-pad or left stick to move, A to choose, B to
        // back out, LB/RB between windows.
        let nav = self.pad_nav;
        // A combo (Hold + a button) is for the jukebox, not the menu.
        let combo = self.pad_hold_down;
        if nav {
            io.config_flags |= ConfigFlags::NAV_ENABLE_GAMEPAD;
            io.backend_flags |= BackendFlags::HAS_GAMEPAD;
        } else {
            io.config_flags &= !ConfigFlags::NAV_ENABLE_GAMEPAD;
            io.backend_flags &= !BackendFlags::HAS_GAMEPAD;
        }
        if !self.pad_now.held(PadButton::B) {
            self.pad_swallow_b = false;
        }
        let p = &self.pad_now;
        let [up, down, left, right] = p.nav();
        let keys = [
            (Key::GamepadDpadUp, up),
            (Key::GamepadDpadDown, down),
            (Key::GamepadDpadLeft, left),
            (Key::GamepadDpadRight, right),
            (Key::GamepadFaceDown, p.held(PadButton::A)),
            (Key::GamepadFaceRight, p.held(PadButton::B) && !self.pad_swallow_b),
            (Key::GamepadFaceLeft, p.held(PadButton::X)),
            (Key::GamepadFaceUp, p.held(PadButton::Y)),
            (Key::GamepadL1, p.held(PadButton::LB)),
            (Key::GamepadR1, p.held(PadButton::RB)),
        ];
        for (i, (key, down)) in keys.into_iter().enumerate() {
            let down = nav && !combo && down;
            if self.pad_fed[i] != down {
                io.add_key_event(key, down);
                self.pad_fed[i] = down;
            }
        }
    }

    fn render(&mut self, ui: &mut Ui) {
        let key = state::lock().config.menu_key;
        // Typing in the jukebox's search box or in one of the game's own text
        // boxes (chat, names) must not also skip songs or change the volume.
        let typing = ui.io().want_text_input || crate::game::GAME_TYPING.load(std::sync::atomic::Ordering::Relaxed);
        self.poll_keys(typing);

        let mut s = state::lock();
        let battle = s.scene == crate::config::Scene::Battle;
        if battle != self.was_battle {
            self.was_battle = battle;
            self.battle_changed = Some(Instant::now());
            self.battle_toast_due = s.config.battle_toast;
        }
        let since_battle = self.battle_changed.map_or(f32::MAX, |t| t.elapsed().as_secs_f32());
        if s.track_serial != self.toast_serial {
            self.toast_serial = s.track_serial;
            // Swapping to the battle playlist and back is a track change too;
            // those stay quiet, and the battle toast below speaks for them
            // (or not, when it's off or the game's music has the battle).
            if since_battle > BATTLE_SWAP_SECS {
                self.toast_since = Some(Instant::now());
            }
        }
        // Battle start or end: the player settles which song plays (or holds
        // for the game's music) a tick or two after the scene changes, so the
        // toast waits a moment and only shows if the jukebox is playing.
        if self.battle_toast_due && since_battle >= BATTLE_TOAST_DELAY {
            self.battle_toast_due = false;
            if s.playing && !s.held {
                self.toast_since = Some(Instant::now());
            }
        }
        let title = s.current.and_then(|i| s.tracks.get(i)).map(|t| t.title.clone());

        if self.view == View::Mini {
            // Story events and movies get the screen to themselves; the
            // player comes back when they end.
            let cutscene = matches!(s.scene, crate::config::Scene::Event | crate::config::Scene::Movie);
            self.mini_away = (s.config.mini_hide_in_cutscenes && cutscene)
                || (s.config.mini_hide_while_talking && s.talking);
            if self.mini_away {
                self.mini_hovered = false;
            } else {
                self.mini(ui, &mut s, &title);
            }
            return;
        }
        if self.view == View::Hidden {
            // The volume popup, while a volume key is in use; it takes the
            // now-playing toast's place.
            let flags = WindowFlags::NO_DECORATION
                | WindowFlags::NO_INPUTS
                | WindowFlags::ALWAYS_AUTO_RESIZE
                | WindowFlags::NO_FOCUS_ON_APPEARING
                | WindowFlags::NO_NAV;
            if self.volume_shown.is_some_and(|t| t.elapsed().as_secs_f32() < VOLUME_POPUP_SECS) {
                let [w, _] = ui.io().display_size;
                let volume = s.config.volume;
                let bar_w = 160.0 * self.scale;
                ui.window("##jukebox_volume")
                    .position([w - 24.0, 24.0], Condition::Always)
                    .position_pivot([1.0, 0.0])
                    .flags(flags)
                    .build(|| {
                        theme::backdrop(ui);
                        theme::shadow_text(ui, ACCENT, &format!("{} Volume {:.0}%", theme::NOTE, volume * 100.0));
                        ProgressBar::new(volume / 1.5)
                            .overlay_text("")
                            .size([bar_w, ui.frame_height() * 0.35])
                            .build(ui);
                    });
                return;
            }
            // Now-playing toast after a track change.
            if let (Some(since), Some(title)) = (self.toast_since, &title) {
                if since.elapsed().as_secs_f32() < TOAST_SECS {
                    let [w, _] = ui.io().display_size;
                    ui.window("##jukebox_toast")
                        .position([w - 24.0, 24.0], Condition::Always)
                        .position_pivot([1.0, 0.0])
                        .flags(
                            WindowFlags::NO_DECORATION
                                | WindowFlags::NO_INPUTS
                                | WindowFlags::ALWAYS_AUTO_RESIZE
                                | WindowFlags::NO_FOCUS_ON_APPEARING
                                | WindowFlags::NO_NAV,
                        )
                        .build(|| {
                            theme::backdrop(ui);
                            theme::shadow_text(ui, ACCENT, &format!("{} Now playing", theme::NOTE));
                            theme::shadow_text(ui, theme::WHITE, title);
                        });
                }
            }
            return;
        }

        let mut next_view = None;
        let mut toggle_settings = false;
        let mut main_rect = None;
        let mut config_changed = false;
        // Where the menu was left last time, kept on screen in case the
        // resolution has changed since.
        let [dw, dh] = ui.io().display_size;
        let size = s.config.menu_size.unwrap_or([460.0 * self.scale, 720.0 * self.scale]);
        let size = [size[0].clamp(300.0, dw.max(300.0)), size[1].clamp(200.0, dh.max(200.0))];
        let pos = s.config.menu_pos.map(|[x, y]| {
            [x.clamp(0.0, (dw - 100.0).max(0.0)), y.clamp(0.0, (dh - 60.0).max(0.0))]
        });
        let heading = self.heading;
        let song_font = self.song_font;
        let small_font = self.small_font;
        let hovered_before = self.hovered;
        let hovered_now = std::cell::Cell::new(None);
        let renaming = std::cell::RefCell::new(self.renaming.take());
        // The title bar is drawn by hand below, in the FF style; the window
        // is dragged from any empty spot instead.
        // Opened with the controller, the menu takes the focus back on the
        // spot the controller was on last time. Focusing it as it appears
        // would start over at the first item (the menu library does that
        // for windows that appear focused), so it appears unfocused and is
        // focused from inside instead.
        let pad_focus = std::mem::take(&mut self.pad_focus);
        let mut flags = WindowFlags::NO_TITLE_BAR | WindowFlags::NO_COLLAPSE;
        if pad_focus {
            flags |= WindowFlags::NO_FOCUS_ON_APPEARING;
        }
        let mut window = ui.window("Jukebox").size(size, Condition::FirstUseEver).flags(flags);
        if let Some(pos) = pos {
            window = window.position(pos, Condition::FirstUseEver);
        }
        let moving = self.pad_move != [0.0; 2];
        if let (true, Some(at)) = (moving, self.full_at) {
            window = window.position(stick_moved(ui, at, self.pad_move, [100.0, 60.0]), Condition::Always);
        }
        let full_at = std::cell::Cell::new(None);
        // The controller is inside one of the menu's lists (a child window)
        // rather than on the menu itself; B steps back out of it.
        let in_list = std::cell::Cell::new(false);
        window.build(|| {
                if pad_focus {
                    unsafe { hudhook::imgui::sys::igSetWindowFocus_Nil() };
                }
                theme::backdrop(ui);
                // Header: title on the left, mini-player and hide buttons on
                // the right.
                {
                    let _font = heading.map(|f| ui.push_font(f));
                    theme::shadow_text(ui, theme::WHITE, "JUKEBOX");
                }
                ui.same_line();
                match header_buttons(ui, &[(Icon::Settings, "Settings"), (Icon::Mini, "Mini player"), (Icon::Close, "Hide")]) {
                    Some(0) => toggle_settings = true,
                    Some(1) => next_view = Some(View::Mini),
                    Some(_) => next_view = Some(View::Hidden),
                    None => {}
                }
                main_rect = Some((ui.window_pos(), ui.window_size()));
                full_at.set(Some(ui.window_pos()));
                in_list.set(
                    !ui.is_window_focused()
                        && ui.is_window_focused_with_flags(hudhook::imgui::WindowFocusedFlags::CHILD_WINDOWS),
                );
                theme::heading_rule(ui);
                // Saved once a drag, resize or stick move is let go, not
                // every frame of it.
                if !ui.is_mouse_down(MouseButton::Left) && !moving {
                    let (p, z) = (ui.window_pos(), ui.window_size());
                    let moved = |a: Option<[f32; 2]>, b: [f32; 2]| {
                        a.is_none_or(|a| (a[0] - b[0]).abs() > 0.5 || (a[1] - b[1]).abs() > 0.5)
                    };
                    if moved(s.config.menu_pos, p) || moved(s.config.menu_size, z) {
                        s.config.menu_pos = Some(p);
                        s.config.menu_size = Some(z);
                        config_changed = true;
                    }
                }
                // Now playing.
                song_title(ui, &title, song_font, None);
                let scene = match (s.boss, s.scene, s.area) {
                    (true, ..) => "Boss battle",
                    (_, crate::config::Scene::Exploring, Some(Area::Field)) => "Exploring the field",
                    (_, crate::config::Scene::Exploring, Some(Area::Town)) => "Exploring a town",
                    (_, crate::config::Scene::Exploring, Some(Area::Dungeon)) if s.shrine => "Exploring a Vision shrine",
                    (_, crate::config::Scene::Exploring, Some(Area::Dungeon)) => "Exploring a dungeon",
                    _ => s.scene.label(),
                };
                let status = if s.held {
                    format!("{scene} - waiting while the game's music plays")
                } else {
                    scene.to_string()
                };
                ui.text_disabled(status);
                progress(ui, &mut s);
                let b = transport(ui, &mut s, song_font, [ui.frame_height() * 1.8, ui.frame_height() * 1.35]);
                ui.same_line();
                config_changed |= repeat(ui, &mut s, b);
                ui.same_line();
                let shuffle_w = ui.calc_text_size("Shuffle")[0] + ui.frame_height();
                config_changed |= theme::toggle(ui, "Shuffle", &mut s.config.shuffle, [shuffle_w, b[1]]);
                ui.spacing();
                // The settings rows sit closer together than the rest.
                let gap = ui.clone_style().item_spacing;
                let _tight = ui.push_style_var(StyleVar::ItemSpacing([gap[0], gap[1] * 0.5]));
                // Settings read left to right like the game's own menus: the
                // label, then its control filling the rest of the row.
                let label_w = ui.calc_text_size("Play during")[0] + ui.frame_height() * 1.2;
                let field = |text: &str| {
                    ui.align_text_to_frame_padding();
                    ui.text_disabled(text);
                    ui.same_line_with_pos(label_w);
                    ui.set_next_item_width(-1.0);
                };

                // One toggle per place, so any mix can be on.
                field("Play during");
                let mut play = s.config.play_in();
                let w = (ui.content_region_avail()[0] - gap[0] * 2.0) / 3.0;
                let places = [
                    ("Exploring", "Field, towns and dungeons.", &mut play.exploring),
                    ("Battles", "Replaces the battle music.", &mut play.battles),
                    ("Cutscenes", "Replaces the music of story events.\nPre-rendered movies keep their own.", &mut play.cutscenes),
                ];
                let mut toggled = false;
                for (n, (label, tip, on)) in places.into_iter().enumerate() {
                    if n > 0 {
                        ui.same_line();
                    }
                    toggled |= theme::toggle(ui, label, on, [w, ui.frame_height()]);
                    if ui.is_item_hovered() {
                        ui.tooltip_text(tip);
                    }
                }
                if toggled {
                    s.config.play_in = Some(play);
                    config_changed = true;
                }
                let choices = s.play_from_choices();
                let mut chosen = choices.iter().position(|p| *p == s.config.playlist).unwrap_or(0);
                let labels: Vec<_> = choices.iter().map(|p| p.label()).collect();
                field("Play from");
                if theme::combo(ui, "##from", &mut chosen, &labels) {
                    s.config.playlist = choices[chosen].clone();
                    s.push(Command::ListChanged);
                    config_changed = true;
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text("Which songs Next and Shuffle choose from.\nFolders inside your music folder are listed too.");
                }
                // Music by place: one row of place tabs, then the list and
                // Shuffle for the chosen one. Every place offers "Same as Play
                // from" and the lists; the areas also the game's own music.
                let shuffle_w = ui.calc_text_size("Shuffle")[0] + ui.frame_height();
                field("Music for");
                let places = ["Battles", "Field", "Towns", "Dungeons"];
                let tab_w = (ui.content_region_avail()[0] - gap[0] * 3.0) / 4.0;
                for (n, label) in places.iter().enumerate() {
                    if n > 0 {
                        ui.same_line();
                    }
                    let mut on = self.place_tab == n;
                    if theme::toggle(ui, &format!("{label}##place"), &mut on, [tab_w, ui.frame_height()]) {
                        self.place_tab = n;
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip_text(match n {
                            0 => "Music for battles. A battle starts a song from it, and your\n\
                                  exploring song picks up where it left off afterwards.\n\
                                  Game's own music is the same as Battles off under Play during.",
                            1 => "Music while exploring the open field.",
                            2 => "Music while exploring towns.",
                            _ => "Music while exploring dungeons, caves and shrines.",
                        });
                    }
                }
                let area = [None, Some(Area::Field), Some(Area::Town), Some(Area::Dungeon)][self.place_tab.min(3)];
                // For battles, the game's own music is the Battles switch
                // under Play during: picking one sets the other.
                let mut labels = vec!["Same as Play from".to_string(), "Game's own music".to_string()];
                let lists_from = labels.len();
                labels.extend(choices.iter().map(|p| p.label()));
                let mut chosen = match area {
                    None if !s.config.play_in().battles => 1,
                    None => s.config.battle_playlist.as_ref().and_then(|b| choices.iter().position(|p| p == b)).map_or(0, |i| i + lists_from),
                    Some(a) => match s.config.area_music(a) {
                        AreaMusic::PlayFrom => 0,
                        AreaMusic::Game => 1,
                        AreaMusic::List(p) => choices.iter().position(|c| c == p).map_or(0, |i| i + lists_from),
                    },
                };
                field("");
                let combo_w = ui.content_region_avail()[0] - shuffle_w - gap[0];
                if theme::combo_width(ui, "##placemusic", &mut chosen, &labels, combo_w) {
                    let list = (chosen >= lists_from).then(|| choices[chosen - lists_from].clone());
                    match area {
                        None => {
                            let mut play = s.config.play_in();
                            play.battles = chosen != 1;
                            s.config.play_in = Some(play);
                            if chosen != 1 {
                                s.config.battle_playlist = list;
                            }
                        }
                        Some(a) => {
                            *s.config.area_music_mut(a) = match (chosen, list) {
                                (_, Some(p)) => AreaMusic::List(p),
                                (1, None) => AreaMusic::Game,
                                _ => AreaMusic::PlayFrom,
                            }
                        }
                    }
                    s.push(Command::ListChanged);
                    config_changed = true;
                }
                ui.same_line();
                // A place with a list of its own shuffles on its own.
                let own_list = chosen >= lists_from;
                {
                    let _off = ui.begin_disabled(!own_list);
                    let shuffle = match area {
                        None => &mut s.config.battle_shuffle,
                        Some(a) => s.config.area_shuffle_mut(a),
                    };
                    config_changed |= theme::toggle(ui, "Shuffle##place", shuffle, [shuffle_w, ui.frame_height()]);
                }
                if ui.is_item_hovered_with_flags(ItemHoveredFlags::ALLOW_WHEN_DISABLED) {
                    ui.tooltip_text(if own_list {
                        "Shuffle this list. Separate from the main Shuffle."
                    } else {
                        "Pick a list here to shuffle it on its own.
Until then, it follows the main Shuffle."
                    });
                }
                let mut vol = s.config.volume * 100.0;
                field("Volume");
                if ui.slider_config("##volume", 0.0, 150.0).display_format("%.0f%%").build(&mut vol) {
                    s.config.volume = vol / 100.0;
                    config_changed = true;
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text("Above 100% boosts the music; very loud songs may distort.");
                }
                if let Some(e) = &s.error {
                    ui.text_colored([1.0, 0.45, 0.4, 1.0], e);
                }

                drop(_tight);
                ui.spacing();
                ui.set_next_item_width(-1.0);
                ui.input_text("##search", &mut self.search).hint("Search songs").build();
                let needle = self.search.to_lowercase();

                let mut actions = Vec::new();
                // One song row: favorite checkbox, add-to-playlist button,
                // then the clickable title.
                let row = |s: &state::Shared, i: usize, actions: &mut Vec<Action>, drag: Option<List>| {
                    let fav = s.contains(List::Favorites, i);
                    {
                        let _text = ui.push_style_color(StyleColor::Text, if fav { ACCENT } else { DIM });
                        let _bg = ui.push_style_color(StyleColor::Button, [0.0; 4]);
                        let _edge = ui.push_style_color(StyleColor::Border, [0.0; 4]);
                        let star = if fav { theme::STAR } else { theme::STAR_EMPTY };
                        if ui.small_button(format!("{star}##fav{i}")) {
                            actions.push(Action::Toggle(List::Favorites, i));
                        }
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip_text(if fav { "Remove from Favorites" } else { "Add to Favorites" });
                    }
                    ui.same_line();
                    let popup = format!("addto{i}");
                    {
                        let _text = ui.push_style_color(StyleColor::Text, DIM);
                        let _bg = ui.push_style_color(StyleColor::Button, [0.0; 4]);
                        let _edge = ui.push_style_color(StyleColor::Border, [0.0; 4]);
                        if ui.small_button(format!("+##add{i}")) {
                            ui.open_popup(&popup);
                        }
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip_text("Add to a playlist, or hide (also on right-click)");
                    }
                    ui.popup(&popup, || {
                        let _hover = ui.push_style_color(StyleColor::HeaderHovered, [0.45, 0.8, 0.95, 0.35]);
                        ui.text_disabled("Add to playlist");
                        for (p, pl) in s.config.playlists.iter().enumerate() {
                            let mut inside = s.contains(List::Custom(p), i);
                            if ui.checkbox(format!("{}##pl{p}", pl.name), &mut inside) {
                                actions.push(Action::Toggle(List::Custom(p), i));
                            }
                        }
                        if ui.selectable("New playlist with this song") {
                            actions.push(Action::Create(Some(i)));
                        }
                        ui.separator();
                        // Rename in place: the name box opens inside this menu.
                        let mut r = renaming.borrow_mut();
                        match &mut *r {
                            Some((ri, buf, fresh)) if *ri == i => {
                                if std::mem::take(fresh) {
                                    ui.set_keyboard_focus_here();
                                }
                                ui.set_next_item_width(ui.calc_text_size("M")[0] * 18.0);
                                let enter = ui.input_text("##rename", buf).enter_returns_true(true).build();
                                if enter || ui.small_button("Save") {
                                    actions.push(Action::RenameTrack(i, Some(buf.clone())));
                                    *r = None;
                                    ui.close_current_popup();
                                } else {
                                    ui.same_line();
                                    if ui.small_button("Cancel") || ui.is_key_pressed(Key::Escape) {
                                        *r = None;
                                    }
                                }
                            }
                            _ => {
                                if ui.selectable_config("Rename...").flags(SelectableFlags::DONT_CLOSE_POPUPS).build() {
                                    *r = Some((i, s.tracks[i].title.clone(), true));
                                }
                            }
                        }
                        drop(r);
                        if s.is_renamed(i) && ui.selectable("Reset name") {
                            actions.push(Action::RenameTrack(i, None));
                        }
                        ui.separator();
                        let hidden = s.contains(List::Hidden, i);
                        if ui.selectable(if hidden { "Unhide this song" } else { "Hide this song" }) {
                            actions.push(Action::Toggle(List::Hidden, i));
                        }
                    });
                    ui.same_line();
                    // The FF Resonance selection: a cyan bar under the row the
                    // mouse is on (fainter for the playing song) and the
                    // double-diamond cursor at its left. Hover is the last
                    // frame's, since the bar is drawn before the row.
                    let start = ui.cursor_screen_pos();
                    let end = [start[0] + ui.content_region_avail()[0], start[1] + ui.text_line_height_with_spacing()];
                    let current = s.current == Some(i);
                    let hovered = hovered_before == Some(i);
                    if current || hovered {
                        theme::row_bar(ui, start, end, hovered);
                    } else {
                        theme::row_rule(ui, start, end);
                    }
                    let note = if current { format!("{} ", theme::NOTE) } else { String::new() };
                    let label = format!("      {note}{}##{i}", s.tracks[i].title);
                    if ui.selectable_config(label).build() {
                        actions.push(Action::Play(i));
                    }
                    // In a list you order yourself, a song is dragged by its
                    // title onto another song to move it there; a click still
                    // plays it.
                    if let Some(list) = drag {
                        if let Some(tip) = ui.drag_drop_source_config("JUKEBOX_SONG").begin_payload(i) {
                            ui.text(&s.tracks[i].title);
                            tip.end();
                        }
                        if let Some(target) = ui.drag_drop_target() {
                            if let Some(Ok(drop)) = target.accept_payload::<usize, _>("JUKEBOX_SONG", DragDropFlags::empty()) {
                                actions.push(Action::MoveTo(list, drop.data, i));
                            }
                            target.pop();
                        }
                    }
                    if ui.is_item_hovered() {
                        hovered_now.set(Some(i));
                    }
                    if ui.is_item_clicked_with_button(MouseButton::Right) {
                        ui.open_popup(&popup);
                    }
                    if hovered {
                        let h = ui.text_line_height();
                        theme::cursor(ui, [start[0] + h * 0.6, (start[1] + end[1]) * 0.5 - 1.0], h * 0.38);
                    }
                };
                // A list in play order, with up/down buttons and, for your own
                // playlists, a remove button.
                let ordered = |s: &state::Shared, list: List, actions: &mut Vec<Action>| {
                    for i in s.indices(list) {
                        if !needle.is_empty() && !s.tracks[i].title.to_lowercase().contains(&needle) {
                            continue;
                        }
                        if let List::Custom(_) = list {
                            if ui.small_button(format!("x##rm{i}")) {
                                actions.push(Action::Toggle(list, i));
                            }
                            if ui.is_item_hovered() {
                                ui.tooltip_text("Remove from this playlist");
                            }
                            ui.same_line();
                        }
                        row(s, i, actions, Some(list));
                    }
                };
                let list_height = [0.0, -ui.frame_height_with_spacing()];
                // Tab labels in the small font, so all six fit across.
                let tab = |name: &str| {
                    let _font = small_font.map(|f| ui.push_font(f));
                    ui.tab_item(name)
                };
                if let Some(_bar) = ui.tab_bar_with_flags("sources", TabBarFlags::FITTING_POLICY_SCROLL) {
                    if let Some(_tab) = tab("Favorites") {
                        if s.indices(List::Favorites).is_empty() {
                            ui.text_disabled(format!("Click the {} next to any song to add it here.", theme::STAR_EMPTY));
                        }
                        ui.child_window("favs").size(list_height).build(|| {
                            ordered(&s, List::Favorites, &mut actions);
                        });
                    }
                    if let Some(_tab) = tab("Playlists") {
                        // A playlist that was opened may since have been
                        // renamed or deleted, or its folder emptied.
                        if let Some(open) = &self.open_list {
                            if !s.play_from_choices().contains(open) {
                                self.open_list = None;
                            }
                        }
                        match self.open_list.clone() {
                            None => self.playlist_cards(ui, &s, &mut actions, list_height),
                            Some(open) => {
                                self.playlist_page(ui, &s, &open, &mut actions, list_height, song_font, &row)
                            }
                        }
                    }
                    // Game themes loop until stopped; the short pieces are
                    // the one-shot fanfares and stingers.
                    let tabs: [(&str, fn(&state::Track) -> bool); 3] = [
                        ("Themes", |t| t.is_game_loop()),
                        ("Jingles", |t| t.is_game() && !t.is_game_loop()),
                        ("My Music", |t| !t.is_game()),
                    ];
                    for (label, shows) in tabs {
                        let Some(_tab) = tab(label) else { continue };
                        let game = label != "My Music";
                        if game && !s.game_tracks_ready {
                            ui.text_disabled("The game's songs appear once a save is loaded.");
                        }
                        if let (true, Some((done, total))) = (game, s.check_progress) {
                            ui.text_disabled(format!("Checking songs for missing audio: {done} / {total}"));
                        }
                        if label == "Themes" {
                            let mut mins = s.config.loop_minutes;
                            field("Per theme");
                            if ui.slider_config("##mins", 1.0, 15.0).display_format("%.0f min").build(&mut mins) {
                                s.config.loop_minutes = mins;
                                config_changed = true;
                            }
                            if ui.is_item_hovered() {
                                ui.tooltip_text(
                                    "Themes loop, so each one plays this long, then fades\n\
                                     out into the next song. Applies from the next theme.",
                                );
                            }
                        }
                        if !game {
                            if ui.small_button("Open music folder") {
                                let _ = std::fs::create_dir_all(&s.music_dir);
                                if let Err(e) = std::process::Command::new("explorer.exe").arg(&s.music_dir).spawn() {
                                    log::error!("menu: could not open {}: {e}", s.music_dir.display());
                                }
                            }
                            if ui.is_item_hovered() {
                                ui.tooltip_text(format!(
                                    "Put your songs here, then Rescan folder.\n{}",
                                    s.music_dir.display()
                                ));
                            }
                            ui.same_line();
                            if ui.small_button("Rescan folder") {
                                s.push(Command::Rescan);
                            }
                            // Songs are levelled to the game's music by
                            // themselves; this nudges all of them.
                            let mut level = s.config.my_music_level_db;
                            field("Level");
                            if ui.slider_config("##level", -12.0, 6.0).display_format("%+.0f dB").build(&mut level) {
                                s.config.my_music_level_db = level.round();
                                config_changed = true;
                            }
                            if ui.is_item_hovered() {
                                ui.tooltip_text(
                                    "Your songs are measured and levelled to match the game's\n\
                                     music, and follow the game's own Music volume.\n\
                                     Use this to make all of them a little louder or quieter.",
                                );
                            }
                        }
                        ui.child_window("list").size(list_height).build(|| {
                            for i in 0..s.tracks.len() {
                                let t = &s.tracks[i];
                                if !shows(t) || s.contains(List::Hidden, i) || s.is_unplayable(i) {
                                    continue;
                                }
                                if !needle.is_empty() && !t.title.to_lowercase().contains(&needle) {
                                    continue;
                                }
                                row(&s, i, &mut actions, None);
                            }
                        });
                    }
                    if let Some(_tab) = tab("Hidden") {
                        let hidden = s.indices(List::Hidden);
                        if hidden.is_empty() {
                            ui.text_disabled("Nothing hidden. Hide a song from its + menu or by right-clicking it.");
                        } else if ui.small_button("Unhide all") {
                            actions.push(Action::UnhideAll);
                        }
                        // Leaves a row under the list for the song check.
                        let short = [0.0, list_height[1] - ui.frame_height_with_spacing()];
                        ui.child_window("hidden").size(short).build(|| {
                            for i in hidden {
                                if !needle.is_empty() && !s.tracks[i].title.to_lowercase().contains(&needle) {
                                    continue;
                                }
                                if ui.small_button(format!("Unhide##uh{i}")) {
                                    actions.push(Action::Toggle(List::Hidden, i));
                                }
                                ui.same_line();
                                row(&s, i, &mut actions, None);
                            }
                        });
                        // Rarely needed, so it lives here rather than over the
                        // song lists.
                        match s.check_progress {
                            Some((done, total)) => {
                                ui.text_disabled(format!("Checking songs for missing audio: {done} / {total}"))
                            }
                            None if s.game_tracks_ready => {
                                if ui.small_button("Check songs again") {
                                    s.push(Command::CheckSongs);
                                }
                                if ui.is_item_hovered() {
                                    ui.tooltip_text(
                                        "Silently plays every game song for a moment and\n\
                                         removes any with no audio. Runs by itself once\n\
                                         per game version.",
                                    );
                                }
                            }
                            None => {}
                        }
                    }
                }
                for action in actions {
                    match action {
                        Action::Play(i) => s.push(Command::Play(i)),
                        Action::Toggle(list, i) => s.toggle(list, i),
                        Action::Create(song) => {
                            let p = s.create_playlist("New playlist");
                            if let Some(i) = song {
                                s.toggle(List::Custom(p), i);
                            }
                        }
                        Action::CreateNamed(name) => {
                            s.create_playlist(&name);
                        }
                        Action::Rename(p, name) => {
                            let old = s.config.playlists.get(p).map(|pl| Playlist::Custom(pl.name.clone()));
                            s.rename_playlist(p, &name);
                            // A playlist renamed while open stays open.
                            if old.is_some() && self.open_list == old {
                                self.open_list = s.config.playlists.get(p).map(|pl| Playlist::Custom(pl.name.clone()));
                            }
                        }
                        Action::Delete(p) => s.delete_playlist(p),
                        Action::UnhideAll => s.unhide_all(),
                        Action::RenameTrack(i, name) => s.rename_track(i, name.as_deref()),
                        Action::MoveTo(list, from, to) => s.move_to(list, from, to),
                        Action::PlayFrom(list, shuffle) => {
                            s.config.playlist = list.clone();
                            config_changed = true;
                            if shuffle {
                                s.config.shuffle = true;
                                s.push(Command::Next);
                            } else if let Some(&first) = s.songs_of(&list, None).first() {
                                s.push(Command::Play(first));
                            }
                        }
                    }
                }
                // Which keys step on from here, centred at the foot.
                if !s.config.hide_key_hint {
                    let mut hint = keys::name(key);
                    if s.config.controller {
                        hint = format!("{hint}  or  {} + {}", s.config.pad_hold.label(), s.config.pad_menu.label());
                    }
                    let hint = format!("{hint}  \u{00B7}  mini player");
                    let _font = small_font.map(|f| ui.push_font(f));
                    let w = ui.calc_text_size(&hint)[0];
                    let avail = ui.content_region_avail()[0];
                    if w < avail {
                        ui.set_cursor_pos([ui.cursor_pos()[0] + (avail - w) / 2.0, ui.cursor_pos()[1]]);
                    }
                    ui.text_disabled(hint);
                }
            });
        if let Some(v) = next_view {
            self.view = v;
        }
        if let Some(at) = full_at.get() {
            self.full_at = Some(at);
        }
        // B closes a popup first (the menu does that itself), then the
        // settings; then it's the Back button where one shows; then it backs
        // out of a list (the menu again), and last steps on to the mini
        // player like the menu key.
        let back = std::mem::take(&mut self.pad_back) && !any_popup_open() && !ui.is_any_item_active();
        let back_shown = std::mem::take(&mut self.back_shown);
        if back && self.settings_open {
            self.settings_open = false;
            self.capturing = None;
            self.pad_capturing = None;
            self.pad_swallow_b = true;
        } else if back && back_shown {
            // Where the menu shows a Back button, B presses it, even from
            // inside the list under it.
            self.open_list = None;
            self.pad_swallow_b = true;
        } else if back && !in_list.get() {
            if self.pad_nav {
                self.pad_nav = false;
                self.view = self.view.next();
                self.pad_swallow_b = true;
            }
        }
        self.hovered = hovered_now.get();
        self.renaming = renaming.into_inner();
        if config_changed {
            s.save_config();
        }
        if toggle_settings {
            self.settings_open = !self.settings_open;
            self.capturing = None;
        }
        // Settings open beside the menu, or inside its right edge if the
        // menu is against the right of the screen.
        if let (true, Some(([x, y], [w, _]))) = (self.settings_open && self.view == View::Full, main_rect) {
            let gap = 10.0 * self.scale;
            let want = 300.0 * self.scale;
            let at = if x + w + gap + want < dw { [x + w + gap, y] } else { [(x - want - gap).max(0.0), y] };
            self.settings(ui, &mut s, at);
        }
    }

    /// The game keeps its input while the menu is open, so play goes on. The
    /// menu takes the mouse only while it is over the window, and the
    /// keyboard only while the search box is being typed in. The mini player
    /// takes only clicks on itself, never the raw mouse movement the camera
    /// uses, since the game's hidden cursor can rest over it while exploring.
    fn message_filter(&self, io: &Io) -> MessageFilter {
        let mut filter = MessageFilter::empty();
        // A key being rebound must not also reach the game.
        if self.capturing.is_some() {
            filter |= MessageFilter::InputKeyboard;
        }
        match self.view {
            View::Hidden => return filter,
            View::Mini => {
                if io.want_capture_mouse {
                    filter |= MessageFilter::InputMouse;
                }
                return filter;
            }
            View::Full => {}
        }
        if io.want_capture_mouse {
            filter |= MessageFilter::InputMouse | MessageFilter::InputRaw;
        }
        if io.want_text_input {
            filter |= MessageFilter::InputKeyboard | MessageFilter::InputRaw;
        }
        filter
    }
}
