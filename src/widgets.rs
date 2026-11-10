//! Small pieces of UI shared by the tabs.

use crate::*;

/// A row with `right` drawn against the right edge first and `left` in the space left over, so a
/// long name is cut short ("…") instead of running into the details. Returns whether the pointer
/// is over the row.
pub(crate) fn row(ui: &mut egui::Ui, right: impl FnOnce(&mut egui::Ui), left: impl FnOnce(&mut egui::Ui)) -> bool {
    // The outer horizontal keeps the row as tall as its content; a bare right-to-left layout
    // would take all the height left in the window.
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            right(ui);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), left);
        });
    })
    .response
    .contains_pointer()
}

/// A label that ends in "…" when it doesn't fit; egui shows the full text on hover then.
pub(crate) fn fit(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Label::new(text).truncate())
}

/// A square cover from a URL, or blank space of the same size so rows line up.
pub(crate) fn cover(ui: &mut egui::Ui, url: &str, size: f32) {
    if url.starts_with("https://") {
        ui.add(egui::Image::new(url).fit_to_exact_size(egui::vec2(size, size)).corner_radius(3.0));
    } else {
        ui.add_space(size + ui.spacing().item_spacing.x);
    }
}

/// An error in plain words, with a button that copies the full details for a bug report.
pub(crate) fn error_line(ui: &mut egui::Ui, raw: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(ui.visuals().error_fg_color, backend::friendly(raw));
        if ui.small_button("Copy details").on_hover_text("Copy the full error, for a bug report").clicked() {
            ui.ctx().copy_text(raw.to_string());
        }
    });
}

/// A button filled with the theme accent, text dark or light for contrast.
pub(crate) fn accent_button(ui: &egui::Ui, label: String) -> egui::Button<'static> {
    let accent = ui.visuals().selection.bg_fill;
    let [r, g, b, _] = accent.to_array();
    let text = if 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 150.0 { egui::Color32::BLACK } else { egui::Color32::WHITE };
    egui::Button::new(egui::RichText::new(label).strong().color(text)).fill(accent)
}
