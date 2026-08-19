mod ann;
mod db;
mod embedding;
mod fs_watch;
mod indexer;
mod library_profile;
mod material_eval;
mod material_texture;
mod metadata;
mod model_benchmark;
mod portable;
mod preview_benchmark;
mod runtime_benchmark;
mod settings;
mod text_search;
mod texture_benchmark;
mod thumbnail_cache;
mod ui;

use anyhow::{Context, Result};
use eframe::egui;
use std::path::{Path, PathBuf};

const APP_TITLE: &str = concat!("Windows Image Search v", env!("CARGO_PKG_VERSION"));

enum StartupMode {
    Gui,
    Version,
    AnnBenchmark(usize),
    ClipPreviewBenchmark(usize),
    ClipRuntimeBenchmark(usize),
    ImageModelBenchmark(usize),
    LibraryProfile,
    MaterialEval(PathBuf),
    MaterialTextureBenchmark(usize),
}

fn app_paths() -> Result<(PathBuf, PathBuf)> {
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or(std::env::current_dir().context("determining application data directory")?)
        .join("WindowsImageSearch");
    std::fs::create_dir_all(&base)?;
    let db_path = base.join("index.sqlite3");
    let model_cache = base.join("models");
    Ok((db_path, model_cache))
}

fn startup_mode() -> StartupMode {
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        if arg == "--version" || arg == "-V" {
            return StartupMode::Version;
        }
        if let Some(value) = arg.strip_prefix("--benchmark-ann=") {
            let queries = value
                .parse::<usize>()
                .unwrap_or_else(|_| ann::default_benchmark_queries())
                .max(1);
            return StartupMode::AnnBenchmark(queries);
        }
        if arg == "--benchmark-ann" {
            let queries = args
                .peek()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_else(ann::default_benchmark_queries)
                .max(1);
            return StartupMode::AnnBenchmark(queries);
        }
        if let Some(value) = arg.strip_prefix("--benchmark-clip-preview=") {
            let samples = value
                .parse::<usize>()
                .unwrap_or_else(|_| preview_benchmark::default_sample_count())
                .max(3);
            return StartupMode::ClipPreviewBenchmark(samples);
        }
        if arg == "--benchmark-clip-preview" {
            let samples = args
                .peek()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_else(preview_benchmark::default_sample_count)
                .max(3);
            return StartupMode::ClipPreviewBenchmark(samples);
        }
        if let Some(value) = arg.strip_prefix("--benchmark-clip-runtime=") {
            let samples = value
                .parse::<usize>()
                .unwrap_or_else(|_| runtime_benchmark::default_sample_count())
                .max(1);
            return StartupMode::ClipRuntimeBenchmark(samples);
        }
        if arg == "--benchmark-clip-runtime" {
            let samples = args
                .peek()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_else(runtime_benchmark::default_sample_count)
                .max(1);
            return StartupMode::ClipRuntimeBenchmark(samples);
        }
        if let Some(value) = arg.strip_prefix("--benchmark-image-models=") {
            let queries = value
                .parse::<usize>()
                .unwrap_or_else(|_| model_benchmark::default_query_count())
                .max(1);
            return StartupMode::ImageModelBenchmark(queries);
        }
        if arg == "--benchmark-image-models" {
            let queries = args
                .peek()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_else(model_benchmark::default_query_count)
                .max(1);
            return StartupMode::ImageModelBenchmark(queries);
        }
        if arg == "--benchmark-library-profile" {
            return StartupMode::LibraryProfile;
        }
        if let Some(value) = arg.strip_prefix("--benchmark-material-eval=") {
            if !value.trim().is_empty() {
                return StartupMode::MaterialEval(PathBuf::from(value));
            }
        }
        if arg == "--benchmark-material-eval" {
            return StartupMode::MaterialEval(args.next().map(PathBuf::from).unwrap_or_default());
        }
        if let Some(value) = arg.strip_prefix("--benchmark-material-texture=") {
            let samples = value
                .parse::<usize>()
                .unwrap_or_else(|_| texture_benchmark::default_sample_count())
                .max(1);
            return StartupMode::MaterialTextureBenchmark(samples);
        }
        if arg == "--benchmark-material-texture" {
            let samples = args
                .peek()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_else(texture_benchmark::default_sample_count)
                .max(1);
            return StartupMode::MaterialTextureBenchmark(samples);
        }
    }
    StartupMode::Gui
}

fn ann_benchmark_report_path(db_path: &Path) -> PathBuf {
    benchmark_report_path(db_path, "ann")
}

fn preview_benchmark_report_path(db_path: &Path) -> PathBuf {
    benchmark_report_path(db_path, "clip-preview")
}

fn runtime_benchmark_report_path(db_path: &Path) -> PathBuf {
    benchmark_report_path(db_path, "clip-runtime")
}

fn image_model_benchmark_report_path(db_path: &Path) -> PathBuf {
    benchmark_report_path(db_path, "image-models")
}

fn library_profile_report_path(db_path: &Path) -> PathBuf {
    benchmark_report_path(db_path, "library-profile")
}

fn material_eval_report_path(db_path: &Path) -> PathBuf {
    benchmark_report_path(db_path, "material-eval")
}

fn material_texture_benchmark_report_path(db_path: &Path) -> PathBuf {
    benchmark_report_path(db_path, "material-texture")
}

fn benchmark_report_path(db_path: &Path, label: &str) -> PathBuf {
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            "{label}-benchmark-v{}-{timestamp}.txt",
            env!("CARGO_PKG_VERSION")
        ))
}

fn write_benchmark_report(destination: &Path, report: &str, label: &str) {
    match std::fs::write(destination, report) {
        Ok(()) => {
            println!("{report}");
            println!("report={}", destination.display());
        }
        Err(err) => {
            println!("{report}");
            eprintln!("Cannot save {label} benchmark report: {err}");
        }
    }
}

fn benchmark_failed(label: &str, err: &anyhow::Error) -> ! {
    eprintln!("{label} failed: {err:#}");
    std::process::exit(1);
}

fn main() -> eframe::Result<()> {
    let mode = startup_mode();
    if matches!(&mode, StartupMode::Version) {
        println!("{APP_TITLE}");
        return Ok(());
    }

    let (db_path, model_cache) = app_paths().unwrap_or_else(|_| {
        let fallback = PathBuf::from(".");
        (fallback.join("index.sqlite3"), fallback.join("models"))
    });
    let _ = db::open(&db_path);

    if let StartupMode::AnnBenchmark(query_count) = &mode {
        match ann::benchmark(&db_path, *query_count) {
            Ok(report) => {
                write_benchmark_report(&ann_benchmark_report_path(&db_path), &report, "ANN")
            }
            Err(err) => benchmark_failed("ANN benchmark", &err),
        }
        return Ok(());
    }

    if let StartupMode::ClipPreviewBenchmark(sample_count) = &mode {
        match preview_benchmark::benchmark(&db_path, &model_cache, *sample_count) {
            Ok(report) => write_benchmark_report(
                &preview_benchmark_report_path(&db_path),
                &report,
                "CLIP preview",
            ),
            Err(err) => benchmark_failed("CLIP preview benchmark", &err),
        }
        return Ok(());
    }

    if let StartupMode::ClipRuntimeBenchmark(sample_count) = &mode {
        match runtime_benchmark::benchmark(&db_path, &model_cache, *sample_count) {
            Ok(report) => write_benchmark_report(
                &runtime_benchmark_report_path(&db_path),
                &report,
                "CLIP runtime",
            ),
            Err(err) => benchmark_failed("CLIP runtime benchmark", &err),
        }
        return Ok(());
    }

    if let StartupMode::ImageModelBenchmark(query_count) = &mode {
        match model_benchmark::benchmark(&db_path, &model_cache, *query_count) {
            Ok(report) => write_benchmark_report(
                &image_model_benchmark_report_path(&db_path),
                &report,
                "image models",
            ),
            Err(err) => benchmark_failed("Image model benchmark", &err),
        }
        return Ok(());
    }

    if matches!(&mode, StartupMode::LibraryProfile) {
        match library_profile::benchmark(&db_path) {
            Ok(report) => write_benchmark_report(
                &library_profile_report_path(&db_path),
                &report,
                "library profile",
            ),
            Err(err) => benchmark_failed("Library profile benchmark", &err),
        }
        return Ok(());
    }

    if let StartupMode::MaterialEval(manifest_path) = &mode {
        match material_eval::benchmark(&db_path, &model_cache, manifest_path) {
            Ok(report) => write_benchmark_report(
                &material_eval_report_path(&db_path),
                &report,
                "labeled material evaluation",
            ),
            Err(err) => benchmark_failed("Labeled material evaluation", &err),
        }
        return Ok(());
    }

    if let StartupMode::MaterialTextureBenchmark(sample_count) = &mode {
        match texture_benchmark::benchmark(&db_path, *sample_count) {
            Ok(report) => write_benchmark_report(
                &material_texture_benchmark_report_path(&db_path),
                &report,
                "material texture",
            ),
            Err(err) => benchmark_failed("Material texture benchmark", &err),
        }
        return Ok(());
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_inner_size([1380.0, 860.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        APP_TITLE,
        native_options,
        Box::new(move |_cc| Ok(Box::new(ui::ImageSearchApp::new(db_path, model_cache)))),
    )
}
