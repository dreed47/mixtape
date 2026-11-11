//! Around the tabs: the footer, setup notices, keyboard shortcuts and text size keys.

use crate::*;

impl App {
    /// Keyboard shortcuts. Single keys (Space, Delete, ?) only count when no text box has the cursor.
    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let ctrl = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let typing = ctx.egui_wants_keyboard_input();
        let hovered = self.preview.hovered.take();
        let mut toggle_preview = None;
        ctx.input_mut(|i| {
            if i.consume_shortcut(&ctrl(Key::Num1)) {
                self.tab = Tab::Search;
            }
            if i.consume_shortcut(&ctrl(Key::Num2)) {
                self.tab = Tab::Library;
            }
            if i.consume_shortcut(&ctrl(Key::Comma)) {
                self.tab = Tab::Settings;
            }
            if i.consume_shortcut(&ctrl(Key::F)) {
                match self.tab {
                    Tab::Search => self.autofocus = true,
                    _ => {
                        self.tab = Tab::Library;
                        self.focus_filter = true;
                    }
                }
            }
            if i.consume_shortcut(&ctrl(Key::D)) && self.tab == Tab::Search {
                self.download_now = true;
            }
            if i.consume_key(Modifiers::NONE, Key::F5) {
                self.rescan_requested = true;
            }
            if i.consume_key(Modifiers::NONE, Key::F1) {
                self.show_keys = true;
            }
            if typing
                || self.confirm.is_some()
                || self.trash_confirm.is_some()
                || self.fix_confirm.is_some()
                || self.delete_confirm.is_some()
            {
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::Questionmark) || i.consume_key(Modifiers::SHIFT, Key::Slash) {
                self.show_keys = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Delete) && self.tab == Tab::Library {
                self.trash_now = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Space) {
                toggle_preview = Some(hovered.clone());
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                self.preview.stop();
                self.library_filter.clear();
            }
        });
        // Space previews the result under the pointer, or stops the one playing.
        match toggle_preview {
            Some(Some(id)) => {
                if let Err(e) = self.preview.toggle(id) {
                    self.status = Some(e);
                }
            }
            Some(None) => self.preview.stop(),
            None => {}
        }
        if std::mem::take(&mut self.rescan_requested) {
            self.rescan(ctx);
        }
    }

    pub(crate) fn keys_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_keys {
            return;
        }
        const KEYS: [(&str, &str); 12] = [
            ("Ctrl+1 / Ctrl+2", "Search / Library"),
            ("Ctrl+,", "Settings"),
            ("Ctrl+F", "Search box, or the Library filter"),
            ("Enter", "Search"),
            ("Space", "Preview the result under the pointer, or stop it"),
            ("Ctrl+D", "Download selected"),
            ("Delete", "Move the selected songs and folders to the Trash"),
            ("F5", "Rescan the music folder"),
            ("Esc", "Stop the preview and clear the Library filter"),
            ("Ctrl + / Ctrl − / Ctrl 0", "Bigger, smaller, normal text"),
            ("? or F1", "This list"),
            ("Double-click a song", "Play it (right-click for more)"),
        ];
        let modal = egui::Modal::new(egui::Id::new("keys")).show(ctx, |ui| {
            ui.heading("Keyboard shortcuts");
            ui.add_space(6.0);
            egui::Grid::new("keys-grid").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                for (keys, what) in KEYS {
                    ui.strong(keys);
                    ui.label(what);
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            if ui.button("Close").clicked() {
                self.show_keys = false;
            }
        });
        if modal.should_close() {
            self.show_keys = false;
        }
    }

    /// Ctrl +, Ctrl − and Ctrl 0 change the text size and save it.
    pub(crate) fn zoom_keys(&mut self, ctx: &egui::Context) {
        use egui::gui_zoom::kb_shortcuts::*;
        let step = ctx.input_mut(|i| {
            if i.consume_shortcut(&ZOOM_IN) || i.consume_shortcut(&ZOOM_IN_SECONDARY) {
                Some(0.1)
            } else if i.consume_shortcut(&ZOOM_OUT) {
                Some(-0.1)
            } else if i.consume_shortcut(&ZOOM_RESET) {
                Some(0.0)
            } else {
                None
            }
        });
        let Some(step) = step else { return };
        let (lo, hi) = (*backend::TEXT_SCALES.start(), *backend::TEXT_SCALES.end());
        let scale = if step == 0.0 { 1.0 } else { ((self.settings.text_scale + step) * 10.0).round() / 10.0 };
        self.settings.text_scale = scale.clamp(lo, hi);
        self.draft.text_scale = self.settings.text_scale;
        ctx.set_zoom_factor(self.settings.text_scale);
        if let Err(e) = self.settings.save() {
            self.status = Some(format!("Could not save settings: {e}"));
        }
    }

    /// Shown while a required tool is missing: what it's for and the command that installs it.
    pub(crate) fn setup_banner(&mut self, ui: &mut egui::Ui) {
        self.challenge_banner(ui);
        let required: Vec<_> = self.missing.iter().copied().filter(|t| t.required).collect();
        if required.is_empty() {
            return;
        }
        let warn = ui.visuals().warn_fg_color;
        egui::Frame::group(ui.style()).stroke(egui::Stroke::new(1.5, warn)).inner_margin(12.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.colored_label(warn, egui::RichText::new("Mixtape needs a few more programs").strong());
            for t in &required {
                ui.label(format!("•  {} for {}", t.bin, t.needed_for));
            }
            let (command, notes) = backend::install_hint(&required);
            if let Some(command) = &command {
                ui.label("Install them with:");
                ui.horizontal(|ui| {
                    ui.code(command);
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(command.clone());
                    }
                });
            }
            for note in notes {
                ui.label(note);
            }
            if ui.button("Check again").clicked() {
                self.missing = backend::missing_tools();
                self.stems = backend::stems_installed();
                self.preview.mpv = backend::mpv_installed();
                self.read_versions(ui.ctx());
            }
        });
        ui.add_space(6.0);
    }

    /// YouTube makes downloaders solve a JavaScript puzzle; without a runtime and yt-dlp's solver
    /// scripts, some songs are missing formats or fail.
    pub(crate) fn challenge_banner(&mut self, ui: &mut egui::Ui) {
        let Some((runtime, solver)) = self.challenges else { return };
        if runtime.is_some() && solver {
            return;
        }
        let mut steps = Vec::new();
        if runtime.is_none() {
            steps.push(("a JavaScript runtime (Deno)", backend::js_runtime_hint()));
        }
        if !solver {
            steps.push(("yt-dlp's challenge solver (yt-dlp-ejs)", backend::challenge_solver_hint()));
        }
        let warn = ui.visuals().warn_fg_color;
        egui::Frame::group(ui.style()).stroke(egui::Stroke::new(1.5, warn)).inner_margin(12.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.colored_label(warn, egui::RichText::new("YouTube downloads need one more piece").strong());
            ui.label(
                "YouTube now has downloaders solve a small JavaScript puzzle. Without this, some songs download in lower quality or fail.",
            );
            for (what, how) in &steps {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("•  {what}:"));
                    if how.starts_with("See ") {
                        ui.label(how);
                    } else {
                        ui.code(how);
                        if ui.small_button("Copy").clicked() {
                            ui.ctx().copy_text(how.clone());
                        }
                    }
                });
            }
            if ui.button("Check again").clicked() {
                self.read_versions(ui.ctx());
            }
        });
        ui.add_space(6.0);
    }

    pub(crate) fn read_versions(&self, ctx: &egui::Context) {
        self.spawn(ctx, |_| Msg::Challenges(backend::js_runtime(), backend::challenge_solver_installed()));
        self.spawn(ctx, |_| {
            Msg::Versions(backend::TOOLS.iter().map(|t| (t.bin, backend::tool_version(t), backend::tool_path(t.bin))).collect())
        });
    }

    pub(crate) fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.weak(concat!("Mixtape ", env!("CARGO_PKG_VERSION")));
            // YouTube changes often, and an out-of-date yt-dlp starts failing.
            if let Some((installed, latest, command)) = &self.ytdlp_outdated {
                let command = command.clone();
                ui.colored_label(ui.visuals().warn_fg_color, format!("· yt-dlp {installed} is out of date ({latest} is out)"))
                    .on_hover_text(if command.starts_with("uv tool install") {
                        format!(
                            "YouTube changes often, and older yt-dlp versions stop working. Your system's package \
                             doesn't have {latest} yet, so install the newest yt-dlp just for you (no sudo):\n\n{command}"
                        )
                    } else {
                        format!("YouTube changes often, and older yt-dlp versions stop working. Update it with:\n\n{command}")
                    });
                if ui.small_button("Copy update command").clicked() {
                    ui.ctx().copy_text(command);
                    self.notify("Copied".into());
                }
            }
            match &self.update {
                Update::None => {}
                Update::Available(v) => {
                    let v = *v;
                    ui.label(format!("· {} available", backend::version_text(v)));
                    match backend::source() {
                        backend::Source::Clone(dir) => {
                            if ui.small_button("Update").clicked() {
                                self.update = Update::Running(v);
                                self.spawn(ui.ctx(), move |_| Msg::Updated(backend::self_update(&dir)));
                            }
                        }
                        backend::Source::Development => {
                            ui.weak("(development copy: update it with git)");
                        }
                        backend::Source::Unknown => {
                            ui.weak("(run bin/update.sh in your Mixtape folder)");
                        }
                    }
                }
                Update::Running(v) => {
                    ui.spinner();
                    ui.label(format!("Updating to {}…", backend::version_text(*v)));
                }
                Update::Done(v) => {
                    ui.label(format!("· {} installed", backend::version_text(*v)));
                    let busy = self.queue_counts() != (0, 0);
                    let restart = ui.add_enabled(!busy, egui::Button::new("Restart to finish").small());
                    if restart.on_disabled_hover_text("Wait for the downloads to finish").clicked() {
                        match backend::relaunch() {
                            Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(e) => self.status = Some(e),
                        }
                    }
                }
                Update::Failed(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, "· Update failed").on_hover_text(backend::friendly(e));
                    if ui.small_button("Dismiss").clicked() {
                        self.update = Update::None;
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⌨ Shortcuts").on_hover_text("? or F1").clicked() {
                    self.show_keys = true;
                }
            });
        });
    }
}
