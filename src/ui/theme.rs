use super::{AppearanceMode, ImageSearchApp};
use eframe::egui;

pub(super) const SEARCH_SIDEBAR_DEFAULT: f32 = 310.0;
pub(super) const SEARCH_SIDEBAR_MIN: f32 = 270.0;
pub(super) const SEARCH_SIDEBAR_MAX: f32 = 430.0;
pub(super) const INSPECTOR_DEFAULT: f32 = 320.0;
pub(super) const INSPECTOR_MIN: f32 = 260.0;
pub(super) const INSPECTOR_MAX: f32 = 430.0;
pub(super) const TOP_BAR_HEIGHT: f32 = 38.0;
pub(super) const STATUS_BAR_HEIGHT: f32 = 34.0;

impl ImageSearchApp {
    pub(super) fn apply_design_system(&mut self, ctx: &egui::Context) {
        let inherited_dark = ctx.style().visuals.dark_mode;
        let system_dark = *self.system_dark_mode.get_or_insert(inherited_dark);
        let dark = match self.appearance_mode {
            AppearanceMode::System => system_dark,
            AppearanceMode::Light => false,
            AppearanceMode::Dark => true,
        };

        let mut style = (*ctx.style()).clone();
        style.visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.indent = 16.0;
        style.spacing.interact_size.y = 28.0;
        style.spacing.icon_spacing = 6.0;

        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(18.0));
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));

        let control_radius = egui::CornerRadius::same(6);
        style.visuals.window_corner_radius = egui::CornerRadius::same(8);
        style.visuals.menu_corner_radius = control_radius;
        style.visuals.widgets.noninteractive.corner_radius = control_radius;
        style.visuals.widgets.inactive.corner_radius = control_radius;
        style.visuals.widgets.hovered.corner_radius = control_radius;
        style.visuals.widgets.active.corner_radius = control_radius;
        style.visuals.widgets.open.corner_radius = control_radius;
        style.visuals.selection.stroke.width = 2.0;

        ctx.set_style(style);
    }
}
