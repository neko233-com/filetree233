//! One cohesive dark appearance for the desktop app: palette, fonts, spacing.
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
    Stroke, TextStyle, Visuals,
};

pub const BG: Color32 = Color32::from_rgb(16, 20, 25);
pub const PANEL: Color32 = Color32::from_rgb(22, 27, 34);
pub const SURFACE: Color32 = Color32::from_rgb(30, 37, 46);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(39, 49, 60);
pub const STROKE: Color32 = Color32::from_rgb(48, 60, 73);
pub const TEXT: Color32 = Color32::from_rgb(230, 237, 243);
pub const MUTED: Color32 = Color32::from_rgb(154, 169, 184);
pub const FAINT: Color32 = Color32::from_rgb(108, 124, 140);
pub const ACCENT: Color32 = Color32::from_rgb(63, 184, 160);
pub const ACCENT_STRONG: Color32 = Color32::from_rgb(87, 207, 182);
pub const ACCENT_INK: Color32 = Color32::from_rgb(8, 20, 17);
pub const DANGER: Color32 = Color32::from_rgb(212, 106, 94);
pub const AMBER: Color32 = Color32::from_rgb(224, 168, 76);
pub const HATCH: Color32 = Color32::from_rgb(232, 226, 210);

/// Apply fonts, text sizes, spacing and the dark palette to the whole context.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    let mut style = (*ctx.style()).clone();
    style.text_styles = text_styles();
    style.spacing = spacing();
    style.visuals = visuals();
    ctx.set_style(style);
}

fn text_styles() -> std::collections::BTreeMap<TextStyle, FontId> {
    [
        (TextStyle::Heading, FontId::proportional(20.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Small, FontId::proportional(11.5)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into()
}

fn spacing() -> egui::Spacing {
    let mut spacing = egui::Spacing::default();
    spacing.item_spacing = egui::vec2(8.0, 6.0);
    spacing.button_padding = egui::vec2(10.0, 6.0);
    spacing.window_margin = Margin::same(12);
    spacing.menu_margin = Margin::same(6);
    spacing.indent = 18.0;
    spacing.interact_size = egui::vec2(44.0, 26.0);
    spacing.combo_width = 190.0;
    spacing.slider_width = 140.0;
    spacing.scroll = egui::style::ScrollStyle {
        bar_width: 8.0,
        ..Default::default()
    };
    spacing
}

fn visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    visuals.dark_mode = true;
    visuals.panel_fill = PANEL;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = SURFACE;
    visuals.code_bg_color = BG;
    visuals.override_text_color = Some(TEXT);
    visuals.weak_text_color = Some(MUTED);
    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = AMBER;
    visuals.error_fg_color = DANGER;
    visuals.window_stroke = Stroke::new(1.0, STROKE);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(130),
    };
    visuals.popup_shadow = visuals.window_shadow;
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.30);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT_STRONG);
    visuals.text_cursor.stroke = Stroke::new(1.5, ACCENT_STRONG);
    visuals.striped = true;
    visuals.slider_trailing_fill = true;
    visuals.indent_has_left_vline = false;
    visuals.clip_rect_margin = 4.0;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = PANEL;
    widgets.noninteractive.weak_bg_fill = PANEL;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, STROKE);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.noninteractive.corner_radius = CornerRadius::same(6);

    widgets.inactive.bg_fill = SURFACE;
    widgets.inactive.weak_bg_fill = SURFACE;
    widgets.inactive.bg_stroke = Stroke::new(1.0, STROKE);
    widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.inactive.corner_radius = CornerRadius::same(6);

    widgets.hovered.bg_fill = SURFACE_HOVER;
    widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.65));
    widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.hovered.corner_radius = CornerRadius::same(6);

    widgets.active.bg_fill = ACCENT.gamma_multiply(0.55).to_opaque();
    widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.55).to_opaque();
    widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.active.corner_radius = CornerRadius::same(6);

    widgets.open.bg_fill = SURFACE_HOVER;
    widgets.open.weak_bg_fill = SURFACE_HOVER;
    widgets.open.bg_stroke = Stroke::new(1.0, STROKE);
    widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.open.corner_radius = CornerRadius::same(6);
    visuals
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut proportional = Vec::new();
    let mut monospace = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
            fonts
                .font_data
                .insert("segoe-ui".into(), FontData::from_owned(bytes).into());
            proportional.push("segoe-ui");
        }
        if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\consola.ttf") {
            fonts
                .font_data
                .insert("consolas".into(), FontData::from_owned(bytes).into());
            monospace.push("consolas");
        }
    }
    for &(name, path) in cjk_fonts() {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.into(), FontData::from_owned(bytes).into());
            proportional.push(name);
            monospace.push(name);
            break;
        }
    }
    let families = [
        (FontFamily::Proportional, proportional),
        (FontFamily::Monospace, monospace),
    ];
    for (family, preferred) in families {
        let entry = fonts.families.entry(family).or_default();
        for (index, name) in preferred.into_iter().enumerate() {
            entry.insert(index, name.into());
        }
    }
    ctx.set_fonts(fonts);
}

fn cjk_fonts() -> &'static [(&'static str, &'static str)] {
    #[cfg(windows)]
    return &[
        ("yahei", r"C:\Windows\Fonts\msyh.ttc"),
        ("simsun", r"C:\Windows\Fonts\simsun.ttc"),
    ];
    #[cfg(target_os = "linux")]
    return &[
        (
            "noto-cjk",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        ),
        ("wqy", "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc"),
    ];
    #[cfg(target_os = "macos")]
    return &[("pingfang", "/System/Library/Fonts/PingFang.ttc")];
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    return &[];
}

/// A raised card used for panels and grouped controls.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
}

/// A quiet inset used inside cards for rows and readouts.
pub fn inset() -> egui::Frame {
    egui::Frame::new()
        .fill(BG)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 6))
}

/// Draw a small rounded badge with a label; returns the response for tooltips.
pub fn badge(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) -> egui::Response {
    let text = text.into();
    let font = FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text, font, color);
    let size = egui::vec2(galley.size().x + 14.0, galley.size().y + 4.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), color.gamma_multiply(0.18));
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}
