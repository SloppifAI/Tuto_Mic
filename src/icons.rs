//! Selbst gezeichnete Icons. egui bringt für Symbole wie ●, ■ oder ▶ keine Glyphen mit (sie erscheinen als
//! Kästchen), daher werden sie als Vektorformen gemalt – sieht auf jedem System gleich aus.

use eframe::egui::{self, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

pub const RECORD_RED: Color32 = Color32::from_rgb(220, 50, 50);

#[derive(Clone, Copy)]
pub enum Icon {
    Record,
    Pause,
    Play,
    Stop,
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    match icon {
        Icon::Record => {
            painter.circle_filled(c, s * 0.45, color);
        }
        Icon::Pause => {
            let bar = vec2(s * 0.28, s * 0.8);
            for dx in [-s * 0.2, s * 0.2] {
                painter.rect_filled(Rect::from_center_size(c + vec2(dx, 0.0), bar), 1.5, color);
            }
        }
        Icon::Play => {
            let h = s * 0.42;
            let pts = vec![
                pos2(c.x - h * 0.8, c.y - h),
                pos2(c.x + h, c.y),
                pos2(c.x - h * 0.8, c.y + h),
            ];
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 0.72)), 2.0, color);
        }
    }
}

/// Icon in einer Textzeile (z. B. im Status-Banner).
pub fn inline(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui.painter(), rect, icon, color);
}

/// Großer Button mit Icon links vom Text.
pub fn button(ui: &mut egui::Ui, icon: Icon, icon_color: Option<Color32>, text: &str, enabled: bool) -> egui::Response {
    const ICON: f32 = 18.0;
    const GAP: f32 = 10.0;
    let pad = vec2(18.0, 12.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(20.0), Color32::PLACEHOLDER);
    let size = vec2(
        pad.x * 2.0 + ICON + GAP + galley.size().x,
        (pad.y * 2.0 + galley.size().y).max(48.0),
    );
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        let visuals = if enabled {
            ui.style().interact(&resp)
        } else {
            &ui.style().visuals.widgets.inactive
        };
        let text_color = if enabled {
            visuals.text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().rect(
            rect,
            visuals.corner_radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
        let icon_rect = Rect::from_center_size(
            pos2(rect.left() + pad.x + ICON / 2.0, rect.center().y),
            Vec2::splat(ICON),
        );
        let color = if enabled {
            icon_color.unwrap_or(text_color)
        } else {
            text_color
        };
        paint(ui.painter(), icon_rect, icon, color);
        let text_pos = Pos2::new(icon_rect.right() + GAP, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(text_pos, galley, text_color);
    }
    resp
}
