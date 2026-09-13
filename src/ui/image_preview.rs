use super::ImageSearchApp;
use crate::{face_detection, oversized_preview, portable, settings, thumbnail_cache};
use anyhow::{bail, Context, Result};
use eframe::egui;
use image::{DynamicImage, GenericImageView};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

const PREVIEW_EDGE: u32 = 2_560;
const MAX_DIRECT_DECODE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Default)]
pub(super) struct ImagePreviewState {
    pub open: bool,
    path: Option<PathBuf>,
    generation: u64,
    pending: Option<Receiver<PreviewMessage>>,
    texture: Option<egui::TextureHandle>,
    quality: Option<String>,
    error: Option<String>,
}

struct PreviewMessage {
    generation: u64,
    path: PathBuf,
    result: Result<DecodedPreview, String>,
}

struct DecodedPreview {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    quality: PreviewQuality,
}

#[derive(Clone, Copy)]
enum PreviewQuality {
    HighResolution,
    BoundedDerivative,
    ExistingThumbnail,
}

impl PreviewQuality {
    fn label(self) -> &'static str {
        match self {
            Self::HighResolution => "High-resolution preview",
            Self::BoundedDerivative => "Bounded preview for a very large image",
            Self::ExistingThumbnail => "Thumbnail fallback for a very large image",
        }
    }
}

impl ImageSearchApp {
    pub(super) fn open_image_preview(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.image_preview.open = true;
        if self.image_preview.path.as_ref() == Some(&path)
            && (self.image_preview.texture.is_some() || self.image_preview.pending.is_some())
        {
            return;
        }

        self.image_preview.generation = self.image_preview.generation.wrapping_add(1);
        let generation = self.image_preview.generation;
        self.image_preview.path = Some(path.clone());
        self.image_preview.texture = None;
        self.image_preview.quality = None;
        self.image_preview.error = None;
        let roots = self.roots.clone();
        let repaint = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.image_preview.pending = Some(rx);
        std::thread::spawn(move || {
            let result = decode_preview(&path, &roots).map_err(|error| format!("{error:#}"));
            let _ = tx.send(PreviewMessage {
                generation,
                path,
                result,
            });
            repaint.request_repaint();
        });
    }

    pub(super) fn close_image_preview(&mut self) {
        self.image_preview.open = false;
        self.image_preview.pending = None;
        self.image_preview.texture = None;
        self.image_preview.quality = None;
        self.image_preview.error = None;
    }

    pub(super) fn preview_path(&self) -> Option<&Path> {
        self.image_preview.path.as_deref()
    }

    pub(super) fn preview_open(&self) -> bool {
        self.image_preview.open
    }

    pub(super) fn show_image_preview(&mut self, ctx: &egui::Context) {
        self.poll_image_preview(ctx);
        if !self.image_preview.open {
            return;
        }

        let screen = ctx.input(|input| input.screen_rect());
        let mut close = false;
        let mut previous = false;
        let mut next = false;
        let path = self.image_preview.path.clone();
        let texture = self.image_preview.texture.clone();
        let quality = self.image_preview.quality.clone();
        let error = self.image_preview.error.clone();

        egui::Area::new(egui::Id::new("image-preview-overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                let rect = egui::Rect::from_min_size(ui.min_rect().min, screen.size());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(245));
                ui.allocate_rect(rect, egui::Sense::click());

                let top = egui::Rect::from_min_max(
                    rect.min + egui::vec2(16.0, 12.0),
                    egui::pos2(rect.right() - 16.0, rect.top() + 52.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(top), |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Close  Space").clicked() {
                            close = true;
                        }
                        ui.separator();
                        if let Some(path) = &path {
                            let name = path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("Image preview");
                            ui.strong(name);
                            ui.small(super::views::truncate_middle(
                                &path.display().to_string(),
                                90,
                            ))
                            .on_hover_text(path.display().to_string());
                        }
                    });
                });

                let footer_height = 38.0;
                let image_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 64.0, top.bottom() + 8.0),
                    egui::pos2(rect.right() - 64.0, rect.bottom() - footer_height),
                );
                if let Some(texture) = texture {
                    paint_contained(ui, &texture, image_rect);
                } else if let Some(error) = error {
                    let error_rect = image_rect.shrink2(egui::vec2(80.0, 80.0));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(error_rect), |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("Preview unavailable");
                            ui.label(error);
                        });
                    });
                } else {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(image_rect), |ui| {
                        ui.centered_and_justified(|ui| {
                            ui.spinner();
                        });
                    });
                }

                let previous_rect = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 30.0, rect.center().y),
                    egui::vec2(44.0, 72.0),
                );
                let next_rect = egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 30.0, rect.center().y),
                    egui::vec2(44.0, 72.0),
                );
                previous = ui.put(previous_rect, egui::Button::new("◀")).clicked();
                next = ui.put(next_rect, egui::Button::new("▶")).clicked();

                let footer = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 16.0, rect.bottom() - footer_height),
                    egui::pos2(rect.right() - 16.0, rect.bottom() - 8.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.small(
                            "← / → navigate   ·   Space or Esc close   ·   Shift+Space Inspector",
                        );
                        if let Some(quality) = &quality {
                            ui.separator();
                            ui.small(quality);
                        }
                    });
                });
            });

        if close {
            self.close_image_preview();
        } else if previous {
            self.navigate_image_preview(-1, ctx);
        } else if next {
            self.navigate_image_preview(1, ctx);
        }
    }

    pub(super) fn navigate_image_preview(&mut self, delta: isize, ctx: &egui::Context) {
        let visible = self.visible_result_paths();
        let current = self
            .preview_path()
            .and_then(|path| visible.iter().position(|candidate| candidate == path));
        let Some(target) = super::ux::navigation_target(current, delta, visible.len()) else {
            return;
        };
        let path = visible[target].clone();
        self.select_path(&path, false);
        self.open_image_preview(path, ctx);
    }

    fn poll_image_preview(&mut self, ctx: &egui::Context) {
        let message = self
            .image_preview
            .pending
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        let Some(message) = message else {
            return;
        };
        self.image_preview.pending = None;
        if message.generation != self.image_preview.generation
            || self.image_preview.path.as_ref() != Some(&message.path)
        {
            return;
        }
        match message.result {
            Ok(decoded) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [decoded.width, decoded.height],
                    &decoded.rgba,
                );
                self.image_preview.texture = Some(ctx.load_texture(
                    format!("preview:{}", message.path.display()),
                    image,
                    egui::TextureOptions::LINEAR,
                ));
                self.image_preview.quality = Some(decoded.quality.label().to_owned());
            }
            Err(error) => self.image_preview.error = Some(error),
        }
        ctx.request_repaint();
    }
}

fn paint_contained(ui: &egui::Ui, texture: &egui::TextureHandle, rect: egui::Rect) {
    let source = texture.size_vec2();
    if source.x <= 0.0 || source.y <= 0.0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let scale = (rect.width() / source.x).min(rect.height() / source.y);
    let target = egui::Rect::from_center_size(rect.center(), source * scale);
    ui.painter().image(
        texture.id(),
        target,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

fn decode_preview(path: &Path, roots: &[PathBuf]) -> Result<DecodedPreview> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("reading image metadata {}", path.display()))?;
    let dimensions = image::image_dimensions(path)
        .with_context(|| format!("reading image dimensions {}", path.display()))?;
    let decoded_bytes = u64::from(dimensions.0)
        .saturating_mul(u64::from(dimensions.1))
        // Some supported inputs decode to 16-bit RGBA before conversion.
        .saturating_mul(8);
    let direct = metadata.len() <= settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES
        && decoded_bytes <= MAX_DIRECT_DECODE_BYTES;

    let (image, quality) = if direct {
        let (image, _) = face_detection::decode_oriented_with_orientation(path)?;
        (
            image.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE),
            PreviewQuality::HighResolution,
        )
    } else {
        load_or_build_bounded_preview(path, roots)?
    };
    rgba_preview(image, quality)
}

fn load_or_build_bounded_preview(
    path: &Path,
    roots: &[PathBuf],
) -> Result<(DynamicImage, PreviewQuality)> {
    let root = portable::indexed_root_for_path(path, roots)
        .context("very large image is outside an attached indexed root")?;
    let metadata = std::fs::metadata(path)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs() as i64);

    if let Ok(candidate) =
        oversized_preview::cache_path_for_state(root, path, metadata.len(), modified)
    {
        if let Some(image) = decode_existing(&candidate) {
            return Ok((image, PreviewQuality::BoundedDerivative));
        }
    }

    match oversized_preview::load_or_build(root, path, metadata.len(), modified) {
        Ok(asset) => return Ok((asset.image, PreviewQuality::BoundedDerivative)),
        Err(build_error) => {
            let thumbnail = thumbnail_cache::cache_path_for_root(root, path)?;
            if let Some(image) = decode_existing(&thumbnail) {
                return Ok((image, PreviewQuality::ExistingThumbnail));
            }
            return Err(build_error).with_context(|| {
                format!(
                    "no bounded derivative or safe thumbnail could be created for {}",
                    path.display()
                )
            });
        }
    }
}

fn decode_existing(path: &Path) -> Option<DynamicImage> {
    if !path.is_file() {
        return None;
    }
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}

fn rgba_preview(image: DynamicImage, quality: PreviewQuality) -> Result<DecodedPreview> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        bail!("decoded preview has zero dimensions")
    }
    let rgba = image.to_rgba8();
    Ok(DecodedPreview {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        rgba: rgba.into_raw(),
        quality,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wis-image-preview-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn regular_preview_contains_the_whole_image_and_is_bounded() {
        let path = std::env::temp_dir().join(format!(
            "wis-image-preview-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let image = ImageBuffer::from_fn(3_000, 1_500, |x, _| {
            if x < 1_500 {
                Rgb([255u8, 0, 0])
            } else {
                Rgb([0u8, 0, 255])
            }
        });
        image.save(&path).unwrap();
        let preview = decode_preview(&path, &[]).unwrap();
        assert_eq!((preview.width, preview.height), (2_560, 1_280));
        assert_eq!(&preview.rgba[..3], &[255, 0, 0]);
        let last = (preview.width * preview.height - 1) * 4;
        assert_eq!(&preview.rgba[last..last + 3], &[0, 0, 255]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn bounded_preview_can_be_built_on_demand_off_the_ui_path() {
        let root = temp_root("on-demand");
        std::fs::create_dir_all(root.join("images")).unwrap();
        let source = root.join("images").join("large.jpg");
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(3200, 1800, Rgb([20, 40, 60])))
            .save(&source)
            .unwrap();

        let (image, quality) =
            load_or_build_bounded_preview(&source, std::slice::from_ref(&root)).unwrap();
        assert!(matches!(quality, PreviewQuality::BoundedDerivative));
        assert!(image.width() <= oversized_preview::PREVIEW_EDGE);
        assert!(image.height() <= oversized_preview::PREVIEW_EDGE);
        assert!(thumbnail_cache::load_cached_for_root(&root, &source).is_some());
        let _ = std::fs::remove_dir_all(root);
    }
}
