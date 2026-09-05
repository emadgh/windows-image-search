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
            .open(&mut open)
            .resizable(true)
            .default_size([860.0, 680.0])
            .min_size([620.0, 460.0])
            .max_height((ctx.available_rect().height() - 48.0).max(360.0))
            .show(ctx, |ui| {
                ui.heading("Collections");
                ui.label(
                    "Organize indexed folders and images without moving or deleting source files.",
                );
                ui.add_space(6.0);
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("collections-workspace-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.show_collections_settings(ui));
            });
        self.collections_open = open;
    }
}
