#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod ann;
mod db;
mod embedding;
mod face_benchmark;
mod face_detection;
mod face_embedding;
mod face_embedding_pipeline;
mod face_embedding_store;
mod face_pipeline;
#[cfg(test)]
mod face_portable_tests;
mod face_scope;
mod face_search;
mod face_settings;
mod face_sface_adapter;
mod face_sface_benchmark;
mod face_sface_production;
mod face_similarity;
mod face_store;
mod fs_watch;
mod indexer;
mod library_profile;
mod material_eval;
mod material_texture;
mod metadata;
mod model_benchmark;
mod portable;
mod portable_verify;
mod preview_benchmark;
mod runtime_benchmark;
mod settings;
mod text_search;
mod texture_benchmark;
mod thumbnail_cache;
mod ui;
mod windows_shell;

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
    FaceBenchmark(PathBuf),
    FaceBenchmarkValidate(PathBuf),
    SFaceBenchmark(PathBuf),
    PortableVerify(PathBuf, bool),
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
        if let Some(value) = arg.strip_prefix("--benchmark-face=") {
            if !value.trim().is_empty() {
                return StartupMode::FaceBenchmark(PathBuf::from(value));
            }
        }
        if arg == "--benchmark-face" {
            return StartupMode::FaceBenchmark(args.next().map(PathBuf::from).unwrap_or_default());
        }
        if let Some(value) = arg.strip_prefix("--benchmark-sface=") {
            if !value.trim().is_empty() {
                return StartupMode::SFaceBenchmark(PathBuf::from(value));
            }
        }
        if arg == "--benchmark-sface" {
            return StartupMode::SFaceBenchmark(args.next().map(PathBuf::from).unwrap_or_default());
        }
        if let Some(value) = arg.strip_prefix("--validate-face-benchmark=") {
            if !value.trim().is_empty() {
                return StartupMode::FaceBenchmarkValidate(PathBuf::from(value));
            }
        }
        if arg == "--validate-face-benchmark" {
            return StartupMode::FaceBenchmarkValidate(
                args.next().map(PathBuf::from).unwrap_or_default(),
            );
        }
        if let Some(value) = arg.strip_prefix("--verify-portable=") {
            if !value.trim().is_empty() {
                return StartupMode::PortableVerify(PathBuf::from(value), false);
            }
        }
        if arg == "--verify-portable" {
            return StartupMode::PortableVerify(
                args.next().map(PathBuf::from).unwrap_or_default(),
                false,
            );
        }
        if let Some(value) = arg.strip_prefix("--verify-portable-deep=") {
            if !value.trim().is_empty() {
                return StartupMode::PortableVerify(PathBuf::from(value), true);
            }
        }
        if arg == "--verify-portable-deep" {
            return StartupMode::PortableVerify(
                args.next().map(PathBuf::from).unwrap_or_default(),
                true,
            );
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

fn face_benchmark_report_path(db_path: &Path) -> PathBuf {
    benchmark_report_path(db_path, "face")
}

fn sface_benchmark_report_path(db_path: &Path) -> PathBuf {
    benchmark_report_path(db_path, "sface")
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

#[cfg(target_os = "windows")]
fn attach_parent_console_for_cli() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(target_os = "windows"))]
fn attach_parent_console_for_cli() {}

fn main() -> eframe::Result<()> {
    let mode = startup_mode();
    if !matches!(&mode, StartupMode::Gui) {
        attach_parent_console_for_cli();
    }
    if matches!(&mode, StartupMode::Version) {
        println!("{APP_TITLE}");
        return Ok(());
    }

    let (db_path, model_cache) = app_paths().unwrap_or_else(|_| {
        let fallback = PathBuf::from(".");
        (fallback.join("index.sqlite3"), fallback.join("models"))
    });
    if let StartupMode::PortableVerify(root, deep) = &mode {
        let verify_mode = if *deep {
            portable_verify::VerifyMode::DeepFingerprint
        } else {
            portable_verify::VerifyMode::Quick
        };
        match portable_verify::verify_root(
            root,
            portable_verify::VerifyOptions {
                mode: verify_mode,
                ..portable_verify::VerifyOptions::default()
            },
            |_| {},
        ) {
            Ok(report) => println!("{}", report.render_text(root, verify_mode)),
            Err(err) => benchmark_failed("Portable index verification", &err),
        }
        return Ok(());
    }

    // Keep GUI launch lightweight: database open/migration, portable-root hydration,
    // and the initial image list are loaded by ImageSearchApp on a background thread.
    // CLI modes still prepare the database synchronously before running diagnostics.
    if !matches!(&mode, StartupMode::Gui) {
        let _ = db::open(&db_path);
        let registered_roots = db::load_roots(&db_path).unwrap_or_default();
        let _ = portable::prepare_registered_roots(&db_path, &registered_roots);
    }

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

    if let StartupMode::FaceBenchmarkValidate(manifest_path) = &mode {
        match face_benchmark::validate_manifest(manifest_path) {
            Ok(report) => println!("{report}"),
            Err(err) => benchmark_failed("Face benchmark manifest validation", &err),
        }
        return Ok(());
    }

    if let StartupMode::FaceBenchmark(manifest_path) = &mode {
        match face_benchmark::benchmark(manifest_path) {
            Ok(report) => {
                write_benchmark_report(&face_benchmark_report_path(&db_path), &report, "face")
            }
            Err(err) => benchmark_failed("Face benchmark", &err),
        }
        return Ok(());
    }

    if let StartupMode::SFaceBenchmark(manifest_path) = &mode {
        match face_sface_benchmark::benchmark(manifest_path) {
            Ok(report) => write_benchmark_report(
                &sface_benchmark_report_path(&db_path),
                &report,
                "SFace ONNX",
            ),
            Err(err) => benchmark_failed("SFace ONNX benchmark", &err),
        }
        return Ok(());
    }

    if let StartupMode::FaceBenchmarkValidate(manifest_path) = &mode {
        match face_benchmark::validate_manifest(manifest_path) {
            Ok(report) => println!("{report}"),
            Err(err) => benchmark_failed("Face benchmark manifest validation", &err),
        }
        return Ok(());
    }

    if let StartupMode::FaceBenchmark(manifest_path) = &mode {
        match face_benchmark::benchmark(manifest_path) {
            Ok(report) => {
                write_benchmark_report(&face_benchmark_report_path(&db_path), &report, "face")
            }
            Err(err) => benchmark_failed("Face benchmark", &err),
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
