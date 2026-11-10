//! The download queue and the Downloads panel.
//!
//! The queue lives in `Shared`, and the download threads run it themselves: take the next song,
//! write the download log and the saved queue, pause on a bot check, and send the "finished"
//! notification. The window only shows it. That matters because on Wayland a window on a hidden
//! workspace stops drawing (and so stops running any of its code) until you come back.

use crate::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The queue's state, behind `Shared`'s lock.
#[derive(Default)]
pub(crate) struct Queue {
    pub(crate) waiting: VecDeque<Song>,
    /// Songs downloading now.
    pub(crate) running: Vec<Song>,
    /// yt-dlp's process id for each running song, to stop it.
    pids: HashMap<String, u32>,
    /// Why new downloads aren't starting (bot check, rate limit), until Resume.
    pub(crate) paused: Option<String>,
    /// Songs to replace on disk ("Download again").
    pub(crate) overwrite: HashSet<String>,
    /// Running songs the user cancelled; not counted or reported as failed.
    cancelled: HashSet<String>,
    /// Songs finished and failed since the queue was last empty.
    batch: (usize, usize),
}

impl Queue {
    /// Writes the waiting and running songs to disk, so they resume after a restart.
    fn persist(&self) -> Result<(), String> {
        let pending: Vec<Song> = self.running.iter().chain(&self.waiting).cloned().collect();
        backend::save_queue(&pending)
    }
}

pub(crate) struct Shared {
    pub(crate) queue: Mutex<Queue>,
    workers: AtomicUsize,
    /// From Settings and the window, for deciding on a desktop notification.
    pub(crate) notify: AtomicBool,
    pub(crate) focused: AtomicBool,
    pub(crate) last_drawn: Mutex<Instant>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            queue: Mutex::default(),
            workers: AtomicUsize::new(0),
            notify: AtomicBool::new(true),
            focused: AtomicBool::new(true),
            last_drawn: Mutex::new(Instant::now()),
        }
    }
}

impl Shared {
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Not looking at Mixtape: another window has focus, or Mixtape hasn't drawn for a few seconds
    /// (while it's visible it redraws at least every 2 seconds), so it's on a hidden workspace.
    fn away(&self) -> bool {
        let hidden = self.last_drawn.lock().map(|t| t.elapsed().as_secs() >= 4).unwrap_or(false);
        hidden || !self.focused.load(Ordering::Relaxed)
    }
}

/// Owns the queue; stops the running downloads when the window closes (they resume next time).
pub(crate) struct Downloads(pub(crate) Arc<Shared>);

impl Drop for Downloads {
    fn drop(&mut self) {
        self.0.lock().pids.values().for_each(|pid| backend::stop(*pid));
    }
}

/// One download thread: runs songs from the queue until it's empty or paused.
fn worker(shared: Arc<Shared>, settings: Settings, tx: Sender<Msg>, ctx: egui::Context) {
    loop {
        let next = {
            let mut q = shared.lock();
            if q.paused.is_some() {
                None
            } else if let Some(song) = q.waiting.pop_front() {
                q.running.push(song.clone());
                let overwrite = q.overwrite.remove(&song.id);
                let _ = q.persist();
                Some((song, overwrite))
            } else {
                None
            }
        };
        let Some((song, overwrite)) = next else { break };
        let id = song.id.clone();
        let _ = tx.send(Msg::Started(id.clone()));
        ctx.request_repaint();
        let result = backend::download(
            &settings,
            &song,
            overwrite,
            |pid| {
                shared.lock().pids.insert(id.clone(), pid);
            },
            |f| {
                let _ = tx.send(Msg::Progress(id.clone(), f));
                ctx.request_repaint();
            },
        );
        let (cancelled, batch) = {
            let mut q = shared.lock();
            q.pids.remove(&id);
            q.running.retain(|s| s.id != id);
            let cancelled = q.cancelled.remove(&id);
            if !cancelled {
                match &result {
                    Ok(done) => {
                        let _ = backend::append_log(&id, &done.path);
                        q.batch.0 += 1;
                    }
                    Err(e) => {
                        q.batch.1 += 1;
                        if backend::is_blocked(e) && q.paused.is_none() {
                            q.paused = Some(backend::friendly(e));
                        }
                    }
                }
            }
            let _ = q.persist();
            let finished = q.running.is_empty() && (q.waiting.is_empty() || q.paused.is_some());
            (cancelled, finished.then(|| std::mem::take(&mut q.batch)))
        };
        let _ = tx.send(Msg::Done(song, result, cancelled));
        if let Some((ok, failed)) = batch
            && ok + failed > 0
        {
            let summary = batch_summary(ok, failed);
            if shared.notify.load(Ordering::Relaxed) && shared.away() {
                backend::desktop_notification("Downloads finished", &summary);
            }
            let _ = tx.send(Msg::BatchDone(ok, failed));
        }
        ctx.request_repaint();
    }
    shared.workers.fetch_sub(1, Ordering::Relaxed);
    ctx.request_repaint();
}

pub(crate) fn batch_summary(ok: usize, failed: usize) -> String {
    let songs = |n| if n == 1 { "1 song".to_string() } else { format!("{n} songs") };
    match failed {
        0 => format!("Downloaded {}", songs(ok)),
        _ => format!("Downloaded {}, {} failed", songs(ok), failed),
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
        let saved = {
            let mut q = self.downloads.0.lock();
            q.waiting.push_back(song);
            q.persist()
        };
        if let Err(e) = saved {
            self.status = Some(format!("Could not save the download queue: {e}"));
        }
    }

    /// (downloading, waiting) right now.
    pub(crate) fn queue_counts(&self) -> (usize, usize) {
        let q = self.downloads.0.lock();
        (q.running.len(), q.waiting.len())
    }

    pub(crate) fn paused(&self) -> Option<String> {
        self.downloads.0.lock().paused.clone()
    }

    /// Drops songs that haven't started downloading: one, or all of them.
    pub(crate) fn cancel_waiting(&mut self, only: Option<&str>) {
        let cancel: Vec<String> = {
            let mut q = self.downloads.0.lock();
            let cancel: Vec<String> = q.waiting.iter().filter(|s| only.is_none_or(|id| s.id == id)).map(|s| s.id.clone()).collect();
            q.waiting.retain(|s| !cancel.contains(&s.id));
            let _ = q.persist();
            cancel
        };
        self.history.retain(|s| !cancel.contains(&s.id));
        for id in &cancel {
            self.jobs.remove(id);
        }
    }

    pub(crate) fn downloads_panel(&mut self, ui: &mut egui::Ui) {
        let (mut retry, mut cancel, mut stop) = (None, None, None);
        if let Some(why) = self.paused() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ui.visuals().warn_fg_color, format!("Downloads paused. {why}"));
                if ui.button("Resume").clicked() {
                    self.downloads.0.lock().paused = None;
                }
            });
        }
        let (running, waiting) = self.queue_counts();
        ui.horizontal(|ui| {
            ui.heading("Downloads");
            if running + waiting == 0 && self.paused().is_none() && ui.small_button("Hide").clicked() {
                self.downloads_open = false;
            }
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

    /// Starts download threads (up to PARALLEL_DOWNLOADS) for songs waiting; each keeps going on
    /// its own until the queue is empty or paused, whether or not the window is drawing.
    pub(crate) fn start_downloads(&mut self, ctx: &egui::Context) {
        let shared = &self.downloads.0;
        shared.notify.store(self.settings.notify, Ordering::Relaxed);
        loop {
            let ready = {
                let q = shared.lock();
                q.paused.is_none() && !q.waiting.is_empty()
            };
            if !ready || shared.workers.load(Ordering::Relaxed) >= PARALLEL_DOWNLOADS {
                break;
            }
            shared.workers.fetch_add(1, Ordering::Relaxed);
            let (shared, settings, tx, ctx) = (shared.clone(), self.settings.clone(), self.tx.clone(), ctx.clone());
            std::thread::spawn(move || worker(shared, settings, tx, ctx));
        }
    }

    /// Stops a download that's running; the song drops out of the list.
    pub(crate) fn cancel_running(&mut self, id: &str) {
        let mut q = self.downloads.0.lock();
        q.cancelled.insert(id.to_string());
        if let Some(pid) = q.pids.get(id) {
            backend::stop(*pid);
        }
    }
}
