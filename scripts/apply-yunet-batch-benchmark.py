from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# 1) Add a benchmark-only batched inference probe to the real YuNet adapter.
adapter_path = Path("src/face_detection/yunet_adapter.rs")
adapter = adapter_path.read_text(encoding="utf-8")
adapter = replace_once(
    adapter,
    "use std::path::Path;\n",
    "use std::path::Path;\nuse std::time::{Duration, Instant};\n",
    "YuNet time imports",
)

input_struct = '''struct YuNetInput {
    values: Vec<f32>,
    width: u32,
    height: u32,
    padded_width: u32,
    padded_height: u32,
}
'''
stats_struct = input_struct + '''
#[derive(Clone, Debug)]
pub struct YuNetBatchInferenceStats {
    pub provider: YuNetExecutionProvider,
    pub inference: Duration,
    pub batch_size: usize,
    pub padded_width: u32,
    pub padded_height: u32,
}
'''
adapter = replace_once(adapter, input_struct, stats_struct, "YuNet batch stats")

method_marker = '''    pub fn detect(&mut self, image: &DynamicImage) -> Result<Vec<DetectedFace>> {
'''
batch_method = '''    pub fn benchmark_batch_inference(
        &mut self,
        image: &DynamicImage,
        batch_size: usize,
    ) -> Result<YuNetBatchInferenceStats> {
        let input = preprocess(image)?;
        let values = repeat_input_for_batch(&input, batch_size)?;
        let tensor = TensorRef::from_array_view((
            [
                batch_size,
                3usize,
                input.padded_height as usize,
                input.padded_width as usize,
            ],
            values.as_slice(),
        ))
        .context("creating batched YuNet input tensor")?;
        let started = Instant::now();
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .context("running batched YuNet ONNX inference")?;
        let inference = started.elapsed();

        for stride in YUNET_STRIDES {
            let cells = (input.padded_width / stride) as usize
                * (input.padded_height / stride) as usize;
            for (prefix, values_per_cell) in [
                ("cls", 1usize),
                ("obj", 1usize),
                ("bbox", 4usize),
                ("kps", 10usize),
            ] {
                let name = format!("{prefix}_{stride}");
                let value = outputs
                    .get(name.as_str())
                    .with_context(|| format!("YuNet output `{name}` is missing"))?;
                let (_shape, data) = value
                    .try_extract_tensor::<f32>()
                    .with_context(|| format!("extracting batched YuNet output `{name}`"))?;
                let expected = batch_size
                    .checked_mul(cells)
                    .and_then(|count| count.checked_mul(values_per_cell))
                    .context("YuNet batched output size overflow")?;
                if data.len() != expected {
                    bail!(
                        "batched YuNet output `{name}` returned {} values; expected {} for batch size {}",
                        data.len(),
                        expected,
                        batch_size
                    );
                }
            }
        }

        Ok(YuNetBatchInferenceStats {
            provider: self.provider,
            inference,
            batch_size,
            padded_width: input.padded_width,
            padded_height: input.padded_height,
        })
    }

''' + method_marker
adapter = replace_once(adapter, method_marker, batch_method, "YuNet batch method")

preprocess_marker = '''fn validate_thresholds(score: f32, nms: f32, top_k: usize) -> Result<()> {
'''
repeat_helper = '''fn repeat_input_for_batch(input: &YuNetInput, batch_size: usize) -> Result<Vec<f32>> {
    if batch_size == 0 {
        bail!("YuNet batch size must be greater than zero");
    }
    let capacity = input
        .values
        .len()
        .checked_mul(batch_size)
        .context("YuNet batched input size overflow")?;
    let mut values = Vec::with_capacity(capacity);
    for _ in 0..batch_size {
        values.extend_from_slice(&input.values);
    }
    Ok(values)
}

''' + preprocess_marker
adapter = replace_once(adapter, preprocess_marker, repeat_helper, "YuNet repeat helper")

existing_test = '''    #[test]
    fn stride_decode_matches_opencv_geometry() {
'''
new_test = '''    #[test]
    fn batch_preprocessing_repeats_preprocessed_image_in_batch_order() {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(33, 17, Rgb([10u8, 20, 30])));
        let input = preprocess(&image).unwrap();
        let batched = repeat_input_for_batch(&input, 2).unwrap();
        assert_eq!(batched.len(), input.values.len() * 2);
        assert_eq!(&batched[..input.values.len()], input.values.as_slice());
        assert_eq!(&batched[input.values.len()..], input.values.as_slice());
        assert!(repeat_input_for_batch(&input, 0).is_err());
    }

''' + existing_test
adapter = replace_once(adapter, existing_test, new_test, "YuNet batch preprocessing test")
adapter_path.write_text(adapter, encoding="utf-8")


# 2) Extend the YuNet benchmark report with an inference-only batch support/throughput sweep.
bench_path = Path("src/face_yunet_benchmark.rs")
bench = bench_path.read_text(encoding="utf-8")
bench = replace_once(
    bench,
    '''    let mut predictions: BTreeMap<String, Vec<face_detection::DetectedFace>> = BTreeMap::new();
    let mut inference_times = Vec::with_capacity(manifest.images.len());

    for image in &manifest.images {
''',
    '''    let mut predictions: BTreeMap<String, Vec<face_detection::DetectedFace>> = BTreeMap::new();
    let mut inference_times = Vec::with_capacity(manifest.images.len());
    let mut batch_probe_image = None;

    for image in &manifest.images {
''',
    "YuNet batch probe cache",
)
bench = replace_once(
    bench,
    '''        let started = Instant::now();
        let detected = adapter
''',
    '''        if batch_probe_image.is_none() {
            batch_probe_image = Some(oriented.clone());
        }
        let started = Instant::now();
        let detected = adapter
''',
    "YuNet batch probe image",
)
bench = replace_once(
    bench,
    '''    append_latency(&mut report, &inference_times)?;
    writeln!(report)?;
''',
    '''    append_latency(&mut report, &inference_times)?;
    writeln!(
        report,
        "batch_probe_image_id={}",
        tsv_field(&manifest.images[0].image_id)
    )?;
    append_batch_sweep(
        &mut report,
        &mut adapter,
        batch_probe_image
            .as_ref()
            .context("YuNet batch probe image is unavailable")?,
    )?;
    writeln!(report)?;
''',
    "YuNet append batch sweep",
)

percentile_marker = '''fn percentile(sorted: &[f64], p: f64) -> f64 {
'''
batch_helper = '''fn append_batch_sweep(
    report: &mut String,
    adapter: &mut YuNetOnnxAdapter,
    image: &image::DynamicImage,
) -> Result<()> {
    const BATCH_SIZES: [usize; 5] = [1, 2, 4, 8, 16];
    const MEASURED_ITERATIONS: usize = 5;

    writeln!(report, "batch_sweep_begin")?;
    writeln!(report, "batch_sweep_iterations={MEASURED_ITERATIONS}")?;
    for batch_size in BATCH_SIZES {
        match adapter.benchmark_batch_inference(image, batch_size) {
            Ok(warmup) => {
                let mut times = Vec::with_capacity(MEASURED_ITERATIONS);
                let mut measurement_error = None;
                let mut geometry_stable = true;
                for iteration in 0..MEASURED_ITERATIONS {
                    match adapter.benchmark_batch_inference(image, batch_size) {
                        Ok(stats) => {
                            geometry_stable &= stats.batch_size == batch_size
                                && stats.padded_width == warmup.padded_width
                                && stats.padded_height == warmup.padded_height;
                            times.push(stats.inference.as_secs_f64() * 1000.0);
                        }
                        Err(err) => {
                            measurement_error = Some(format!(
                                "iteration {} of {} failed: {err:#}",
                                iteration + 1,
                                MEASURED_ITERATIONS
                            ));
                            break;
                        }
                    }
                }
                writeln!(report, "batch_{batch_size}_supported=true")?;
                writeln!(
                    report,
                    "batch_{batch_size}_warmup_ms={:.3}",
                    warmup.inference.as_secs_f64() * 1000.0
                )?;
                writeln!(
                    report,
                    "batch_{batch_size}_input={}x{}",
                    warmup.padded_width,
                    warmup.padded_height
                )?;
                writeln!(
                    report,
                    "batch_{batch_size}_measured_iterations={}",
                    times.len()
                )?;
                if let Some(error) = measurement_error {
                    writeln!(report, "batch_{batch_size}_measurement_succeeded=false")?;
                    writeln!(
                        report,
                        "batch_{batch_size}_measurement_error={}",
                        tsv_field(&error)
                    )?;
                    continue;
                }
                times.sort_by(f64::total_cmp);
                let total_ms = times.iter().sum::<f64>();
                let mean_ms = total_ms / times.len() as f64;
                let p50_ms = percentile(&times, 0.50);
                let images_per_second = if total_ms <= f64::EPSILON {
                    0.0
                } else {
                    (batch_size * times.len()) as f64 / (total_ms / 1000.0)
                };
                writeln!(report, "batch_{batch_size}_measurement_succeeded=true")?;
                writeln!(report, "batch_{batch_size}_mean_ms={mean_ms:.3}")?;
                writeln!(report, "batch_{batch_size}_p50_ms={p50_ms:.3}")?;
                writeln!(
                    report,
                    "batch_{batch_size}_images_per_second={images_per_second:.3}"
                )?;
                writeln!(report, "batch_{batch_size}_output_shapes_valid=true")?;
                writeln!(
                    report,
                    "batch_{batch_size}_input_geometry_stable={geometry_stable}"
                )?;
            }
            Err(err) => {
                writeln!(report, "batch_{batch_size}_supported=false")?;
                writeln!(report, "batch_{batch_size}_measurement_succeeded=false")?;
                writeln!(report, "batch_{batch_size}_measured_iterations=0")?;
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

''' + percentile_marker
bench = replace_once(bench, percentile_marker, batch_helper, "YuNet batch sweep helper")
bench_path.write_text(bench, encoding="utf-8")


# 3) Document detector and embedder batch evidence separately.
doc_path = Path("docs/face-benchmark-gate.md")
doc = doc_path.read_text(encoding="utf-8")
old_doc = '''The adapter reports retain the shared #92 evaluator metrics. YuNet reports IoU-based precision/recall/F1, no-face false positives and face-size recall buckets. SFace reports Recall@1/5/10, MRR, same/different-person distance distributions and threshold-sweep results. Both adapters also report model initialization time and persistent-session inference throughput. SFace additionally performs a benchmark-only batch-size sweep for 1/2/4/8/16 aligned faces using the same persistent ONNX session, recording support/failure, warm-up latency, mean/P50 batch latency, throughput, and output-dimension validation for each size. Batch failures are retained as evidence and do not change production embedding behavior.
'''
new_doc = '''The adapter reports retain the shared #92 evaluator metrics. YuNet reports IoU-based precision/recall/F1, no-face false positives and face-size recall buckets. SFace reports Recall@1/5/10, MRR, same/different-person distance distributions and threshold-sweep results. Both adapters also report model initialization time and persistent-session inference throughput. YuNet additionally performs a benchmark-only batch-size sweep for 1/2/4/8/16 copies of the first oriented benchmark image at its native padded input geometry, recording support/failure, warm-up latency, mean/P50 batch latency, throughput, output-shape validation, and geometry stability for each size. This detector sweep isolates ONNX session batch behavior and does not replace the normal per-image detector quality evaluation. SFace performs the corresponding benchmark-only 1/2/4/8/16 aligned-face sweep on the same persistent ONNX session. Batch failures are retained as evidence and do not change production detector or embedding behavior.
'''
doc = replace_once(doc, old_doc, new_doc, "face benchmark gate YuNet batch documentation")
doc_path.write_text(doc, encoding="utf-8")
