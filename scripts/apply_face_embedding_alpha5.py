from pathlib import Path

main = Path("src/main.rs")
text = main.read_text(encoding="utf-8")
old = "mod face_detection;\nmod face_pipeline;"
new = "mod face_detection;\nmod face_embedding;\nmod face_embedding_pipeline;\nmod face_embedding_store;\nmod face_pipeline;"
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"main module anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
main.write_text(text, encoding="utf-8")

cargo = Path("Cargo.toml")
text = cargo.read_text(encoding="utf-8")
old = 'version = "0.3.0-alpha.4"'
new = 'version = "0.3.0-alpha.5"'
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"Cargo.toml version anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
cargo.write_text(text, encoding="utf-8")

lock = Path("Cargo.lock")
text = lock.read_text(encoding="utf-8")
old = 'name = "windows-image-search"\nversion = "0.3.0-alpha.4"'
new = 'name = "windows-image-search"\nversion = "0.3.0-alpha.5"'
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"Cargo.lock package version anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
lock.write_text(text, encoding="utf-8")

embedding = Path("src/face_embedding.rs")
text = embedding.read_text(encoding="utf-8")
old = '''/// Implementations receive a deterministic, EXIF-corrected square face crop\n/// resized to `input_size()`. The pipeline normalizes returned vectors before\n/// persistence, so model adapters may return their native finite output.\npub trait FaceEmbedder: Send {\n    fn model_id(&self) -> &'static str;\n    fn model_version(&self) -> &'static str;\n    fn input_size(&self) -> u32;\n    fn embedding_dimension(&self) -> usize;\n    fn embed(&mut self, aligned_face: &DynamicImage) -> Result<Vec<f32>>;\n}\n'''
new = '''/// The pipeline owns source-state checks and persistence, while each model may\n/// override alignment/preprocessing geometry. This keeps a generic default crop\n/// for simple embedders without locking production models (for example SFace)\n/// to an incompatible alignment contract.\npub trait FaceEmbedder: Send {\n    fn model_id(&self) -> &'static str;\n    fn model_version(&self) -> &'static str;\n    fn input_size(&self) -> u32;\n    fn embedding_dimension(&self) -> usize;\n\n    fn alignment_revision(&self) -> i64 {\n        ALIGNMENT_REVISION\n    }\n\n    fn align_face(\n        &self,\n        image: &DynamicImage,\n        bbox: FaceBox,\n        landmarks: &[FaceLandmark],\n    ) -> Result<DynamicImage> {\n        aligned_face_crop(image, bbox, landmarks, self.input_size())\n    }\n\n    fn embed(&mut self, aligned_face: &DynamicImage) -> Result<Vec<f32>>;\n}\n'''
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"FaceEmbedder contract anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
embedding.write_text(text, encoding="utf-8")

pipeline = Path("src/face_embedding_pipeline.rs")
text = pipeline.read_text(encoding="utf-8")
old = '''    let dimension = embedder.embedding_dimension();\n    let input_size = embedder.input_size();\n    if dimension == 0 {\n        anyhow::bail!("face embedder dimension must be non-zero");\n    }\n    if input_size == 0 {\n        anyhow::bail!("face embedder input size must be non-zero");\n    }\n'''
new = '''    let dimension = embedder.embedding_dimension();\n    let input_size = embedder.input_size();\n    let alignment_revision = embedder.alignment_revision();\n    if dimension == 0 {\n        anyhow::bail!("face embedder dimension must be non-zero");\n    }\n    if input_size == 0 {\n        anyhow::bail!("face embedder input size must be non-zero");\n    }\n    if alignment_revision <= 0 {\n        anyhow::bail!("face embedder alignment revision must be positive");\n    }\n'''
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"pipeline revision anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
text = text.replace("face_embedding::ALIGNMENT_REVISION,", "alignment_revision,")
old = '''                    let aligned = face_embedding::aligned_face_crop(\n                        &oriented,\n                        candidate.bbox,\n                        &candidate.landmarks,\n                        input_size,\n                    )?;\n'''
new = '''                    let aligned = embedder.align_face(\n                        &oriented,\n                        candidate.bbox,\n                        &candidate.landmarks,\n                    )?;\n'''
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"model-specific alignment anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
pipeline.write_text(text, encoding="utf-8")

print("face embedding alpha5 integration applied")
