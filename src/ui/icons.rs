//! Font-independent icons for application controls.
use eframe::egui;

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Expand,
    Collapse,
    Previous,
    Next,
}

impl Icon {
    fn label(self) -> &'static str {
        match self {
            Self::Expand => "Expand",
            Self::Collapse => "Collapse",
            Self::Previous => "Previous image",
            Self::Next => "Next image",
        }
    }
}

fn paint(ui: &egui::Ui, response: &egui::Response, icon: Icon) {
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), icon.label())
    });
    if !ui.is_rect_visible(response.rect) {
        return;
    }
    let center = response.rect.center();
    let unit = (response.rect.width().min(response.rect.height()) * 0.25).clamp(4.0, 10.0);
    let stroke = ui.style().interact(response).fg_stroke;
    match icon {
        Icon::Expand | Icon::Collapse => {
            ui.painter().line_segment(
                [
                    center - egui::vec2(unit * 0.6, 0.0),
                    center + egui::vec2(unit * 0.6, 0.0),
                ],
                stroke,
            );
            if matches!(icon, Icon::Expand) {
                ui.painter().line_segment(
                    [
                        center - egui::vec2(0.0, unit * 0.6),
                        center + egui::vec2(0.0, unit * 0.6),
                    ],
                    stroke,
                );
            }
        }
        Icon::Previous | Icon::Next => {
            let direction = if matches!(icon, Icon::Next) {
                1.0
            } else {
                -1.0
            };
            ui.painter().add(egui::Shape::line(
                vec![
                    center + egui::vec2(-direction * unit * 0.5, -unit),
                    center + egui::vec2(direction * unit * 0.5, 0.0),
                    center + egui::vec2(-direction * unit * 0.5, unit),
                ],
                stroke,
            ));
        }
    }
}

pub(super) fn button(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    icon: Icon,
    frame: bool,
) -> egui::Response {
    let response = ui
        .add_sized(size, egui::Button::new("").frame(frame))
        .on_hover_text(icon.label());
    paint(ui, &response, icon);
    response
}

pub(super) fn put_button(ui: &mut egui::Ui, rect: egui::Rect, icon: Icon) -> egui::Response {
    let response = ui
        .put(rect, egui::Button::new(""))
        .on_hover_text(icon.label());
    paint(ui, &response, icon);
    response
}

pub(super) fn sort_indicator(ui: &egui::Ui, response: &egui::Response, descending: bool) {
    let center = egui::pos2(response.rect.right() - 9.0, response.rect.center().y);
    let direction = if descending { 1.0 } else { -1.0 };
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            center + egui::vec2(-4.0, -direction * 3.0),
            center + egui::vec2(4.0, -direction * 3.0),
            center + egui::vec2(0.0, direction * 3.0),
        ],
        ui.style().interact(response).fg_stroke.color,
        egui::Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_icons_use_shapes_instead_of_font_glyphs() {
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for icon in [Icon::Expand, Icon::Collapse, Icon::Previous, Icon::Next] {
                    button(ui, egui::vec2(28.0, 28.0), icon, false);
                }
            });
        });
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if !text.galley.job.text.is_empty())));
        assert!(
            output
                .shapes
                .iter()
                .filter(|shape| matches!(
                    shape.shape,
                    egui::Shape::LineSegment { .. } | egui::Shape::Path(_)
                ))
                .count()
                >= 5
        );
    }
}
