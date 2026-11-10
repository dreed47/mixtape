//! The download queue: starting, cancelling and stopping yt-dlp, and the Downloads panel.

use crate::*;

/// Process ids of the yt-dlp downloads running now. Dropping it (when Mixtape closes) stops them,
/// so the saved queue resumes them next time instead of a second copy writing the same file.
#[derive(Default, Clone)]
pub(crate) struct Running(pub(crate) std::sync::Arc<std::sync::Mutex<HashMap<String, u32>>>);

impl Running {
    pub(crate) fn set(&self, id: &str, pid: Option<u32>) {
        if let Ok(mut map) = self.0.lock() {
            match pid {
                Some(pid) => map.insert(id.to_string(), pid),
                None => map.remove(id),
            };
        }
    }

    pub(crate) fn stop(&self, id: &str) {
        if let Some(pid) = self.0.lock().ok().and_then(|m| m.get(id).copied()) {
            backend::stop(pid);
        }
    }
}

/// Owns the running downloads; stops them when the window closes.
pub(crate) struct Downloads(pub(crate) Running);

impl Drop for Downloads {
    fn drop(&mut self) {
        if let Ok(map) = self.0.0.lock() {
            map.values().for_each(|pid| backend::stop(*pid));
        }
    }
}

pub(crate) enum Job {
    Queued,
    Running(f32),
    /// The source quality, e.g. "256k AAC".
    Done(String),
    Failed(String),
}

pub(crate) fn job_status(ui: &mut egui::Ui, job: Option<&Job>, have: bool) {
    match job {
        Some(Job::Queued) => {
            ui.label("Waiting…");
        }
        Some(Job::Running(f)) if *f >= 1.0 => {
            ui.spinner();
            ui.label("Converting…");
        }
        Some(Job::Running(f)) => {
            ui.add(egui::ProgressBar::new(*f).desired_width(140.0).show_percentage());
        }
        Some(Job::Failed(e)) => {
            ui.colored_label(ui.visuals().error_fg_color, "Failed")
                .on_hover_text(format!("{}\n\nRight-click to copy the details.", backend::friendly(e)))
                .context_menu(|ui| {
                    if ui.button("Copy error details").clicked() {
                        ui.ctx().copy_text(e.clone());
                        ui.close();
                    }
                });
        }
        Some(Job::Done(source)) => {
            let label = if source.is_empty() { "✔ Downloaded".to_string() } else { format!("✔ Downloaded · {source}") };
            ui.colored_label(ui.visuals().hyperlink_color, label)
                .on_hover_text("The quality YouTube sent. Premium gets 256k AAC; free accounts get about 130k Opus.");
        }
        None if have => {
            ui.colored_label(ui.visuals().hyperlink_color, "✔ In library");
        }
        None => {}
    }
}

impl App {
    pub(crate) fn enqueue(&mut self, song: Song) {
        if matches!(self.jobs.get(&song.id), Some(Job::Queued | Job::Running(_))) {
            return;
        }
        self.jobs.insert(song.id.clone(), Job::Queued);
        self.history.retain(|s| s.id != song.id);
        self.history.push(song.clone());
        self.queue.push_back(song);
        self.save_queue();
    }

    /// Writes the waiting and running songs to disk, so they resume after a restart.
    pub(crate) fn save_queue(&mut self) {
        let pending: Vec<Song> =
            self.history.iter().filter(|s| matches!(self.jobs.get(&s.id), Some(Job::Queued | Job::Running(_)))).cloned().collect();
        if let Err(e) = backend::save_queue(&pending) {
            self.status = Some(format!("Could not save the download queue: {e}"));
        }
    }

    /// Drops songs that haven't started downloading: one, or all of them.
    pub(crate) fn cancel_waiting(&mut self, only: Option<&str>) {
        let cancel: Vec<String> = self.queue.iter().filter(|s| only.is_none_or(|id| s.id == id)).map(|s| s.id.clone()).collect();
        self.queue.retain(|s| !cancel.contains(&s.id));
        self.history.retain(|s| !cancel.contains(&s.id));
        for id in &cancel {
            self.jobs.remove(id);
        }
        self.save_queue();
    }

    pub(crate) fn downloads_panel(&mut self, ui: &mut egui::Ui) {
        let (mut retry, mut cancel, mut stop) = (None, None, None);
        if let Some(why) = self.paused.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ui.visuals().warn_fg_color, format!("Downloads paused. {why}"));
                if ui.button("Resume").clicked() {
                    self.paused = None;
                }
            });
        }
        ui.horizontal(|ui| {
            ui.heading("Downloads");
            if self.running + self.queue.len() == 0 && self.paused.is_none() && ui.small_button("Hide").clicked() {
                self.downloads_open = false;
            }
            let waiting = self.queue.len();
            if waiting > 0 && ui.button(format!("Cancel waiting ({waiting})")).on_hover_text("Songs already downloading finish").clicked() {
                cancel = Some(None);
            }
            let finished = |j: Option<&Job>| matches!(j, Some(Job::Done(_) | Job::Failed(_)));
            if ui.add_enabled(self.history.iter().any(|s| finished(self.jobs.get(&s.id))), egui::Button::new("Clear finished")).clicked() {
                let jobs = &self.jobs;
                self.history.retain(|s| !finished(jobs.get(&s.id)));
            }
        });
        if self.history.is_empty() {
            ui.weak("Tick songs or albums, then click Download selected.");
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            for song in &self.history {
                row(
                    ui,
                    |ui| {
                        match self.jobs.get(&song.id) {
                            Some(Job::Failed(_)) if ui.small_button("Retry").clicked() => retry = Some(song.clone()),
                            Some(Job::Queued) if ui.small_button("Cancel").clicked() => cancel = Some(Some(song.id.clone())),
                            Some(Job::Running(_)) if ui.small_button("Cancel").clicked() => stop = Some(song.id.clone()),
                            _ => {}
                        }
                        job_status(ui, self.jobs.get(&song.id), false);
                    },
                    |ui| {
                        fit(ui, egui::RichText::new(format!("{}  ", song.title)).strong());
                        fit(ui, &song.artists);
                    },
                );
            }
        });
        if let Some(song) = retry {
            self.jobs.remove(&song.id);
            self.enqueue(song);
        }
        if let Some(only) = cancel {
            self.cancel_waiting(only.as_deref());
        }
        if let Some(id) = stop {
            self.cancel_running(&id);
        }
    }

    pub(crate) fn start_downloads(&mut self, ctx: &egui::Context) {
        while self.running < PARALLEL_DOWNLOADS && self.paused.is_none() {
            let Some(song) = self.queue.pop_front() else { break };
            self.running += 1;
            self.jobs.insert(song.id.clone(), Job::Running(0.0));
            let settings = self.settings.clone();
            let overwrite = self.overwrite.remove(&song.id);
            let running = self.downloads.0.clone();
            self.spawn(ctx, move |tx| {
                let id = song.id.clone();
                let result = backend::download(
                    &settings,
                    &song,
                    overwrite,
                    |pid| running.set(&id, Some(pid)),
                    |f| {
                        let _ = tx.send(Msg::Progress(id.clone(), f));
                    },
                );
                running.set(&id, None);
                Msg::Done(song, result)
            });
        }
    }

    /// Stops a download that's running; the song drops out of the list.
    pub(crate) fn cancel_running(&mut self, id: &str) {
        self.cancelled.insert(id.to_string());
        self.downloads.0.stop(id);
    }
}
