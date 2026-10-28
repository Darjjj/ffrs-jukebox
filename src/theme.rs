//! FF Resonance's own menu look: see-through teal panels with thin
//! silver-cyan lines, a serif capital heading over a rule, clean white text,
//! and a cyan bar fading to the right under a double-diamond cursor for the
//! selected entry.

use hudhook::imgui::{
    ComboBoxFlags, Context, FontConfig, FontGlyphRanges, FontId, FontSource, StyleColor, Ui,
};

/// The cyan of the selection bar and the MP gauges.
pub const ACCENT: [f32; 4] = [0.55, 0.86, 1.0, 1.0];
pub const DIM: [f32; 4] = [0.66, 0.78, 0.88, 1.0];
pub const WHITE: [f32; 4] = [0.97, 0.98, 1.0, 1.0];
const SHADOW: [f32; 4] = [0.0, 0.04, 0.1, 0.8];
const LINE: [f32; 4] = [0.78, 0.9, 0.98, 0.75];
const TOP: [f32; 4] = [0.07, 0.24, 0.36, 0.86];
const BOTTOM: [f32; 4] = [0.03, 0.11, 0.2, 0.9];
const CLEAR: [f32; 4] = [0.0; 4];

pub const NOTE: &str = "\u{266A}";
pub const STAR: &str = "\u{2605}";
pub const STAR_EMPTY: &str = "\u{2606}";
pub const PREV: &str = "\u{23EE}";
pub const PLAY: &str = "\u{25B6}";
pub const PAUSE: &str = "\u{23F8}";
pub const NEXT: &str = "\u{23ED}";
pub const STOP: &str = "\u{25A0}";
pub const MORE: &str = "\u{22EF}";
pub const SHUFFLE: &str = "\u{21C4}";
pub const CLOSE_SMALL: &str = "\u{2715}";

/// The symbols above, from Segoe UI Symbol.
static SYMBOLS: [u32; 9] = [0x2190, 0x23FF, 0x25A0, 0x25FF, 0x2600, 0x26FF, 0x2700, 0x27BF, 0];

pub struct Fonts {
    pub heading: Option<FontId>,
    /// The now-playing song title, and the transport icons.
    pub song: Option<FontId>,
    /// The tab labels, so all of them fit across the window.
    pub small: Option<FontId>,
}

fn read(name: &str) -> Option<&'static [u8]> {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let data = std::fs::read(format!(r"{dir}\Fonts\{name}")).ok()?;
    Some(Box::leak(data.into_boxed_slice()))
}

/// Segoe UI for text (the first font added is the default), Palatino for the
/// heading, Segoe UI Symbol merged into both for the icons. Rasterised at the
/// menu's scale so they stay sharp. Falls back to imgui's own font when the
/// Windows fonts are missing.
pub fn load_fonts(ctx: &mut Context, scale: f32) -> Fonts {
    let symbols = read("seguisym.ttf");
    let mut add = |file: &str, size: f32| -> Option<FontId> {
        let text = read(file)?;
        let mut sources = vec![FontSource::TtfData {
            data: text,
            size_pixels: size * scale,
            config: Some(FontConfig { oversample_h: 2, ..FontConfig::default() }),
        }];
        if let Some(sym) = symbols {
            sources.push(FontSource::TtfData {
                data: sym,
                size_pixels: size * scale,
                config: Some(FontConfig {
                    glyph_ranges: FontGlyphRanges::from_slice(&SYMBOLS),
                    ..FontConfig::default()
                }),
            });
        }
        Some(ctx.fonts().add_font(&sources))
    };
    let body = add("segoeui.ttf", 20.0);
    let heading = add("pala.ttf", 34.0);
    let song = add("pala.ttf", 26.0);
    let small = add("segoeui.ttf", 16.5);
    if body.is_none() {
        log::warn!("theme: Windows fonts not found, using the default font");
        ctx.io_mut().font_global_scale = scale;
    }
    Fonts { heading, song, small }
}

pub fn apply(ctx: &mut Context, scale: f32) {
    let style = ctx.style_mut();
    style.window_rounding = 3.0;
    style.child_rounding = 0.0;
    style.frame_rounding = 2.0;
    style.popup_rounding = 3.0;
    style.grab_rounding = 2.0;
    style.tab_rounding = 2.0;
    style.scrollbar_rounding = 2.0;
    // The window outline is drawn by `backdrop`, over the gradient.
    style.window_border_size = 0.0;
    style.child_border_size = 0.0;
    style.frame_border_size = 1.0;
    style.popup_border_size = 1.0;
    style.tab_border_size = 1.0;
    style.window_padding = [16.0, 12.0];
    style.frame_padding = [8.0, 4.0];
    style.item_spacing = [8.0, 6.0];
    style.scrollbar_size = 8.0;
    style.grab_min_size = 10.0;
    style.scale_all_sizes(scale);

    let c = &mut style.colors;
    let rgba = |r: f32, g: f32, b: f32, a: f32| [r, g, b, a];
    c[StyleColor::Text as usize] = WHITE;
    c[StyleColor::TextDisabled as usize] = DIM;
    c[StyleColor::WindowBg as usize] = CLEAR;
    c[StyleColor::ChildBg as usize] = CLEAR;
    c[StyleColor::PopupBg as usize] = rgba(0.04, 0.15, 0.25, 0.97);
    c[StyleColor::Border as usize] = rgba(0.7, 0.85, 0.95, 0.45);
    c[StyleColor::BorderShadow as usize] = CLEAR;
    c[StyleColor::FrameBg as usize] = rgba(0.02, 0.1, 0.17, 0.7);
    c[StyleColor::FrameBgHovered as usize] = rgba(0.1, 0.3, 0.42, 0.8);
    c[StyleColor::FrameBgActive as usize] = rgba(0.16, 0.42, 0.56, 0.9);
    c[StyleColor::TitleBg as usize] = CLEAR;
    c[StyleColor::TitleBgActive as usize] = CLEAR;
    c[StyleColor::TitleBgCollapsed as usize] = CLEAR;
    c[StyleColor::ScrollbarBg as usize] = CLEAR;
    c[StyleColor::ScrollbarGrab as usize] = rgba(0.6, 0.8, 0.92, 0.45);
    c[StyleColor::ScrollbarGrabHovered as usize] = rgba(0.7, 0.88, 0.98, 0.7);
    c[StyleColor::ScrollbarGrabActive as usize] = ACCENT;
    c[StyleColor::CheckMark as usize] = ACCENT;
    c[StyleColor::SliderGrab as usize] = ACCENT;
    c[StyleColor::SliderGrabActive as usize] = WHITE;
    c[StyleColor::Button as usize] = rgba(0.06, 0.2, 0.3, 0.75);
    c[StyleColor::ButtonHovered as usize] = rgba(0.2, 0.48, 0.62, 0.85);
    c[StyleColor::ButtonActive as usize] = rgba(0.35, 0.68, 0.84, 0.95);
    // Song rows draw their own cyan bar; imgui's flat highlight stays off.
    c[StyleColor::Header as usize] = CLEAR;
    c[StyleColor::HeaderHovered as usize] = CLEAR;
    c[StyleColor::HeaderActive as usize] = rgba(0.55, 0.86, 1.0, 0.15);
    c[StyleColor::Separator as usize] = rgba(0.75, 0.88, 0.97, 0.35);
    c[StyleColor::ResizeGrip as usize] = rgba(0.75, 0.88, 0.97, 0.2);
    c[StyleColor::ResizeGripHovered as usize] = rgba(0.75, 0.88, 0.97, 0.5);
    c[StyleColor::ResizeGripActive as usize] = ACCENT;
    c[StyleColor::Tab as usize] = rgba(0.04, 0.15, 0.24, 0.6);
    c[StyleColor::TabHovered as usize] = rgba(0.25, 0.55, 0.7, 0.8);
    c[StyleColor::TabActive as usize] = rgba(0.3, 0.62, 0.78, 0.85);
    c[StyleColor::TabUnfocused as usize] = rgba(0.04, 0.15, 0.24, 0.6);
    c[StyleColor::TabUnfocusedActive as usize] = rgba(0.2, 0.45, 0.6, 0.8);
    c[StyleColor::PlotHistogram as usize] = rgba(0.35, 0.72, 0.98, 1.0);
    c[StyleColor::TextSelectedBg as usize] = rgba(0.4, 0.7, 0.9, 0.45);
    c[StyleColor::NavHighlight as usize] = ACCENT;
}

/// The teal panel and its thin outline behind the current window, on the
/// background layer so the window's contents sit on top.
pub fn backdrop(ui: &Ui) {
    let [x, y] = ui.window_pos();
    let [w, h] = ui.window_size();
    let draw = ui.get_background_draw_list();
    draw.add_rect_filled_multicolor([x + 1.0, y + 1.0], [x + w - 1.0, y + h - 1.0], TOP, TOP, BOTTOM, BOTTOM);
    draw.add_rect([x, y], [x + w, y + h], LINE).rounding(3.0).thickness(1.2).build();
    // A second line just inside, like the double frames on the party cards.
    draw.add_rect([x + 4.0, y + 4.0], [x + w - 4.0, y + h - 4.0], [LINE[0], LINE[1], LINE[2], 0.25])
        .thickness(1.0)
        .build();
}

/// Text with a soft shadow, at the cursor.
pub fn shadow_text(ui: &Ui, color: [f32; 4], text: &str) {
    let [x, y] = ui.cursor_screen_pos();
    ui.get_window_draw_list().add_text([x + 1.5, y + 1.5], SHADOW, text);
    ui.text_colored(color, text);
}

/// A rule across the window under a heading, bright at the left and fading
/// out, with a small diamond where it starts, like "MAIN MENU".
pub fn heading_rule(ui: &Ui) {
    let [x, y] = ui.cursor_screen_pos();
    let w = ui.content_region_avail()[0];
    let draw = ui.get_window_draw_list();
    let faded = [LINE[0], LINE[1], LINE[2], 0.05];
    draw.add_rect_filled_multicolor([x, y], [x + w, y + 1.5], LINE, faded, faded, LINE);
    let d = 3.5;
    draw.add_polyline(vec![[x, y - d], [x + d, y], [x, y + d], [x - d, y]], WHITE).filled(true).build();
    drop(draw);
    ui.dummy([w, 6.0]);
}

/// The selection bar behind a song row: cyan at the left fading to clear,
/// with a thin bright line along its bottom. `strong` is the hovered row;
/// the playing song gets a fainter bar.
pub fn row_bar(ui: &Ui, min: [f32; 2], max: [f32; 2], strong: bool) {
    let draw = ui.get_window_draw_list();
    let a = if strong { 0.55 } else { 0.25 };
    let left = [0.45, 0.8, 0.95, a];
    let right = [0.45, 0.8, 0.95, 0.0];
    draw.add_rect_filled_multicolor(min, max, left, right, right, left);
    let line = [0.8, 0.95, 1.0, if strong { 0.9 } else { 0.4 }];
    draw.add_rect_filled_multicolor([min[0], max[1] - 1.0], max, line, right, right, line);
}

/// A thin divider under a row, fading to the right, like the main menu's.
pub fn row_rule(ui: &Ui, min: [f32; 2], max: [f32; 2]) {
    let faint = [0.75, 0.88, 0.97, 0.18];
    let clear = [0.75, 0.88, 0.97, 0.0];
    ui.get_window_draw_list().add_rect_filled_multicolor([min[0], max[1]], [max[0], max[1] + 1.0], faint, clear, clear, faint);
}

/// A dropdown in the game's style: no arrow box, just a thin cyan chevron at
/// the right of the field. Fills the rest of the row.
pub fn combo<S: AsRef<str>>(ui: &Ui, id: &str, current: &mut usize, items: &[S]) -> bool {
    combo_width(ui, id, current, items, ui.content_region_avail()[0])
}

/// [`combo`], `w` wide, leaving room on the row for something after it.
pub fn combo_width<S: AsRef<str>>(ui: &Ui, id: &str, current: &mut usize, items: &[S], w: f32) -> bool {
    let [x, y] = ui.cursor_screen_pos();
    let h = ui.frame_height();
    let preview = items.get(*current).map_or("", |s| s.as_ref());
    let mut changed = false;
    ui.set_next_item_width(w);
    if let Some(_combo) = ui.begin_combo_with_flags(id, preview, ComboBoxFlags::NO_ARROW_BUTTON) {
        // The song rows keep imgui's flat highlight off; a dropdown needs it.
        let _hover = ui.push_style_color(StyleColor::HeaderHovered, [0.45, 0.8, 0.95, 0.35]);
        let _sel = ui.push_style_color(StyleColor::Header, [0.45, 0.8, 0.95, 0.2]);
        for (i, item) in items.iter().enumerate() {
            if ui.selectable_config(item.as_ref()).selected(i == *current).build() {
                *current = i;
                changed = true;
            }
        }
    }
    // Drawn on the top layer, so it sits over the field even while the list
    // is open (and the popup has become the current window).
    let (cx, cy, r) = (x + w - h * 0.5, y + h * 0.5, h * 0.16);
    ui.get_foreground_draw_list()
        .add_polyline(vec![[cx - r * 1.4, cy - r * 0.6], [cx, cy + r * 0.8], [cx + r * 1.4, cy - r * 0.6]], ACCENT)
        .thickness(1.6)
        .build();
    changed
}

/// An on/off button: outlined when off, lit cyan when on.
pub fn toggle(ui: &Ui, label: &str, on: &mut bool, size: [f32; 2]) -> bool {
    let (bg, text) = if *on {
        ([0.35, 0.68, 0.84, 0.75], WHITE)
    } else {
        ([0.0, 0.0, 0.0, 0.0], DIM)
    };
    let _bg = ui.push_style_color(StyleColor::Button, bg);
    let _text = ui.push_style_color(StyleColor::Text, text);
    let clicked = ui.button_with_size(label, size);
    if clicked {
        *on = !*on;
    }
    clicked
}

/// The header buttons' icons, drawn as thin lines so they stay crisp at any
/// size.
#[derive(Clone, Copy)]
pub enum Icon {
    /// A speaker with sound waves for the volume (0.0 - 1.5); crossed out
    /// at zero.
    Volume(f32),
    Settings,
    /// Shrink to the mini player: a window with only its bottom bar filled.
    Mini,
    /// Back to the full menu: a whole window.
    Expand,
    Close,
}

/// A square icon button: no box until hovered, then a soft cyan fill with a
/// thin bright outline; the icon brightens to white, and cyan while held.
pub fn icon_button(ui: &Ui, id: &str, icon: Icon, size: f32) -> bool {
    let p = ui.cursor_screen_pos();
    let clicked = ui.invisible_button(id, [size, size]);
    let hovered = ui.is_item_hovered();
    let held = ui.is_item_active();
    let draw = ui.get_window_draw_list();
    let q = [p[0] + size, p[1] + size];
    if hovered {
        draw.add_rect(p, q, [0.45, 0.8, 0.95, if held { 0.45 } else { 0.22 }]).filled(true).rounding(3.0).build();
        draw.add_rect(p, q, [0.8, 0.95, 1.0, 0.75]).rounding(3.0).thickness(1.0).build();
    }
    let col = if held { ACCENT } else if hovered { WHITE } else { DIM };
    let (cx, cy) = (p[0] + size * 0.5, p[1] + size * 0.5);
    let r = size * 0.26;
    let t = (size * 0.06).max(1.4);
    match icon {
        Icon::Volume(level) => {
            // The speaker: a small box and its cone, left of centre.
            let x = cx - r * 0.55;
            draw.add_rect([x - r * 0.75, cy - r * 0.38], [x - r * 0.2, cy + r * 0.38], col).filled(true).build();
            draw.add_triangle([x - r * 0.25, cy - r * 0.38], [x + r * 0.45, cy - r * 0.95], [x + r * 0.45, cy + r * 0.95], col)
                .filled(true)
                .build();
            draw.add_triangle([x - r * 0.25, cy - r * 0.38], [x + r * 0.45, cy + r * 0.95], [x - r * 0.25, cy + r * 0.38], col)
                .filled(true)
                .build();
            if level <= 0.0 {
                let (a, b) = (x + r * 0.85, x + r * 1.65);
                draw.add_line([a, cy - r * 0.4], [b, cy + r * 0.4], col).thickness(t).build();
                draw.add_line([a, cy + r * 0.4], [b, cy - r * 0.4], col).thickness(t).build();
            } else {
                // One wave per 50% of volume: three above 100%.
                let waves = ((level / 0.5).ceil() as usize).clamp(1, 3);
                for w in 0..waves {
                    let rad = r * (0.55 + 0.42 * w as f32);
                    let pts: Vec<[f32; 2]> = (0..=8)
                        .map(|k| {
                            let a = -0.85 + 1.7 * k as f32 / 8.0;
                            [x + r * 0.45 + rad * a.cos() * 0.9, cy + rad * a.sin()]
                        })
                        .collect();
                    draw.add_polyline(pts, col).thickness(t * 0.9).build();
                }
            }
        }
        Icon::Close => {
            draw.add_line([cx - r, cy - r], [cx + r, cy + r], col).thickness(t).build();
            draw.add_line([cx - r, cy + r], [cx + r, cy - r], col).thickness(t).build();
        }
        Icon::Mini => {
            let (a, b) = ([cx - r * 1.2, cy - r * 0.85], [cx + r * 1.2, cy + r * 0.85]);
            draw.add_rect(a, b, col).rounding(1.5).thickness(t * 0.8).build();
            draw.add_rect([a[0], cy + r * 0.2], b, col).filled(true).rounding(1.5).build();
        }
        Icon::Expand => {
            let (a, b) = ([cx - r * 1.1, cy - r * 1.0], [cx + r * 1.1, cy + r * 1.0]);
            draw.add_rect(a, b, col).rounding(1.5).thickness(t * 0.8).build();
            draw.add_line([a[0], a[1] + r * 0.55], [b[0], a[1] + r * 0.55], col).thickness(t * 0.8).build();
        }
        Icon::Settings => {
            draw.add_circle([cx, cy], r * 0.5, col).thickness(t).build();
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let (s, c) = a.sin_cos();
                draw.add_line([cx + c * r * 0.72, cy + s * r * 0.72], [cx + c * r * 1.12, cy + s * r * 1.12], col)
                    .thickness(t * 1.3)
                    .build();
            }
        }
    }
    clicked
}

/// The repeat button, sized like the transport buttons: a near-full circle
/// ending in an arrowhead, dimmed when off, lit for the playlist, and lit with
/// a small 1 inside for one song.
pub fn repeat_button(ui: &Ui, id: &str, on: bool, one: bool, size: [f32; 2]) -> bool {
    let p = ui.cursor_screen_pos();
    let (bg, col) = if on { ([0.35, 0.68, 0.84, 0.75], WHITE) } else { ([0.0, 0.0, 0.0, 0.0], DIM) };
    let clicked = {
        let _bg = ui.push_style_color(StyleColor::Button, bg);
        ui.button_with_size(id, size)
    };
    let draw = ui.get_window_draw_list();
    let (cx, cy) = (p[0] + size[0] * 0.5, p[1] + size[1] * 0.5);
    let r = size[1] * 0.24;
    let t = (size[1] * 0.07).max(1.4);
    // Clockwise from the right, leaving a gap at the top right for the head.
    let (start, end) = (0.1 * std::f32::consts::PI, 1.7 * std::f32::consts::PI);
    let pts: Vec<[f32; 2]> = (0..=24)
        .map(|k| {
            let a = start + (end - start) * k as f32 / 24.0;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect();
    draw.add_polyline(pts, col).thickness(t).build();
    // The arrowhead, pointing on along the circle.
    let (s, c) = end.sin_cos();
    let tip = [cx + r * c - s * r * 0.55, cy + r * s + c * r * 0.55];
    let (w, back) = (r * 0.42, r * 0.05);
    draw.add_triangle(
        [cx + (r + w) * c + s * back, cy + (r + w) * s - c * back],
        [cx + (r - w) * c + s * back, cy + (r - w) * s - c * back],
        tip,
        col,
    )
    .filled(true)
    .build();
    // A small 1, drawn as lines, tucked against the circle's bottom right.
    if one {
        let (h, x, y) = (r * 0.38, cx + r * 1.3, cy + r * 0.8);
        let tt = t * 0.75;
        draw.add_line([x, y - h], [x, y + h], col).thickness(tt).build();
        draw.add_line([x, y - h], [x - h * 0.6, y - h * 0.45], col).thickness(tt).build();
    }
    clicked
}

/// The double-diamond cursor, centred on `at`.
pub fn cursor(ui: &Ui, at: [f32; 2], size: f32) {
    let [x, y] = at;
    let draw = ui.get_window_draw_list();
    for (r, color, thick) in [(size, ACCENT, 1.6), (size * 0.55, WHITE, 1.4)] {
        draw.add_polyline(vec![[x, y - r], [x + r, y], [x, y + r], [x - r, y], [x, y - r]], color)
            .thickness(thick)
            .build();
    }
}
