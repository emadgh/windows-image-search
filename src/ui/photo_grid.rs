use super::ThumbnailFit;
use crate::face_detection::FaceBox;
use eframe::egui;

#[derive(Clone, Copy, Debug)]
pub(super) struct PhotoGridSpec {
    pub id_salt: &'static str,
    pub cell_width: f32,
    pub row_height: f32,
    pub max_height: Option<f32>,
}

impl PhotoGridSpec {
    pub(super) fn new(id_salt: &'static str, cell_width: f32, row_height: f32) -> Self {
        Self {
            id_salt,
            cell_width,
            row_height,
            max_height: None,
        }
    }

    pub(super) fn max_height(mut self, max_height: f32) -> Self {
        self.max_height = Some(max_height);
        self
    }
}

#[derive(Clone, Copy)]
pub(super) enum PhotoTileMode {
    Full(ThumbnailFit),
    Face(FaceBox),
}

pub(super) fn show(
    ui: &mut egui::Ui,
    item_count: usize,
    spec: PhotoGridSpec,
    mut render_cell: impl FnMut(&mut egui::Ui, usize),
) {
    if item_count == 0 {
        return;
    }

    let spacing = ui.spacing().item_spacing.x;
    let columns = columns_for_width(ui.available_width(), spec.cell_width, spacing);
    let rows = row_count(item_count, columns);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(spec.id_salt)
        .auto_shrink([false, false]);
    if let Some(max_height) = spec.max_height {
        scroll = scroll.max_height(max_height);
    }

    scroll.show_rows(ui, spec.row_height, rows, |ui, row_range| {
        for row in row_range {
            ui.horizontal(|ui| {
                for column in 0..columns {
                    let index = row * columns + column;
                    if index >= item_count {
                        break;
                    }
                    ui.allocate_ui_with_layout(
                        egui::vec2(spec.cell_width, spec.row_height),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| render_cell(ui, index),
                    );
                }
            });
        }
    });
}

pub(super) fn photo_tile(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    desired: egui::Vec2,
    mode: PhotoTileMode,
    selected: bool,
    sense: egui::Sense,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(desired, sense);
    ui.painter()
        .rect_filled(rect, 5.0, ui.visuals().extreme_bg_color);

    let source = texture.size_vec2();
    if source.x > 0.0 && source.y > 0.0 {
        match mode {
            PhotoTileMode::Full(ThumbnailFit::Contain) => {
                let scale = (rect.width() / source.x).min(rect.height() / source.y);
                let size = source * scale;
                let paint_rect = egui::Rect::from_center_size(rect.center(), size);
                ui.painter()
                    .image(texture.id(), paint_rect, full_uv(), egui::Color32::WHITE);
            }
            PhotoTileMode::Full(ThumbnailFit::Cover) => {
                ui.painter().image(
                    texture.id(),
                    rect,
                    cover_uv(source, rect.size()),
                    egui::Color32::WHITE,
                );
            }
            PhotoTileMode::Face(bbox) => {
                ui.painter()
                    .image(texture.id(), rect, face_crop_uv(bbox), egui::Color32::WHITE);
            }
        }
    }

    if selected {
        ui.painter().rect_stroke(
            rect,
            5.0,
            egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    response
}

pub(super) fn columns_for_width(available: f32, cell_width: f32, spacing: f32) -> usize {
    let cell_width = cell_width.max(1.0);
    let spacing = spacing.max(0.0);
    (((available.max(0.0) + spacing) / (cell_width + spacing)).floor() as usize).max(1)
}

pub(super) fn row_count(item_count: usize, columns: usize) -> usize {
    item_count.div_ceil(columns.max(1))
}

fn full_uv() -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
}

fn cover_uv(source: egui::Vec2, target: egui::Vec2) -> egui::Rect {
    let source_aspect = source.x / source.y.max(1.0);
    let target_aspect = target.x / target.y.max(1.0);
    if source_aspect > target_aspect {
        let visible = (target_aspect / source_aspect).clamp(0.0, 1.0);
        let margin = (1.0 - visible) * 0.5;
        egui::Rect::from_min_max(egui::pos2(margin, 0.0), egui::pos2(1.0 - margin, 1.0))
    } else {
        let visible = (source_aspect / target_aspect).clamp(0.0, 1.0);
        let margin = (1.0 - visible) * 0.5;
        egui::Rect::from_min_max(egui::pos2(0.0, margin), egui::pos2(1.0, 1.0 - margin))
    }
}

fn face_crop_uv(bbox: FaceBox) -> egui::Rect {
    let bbox = bbox.clamped();
    let center_x = bbox.x + bbox.width * 0.5;
    let center_y = bbox.y + bbox.height * 0.5;
    let square = (bbox.width.max(bbox.height) * 1.45).clamp(0.06, 1.0);
    let half = square * 0.5;
    let mut min_x = (center_x - half).clamp(0.0, 1.0);
    let mut min_y = (center_y - half).clamp(0.0, 1.0);
    let mut max_x = (center_x + half).clamp(0.0, 1.0);
    let mut max_y = (center_y + half).clamp(0.0, 1.0);

    if max_x - min_x < square {
        if min_x <= 0.0 {
            max_x = square.min(1.0);
        } else if max_x >= 1.0 {
            min_x = (1.0 - square).max(0.0);
        }
    }
    if max_y - min_y < square {
        if min_y <= 0.0 {
            max_y = square.min(1.0);
        } else if max_y >= 1.0 {
            min_y = (1.0 - square).max(0.0);
        }
    }

    egui::Rect::from_min_max(egui::pos2(min_x, min_y), egui::pos2(max_x, max_y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responsive_columns_and_rows_stay_bounded() {
        assert_eq!(columns_for_width(500.0, 100.0, 8.0), 4);
        assert_eq!(columns_for_width(90.0, 100.0, 8.0), 1);
        assert_eq!(row_count(0, 4), 0);
        assert_eq!(row_count(1, 4), 1);
        assert_eq!(row_count(9, 4), 3);
    }

    #[test]
    fn face_crop_uv_is_square_and_clamped_at_edges() {
        let uv = face_crop_uv(FaceBox {
            x: -0.10,
            y: 0.82,
            width: 0.28,
            height: 0.30,
        });
        assert!(uv.min.x >= 0.0 && uv.min.y >= 0.0);
        assert!(uv.max.x <= 1.0 && uv.max.y <= 1.0);
        assert!((uv.width() - uv.height()).abs() < 0.0001);
    }

    #[test]
    fn face_crop_matches_face_search_padding_semantics() {
        let uv = face_crop_uv(FaceBox {
            x: 0.40,
            y: 0.40,
            width: 0.20,
            height: 0.20,
        });
        assert!((uv.center().x - 0.50).abs() < 0.0001);
        assert!((uv.center().y - 0.50).abs() < 0.0001);
        assert!((uv.width() - 0.29).abs() < 0.0001);
    }
}
