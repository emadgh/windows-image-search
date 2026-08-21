from pathlib import Path


def replace(path: str, old: str, new: str, count: int = 1):
    p = Path(path)
    text = p.read_text(encoding='utf-8')
    actual = text.count(old)
    if actual < count:
        raise RuntimeError(f'{path}: expected {count}, found {actual}: {old[:100]!r}')
    text = text.replace(old, new, count)
    p.write_text(text, encoding='utf-8')

replace(
    'src/ui/views.rs',
    'use super::{ImageSearchApp, ThumbnailFit};\n',
    'use super::{ImageSearchApp, ThumbnailFit};\nuse crate::face_detection::FaceBox;\n',
)

replace(
    'src/ui/views.rs',
    '''                                            thumbnail_widget(\n                                                ui,\n                                                &texture,\n                                                egui::vec2(self.thumb_size, self.thumb_size),\n                                                fit,\n                                                selected,\n                                                egui::Sense::click_and_drag(),\n                                            )\n''',
    '''                                            let response = thumbnail_widget(\n                                                ui,\n                                                &texture,\n                                                egui::vec2(self.thumb_size, self.thumb_size),\n                                                fit,\n                                                selected,\n                                                egui::Sense::click_and_drag(),\n                                            );\n                                            if let Some(bbox) = self.face_match_box(&record.path) {\n                                                draw_face_match_box(ui, &texture, response.rect, fit, bbox);\n                                            }\n                                            response\n''',
)

replace(
    'src/ui/views.rs',
    '''                            thumbnail_widget(\n                                ui,\n                                &texture,\n                                egui::vec2(widths.thumb, 54.0),\n                                fit,\n                                selected,\n                                egui::Sense::click_and_drag(),\n                            )\n''',
    '''                            let response = thumbnail_widget(\n                                ui,\n                                &texture,\n                                egui::vec2(widths.thumb, 54.0),\n                                fit,\n                                selected,\n                                egui::Sense::click_and_drag(),\n                            );\n                            if let Some(bbox) = self.face_match_box(&record.path) {\n                                draw_face_match_box(ui, &texture, response.rect, fit, bbox);\n                            }\n                            response\n''',
)

needle = '''fn swatch(ui: &mut egui::Ui, rgb: [u8; 3]) {\n'''
helper = '''fn draw_face_match_box(\n    ui: &egui::Ui,\n    texture: &egui::TextureHandle,\n    target: egui::Rect,\n    fit: ThumbnailFit,\n    bbox: FaceBox,\n) {\n    let source = texture.size_vec2();\n    if source.x <= 0.0 || source.y <= 0.0 {\n        return;\n    }\n    let bbox = bbox.clamped();\n    let (paint_rect, uv) = match fit {\n        ThumbnailFit::Contain => {\n            let scale = (target.width() / source.x).min(target.height() / source.y);\n            let size = source * scale;\n            (\n                egui::Rect::from_center_size(target.center(), size),\n                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),\n            )\n        }\n        ThumbnailFit::Cover => {\n            let source_aspect = source.x / source.y;\n            let target_aspect = target.width() / target.height().max(1.0);\n            let uv = if source_aspect > target_aspect {\n                let visible = target_aspect / source_aspect;\n                let margin = (1.0 - visible) * 0.5;\n                egui::Rect::from_min_max(\n                    egui::pos2(margin, 0.0),\n                    egui::pos2(1.0 - margin, 1.0),\n                )\n            } else {\n                let visible = source_aspect / target_aspect;\n                let margin = (1.0 - visible) * 0.5;\n                egui::Rect::from_min_max(\n                    egui::pos2(0.0, margin),\n                    egui::pos2(1.0, 1.0 - margin),\n                )\n            };\n            (target, uv)\n        }\n    };\n\n    let uv_width = uv.width().max(f32::EPSILON);\n    let uv_height = uv.height().max(f32::EPSILON);\n    let x0 = ((bbox.x - uv.min.x) / uv_width).clamp(0.0, 1.0);\n    let y0 = ((bbox.y - uv.min.y) / uv_height).clamp(0.0, 1.0);\n    let x1 = ((bbox.x + bbox.width - uv.min.x) / uv_width).clamp(0.0, 1.0);\n    let y1 = ((bbox.y + bbox.height - uv.min.y) / uv_height).clamp(0.0, 1.0);\n    if x1 <= x0 || y1 <= y0 {\n        return;\n    }\n    let rect = egui::Rect::from_min_max(\n        egui::pos2(\n            paint_rect.left() + x0 * paint_rect.width(),\n            paint_rect.top() + y0 * paint_rect.height(),\n        ),\n        egui::pos2(\n            paint_rect.left() + x1 * paint_rect.width(),\n            paint_rect.top() + y1 * paint_rect.height(),\n        ),\n    );\n    ui.painter().rect_stroke(\n        rect,\n        2.0,\n        egui::Stroke::new(2.0, egui::Color32::LIGHT_GREEN),\n        egui::StrokeKind::Inside,\n    );\n}\n\nfn swatch(ui: &mut egui::Ui, rgb: [u8; 3]) {\n'''
replace('src/ui/views.rs', needle, helper)
print('face match overlay patched')
