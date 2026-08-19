mod ann;
mod db;
mod embedding;
mod fs_watch;
mod indexer;
mod material_texture;
mod metadata;
mod settings;
mod text_search;
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
    }
    StartupMode::Gui
}

fn benchmark_report_path(db_path: &Path) -> PathBuf {
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            "ann-benchmark-v{}-{timestamp}.txt",
            env!("CARGO_PKG_VERSION")
        ))
}

fn main() -> eframe::Result<()> {
    let mode = startup_mode();
    if matches!(mode, StartupMode::Version) {
        println!("{APP_TITLE}");
        return Ok(());
    }

    let (db_path, model_cache) = app_paths().unwrap_or_else(|_| {
        let fallback = PathBuf::from(".");
        (fallback.join("index.sqlite3"), fallback.join("models"))
    });
    let _ = db::open(&db_path);

    if let StartupMode::AnnBenchmark(query_count) = mode {
        match ann::benchmark(&db_path, query_count) {
            Ok(report) => {
                let destination = benchmark_report_path(&db_path);
                match std::fs::write(&destination, &report) {
                    Ok(()) => {
                        println!("{report}");
                        println!("report={}", destination.display());
                    }
                    Err(err) => {
                        println!("{report}");
                        eprintln!("Cannot save benchmark report: {err}");
                    }
                }
            }
            Err(err) => eprintln!("ANN benchmark failed: {err:#}"),
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
