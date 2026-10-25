//! Selbst gezeichnete Icons. egui bringt für Symbole wie ●, ■ oder ▶ keine Glyphen mit (sie erscheinen als
//! Kästchen), daher werden sie als Vektorformen gemalt – sieht auf jedem System gleich aus.

use crate::theme;
use eframe::egui::{self, Color32, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

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

/// Großer Button mit Icon links vom Text. `primary`: Hauptaktion in Königsblau mit weißer Schrift.
pub fn button(ui: &mut egui::Ui, icon: Icon, primary: bool, text: &str, enabled: bool) -> egui::Response {
    const ICON: f32 = 16.0;
    const GAP: f32 = 10.0;
    let pad = vec2(20.0, 12.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), theme::heading(16.0), Color32::PLACEHOLDER);
    let size = vec2(
        pad.x * 2.0 + ICON + GAP + galley.size().x,
        (pad.y * 2.0 + galley.size().y).max(46.0),
    );
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
    if ui.is_rect_visible(rect) {
        let visuals = if enabled {
            ui.style().interact(&resp)
        } else {
            &ui.style().visuals.widgets.inactive
        };
        let (fill, stroke, fg) = match (primary, enabled) {
            (true, true) => {
                let fill = if resp.hovered() {
                    theme::BLUE_700
                } else {
                    theme::BLUE_600
                };
                (fill, egui::Stroke::NONE, Color32::WHITE)
            }
            (false, true) => (visuals.weak_bg_fill, visuals.bg_stroke, visuals.text_color()),
            (_, false) => (visuals.weak_bg_fill, visuals.bg_stroke, ui.visuals().weak_text_color()),
        };
        ui.painter()
            .rect(rect, visuals.corner_radius, fill, stroke, StrokeKind::Inside);
        let icon_rect = Rect::from_center_size(
            pos2(rect.left() + pad.x + ICON / 2.0, rect.center().y),
            Vec2::splat(ICON),
        );
        paint(ui.painter(), icon_rect, icon, fg);
        let text_pos = Pos2::new(icon_rect.right() + GAP, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(text_pos, galley, fg);
    }
    resp.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    })
}
