from pathlib import Path

path = Path("src/face_sface_benchmark.rs")
text = path.read_text(encoding="utf-8")
old = '''        let supported = adapter.embed_aligned_batch(&batch);
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
                writeln!(
                    report,
                    "batch_{batch_size}_warmup_ms={:.3}",
                    warmup_stats.inference.as_secs_f64() * 1000.0
                )?;
                writeln!(report, "batch_{batch_size}_mean_ms={mean_ms:.3}")?;
                writeln!(report, "batch_{batch_size}_p50_ms={p50_ms:.3}")?;
                writeln!(
                    report,
                    "batch_{batch_size}_faces_per_second={faces_per_second:.3}"
                )?;
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
'''
new = '''        let supported = adapter.embed_aligned_batch(&batch);
        match supported {
            Ok((_warmup, warmup_stats)) => {
                let mut times = Vec::with_capacity(MEASURED_ITERATIONS);
                let mut dimensions_ok = true;
                let mut measurement_error = None;
                for iteration in 0..MEASURED_ITERATIONS {
                    match adapter.embed_aligned_batch(&batch) {
                        Ok((embeddings, stats)) => {
                            dimensions_ok &= embeddings.len() == batch_size
                                && embeddings.iter().all(|embedding| {
                                    embedding.len() == face_sface_adapter::SFACE_EMBEDDING_DIMENSION
                                });
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
                    warmup_stats.inference.as_secs_f64() * 1000.0
                )?;
                writeln!(report, "batch_{batch_size}_measured_iterations={}", times.len())?;
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
                let faces_per_second = if total_ms <= f64::EPSILON {
                    0.0
                } else {
                    (batch_size * times.len()) as f64 / (total_ms / 1000.0)
                };
                writeln!(report, "batch_{batch_size}_measurement_succeeded=true")?;
                writeln!(report, "batch_{batch_size}_mean_ms={mean_ms:.3}")?;
                writeln!(report, "batch_{batch_size}_p50_ms={p50_ms:.3}")?;
                writeln!(
                    report,
                    "batch_{batch_size}_faces_per_second={faces_per_second:.3}"
                )?;
                writeln!(report, "batch_{batch_size}_dimensions_ok={dimensions_ok}")?;
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
'''
if old not in text:
    raise SystemExit("batch sweep robustness target not found")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
