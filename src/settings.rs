//! The Settings page.

use crate::*;

/// Settings layout: a section heading.
pub(crate) fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(18.0);
    ui.label(egui::RichText::new(title).strong().size(17.0));
    ui.separator();
    ui.add_space(4.0);
}

/// Settings layout: the label in a fixed-width left column, lined up with the first line of the
/// setting, and the setting (with its hint lines) in the space to the right, wrapping there.
pub(crate) fn setting(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    const LABEL_WIDTH: f32 = 130.0;
    ui.horizontal_top(|ui| {
        let height = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(egui::vec2(LABEL_WIDTH, height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(LABEL_WIDTH);
            ui.label(label);
        });
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width().min(720.0));
            add(ui);
        });
    });
    ui.add_space(10.0);
}

/// Settings layout: one line of help under a setting, small and dimmed.
pub(crate) fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).small().weak()).wrap());
}

impl App {
    /// Settings apply and save as they change. The music folder applies when you leave its box
    /// (or pick one with Browse), and the text size when you let go of the slider.
    pub(crate) fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let before = self.settings.clone();
        ui.heading("Settings");
        ui.weak("Changes are saved as you make them.");

        section(ui, "Library");
        setting(ui, "Music folder", |ui| {
            ui.horizontal(|ui| {
                let mut text = self.draft.music_dir.display().to_string();
                let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(360.0));
                if edit.changed() {
                    self.draft.music_dir = PathBuf::from(text);
                }
                if edit.lost_focus() {
                    self.apply_music_dir(ui.ctx());
                }
                if ui.button("Browse…").clicked() {
                    let start = self.draft.music_dir.clone();
                    self.spawn(ui.ctx(), move |_| Msg::Picked(rfd::FileDialog::new().set_directory(start).pick_folder()));
                }
            });
            if self.draft.music_dir != self.settings.music_dir && !self.draft.music_dir.is_dir() {
                ui.colored_label(ui.visuals().warn_fg_color, "That folder doesn't exist, so the music folder hasn't changed.");
            } else {
                hint(ui, "Where songs are saved, as Artist / Album / Song. A network share works too.");
            }
        });
        setting(ui, "Player", |ui| {
            let players = backend::detected_players();
            let current = players.iter().find(|p| p.1 == self.settings.player).map_or("Custom command", |p| p.0);
            egui::ComboBox::from_id_salt("player").selected_text(current).show_ui(ui, |ui| {
                for (name, cmd) in &players {
                    ui.selectable_value(&mut self.settings.player, cmd.to_string(), *name);
                }
                if ui.selectable_label(current == "Custom command", "Custom command").clicked() && current != "Custom command" {
                    self.settings.player.clear();
                }
            });
            if current == "Custom command" {
                ui.add(egui::TextEdit::singleline(&mut self.settings.player).hint_text("e.g. strawberry").desired_width(360.0));
                hint(ui, "Mixtape adds the songs or folders after the command.");
            } else {
                hint(ui, "Plays songs and folders from the Library.");
            }
        });

        section(ui, "Downloads");
        setting(ui, "Format", |ui| {
            ui.radio_value(&mut self.settings.format, Format::Mp3, "MP3");
            hint(ui, "Plays everywhere. Converted, so slightly lower quality and about twice the size.");
            ui.add_space(4.0);
            ui.radio_value(&mut self.settings.format, Format::Original, "Original");
            hint(ui, "Best quality, no conversion: Opus (or AAC with Premium). Some car stereos and older devices can't play Opus.");
        });
        setting(ui, "Premium", |ui| {
            let browsers = backend::detected_browsers();
            let mut premium = !self.settings.cookies_browser.is_empty();
            if ui.add_enabled(!browsers.is_empty(), egui::Checkbox::new(&mut premium, "Use my YouTube Music Premium login")).changed() {
                self.settings.cookies_browser = if premium { browsers[0].1.to_string() } else { String::new() };
            }
            if premium {
                ui.horizontal(|ui| {
                    ui.label("Signed in with");
                    let current = browsers.iter().find(|b| b.1 == self.settings.cookies_browser).map_or("Choose a browser", |b| b.0);
                    egui::ComboBox::from_id_salt("browser").selected_text(current).show_ui(ui, |ui| {
                        for (name, id) in &browsers {
                            ui.selectable_value(&mut self.settings.cookies_browser, id.to_string(), *name);
                        }
                    });
                });
            }
            hint(
                ui,
                "Premium gets 256 kbps AAC instead of 130 kbps. Sign in to music.youtube.com in that browser first. \
                 Mixtape only passes the browser's name to yt-dlp, which reads the login on this computer.",
            );
        });

        section(ui, "App");
        setting(ui, "Text size", |ui| {
            let slider = ui.add(
                egui::Slider::new(&mut self.draft.text_scale, backend::TEXT_SCALES)
                    .step_by(0.1)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
            // Resizing mid-drag would move the slider out from under the pointer.
            if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                self.settings.text_scale = self.draft.text_scale;
                ui.ctx().set_zoom_factor(self.settings.text_scale);
            }
            hint(ui, "Ctrl + and Ctrl − change it anywhere; Ctrl 0 resets it.");
        });
        setting(ui, "Updates", |ui| {
            ui.checkbox(&mut self.settings.check_updates, "Check for a new version of Mixtape");
            hint(ui, "Asks GitHub at startup and once a day.");
        });
        setting(ui, "Notifications", |ui| {
            ui.checkbox(&mut self.settings.notify, "Tell me when downloads finish");
            hint(ui, "A desktop notification, only when Mixtape is in the background.");
        });
        section(ui, "Dependencies");
        hint(ui, "Programs Mixtape runs. Point at a name to see which copy it uses.");
        ui.add_space(4.0);
        egui::Grid::new("dependencies").num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
            for t in &backend::TOOLS {
                let (version, path) = self.versions.get(t.bin).cloned().unwrap_or_default();
                let installed = path.is_some() || (self.versions.is_empty() && backend::tool_installed(t));
                let (mark, color) = if installed {
                    ("✔", ui.visuals().hyperlink_color)
                } else if t.required {
                    ("✖", ui.visuals().error_fg_color)
                } else {
                    ("–", ui.visuals().weak_text_color())
                };
                ui.colored_label(color, mark);
                let name = ui.strong(t.bin);
                if let Some(path) = &path {
                    name.on_hover_text(path.display().to_string());
                }
                match (&version, installed) {
                    (Some(v), _) => ui.label(v),
                    (None, true) => ui.weak("…"),
                    (None, false) => ui.weak("not installed"),
                };
                let optional = if t.required { "" } else { " (optional)" };
                ui.weak(format!("{}{optional}", t.needed_for));
                ui.end_row();
            }
            if let Some((runtime, solver)) = self.challenges {
                let (ok, bad) = (ui.visuals().hyperlink_color, ui.visuals().warn_fg_color);
                ui.colored_label(if runtime.is_some() { ok } else { bad }, if runtime.is_some() { "✔" } else { "✖" });
                ui.strong(runtime.map_or("JavaScript", |(bin, _)| bin));
                ui.label(if runtime.is_some() { "found" } else { "not installed" });
                ui.weak("solving YouTube's download puzzles (Deno, Node, Bun or QuickJS)");
                ui.end_row();
                ui.colored_label(if solver { ok } else { bad }, if solver { "✔" } else { "✖" });
                ui.strong("yt-dlp-ejs");
                ui.label(if solver { "found" } else { "not installed" });
                ui.weak("yt-dlp's scripts for those puzzles");
                ui.end_row();
            }
        });
        if self.settings != before
            && let Err(e) = self.settings.save()
        {
            self.status = Some(format!("Could not save settings: {e}"));
        }
    }

    /// Switches to the folder typed or picked in Settings, if it exists.
    pub(crate) fn apply_music_dir(&mut self, ctx: &egui::Context) {
        if self.draft.music_dir == self.settings.music_dir || !self.draft.music_dir.is_dir() {
            return;
        }
        self.settings.music_dir = self.draft.music_dir.clone();
        match self.settings.save() {
            Ok(()) => self.notify("Music folder changed".into()),
            Err(e) => self.status = Some(format!("Could not save settings: {e}")),
        }
        self.selected.clear();
        self.rescan(ctx);
    }
}
