use super::ImageSearchApp;
use eframe::egui;

impl ImageSearchApp {
    pub(super) fn show_collections_workspace(&mut self, ctx: &egui::Context) {
        if !self.collections_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.collections_open = false;
            return;
        }

        let mut open = self.collections_open;
        egui::Window::new("Collections")
            .id(egui::Id::new("collections-workspace-v4"))
            .open(&mut open)
            .resizable(true)
            .default_size([1080.0, 620.0])
            .min_size([340.0, 280.0])
            .max_size(
                (ctx.available_rect().size() - egui::vec2(24.0, 48.0))
                    .max(egui::vec2(280.0, 240.0)),
            )
            .show(ctx, |ui| {
                egui::TopBottomPanel::bottom("collection-library-status")
                    .show_inside(ui, |ui| self.collection_index_status(ui));
                self.show_collections_settings(ui);
            });
        self.collections_open = open;
    }
}
