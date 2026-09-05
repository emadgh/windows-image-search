from __future__ import annotations

from pathlib import Path
import hashlib
import urllib.request


MODELS = (
    (
        "face_detection_yunet_2026may.onnx",
        "https://raw.githubusercontent.com/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2026may.onnx",
        229_738,
        "ebafce4e3c118d6554634be5c27ab333b4c047a9a8c3faf1d7cf93101c22f0f0",
    ),
    (
        "face_recognition_sface_2021dec.onnx",
        "https://raw.githubusercontent.com/opencv/opencv_zoo/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx",
        38_696_353,
        "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
    ),
)


SELF_RAW_ROOT = "https://raw.githubusercontent.com/emadgh/windows-image-search/main/models/faces"


def download_models() -> None:
    target = Path("models/faces")
    target.mkdir(parents=True, exist_ok=True)

    for name, source_url, expected_size, expected_sha in MODELS:
        path = target / name
        print(f"Downloading {name} from OpenCV Zoo...")
        request = urllib.request.Request(
            source_url,
            headers={"User-Agent": "windows-image-search-model-vendor"},
        )
        with urllib.request.urlopen(request, timeout=180) as response, path.open("wb") as output:
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                output.write(chunk)

        verify_model(path, expected_size, expected_sha)


def verify_model(path: Path, expected_size: int, expected_sha: str) -> None:
    size = path.stat().st_size
    if size != expected_size:
        raise RuntimeError(f"{path.name}: size {size} != {expected_size}")

    digest = hashlib.sha256()
    with path.open("rb") as model:
        while chunk := model.read(1024 * 1024):
            digest.update(chunk)
    actual_sha = digest.hexdigest()
    if actual_sha.lower() != expected_sha.lower():
        raise RuntimeError(f"{path.name}: sha256 {actual_sha} != {expected_sha}")
    print(f"Verified {path.name}: {size} bytes, sha256={actual_sha}")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if new in text:
        return text
    if old not in text:
        raise RuntimeError(f"Could not locate {label}")
    return text.replace(old, new, 1)


def patch_model_manager() -> None:
    path = Path("src/face_model_manager.rs")
    text = path.read_text(encoding="utf-8")

    replacements = (
        (
            "https://github.com/opencv/opencv_zoo/raw/main/models/face_detection_yunet/face_detection_yunet_2026may.onnx",
            f"{SELF_RAW_ROOT}/face_detection_yunet_2026may.onnx",
        ),
        (
            "https://github.com/opencv/opencv_zoo/raw/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx",
            f"{SELF_RAW_ROOT}/face_recognition_sface_2021dec.onnx",
        ),
    )
    for old, new in replacements:
        if old not in text and new not in text:
            raise RuntimeError(f"Expected model URL not found: {old}")
        text = text.replace(old, new)

    path.write_text(text, encoding="utf-8")


def patch_face_runtime() -> None:
    path = Path("src/ui/face_runtime.rs")
    text = path.read_text(encoding="utf-8")

    text = replace_once(
        text,
        "use crate::face_sface_production;\n",
        "use crate::face_sface_production;\nuse crate::face_scope;\n",
        "face_scope import insertion point",
    )

    text = replace_once(
        text,
        """    pub(super) fn schedule_face_pipeline_after_base_index(&mut self) {
        if self.face_runtime.configured_and_available() {
            self.face_runtime.run_after_base_index = true;
        }
    }
""",
        """    pub(super) fn schedule_face_pipeline_after_base_index(&mut self) {
        let face_needed = self.roots.iter().any(|root| {
            face_scope::count_eligible_paths(&self.db_path, root).unwrap_or(0) > 0
        });
        if face_needed {
            self.face_runtime.run_after_base_index = true;
        }
    }
""",
        "schedule_face_pipeline_after_base_index",
    )

    text = replace_once(
        text,
        """    pub(super) fn face_model_download_running(&self) -> bool {
        self.face_runtime.model_download_running
    }
""",
        """    pub(super) fn face_model_download_running(&self) -> bool {
        self.face_runtime.model_download_running
    }

    pub(super) fn face_model_download_progress(&self) -> Option<(&'static str, u64, u64)> {
        if !self.face_runtime.model_download_running {
            return None;
        }
        Some((
            self.face_runtime
                .model_download_kind
                .map(|kind| kind.label())
                .unwrap_or("Face models"),
            self.face_runtime.model_downloaded,
            self.face_runtime.model_download_total,
        ))
    }
""",
        "face-model download progress accessor",
    )

    helper_marker = "    fn start_default_face_model_download(&mut self, force: bool, run_face_after: bool) {\n"
    helper = """    fn managed_default_models_needed(&self) -> bool {
        let yunet_needed = !self.model_path_is_custom(FaceModelKind::YuNet)
            && !matches!(
                face_model_manager::inspect(&self.face_runtime.model_cache_dir, YUNET),
                ManagedModelState::Ready
            );
        let sface_needed = !self.model_path_is_custom(FaceModelKind::SFace)
            && !matches!(
                face_model_manager::inspect(&self.face_runtime.model_cache_dir, SFACE),
                ManagedModelState::Ready
            );
        yunet_needed || sface_needed
    }

"""
    if "fn managed_default_models_needed" not in text:
        if helper_marker not in text:
            raise RuntimeError("Could not locate model bootstrap helper insertion point")
        text = text.replace(helper_marker, helper + helper_marker, 1)

    text = replace_once(
        text,
        """        if !self.face_runtime.settings.configured() || !self.face_embedding_settings.configured() {
            self.start_default_face_model_download(false, true);
            return;
        }
""",
        """        if self.managed_default_models_needed()
            || !self.face_runtime.settings.configured()
            || !self.face_embedding_settings.configured()
        {
            self.start_default_face_model_download(false, true);
            return;
        }
""",
        "automatic face-model bootstrap guard",
    )

    text = text.replace(
        "Defaults are downloaded from OpenCV Zoo, verified by exact size + SHA-256, validated as ONNX, then atomically installed. Browse paths below remain advanced custom overrides.",
        "Defaults are downloaded from this project's GitHub repository, verified by exact size + SHA-256, validated as ONNX, then atomically installed. Browse paths below remain advanced custom overrides.",
    )

    path.write_text(text, encoding="utf-8")


def patch_task_center() -> None:
    path = Path("src/ui/task_center.rs")
    text = path.read_text(encoding="utf-8")

    text = replace_once(
        text,
        """                        if let Some((done, total)) = self.progress.filter(|(_, total)| *total > 0) {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total as f32)
                                    .desired_width(ui.available_width().min(440.0))
                                    .text(format!("{done}/{total}")),
                            );
                        }
""",
        """                        if let Some((label, downloaded, total)) = self.face_model_download_progress() {
                            let fraction = if total == 0 {
                                0.0
                            } else {
                                downloaded as f32 / total as f32
                            };
                            let detail = if total == 0 {
                                format!("Preparing {label} download…")
                            } else {
                                format!(
                                    "{label}: {:.1}% · {:.1}/{:.1} MB",
                                    fraction * 100.0,
                                    downloaded as f64 / 1_048_576.0,
                                    total as f64 / 1_048_576.0
                                )
                            };
                            ui.add(
                                egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                                    .desired_width(ui.available_width().min(440.0))
                                    .text(detail),
                            );
                        } else if let Some((done, total)) =
                            self.progress.filter(|(_, total)| *total > 0)
                        {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total as f32)
                                    .desired_width(ui.available_width().min(440.0))
                                    .text(format!("{done}/{total}")),
                            );
                        }
""",
        "Task Center face-model progress block",
    )

    path.write_text(text, encoding="utf-8")


def write_model_readme() -> None:
    Path("models/faces/README.md").write_text(
        """# Face models

These ONNX files are vendored so installed builds can bootstrap face search from this repository instead of depending on a third-party runtime URL.

- `face_detection_yunet_2026may.onnx` — OpenCV Zoo YuNet, MIT. SHA-256 `ebafce4e3c118d6554634be5c27ab333b4c047a9a8c3faf1d7cf93101c22f0f0`.
- `face_recognition_sface_2021dec.onnx` — OpenCV Zoo SFace, Apache-2.0. SHA-256 `0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79`.

Upstream source: https://github.com/opencv/opencv_zoo
""",
        encoding="utf-8",
    )


def main() -> None:
    download_models()
    patch_model_manager()
    patch_face_runtime()
    patch_task_center()
    write_model_readme()
    print("Face models and runtime bootstrap are ready to commit.")


if __name__ == "__main__":
    main()
