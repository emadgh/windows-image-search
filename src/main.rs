mod ann;
mod db;
mod embedding;
mod fs_watch;
mod indexer;
mod metadata;
mod settings;
mod text_search;
mod thumbnail_cache;
mod ui;

use anyhow::{Context, Result};
use eframe::egui;
use std::path::PathBuf;

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

fn main() -> eframe::Result<()> {
    let (db_path, model_cache) = app_paths().unwrap_or_else(|_| {
        let fallback = PathBuf::from(".");
        (fallback.join("index.sqlite3"), fallback.join("models"))
    });
    let _ = db::open(&db_path);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Windows Image Search")
            .with_inner_size([1380.0, 860.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Windows Image Search",
        native_options,
        Box::new(move |_cc| Ok(Box::new(ui::ImageSearchApp::new(db_path, model_cache)))),
    )
}
