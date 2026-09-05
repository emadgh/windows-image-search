from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# 1) Add a benchmark-only batch inference API to the real SFace ONNX adapter.
adapter_path = Path("src/face_sface_adapter.rs")
adapter = adapter_path.read_text(encoding="utf-8")

old_stats = '''#[derive(Clone, Debug)]
pub struct SFaceInferenceStats {
    pub provider: SFaceExecutionProvider,
    pub init: Duration,
    pub inference: Duration,
    pub dimension: usize,
}
'''
new_stats = old_stats + '''
#[derive(Clone, Debug)]
pub struct SFaceBatchInferenceStats {
    pub provider: SFaceExecutionProvider,
    pub init: Duration,
    pub inference: Duration,
    pub batch_size: usize,
    pub dimension: usize,
}
'''
adapter = replace_once(adapter, old_stats, new_stats, "batch inference stats")

old_method_marker = '''    pub fn align_and_embed(
        &mut self,
        image: &DynamicImage,
        landmarks: [LandmarkPoint; 5],
    ) -> Result<(Vec<f32>, SFaceInferenceStats)> {
'''
new_batch_method = '''    pub fn embed_aligned_batch(
        &mut self,
        aligned: &[DynamicImage],
    ) -> Result<(Vec<Vec<f32>>, SFaceBatchInferenceStats)> {
        if aligned.is_empty() {
            bail!("SFace batch must contain at least one aligned face");
        }
        let batch_size = aligned.len();
        let input = sface_batch_nchw_rgb_f32(aligned)?;
        let tensor = TensorRef::from_array_view((
            [batch_size, 3usize, SFACE_INPUT_SIZE as usize, SFACE_INPUT_SIZE as usize],
            input.as_slice(),
        ))
        .context("creating batched SFace ONNX input tensor")?;
        let started = Instant::now();
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .context("running batched SFace ONNX inference")?;
        let inference = started.elapsed();
        let (_shape, values) = outputs[0]
            .try_extract_tensor::<f32>()
            .context("extracting batched SFace ONNX output tensor")?;
        let expected = batch_size * SFACE_EMBEDDING_DIMENSION;
        if values.len() != expected {
            bail!(
                "batched SFace ONNX returned {} values; expected {} for batch size {}",
                values.len(),
                expected,
                batch_size
            );
        }
        let mut embeddings = Vec::with_capacity(batch_size);
        for chunk in values.chunks_exact(SFACE_EMBEDDING_DIMENSION) {
            embeddings.push(normalize(chunk.to_vec())?);
        }
        Ok((
            embeddings,
            SFaceBatchInferenceStats {
                provider: self.provider,
                init: self.init,
                inference,
                batch_size,
                dimension: SFACE_EMBEDDING_DIMENSION,
            },
        ))
    }

''' + old_method_marker
adapter = replace_once(adapter, old_method_marker, new_batch_method, "batch adapter method")

old_preprocess_end = '''pub fn sface_nchw_rgb_f32(image: &DynamicImage) -> Result<Vec<f32>> {
    if image.dimensions() != (SFACE_INPUT_SIZE, SFACE_INPUT_SIZE) {
        bail!(
            "SFace input must be {}x{}, got {}x{}",
            SFACE_INPUT_SIZE,
            SFACE_INPUT_SIZE,
            image.width(),
            image.height()
        );
    }
    let rgb = image.to_rgb8();
    let plane = (SFACE_INPUT_SIZE * SFACE_INPUT_SIZE) as usize;
    let mut output = vec![0.0f32; plane * 3];
    for (index, pixel) in rgb.pixels().enumerate() {
        output[index] = pixel[0] as f32;
        output[plane + index] = pixel[1] as f32;
        output[2 * plane + index] = pixel[2] as f32;
    }
    Ok(output)
}
'''
new_preprocess_end = old_preprocess_end + '''
pub fn sface_batch_nchw_rgb_f32(images: &[DynamicImage]) -> Result<Vec<f32>> {
    if images.is_empty() {
        bail!("SFace batch must contain at least one aligned face");
    }
    let per_face = (SFACE_INPUT_SIZE * SFACE_INPUT_SIZE) as usize * 3;
    let mut output = Vec::with_capacity(per_face * images.len());
    for image in images {
        output.extend(sface_nchw_rgb_f32(image)?);
    }
    Ok(output)
}
'''
adapter = replace_once(adapter, old_preprocess_end, new_preprocess_end, "batch preprocessing helper")

old_test = '''    #[test]
    fn normalize_produces_unit_vector_and_rejects_zero() {
'''
new_test = '''    #[test]
    fn batch_preprocessing_concatenates_nchw_faces_in_batch_order() {
        let first = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(
            SFACE_INPUT_SIZE,
            SFACE_INPUT_SIZE,
            Rgb([10u8, 20, 30]),
        ));
        let second = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(
            SFACE_INPUT_SIZE,
            SFACE_INPUT_SIZE,
            Rgb([40u8, 50, 60]),
        ));
        let values = sface_batch_nchw_rgb_f32(&[first, second]).unwrap();
        let plane = (SFACE_INPUT_SIZE * SFACE_INPUT_SIZE) as usize;
        let per_face = plane * 3;
        assert_eq!(values.len(), per_face * 2);
        assert_eq!(values[0], 10.0);
        assert_eq!(values[plane], 20.0);
        assert_eq!(values[2 * plane], 30.0);
        assert_eq!(values[per_face], 40.0);
        assert_eq!(values[per_face + plane], 50.0);
        assert_eq!(values[per_face + 2 * plane], 60.0);
        assert!(sface_batch_nchw_rgb_f32(&[]).is_err());
    }

''' + old_test
adapter = replace_once(adapter, old_test, new_test, "batch preprocessing test")
adapter_path.write_text(adapter, encoding="utf-8")


# 2) Extend the production-candidate SFace benchmark report with a batch-size sweep.
bench_path = Path("src/face_sface_benchmark.rs")
bench = bench_path.read_text(encoding="utf-8")

bench = replace_once(
    bench,
    '''    let mut embeddings = Vec::with_capacity(manifest.faces.len());
    let mut inference_times = Vec::with_capacity(manifest.faces.len());
''',
    '''    let mut embeddings = Vec::with_capacity(manifest.faces.len());
    let mut aligned_faces = Vec::with_capacity(manifest.faces.len());
    let mut inference_times = Vec::with_capacity(manifest.faces.len());
''',
    "aligned face cache",
)

bench = replace_once(
    bench,
    '''        let (embedding, stats) = adapter
            .align_and_embed(&oriented, landmarks)
            .with_context(|| format!("embedding SFace case {}", face.face_id))?;
        inference_times.push(stats.inference);
        embeddings.push(embedding);
''',
    '''        let aligned = face_sface_adapter::align_sface_112(&oriented, landmarks)
            .with_context(|| format!("aligning SFace case {}", face.face_id))?;
        let (embedding, stats) = adapter
            .embed_aligned(&aligned)
            .with_context(|| format!("embedding SFace case {}", face.face_id))?;
        inference_times.push(stats.inference);
        embeddings.push(embedding);
        aligned_faces.push(aligned);
''',
    "benchmark aligned embedding path",
)

bench = replace_once(
    bench,
    '''    append_latency(&mut report, &inference_times)?;
    writeln!(report)?;
    writeln!(report, "shared_evaluator_begin")?;
''',
    '''    append_latency(&mut report, &inference_times)?;
    append_batch_sweep(&mut report, &mut adapter, &aligned_faces)?;
    writeln!(report)?;
    writeln!(report, "shared_evaluator_begin")?;
''',
    "append batch sweep",
)

helper_marker = '''fn cosine(left: &[f32], right: &[f32]) -> Result<f32> {
'''
helper = '''fn append_batch_sweep(
    report: &mut String,
    adapter: &mut face_sface_adapter::SFaceOnnxAdapter,
    aligned_faces: &[image::DynamicImage],
) -> Result<()> {
    const BATCH_SIZES: [usize; 5] = [1, 2, 4, 8, 16];
    const MEASURED_ITERATIONS: usize = 5;

    writeln!(report, "batch_sweep_begin")?;
    writeln!(report, "batch_sweep_iterations={MEASURED_ITERATIONS}")?;
    for batch_size in BATCH_SIZES {
        let batch = (0..batch_size)
            .map(|index| aligned_faces[index % aligned_faces.len()].clone())
            .collect::<Vec<_>>();

        let supported = adapter.embed_aligned_batch(&batch);
        match supported {
            Ok((_warmup, warmup_stats)) => {
                let mut times = Vec::with_capacity(MEASURED_ITERATIONS);
                let mut dimensions_ok = true;
                for _ in 0..MEASURED_ITERATIONS {
                    let (embeddings, stats) = adapter.embed_aligned_batch(&batch)?;
                    dimensions_ok &= embeddings.len() == batch_size
                        && embeddings.iter().all(|embedding| {
                            embedding.len() == face_sface_adapter::SFACE_EMBEDDING_DIMENSION
                        });
                    times.push(stats.inference.as_secs_f64() * 1000.0);
                }
                times.sort_by(f64::total_cmp);
                let total_ms = times.iter().sum::<f64>();
                let mean_ms = total_ms / times.len() as f64;
                let p50_ms = percentile(&times, 0.50);
                let faces_per_second = if total_ms <= f64::EPSILON {
                    0.0
                } else {
                    (batch_size * times.len()) as f64 / (total_ms / 1000.0)
                };
                writeln!(report, "batch_{batch_size}_supported=true")?;
                writeln!(report, "batch_{batch_size}_warmup_ms={:.3}", warmup_stats.inference.as_secs_f64() * 1000.0)?;
                writeln!(report, "batch_{batch_size}_mean_ms={mean_ms:.3}")?;
                writeln!(report, "batch_{batch_size}_p50_ms={p50_ms:.3}")?;
                writeln!(report, "batch_{batch_size}_faces_per_second={faces_per_second:.3}")?;
                writeln!(report, "batch_{batch_size}_dimensions_ok={dimensions_ok}")?;
            }
            Err(err) => {
                writeln!(report, "batch_{batch_size}_supported=false")?;
                writeln!(
                    report,
                    "batch_{batch_size}_error={}",
                    tsv_field(&format!("{err:#}"))
                )?;
            }
        }
    }
    writeln!(report, "batch_sweep_end")?;
    Ok(())
}

''' + helper_marker
bench = replace_once(bench, helper_marker, helper, "batch sweep helper")
bench_path.write_text(bench, encoding="utf-8")


# 3) Document the new evidence captured by the existing gate.
doc_path = Path("docs/face-benchmark-gate.md")
doc = doc_path.read_text(encoding="utf-8")
doc = replace_once(
    doc,
    '''The adapter reports retain the shared #92 evaluator metrics. YuNet reports IoU-based precision/recall/F1, no-face false positives and face-size recall buckets. SFace reports Recall@1/5/10, MRR, same/different-person distance distributions and threshold-sweep results. Both adapters also report model initialization time and persistent-session inference throughput.
''',
    '''The adapter reports retain the shared #92 evaluator metrics. YuNet reports IoU-based precision/recall/F1, no-face false positives and face-size recall buckets. SFace reports Recall@1/5/10, MRR, same/different-person distance distributions and threshold-sweep results. Both adapters also report model initialization time and persistent-session inference throughput. SFace additionally performs a benchmark-only batch-size sweep for 1/2/4/8/16 aligned faces using the same persistent ONNX session, recording support/failure, warm-up latency, mean/P50 batch latency, throughput, and output-dimension validation for each size. Batch failures are retained as evidence and do not change production embedding behavior.
''',
    "face benchmark gate batch documentation",
)
doc_path.write_text(doc, encoding="utf-8")
