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

const APPLIED_DARK_MODE_ID: &str = "windows-image-search.applied-design-system-dark-mode";

fn resolved_dark_mode(appearance: AppearanceMode, system_dark: bool) -> bool {
    match appearance {
        AppearanceMode::System => system_dark,
        AppearanceMode::Light => false,
        AppearanceMode::Dark => true,
    }
}

impl ImageSearchApp {
    pub(super) fn apply_design_system(&mut self, ctx: &egui::Context) {
        let inherited_dark = ctx.style().visuals.dark_mode;
        let system_dark = *self.system_dark_mode.get_or_insert(inherited_dark);
        let dark = resolved_dark_mode(self.appearance_mode, system_dark);
        let state_id = egui::Id::new(APPLIED_DARK_MODE_ID);

        if ctx.data_mut(|data| data.get_temp::<bool>(state_id)) == Some(dark) {
            return;
        }

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
        ctx.data_mut(|data| data.insert_temp(state_id, dark));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_mode_resolves_expected_dark_state() {
        assert!(!resolved_dark_mode(AppearanceMode::System, false));
        assert!(resolved_dark_mode(AppearanceMode::System, true));
        assert!(!resolved_dark_mode(AppearanceMode::Light, true));
        assert!(resolved_dark_mode(AppearanceMode::Dark, false));
    }

    #[test]
    fn cached_dark_state_prevents_reapplying_unchanged_style() {
        let ctx = egui::Context::default();
        let state_id = egui::Id::new(APPLIED_DARK_MODE_ID);
        assert_eq!(ctx.data_mut(|data| data.get_temp::<bool>(state_id)), None);
        ctx.data_mut(|data| data.insert_temp(state_id, true));
        assert_eq!(
            ctx.data_mut(|data| data.get_temp::<bool>(state_id)),
            Some(true)
        );
    }
}
