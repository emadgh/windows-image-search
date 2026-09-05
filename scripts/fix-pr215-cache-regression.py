from pathlib import Path

path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")

old_color = "crate::indexer::color_distance(record.dominant, self.target_color)"
new_color = "views::color_distance(record.dominant, self.target_color)"
if old_color not in text:
    raise SystemExit("color_distance target not found")
text = text.replace(old_color, new_color, 1)

if "    fn observe_text_search_input(&mut self) {" in text:
    raise SystemExit("text search helpers already present")

marker = "    pub(super) fn thumbnail(&mut self, path: &Path) -> Option<TextureHandle> {\n"
if marker not in text:
    raise SystemExit("thumbnail marker not found")

helpers = '''    fn observe_text_search_input(&mut self) {
        if self.search_text == self.text_search_observed {
            return;
        }
        self.text_search_observed = self.search_text.clone();
        self.text_search_generation = self.text_search_generation.wrapping_add(1);
        self.text_search_matches = None;

        if self.search_text.trim().is_empty() {
            self.text_search_due = None;
            self.text_search_pending = false;
        } else {
            self.text_search_due = Some(Instant::now() + Duration::from_millis(160));
            self.text_search_pending = true;
        }
    }

    fn refresh_text_search_after_data_change(&mut self) {
        if self.search_text.trim().is_empty() {
            return;
        }
        self.text_search_generation = self.text_search_generation.wrapping_add(1);
        self.text_search_due = Some(Instant::now() + Duration::from_millis(220));
        self.text_search_pending = true;
    }

    fn dispatch_text_search_if_due(&mut self) {
        let Some(due) = self.text_search_due else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        self.text_search_due = None;
        self.text_search_service
            .request(self.text_search_generation, self.search_text.clone());
    }

    fn process_text_search_results(&mut self) {
        while let Some(result) = self.text_search_service.try_recv() {
            if result.generation != self.text_search_generation || result.query != self.search_text
            {
                continue;
            }
            match result.paths {
                Ok(paths) => {
                    let count = paths.len();
                    self.text_search_matches = Some(paths);
                    self.text_search_pending = false;
                    self.status = format!(
                        "Indexed text search: {count} match{} in {} ms",
                        if count == 1 { "" } else { "es" },
                        result.elapsed_ms
                    );
                }
                Err(err) => {
                    self.text_search_matches = Some(HashSet::new());
                    self.text_search_pending = false;
                    self.last_error = Some(format!("Text search failed: {err}"));
                }
            }
        }
    }

'''
text = text.replace(marker, helpers + marker, 1)
path.write_text(text, encoding="utf-8")
