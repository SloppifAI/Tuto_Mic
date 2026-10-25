//! Aussehen nach dem Tuto-Designsystem („Heft“): Königsblau als Markenfarbe, Slate als Neutralfarben, Lexend als
//! Schrift, weiße Karten mit 16 px Radius. Farbwerte aus dem Theme der Tuto-Plattform (OKLCH → sRGB gerundet).

use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    TextStyle, TextureHandle, TextureOptions, Theme, Visuals,
};
use std::sync::Arc;

pub const BLUE_50: Color32 = Color32::from_rgb(0xf0, 0xf5, 0xff);
pub const BLUE_100: Color32 = Color32::from_rgb(0xdd, 0xe8, 0xff);
pub const BLUE_400: Color32 = Color32::from_rgb(0x6c, 0x92, 0xfa);
pub const BLUE_600: Color32 = Color32::from_rgb(0x2a, 0x4a, 0xc9);
pub const BLUE_700: Color32 = Color32::from_rgb(0x21, 0x3c, 0xa7);
pub const BLUE_800: Color32 = Color32::from_rgb(0x1a, 0x31, 0x86);

pub const SLATE_50: Color32 = Color32::from_rgb(0xf8, 0xfa, 0xfc);
pub const SLATE_100: Color32 = Color32::from_rgb(0xf1, 0xf5, 0xf9);
pub const SLATE_200: Color32 = Color32::from_rgb(0xe2, 0xe8, 0xf0);
pub const SLATE_400: Color32 = Color32::from_rgb(0x90, 0xa1, 0xb9);
pub const SLATE_500: Color32 = Color32::from_rgb(0x62, 0x74, 0x8e);
pub const SLATE_700: Color32 = Color32::from_rgb(0x31, 0x41, 0x58);
pub const SLATE_800: Color32 = Color32::from_rgb(0x1d, 0x29, 0x3d);
pub const SLATE_900: Color32 = Color32::from_rgb(0x0f, 0x17, 0x2b);
pub const SLATE_950: Color32 = Color32::from_rgb(0x02, 0x06, 0x18);

/// Statusfarben wie in Tuto: Erfolg Emerald, Warnung Amber, Fehler Rose. Immer zusammen mit Text verwenden.
pub const EMERALD_600: Color32 = Color32::from_rgb(0x00, 0x96, 0x6a);
pub const AMBER_600: Color32 = Color32::from_rgb(0xd9, 0x77, 0x06);
pub const AMBER_700: Color32 = Color32::from_rgb(0xb4, 0x53, 0x09);
pub const ROSE_600: Color32 = Color32::from_rgb(0xe1, 0x1d, 0x48);
pub const ROSE_50: Color32 = Color32::from_rgb(0xff, 0xf1, 0xf2);

pub const CARD_RADIUS: u8 = 16;
pub const BUTTON_RADIUS: u8 = 10;

/// Schriftfamilie für Überschriften und Hervorhebungen (Lexend 600).
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn heading(size: f32) -> FontId {
    FontId::new(size, bold())
}

/// Schrift, Farben und Abstände für helles und dunkles Systemdesign setzen.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_visuals_of(Theme::Light, visuals(false));
    ctx.set_visuals_of(Theme::Dark, visuals(true));
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(15.0)),
            (TextStyle::Button, FontId::proportional(15.0)),
            (TextStyle::Heading, heading(22.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.spacing.interact_size.y = 34.0;
    });
}

fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
    };
    add("lexend", include_bytes!("../assets/fonts/Lexend-latin-400.ttf"));
    add("lexend-ext", include_bytes!("../assets/fonts/Lexend-latin-ext-400.ttf"));
    add("lexend-bold", include_bytes!("../assets/fonts/Lexend-latin-600.ttf"));
    add(
        "lexend-bold-ext",
        include_bytes!("../assets/fonts/Lexend-latin-ext-600.ttf"),
    );

    // Lexend zuerst, die egui-Standardschriften bleiben als Rückfall für fehlende Zeichen.
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let family = |main: &[&str]| {
        main.iter()
            .map(|s| (*s).to_string())
            .chain(fallback.clone())
            .collect::<Vec<_>>()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, family(&["lexend", "lexend-ext"]));
    fonts
        .families
        .insert(bold(), family(&["lexend-bold", "lexend-bold-ext"]));
    fonts
}

fn visuals(dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let (bg, card, text, weak, border, accent) = if dark {
        (SLATE_950, SLATE_900, SLATE_100, SLATE_400, SLATE_800, BLUE_400)
    } else {
        (SLATE_50, Color32::WHITE, SLATE_900, SLATE_500, SLATE_200, BLUE_600)
    };
    v.panel_fill = bg;
    v.window_fill = card;
    v.extreme_bg_color = if dark { SLATE_950 } else { Color32::WHITE };
    v.faint_bg_color = if dark { SLATE_800 } else { SLATE_100 };
    v.window_stroke = Stroke::new(1.0, border);
    v.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.menu_corner_radius = CornerRadius::same(12);
    v.hyperlink_color = accent;
    v.selection.bg_fill = if dark { BLUE_800 } else { BLUE_100 };
    v.selection.stroke = Stroke::new(1.0, accent);
    v.override_text_color = Some(text);
    v.window_shadow = Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0x15, 0x27, 0x64, if dark { 60 } else { 18 }),
    };
    v.popup_shadow = v.window_shadow;

    let radius = CornerRadius::same(BUTTON_RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, text);
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    w.inactive.weak_bg_fill = if dark { SLATE_800 } else { Color32::WHITE };
    w.inactive.bg_fill = w.inactive.weak_bg_fill;
    w.inactive.bg_stroke = Stroke::new(1.0, border);
    w.inactive.fg_stroke = Stroke::new(1.0, text);
    w.hovered.weak_bg_fill = if dark { SLATE_700 } else { BLUE_50 };
    w.hovered.bg_fill = w.hovered.weak_bg_fill;
    w.hovered.bg_stroke = Stroke::new(1.0, accent);
    w.hovered.fg_stroke = Stroke::new(1.0, text);
    w.active.weak_bg_fill = if dark { SLATE_700 } else { BLUE_100 };
    w.active.bg_fill = w.active.weak_bg_fill;
    w.active.bg_stroke = Stroke::new(1.0, accent);
    w.active.fg_stroke = Stroke::new(1.0, text);
    let _ = weak;
    v
}

/// Schwächere Textfarbe für Hinweise.
pub fn weak_text(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode { SLATE_400 } else { SLATE_500 }
}

/// Weiße (bzw. im Dunkelmodus dunkle) Karte mit Rand, Radius und dezentem Schatten.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let v = ui.visuals();
    egui::Frame::new()
        .fill(v.window_fill)
        .stroke(v.window_stroke)
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .shadow(v.window_shadow)
        .inner_margin(Margin::same(24))
}

/// Hauptaktion: Königsblau mit weißer Schrift.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).font(heading(15.0)))
        .fill(BLUE_600)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(BUTTON_RADIUS))
        .min_size(egui::vec2(0.0, 40.0))
}

/// Pillenförmiges Status-Etikett (Text immer mit Farbe kombiniert).
pub fn badge(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    // Gemalt statt als Frame: so bleibt es auch in zentrierten Layouts genau so breit wie der Text.
    let galley = ui.painter().layout_no_wrap(text.to_owned(), heading(12.0), fg);
    let (rect, response) = ui.allocate_exact_size(galley.size() + egui::vec2(20.0, 8.0), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    ui.painter().rect_filled(rect, CornerRadius::same(255), bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
}

/// Logos und Icons der Marke, einmal geladen.
pub struct Brand {
    pub logo: TextureHandle,
    pub logo_dark: TextureHandle,
}

impl Brand {
    pub fn load(ctx: &egui::Context) -> Self {
        Self {
            logo: texture(ctx, "tuto-logo", include_bytes!("../assets/images/tuto-logo.png")),
            logo_dark: texture(
                ctx,
                "tuto-logo-dark",
                include_bytes!("../assets/images/tuto-logo-dark.png"),
            ),
        }
    }

    pub fn logo_for(&self, ui: &egui::Ui) -> &TextureHandle {
        if ui.visuals().dark_mode {
            &self.logo_dark
        } else {
            &self.logo
        }
    }
}

fn texture(ctx: &egui::Context, name: &str, png: &[u8]) -> TextureHandle {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("eingebettetes PNG ist gültig")
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    ctx.load_texture(
        name,
        ColorImage::from_rgba_unmultiplied(size, &image),
        TextureOptions::LINEAR,
    )
}

/// Fenster-Icon (blauer Ring aus dem Tuto-Favicon).
pub fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/images/icon.png"))
        .expect("eingebettetes PNG ist gültig")
}

/// Hinweis in Bernstein: Achtung, aber kein Fehler.
pub fn warning(ui: &mut egui::Ui, text: &str) {
    let (fill, fg) = if ui.visuals().dark_mode {
        (Color32::from_rgb(0x45, 0x1a, 0x03), Color32::from_rgb(0xfc, 0xd3, 0x4d))
    } else {
        (Color32::from_rgb(0xff, 0xf7, 0xe6), AMBER_700)
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(text).color(fg));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_decode() {
        let ctx = egui::Context::default();
        let brand = Brand::load(&ctx);
        assert!(brand.logo.size()[0] > brand.logo.size()[1]);
        assert_eq!(window_icon().width, 256);
    }
}
