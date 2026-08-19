from pathlib import Path


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text(encoding="utf-8")
    if old not in text:
        raise SystemExit(f"expected block not found in {path}: {old[:120]!r}")
    path.write_text(text.replace(old, new, 1), encoding="utf-8")


indexer = Path("src/indexer.rs")
ui = Path("src/ui/mod.rs")

replace_once(
    indexer,
    "const COLOR_HISTOGRAM_BINS: usize = 64;\n\n#[derive(Debug)]",
    '''const COLOR_HISTOGRAM_BINS: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct SimilaritySettings {
    pub color_distribution_weight: f32,
    pub texture_weight: f32,
    pub clip_weight: f32,
    pub dominant_color_weight: f32,
    pub strict_color_rejection: bool,
    pub min_color_distribution_match: f32,
    pub max_dominant_color_difference: f32,
}

impl Default for SimilaritySettings {
    fn default() -> Self {
        Self {
            color_distribution_weight: 44.0,
            texture_weight: 31.0,
            clip_weight: 20.0,
            dominant_color_weight: 5.0,
            strict_color_rejection: true,
            min_color_distribution_match: 18.0,
            max_dominant_color_difference: 35.0,
        }
    }
}

#[derive(Debug)]''',
)

replace_once(
    indexer,
    '''    let _ = tx.send(WorkerMessage::Status("Scanning folders…".to_owned()));
    for root in roots {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if entry.file_type().is_file() && is_supported_image(entry.path()) {
                candidates.push((root.clone(), entry.into_path()));
            }
        }
    }

    let total = candidates.len();''',
    '''    let _ = tx.send(WorkerMessage::Status(
        "Scanning folders recursively…".to_owned(),
    ));
    let mut traversal_errors = 0usize;
    for root in roots {
        if !root.exists() {
            traversal_errors += 1;
            let _ = tx.send(WorkerMessage::Error(format!(
                "Indexed root does not exist: {}",
                root.display()
            )));
            continue;
        }

        for entry in WalkDir::new(root).follow_links(false).into_iter() {
            match entry {
                Ok(entry) => {
                    if entry.file_type().is_file() && is_supported_image(entry.path()) {
                        candidates.push((root.clone(), entry.into_path()));
                    }
                }
                Err(err) => {
                    traversal_errors += 1;
                    if traversal_errors <= 8 {
                        let _ = tx.send(WorkerMessage::Error(format!(
                            "Recursive scan could not access an entry under {}: {err}",
                            root.display()
                        )));
                    }
                }
            }
        }
    }

    let total = candidates.len();''',
)

replace_once(
    indexer,
    '''    let _ = tx.send(WorkerMessage::Status(format!(
        "Index ready: {total} image{}",
        if total == 1 { "" } else { "s" }
    )));''',
    '''    let _ = tx.send(WorkerMessage::Status(format!(
        "Index ready: {total} image{} (recursive scan, {traversal_errors} traversal error{})",
        if total == 1 { "" } else { "s" },
        if traversal_errors == 1 { "" } else { "s" }
    )));''',
)

replace_once(
    indexer,
    '''pub fn spawn_similarity_search(
    db_path: PathBuf,
    model_cache: PathBuf,
    query_path: PathBuf,
    tx: Sender<WorkerMessage>,
) {''',
    '''pub fn spawn_similarity_search(
    db_path: PathBuf,
    model_cache: PathBuf,
    query_path: PathBuf,
    settings: SimilaritySettings,
    tx: Sender<WorkerMessage>,
) {''',
)

replace_once(
    indexer,
    '''        match similarity_search(&db_path, &model_cache, &query_path, &tx) {''',
    '''        match similarity_search(&db_path, &model_cache, &query_path, settings, &tx) {''',
)

replace_once(
    indexer,
    '''fn similarity_search(
    db_path: &Path,
    model_cache: &Path,
    query_path: &Path,
    tx: &Sender<WorkerMessage>,
) -> Result<Vec<ImageRecord>> {''',
    '''fn similarity_search(
    db_path: &Path,
    model_cache: &Path,
    query_path: &Path,
    settings: SimilaritySettings,
    tx: &Sender<WorkerMessage>,
) -> Result<Vec<ImageRecord>> {''',
)

replace_once(
    indexer,
    '''        record.score = Some(hybrid_similarity(
            hash_similarity,
            histogram_similarity,
            clip_similarity,
            dominant_similarity,
            query_dominant,
            record.dominant,
        ));
    }

    records.sort_by(|a, b| {''',
    '''        if !passes_color_gate(histogram_similarity, dominant_similarity, settings) {
            record.score = None;
            continue;
        }

        record.score = Some(hybrid_similarity(
            hash_similarity,
            histogram_similarity,
            clip_similarity,
            dominant_similarity,
            settings,
        ));
    }

    records.retain(|record| {
        normalized_path_key(&record.path) == query_key || record.score.is_some()
    });

    records.sort_by(|a, b| {''',
)

old_hybrid = '''fn hybrid_similarity(
    hash_similarity: Option<f32>,
    histogram_similarity: Option<f32>,
    clip_similarity: Option<f32>,
    dominant_similarity: f32,
    query_dominant: [u8; 3],
    candidate_dominant: [u8; 3],
) -> f32 {
    // For material/texture libraries, local appearance is more important than
    // CLIP's semantic neighborhood. Histogram + perceptual hash therefore own
    // 75% of the normal score; CLIP is deliberately capped at 20% influence.
    let mut weighted = 0.05 * dominant_similarity;
    let mut weight = 0.05;

    if let Some(value) = histogram_similarity {
        weighted += 0.44 * value;
        weight += 0.44;
    }
    if let Some(value) = hash_similarity {
        weighted += 0.31 * value;
        weight += 0.31;
    }
    if let Some(value) = clip_similarity {
        weighted += 0.20 * value;
        weight += 0.20;
    }

    let mut score = if weight > 0.0 { weighted / weight } else { 0.0 };

    // CLIP often places brown marble, grayscale stone, cement, and travertine
    // close together. If the query is clearly chromatic, strongly suppress
    // candidates whose dominant color is essentially achromatic.
    let query_saturation = rgb_saturation(query_dominant);
    let candidate_saturation = rgb_saturation(candidate_dominant);
    if query_saturation > 0.18 && candidate_saturation < query_saturation * 0.45 {
        score *= 0.68;
    }

    // Also suppress globally different color distributions even if CLIP and
    // edge structure happen to agree.
    if histogram_similarity.is_some_and(|similarity| similarity < 0.20) {
        score *= 0.82;
    }

    score.clamp(0.0, 1.0)
}
'''
new_hybrid = '''fn passes_color_gate(
    histogram_similarity: Option<f32>,
    dominant_similarity: f32,
    settings: SimilaritySettings,
) -> bool {
    if !settings.strict_color_rejection {
        return true;
    }

    if histogram_similarity.is_some_and(|similarity| {
        similarity * 100.0 < settings.min_color_distribution_match
    }) {
        return false;
    }

    let dominant_difference = (1.0 - dominant_similarity).clamp(0.0, 1.0) * 100.0;
    dominant_difference <= settings.max_dominant_color_difference
}

fn hybrid_similarity(
    hash_similarity: Option<f32>,
    histogram_similarity: Option<f32>,
    clip_similarity: Option<f32>,
    dominant_similarity: f32,
    settings: SimilaritySettings,
) -> f32 {
    // User-controlled weights are normalized over whichever descriptors are
    // available for a candidate. They do not need to sum to exactly 100%.
    let mut weighted = 0.0f32;
    let mut weight = 0.0f32;

    let dominant_weight = settings.dominant_color_weight.max(0.0);
    if dominant_weight > 0.0 {
        weighted += dominant_weight * dominant_similarity;
        weight += dominant_weight;
    }

    let histogram_weight = settings.color_distribution_weight.max(0.0);
    if let Some(value) = histogram_similarity.filter(|_| histogram_weight > 0.0) {
        weighted += histogram_weight * value;
        weight += histogram_weight;
    }

    let texture_weight = settings.texture_weight.max(0.0);
    if let Some(value) = hash_similarity.filter(|_| texture_weight > 0.0) {
        weighted += texture_weight * value;
        weight += texture_weight;
    }

    let clip_weight = settings.clip_weight.max(0.0);
    if let Some(value) = clip_similarity.filter(|_| clip_weight > 0.0) {
        weighted += clip_weight * value;
        weight += clip_weight;
    }

    if weight <= f32::EPSILON {
        0.0
    } else {
        (weighted / weight).clamp(0.0, 1.0)
    }
}
'''
replace_once(indexer, old_hybrid, new_hybrid)

replace_once(
    indexer,
    '''        let colored_score = hybrid_similarity(
            Some(0.75),
            Some(0.72),
            Some(0.70),
            rgb_similarity(brown, similar_brown),
            brown,
            similar_brown,
        );
        let gray_score = hybrid_similarity(
            Some(0.75),
            Some(0.72),
            Some(0.70),
            rgb_similarity(brown, gray),
            brown,
            gray,
        );

        assert!(colored_score > gray_score);''',
    '''        let settings = SimilaritySettings::default();
        let colored_dominant = rgb_similarity(brown, similar_brown);
        let gray_dominant = rgb_similarity(brown, gray);

        assert!(passes_color_gate(Some(0.72), colored_dominant, settings));
        assert!(!passes_color_gate(Some(0.72), gray_dominant, settings));

        let colored_score = hybrid_similarity(
            Some(0.75),
            Some(0.72),
            Some(0.70),
            colored_dominant,
            settings,
        );
        let gray_score = hybrid_similarity(
            Some(0.75),
            Some(0.72),
            Some(0.70),
            gray_dominant,
            settings,
        );

        assert!(colored_score > gray_score);''',
)

replace_once(
    indexer,
    '''        let good = hybrid_similarity(
            Some(0.90),
            Some(0.88),
            Some(0.62),
            0.90,
            brown,
            [146, 86, 42],
        );
        let semantically_close_but_wrong = hybrid_similarity(
            Some(0.35),
            Some(0.12),
            Some(0.95),
            0.55,
            brown,
            [142, 142, 142],
        );

        assert!(good > semantically_close_but_wrong);''',
    '''        let settings = SimilaritySettings::default();
        let good = hybrid_similarity(
            Some(0.90),
            Some(0.88),
            Some(0.62),
            0.90,
            settings,
        );
        let semantically_close_but_wrong = hybrid_similarity(
            Some(0.35),
            Some(0.12),
            Some(0.95),
            0.55,
            settings,
        );

        assert!(good > semantically_close_but_wrong);''',
)

replace_once(
    indexer,
    '''    #[test]
    fn clip_cannot_outvote_bad_color_and_texture_match() {''',
    '''    #[test]
    fn custom_weights_change_ranking_influence() {
        let mut texture_only = SimilaritySettings::default();
        texture_only.color_distribution_weight = 0.0;
        texture_only.texture_weight = 100.0;
        texture_only.clip_weight = 0.0;
        texture_only.dominant_color_weight = 0.0;
        texture_only.strict_color_rejection = false;

        let texture_score = hybrid_similarity(
            Some(0.92),
            Some(0.05),
            Some(0.10),
            0.10,
            texture_only,
        );
        assert!((texture_score - 0.92).abs() < 1e-6);

        let mut clip_only = texture_only;
        clip_only.texture_weight = 0.0;
        clip_only.clip_weight = 100.0;
        let clip_score = hybrid_similarity(
            Some(0.92),
            Some(0.05),
            Some(0.77),
            0.10,
            clip_only,
        );
        assert!((clip_score - 0.77).abs() < 1e-6);
    }

    #[test]
    fn strict_color_gate_rejects_weak_histogram_match() {
        let mut settings = SimilaritySettings::default();
        settings.min_color_distribution_match = 40.0;
        settings.max_dominant_color_difference = 100.0;
        assert!(!passes_color_gate(Some(0.25), 0.95, settings));
        assert!(passes_color_gate(Some(0.60), 0.95, settings));
    }

    #[test]
    fn all_zero_weights_are_safe() {
        let settings = SimilaritySettings {
            color_distribution_weight: 0.0,
            texture_weight: 0.0,
            clip_weight: 0.0,
            dominant_color_weight: 0.0,
            strict_color_rejection: false,
            min_color_distribution_match: 0.0,
            max_dominant_color_difference: 100.0,
        };
        assert_eq!(
            hybrid_similarity(Some(1.0), Some(1.0), Some(1.0), 1.0, settings),
            0.0
        );
    }

    #[test]
    fn clip_cannot_outvote_bad_color_and_texture_match() {''',
)

# rgb_saturation is no longer needed after hidden penalties are replaced by
# explicit user-controlled hard rejection.
replace_once(
    indexer,
    '''fn rgb_saturation(rgb: [u8; 3]) -> f32 {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max <= f32::EPSILON {
        0.0
    } else {
        (max - min) / max
    }
}

''',
    '',
)

# UI: state + defaults.
replace_once(
    ui,
    '''    pub(super) query_image: Option<PathBuf>,
    pub(super) search_text: String,''',
    '''    pub(super) query_image: Option<PathBuf>,
    pub(super) similarity_settings: indexer::SimilaritySettings,
    pub(super) search_text: String,''',
)
replace_once(
    ui,
    '''            query_image: None,
            search_text: String::new(),''',
    '''            query_image: None,
            similarity_settings: indexer::SimilaritySettings::default(),
            search_text: String::new(),''',
)

replace_once(
    ui,
    '''    fn choose_similarity_image(&mut self) {
        if self.busy {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff"])
            .pick_file()
        else {
            return;
        };
        self.busy = true;
        self.last_error = None;
        self.query_image = Some(path.clone());
        indexer::spawn_similarity_search(
            self.db_path.clone(),
            self.model_cache.clone(),
            path,
            self.tx.clone(),
        );
    }
''',
    '''    fn choose_similarity_image(&mut self) {
        if self.busy {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff"])
            .pick_file()
        else {
            return;
        };
        self.run_similarity_search(path);
    }

    fn rerun_similarity_search(&mut self) {
        if let Some(path) = self.query_image.clone() {
            self.run_similarity_search(path);
        }
    }

    fn run_similarity_search(&mut self, path: PathBuf) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.last_error = None;
        self.query_image = Some(path.clone());
        self.status = "Starting image search with current controls…".into();
        indexer::spawn_similarity_search(
            self.db_path.clone(),
            self.model_cache.clone(),
            path,
            self.similarity_settings,
            self.tx.clone(),
        );
    }
''',
)

replace_once(
    ui,
    '''            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view_mode, ViewMode::Grid, "▦ Grid");
                ui.selectable_value(&mut self.view_mode, ViewMode::Details, "☷ Details");
                if self.view_mode == ViewMode::Grid {
                    ui.add(egui::Slider::new(&mut self.thumb_size, 96.0..=280.0).text("Thumbnail"));
                }
            });''',
    '''            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view_mode, ViewMode::Grid, "▦ Grid");
                ui.selectable_value(&mut self.view_mode, ViewMode::Details, "☷ Details");
                if self.view_mode == ViewMode::Grid {
                    ui.add(egui::Slider::new(&mut self.thumb_size, 96.0..=280.0).text("Thumbnail"));
                }
            });

            ui.collapsing("Image similarity controls", |ui| {
                ui.label("Weights are relative and are normalized automatically; they do not have to total 100%.");
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::Slider::new(
                            &mut self.similarity_settings.color_distribution_weight,
                            0.0..=100.0,
                        )
                        .text("Color distribution")
                        .suffix("%"),
                    );
                    ui.add(
                        egui::Slider::new(
                            &mut self.similarity_settings.texture_weight,
                            0.0..=100.0,
                        )
                        .text("Texture / pattern")
                        .suffix("%"),
                    );
                    ui.add(
                        egui::Slider::new(
                            &mut self.similarity_settings.clip_weight,
                            0.0..=100.0,
                        )
                        .text("CLIP semantic")
                        .suffix("%"),
                    );
                    ui.add(
                        egui::Slider::new(
                            &mut self.similarity_settings.dominant_color_weight,
                            0.0..=100.0,
                        )
                        .text("Dominant color")
                        .suffix("%"),
                    );
                });

                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(
                        &mut self.similarity_settings.strict_color_rejection,
                        "Reject color mismatches",
                    );
                    if self.similarity_settings.strict_color_rejection {
                        ui.add(
                            egui::Slider::new(
                                &mut self.similarity_settings.min_color_distribution_match,
                                0.0..=100.0,
                            )
                            .text("Minimum color-distribution match")
                            .suffix("%"),
                        );
                        ui.add(
                            egui::Slider::new(
                                &mut self.similarity_settings.max_dominant_color_difference,
                                5.0..=100.0,
                            )
                            .text("Maximum dominant-color difference")
                            .suffix("%"),
                        )
                        .on_hover_text("Lower values are stricter and reject cream/beige results sooner for a brown query.");
                    }
                });

                ui.horizontal_wrapped(|ui| {
                    let total = self.similarity_settings.color_distribution_weight
                        + self.similarity_settings.texture_weight
                        + self.similarity_settings.clip_weight
                        + self.similarity_settings.dominant_color_weight;
                    ui.small(format!("Weight total: {total:.0}% (normalized during scoring)"));
                    if ui.button("Reset 44 / 31 / 20 / 5").clicked() {
                        self.similarity_settings = indexer::SimilaritySettings::default();
                    }
                    if ui
                        .add_enabled(
                            !self.busy && self.query_image.is_some(),
                            egui::Button::new("Apply / re-run current image"),
                        )
                        .clicked()
                    {
                        self.rerun_similarity_search();
                    }
                });
            });''',
)

replace_once(
    ui,
    '''                ui.heading("Indexed folders");
                ui.small(format!("{} images", self.images.len()));''',
    '''                ui.heading("Indexed folders");
                ui.small(format!("{} images", self.images.len()));
                ui.small("Recursive indexing: ON (all subfolders)");''',
)

replace_once(
    ui,
    '''                if self.similarity_results.is_some() {
                    ui.small("CLIP similarity order");
                }''',
    '''                if self.similarity_results.is_some() {
                    ui.small("Hybrid similarity order using current weights");
                }''',
)

print("search controls patch applied")
