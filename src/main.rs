//! Mixtape: search YouTube Music, download songs as MP3 into an Artist/Album library, play folders.

mod backend;
mod theme;

use backend::{Album, Folder, Format, Scan, SearchResult, Settings, Song};
use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

const PARALLEL_DOWNLOADS: usize = 2;

enum Msg {
    Search(u64, Result<SearchResult, String>),
    Album(String, Result<Vec<Song>, String>),
    Progress(String, f32),
    Done(Song, Result<backend::Downloaded, String>),
    Scanned(Scan),
    Picked(Option<PathBuf>),
    Release(Result<Option<backend::Version>, String>),
    Updated(Result<(), String>),
    Trashed(Vec<PathBuf>, Result<(), String>),
    Probed(Vec<(PathBuf, backend::AudioInfo)>),
}

/// What a click in the Library tree asks for.
enum LibraryAction {
    Play(PathBuf),
    Stems(PathBuf),
    /// Open a folder in the file manager.
    Reveal(PathBuf),
}

/// The footer's update state.
enum Update {
    None,
    Available(backend::Version),
    Running(backend::Version),
    /// Installed; restart to use it.
    Done(backend::Version),
    Failed(String),
}

/// Plays one search result at a time through mpv, without saving it.
struct Preview {
    mpv: bool,
    playing: Option<(String, std::process::Child)>,
    /// The ▶ or ⏹ clicked this frame.
    clicked: Option<String>,
}

impl Preview {
    fn current(&self) -> Option<&str> {
        self.playing.as_ref().map(|(id, _)| id.as_str())
    }

    /// Starts this song, or stops it if it's the one playing.
    fn toggle(&mut self, id: String) -> Result<(), String> {
        let same = self.current() == Some(id.as_str());
        self.stop();
        if !same {
            self.playing = Some((id.clone(), backend::preview(&id)?));
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some((_, mut child)) = self.playing.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Forgets a preview that has played to the end.
    fn poll(&mut self) {
        if let Some((_, child)) = &mut self.playing {
            if !matches!(child.try_wait(), Ok(None)) {
                self.playing = None;
            }
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.stop();
    }
}

enum Job {
    Queued,
    Running(f32),
    /// The source quality, e.g. "256k AAC".
    Done(String),
    Failed(String),
}

struct Row {
    song: Song,
    checked: bool,
}

struct AlbumRow {
    album: Album,
    open: bool,
    checked: bool,
    loading: bool,
    error: Option<String>,
    tracks: Option<Vec<Row>>,
}

#[derive(PartialEq)]
enum Tab {
    Search,
    Library,
    Settings,
}

struct App {
    tab: Tab,
    theme: theme::Theme,
    settings: Settings,
    draft: Settings,
    status: Option<String>,

    query: String,
    /// Put the cursor in the search box the first time the Search tab shows.
    autofocus: bool,
    search_gen: u64,
    searching: bool,
    search_error: Option<String>,
    songs: Vec<Row>,
    albums: Vec<AlbumRow>,
    confirm: Option<Song>,

    jobs: HashMap<String, Job>,
    queue: VecDeque<Song>,
    running: usize,
    /// Every song queued this session, oldest first, for the Downloads list.
    history: Vec<Song>,

    logged: HashSet<String>,
    scan: Option<Scan>,
    scanning: bool,
    selected: HashSet<PathBuf>,
    stems: bool,
    /// Selected songs and folders waiting on "Move to Trash?".
    trash_confirm: Option<Vec<PathBuf>>,
    update: Update,
    preview: Preview,
    /// When GitHub was last asked for a newer release.
    update_checked: Option<std::time::Instant>,
    /// Format, bitrate, length and size of songs shown in the Library, read in the background.
    details: HashMap<PathBuf, backend::AudioInfo>,
    probing: HashSet<PathBuf>,

    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

fn in_library(logged: &HashSet<String>, scan: &Option<Scan>, s: &Song) -> bool {
    logged.contains(&s.id) || scan.as_ref().is_some_and(|sc| sc.keys.contains(&backend::key(&s.artist, &s.title)))
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let settings = Settings::load();
        theme::install_fonts(ctx);
        ctx.options_mut(|o| o.zoom_with_keyboard = false); // handled in zoom_keys so the size is saved
        ctx.set_zoom_factor(settings.text_scale.clamp(*backend::TEXT_SCALES.start(), *backend::TEXT_SCALES.end()));
        let mut app = Self {
            tab: Tab::Library,
            theme: theme::Theme::new(),
            draft: settings.clone(),
            settings,
            status: None,
            query: String::new(),
            autofocus: true,
            search_gen: 0,
            searching: false,
            search_error: None,
            songs: vec![],
            albums: vec![],
            confirm: None,
            jobs: HashMap::new(),
            queue: VecDeque::new(),
            running: 0,
            history: vec![],
            logged: backend::read_log(),
            scan: None,
            scanning: false,
            selected: HashSet::new(),
            stems: backend::stems_installed(),
            trash_confirm: None,
            update: Update::None,
            preview: Preview { mpv: backend::mpv_installed(), playing: None, clicked: None },
            update_checked: None,
            details: HashMap::new(),
            probing: HashSet::new(),
            tx,
            rx,
        };
        app.rescan(ctx);
        app
    }

    fn spawn(&self, ctx: &egui::Context, job: impl FnOnce(&Sender<Msg>) -> Msg + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let msg = job(&tx);
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn rescan(&mut self, ctx: &egui::Context) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        let dir = self.settings.music_dir.clone();
        self.spawn(ctx, move |_| Msg::Scanned(backend::scan(&dir)));
    }

    fn rows_mut(&mut self) -> impl Iterator<Item = &mut Row> {
        self.songs.iter_mut().chain(self.albums.iter_mut().flat_map(|a| a.tracks.iter_mut().flatten()))
    }

    fn handle(&mut self, ctx: &egui::Context, msg: Msg) {
        match msg {
            Msg::Search(generation, result) if generation == self.search_gen => {
                self.searching = false;
                match result {
                    Ok(r) => {
                        self.songs = r.songs.into_iter().map(|song| Row { song, checked: false }).collect();
                        self.albums = r
                            .albums
                            .into_iter()
                            .map(|album| AlbumRow { album, open: false, checked: false, loading: false, error: None, tracks: None })
                            .collect();
                    }
                    Err(e) => self.search_error = Some(e),
                }
            }
            Msg::Search(..) => {} // results of an older search
            Msg::Album(id, result) => {
                let (logged, scan) = (&self.logged, &self.scan);
                if let Some(a) = self.albums.iter_mut().find(|a| a.album.id == id) {
                    a.loading = false;
                    match result {
                        Ok(tracks) => {
                            let checked = a.checked;
                            a.tracks = Some(
                                tracks.into_iter().map(|song| Row { checked: checked && !in_library(logged, scan, &song), song }).collect(),
                            );
                        }
                        Err(e) => a.error = Some(e),
                    }
                }
            }
            Msg::Progress(id, f) => {
                self.jobs.insert(id, Job::Running(f));
            }
            Msg::Done(song, result) => {
                self.running -= 1;
                let job = match result {
                    Ok(backend::Downloaded { path, source }) => {
                        if let Err(e) = backend::append_log(&song.id, &path) {
                            self.status = Some(format!("Could not write the download log: {e}"));
                        }
                        self.logged.insert(song.id.clone());
                        Job::Done(source)
                    }
                    Err(e) => Job::Failed(e),
                };
                self.jobs.insert(song.id, job);
                if self.queue.is_empty() && self.running == 0 {
                    self.rescan(ctx);
                }
            }
            Msg::Scanned(scan) => {
                self.scanning = false;
                self.selected.retain(|p| p.starts_with(&scan.root.path));
                self.scan = Some(scan);
                self.details.clear(); // files may have changed; read again when shown
            }
            Msg::Probed(found) => {
                for (path, info) in found {
                    self.probing.remove(&path);
                    self.details.insert(path, info);
                }
            }
            Msg::Trashed(paths, result) => {
                match result.and_then(|_| backend::forget(&paths)) {
                    Ok(()) => self.status = None,
                    Err(e) => self.status = Some(format!("Could not move to the Trash: {e}")),
                }
                self.logged = backend::read_log();
                self.selected.clear();
                self.rescan(ctx);
            }
            Msg::Release(Ok(Some(latest))) if latest > backend::current_version() => {
                if !matches!(self.update, Update::Running(_) | Update::Done(_)) {
                    self.update = Update::Available(latest);
                }
            }
            Msg::Release(_) => {} // no newer release, or GitHub unreachable: say nothing
            Msg::Updated(result) => {
                self.update = match (result, &self.update) {
                    (Ok(()), Update::Running(v)) => Update::Done(*v),
                    (Err(e), _) => Update::Failed(e),
                    (Ok(()), _) => Update::None,
                };
            }
            Msg::Picked(dir) => self.draft.music_dir = dir.unwrap_or(self.draft.music_dir.clone()),
        }
    }

    fn enqueue(&mut self, song: Song) {
        if matches!(self.jobs.get(&song.id), Some(Job::Queued | Job::Running(_))) {
            return;
        }
        self.jobs.insert(song.id.clone(), Job::Queued);
        self.history.retain(|s| s.id != song.id);
        self.history.push(song.clone());
        self.queue.push_back(song);
    }

    fn downloads_panel(&mut self, ui: &mut egui::Ui) {
        let mut retry = None;
        ui.horizontal(|ui| {
            ui.heading("Downloads");
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
                ui.horizontal(|ui| {
                    ui.strong(&song.title);
                    ui.label(&song.artists);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(Job::Failed(_)) = self.jobs.get(&song.id) {
                            if ui.small_button("Retry").clicked() {
                                retry = Some(song.clone());
                            }
                        }
                        job_status(ui, self.jobs.get(&song.id), false);
                    });
                });
            }
        });
        if let Some(song) = retry {
            self.jobs.remove(&song.id);
            self.enqueue(song);
        }
    }

    fn start_downloads(&mut self, ctx: &egui::Context) {
        while self.running < PARALLEL_DOWNLOADS {
            let Some(song) = self.queue.pop_front() else { break };
            self.running += 1;
            self.jobs.insert(song.id.clone(), Job::Running(0.0));
            let settings = self.settings.clone();
            self.spawn(ctx, move |tx| {
                let id = song.id.clone();
                let result = backend::download(&settings, &song, |f| {
                    let _ = tx.send(Msg::Progress(id.clone(), f));
                });
                Msg::Done(song, result)
            });
        }
    }

    fn search_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Artist, song or album (spelling doesn't need to be exact)")
                    .desired_width(ui.available_width() - 80.0),
            );
            if std::mem::take(&mut self.autofocus) {
                edit.request_focus();
            }
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Search").clicked() || enter) && !self.query.trim().is_empty() {
                self.search_gen += 1;
                self.searching = true;
                self.search_error = None;
                let (generation, q) = (self.search_gen, self.query.trim().to_string());
                self.spawn(ui.ctx(), move |_| Msg::Search(generation, backend::search(&q)));
            }
        });

        let selected: Vec<Song> = {
            let mut seen = HashSet::new();
            let rows = self.songs.iter().chain(self.albums.iter().flat_map(|a| a.tracks.iter().flatten()));
            rows.filter(|r| r.checked && seen.insert(r.song.id.clone())).map(|r| r.song.clone()).collect()
        };
        ui.horizontal(|ui| {
            let label = format!("Download selected ({})", selected.len());
            if ui.add_enabled(!selected.is_empty(), accent_button(ui, label)).clicked() {
                selected.into_iter().for_each(|song| self.enqueue(song));
                self.rows_mut().for_each(|r| r.checked = false);
                self.albums.iter_mut().for_each(|a| a.checked = false);
            }
            let queued = self.queue.len();
            if self.running + queued > 0 {
                ui.spinner();
                ui.label(format!("Downloading {}, {} waiting", self.running, queued));
            }
            if self.searching {
                ui.spinner();
                ui.label("Searching…");
            }
        });
        if let Some(e) = &self.search_error {
            ui.colored_label(ui.visuals().error_fg_color, format!("Search failed: {e}"));
        }
        ui.separator();

        let mut wants_confirm = None;
        let mut load_album = None;
        let Self { songs, albums, jobs, logged, scan, preview, .. } = self;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if !albums.is_empty() {
                ui.heading("Albums");
                for a in albums.iter_mut() {
                    ui.horizontal(|ui| {
                        let arrow = if a.open { "⏷" } else { "⏵" };
                        if ui.button(arrow).on_hover_text("Show tracks").clicked() {
                            a.open = !a.open;
                        }
                        if ui.checkbox(&mut a.checked, "").on_hover_text("Select every track not already in your library").changed() {
                            if let Some(tracks) = &mut a.tracks {
                                for t in tracks {
                                    t.checked = a.checked && !in_library(logged, scan, &t.song);
                                }
                            }
                        }
                        ui.vertical(|ui| {
                            ui.strong(&a.album.title);
                            ui.label(format!("{}  {}", a.album.artists, a.album.year));
                        });
                        if a.loading {
                            ui.spinner();
                        }
                    });
                    if (a.open || a.checked) && a.tracks.is_none() && !a.loading && a.error.is_none() {
                        a.loading = true;
                        load_album = Some(a.album.id.clone());
                    }
                    if let Some(e) = &a.error {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    if a.open {
                        if let Some(tracks) = &mut a.tracks {
                            ui.indent(&a.album.id, |ui| {
                                for row in tracks.iter_mut() {
                                    song_row(ui, row, jobs, logged, scan, &mut wants_confirm, preview);
                                }
                            });
                        }
                    }
                }
                ui.add_space(8.0);
            }
            if !songs.is_empty() {
                ui.heading("Songs");
                for row in songs.iter_mut() {
                    song_row(ui, row, jobs, logged, scan, &mut wants_confirm, preview);
                }
            }
        });
        if let Some(id) = self.preview.clicked.take() {
            if let Err(e) = self.preview.toggle(id) {
                self.status = Some(e);
            }
        }
        if let Some(song) = wants_confirm {
            self.confirm = Some(song);
        }
        if let Some(id) = load_album {
            self.spawn(ui.ctx(), move |_| {
                let r = backend::album_tracks(&id);
                Msg::Album(id, r)
            });
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(song) = self.confirm.clone() else { return };
        let mut answer = None;
        egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
            ui.heading("Already in your library");
            ui.label(format!("\"{}\" by {} is already in your library. Download it again?", song.title, song.artists));
            ui.horizontal(|ui| {
                if ui.button("Download again").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            if yes {
                self.rows_mut().filter(|r| r.song.id == song.id).for_each(|r| r.checked = true);
            }
            self.confirm = None;
        }
    }

    fn library_tab(&mut self, ui: &mut egui::Ui) {
        let dir = self.settings.music_dir.clone();
        ui.horizontal(|ui| {
            let n = self.selected.len();
            let mut picked: Vec<PathBuf> = self.selected.iter().cloned().collect();
            picked.sort();
            if ui.add_enabled(n > 0, egui::Button::new(format!("Play selected ({n})"))).clicked() {
                if let Err(e) = backend::open_with(&self.settings.player, &picked) {
                    self.status = Some(e);
                }
            }
            // Never the music folder itself, and nothing inside a folder that's also going.
            let trash: Vec<PathBuf> = picked
                .iter()
                .filter(|p| **p != dir && !picked.iter().any(|q| q != *p && p.starts_with(q)))
                .cloned()
                .collect();
            if ui.add_enabled(!trash.is_empty(), egui::Button::new(format!("Move to Trash ({})", trash.len()))).clicked() {
                self.trash_confirm = Some(trash);
            }
            if ui.add_enabled(n > 0, egui::Button::new("Clear selection")).clicked() {
                self.selected.clear();
            }
            if ui.button("Open in file manager").on_hover_text("Open the music folder").clicked() {
                if let Err(e) = backend::open_with("xdg-open", &[dir.clone()]) {
                    self.status = Some(e);
                }
            }
            if ui.button("Refresh").clicked() {
                self.rescan(ui.ctx());
            }
            if self.scanning {
                ui.spinner();
            }
            if let Some(scan) = &self.scan {
                ui.label(format!("{} songs", scan.songs));
            }
        });
        ui.separator();
        if !dir.is_dir() {
            ui.colored_label(ui.visuals().error_fg_color, format!("Music folder not found: {}. Set it in Settings.", dir.display()));
            return;
        }
        let Some(scan) = &self.scan else { return };
        let mut tree = Tree {
            selected: &mut self.selected,
            stems: self.stems,
            action: None,
            details: &self.details,
            probing: &self.probing,
            wanted: vec![],
        };
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| folder_row(ui, &scan.root, &mut tree, true));
        let (action, wanted) = (tree.action, tree.wanted);
        if !wanted.is_empty() {
            self.probing.extend(wanted.iter().cloned());
            self.spawn(ui.ctx(), move |_| Msg::Probed(wanted.into_iter().map(|p| { let i = backend::probe(&p); (p, i) }).collect()));
        }
        let result = match action {
            Some(LibraryAction::Play(song)) => backend::open_with(&self.settings.player, &[song]),
            Some(LibraryAction::Stems(song)) => backend::open_with(backend::STEMS, &[song]),
            Some(LibraryAction::Reveal(folder)) => backend::open_with("xdg-open", &[folder]),
            None => Ok(()),
        };
        if let Err(e) = result {
            self.status = Some(e);
        }
    }

    /// Ctrl +, Ctrl − and Ctrl 0 change the text size and save it.
    fn zoom_keys(&mut self, ctx: &egui::Context) {
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

    fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.weak(concat!("Mixtape ", env!("CARGO_PKG_VERSION")));
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
                    let busy = self.running + self.queue.len() > 0;
                    let restart = ui.add_enabled(!busy, egui::Button::new("Restart to finish").small());
                    if restart.on_disabled_hover_text("Wait for the downloads to finish").clicked() {
                        match backend::relaunch() {
                            Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(e) => self.status = Some(e),
                        }
                    }
                }
                Update::Failed(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, "· Update failed").on_hover_text(e);
                    if ui.small_button("Dismiss").clicked() {
                        self.update = Update::None;
                    }
                }
            }
        });
    }

    fn trash_dialog(&mut self, ctx: &egui::Context) {
        let Some(paths) = &self.trash_confirm else { return };
        let mut answer = None;
        egui::Modal::new(egui::Id::new("trash")).show(ctx, |ui| {
            let n = paths.len();
            ui.heading(if n == 1 { "Move 1 item to the Trash?".to_string() } else { format!("Move {n} items to the Trash?") });
            for p in paths.iter().take(8) {
                let kind = if p.is_dir() { "Folder" } else { "Song" };
                ui.label(format!("{kind}: {}", p.file_name().unwrap_or_default().to_string_lossy()));
            }
            if n > 8 {
                ui.label(format!("…and {} more", n - 8));
            }
            ui.add_space(4.0);
            ui.weak("You can restore them from the Trash in your file manager.");
            ui.horizontal(|ui| {
                if ui.button("Move to Trash").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            let paths = self.trash_confirm.take().unwrap();
            if yes {
                self.spawn(ctx, move |_| {
                    let r = backend::trash(&paths);
                    Msg::Trashed(paths, r)
                });
            }
        }
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("settings").num_columns(2).spacing([12.0, 12.0]).show(ui, |ui| {
            ui.label("Music folder");
            ui.horizontal(|ui| {
                let mut text = self.draft.music_dir.display().to_string();
                if ui.add(egui::TextEdit::singleline(&mut text).desired_width(360.0)).changed() {
                    self.draft.music_dir = PathBuf::from(text);
                }
                if ui.button("Browse…").clicked() {
                    let start = self.draft.music_dir.clone();
                    self.spawn(ui.ctx(), move |_| Msg::Picked(rfd::FileDialog::new().set_directory(start).pick_folder()));
                }
            });
            ui.end_row();

            ui.label("Player");
            ui.vertical(|ui| {
                let players = backend::detected_players();
                let current = players.iter().find(|p| p.1 == self.draft.player).map_or("Custom command", |p| p.0);
                egui::ComboBox::from_id_salt("player").selected_text(current).show_ui(ui, |ui| {
                    for (name, cmd) in &players {
                        ui.selectable_value(&mut self.draft.player, cmd.to_string(), *name);
                    }
                    if ui.selectable_label(current == "Custom command", "Custom command").clicked() && current != "Custom command" {
                        self.draft.player.clear();
                    }
                });
                if current == "Custom command" {
                    ui.add(egui::TextEdit::singleline(&mut self.draft.player).hint_text("e.g. strawberry").desired_width(360.0));
                    ui.small("Mixtape adds the selected folders after the command.");
                }
            });
            ui.end_row();

            ui.label("Format");
            ui.vertical(|ui| {
                ui.radio_value(&mut self.draft.format, Format::Mp3, "MP3: plays everywhere");
                ui.radio_value(&mut self.draft.format, Format::Original, "Original: best quality, no conversion (Opus, or AAC with Premium)");
                ui.small("Original files are about half the size. Some car stereos and older devices can't play Opus.");
            });
            ui.end_row();

            ui.label("Text size");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.draft.text_scale, backend::TEXT_SCALES).step_by(0.1).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                ui.small("Ctrl + and Ctrl − work anywhere");
            });
            ui.end_row();

            ui.label("Updates");
            ui.checkbox(&mut self.draft.check_updates, "Check GitHub for a new version at startup and once a day");
            ui.end_row();

            ui.label("Premium");
            ui.vertical(|ui| {
                let browsers = backend::detected_browsers();
                let mut premium = !self.draft.cookies_browser.is_empty();
                if ui.add_enabled(!browsers.is_empty(), egui::Checkbox::new(&mut premium, "Use my YouTube Music Premium login")).changed() {
                    self.draft.cookies_browser = if premium { browsers[0].1.to_string() } else { String::new() };
                }
                if premium {
                    let current = browsers.iter().find(|b| b.1 == self.draft.cookies_browser).map_or("Choose a browser", |b| b.0);
                    egui::ComboBox::from_id_salt("browser").selected_text(current).show_ui(ui, |ui| {
                        for (name, id) in &browsers {
                            ui.selectable_value(&mut self.draft.cookies_browser, id.to_string(), *name);
                        }
                    });
                }
                ui.small("Premium gets 256 kbps AAC instead of 130 kbps. Sign in to music.youtube.com in that browser first. \
                          Mixtape only passes the browser's name to yt-dlp, which reads the login on this computer.");
            });
            ui.end_row();
        });
        ui.add_space(12.0);
        let changed = self.draft != self.settings;
        if ui.add_enabled(changed, egui::Button::new("Save")).clicked() {
            let folder_changed = self.draft.music_dir != self.settings.music_dir;
            self.settings = self.draft.clone();
            ui.ctx().set_zoom_factor(self.settings.text_scale);
            self.status = self.settings.save().err().map(|e| format!("Could not save settings: {e}"));
            if folder_changed {
                self.selected.clear();
                self.rescan(ui.ctx());
            }
        }
    }
}

fn song_row(
    ui: &mut egui::Ui,
    row: &mut Row,
    jobs: &HashMap<String, Job>,
    logged: &HashSet<String>,
    scan: &Option<Scan>,
    wants_confirm: &mut Option<Song>,
    preview: &mut Preview,
) {
    let have = in_library(logged, scan, &row.song);
    ui.horizontal(|ui| {
        if ui.checkbox(&mut row.checked, "").changed() && row.checked && have {
            row.checked = false;
            *wants_confirm = Some(row.song.clone());
        }
        if preview.mpv {
            let playing = preview.current() == Some(row.song.id.as_str());
            let (icon, tip) = if playing { ("⏹", "Stop the preview") } else { ("▶", "Preview: listen without downloading") };
            if ui.button(icon).on_hover_text(tip).clicked() {
                preview.clicked = Some(row.song.id.clone());
            }
        }
        ui.vertical(|ui| {
            ui.strong(&row.song.title);
            let album = if row.song.album.is_empty() { String::new() } else { format!(" · {}", row.song.album) };
            ui.label(format!("{}{album} · {}", row.song.artists, row.song.duration));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            job_status(ui, jobs.get(&row.song.id), have);
        });
    });
}

fn job_status(ui: &mut egui::Ui, job: Option<&Job>, have: bool) {
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
            ui.colored_label(ui.visuals().error_fg_color, "Failed").on_hover_text(e);
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

/// A button filled with the theme accent, text dark or light for contrast.
fn accent_button(ui: &egui::Ui, label: String) -> egui::Button<'static> {
    let accent = ui.visuals().selection.bg_fill;
    let [r, g, b, _] = accent.to_array();
    let text = if 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 150.0 { egui::Color32::BLACK } else { egui::Color32::WHITE };
    egui::Button::new(egui::RichText::new(label).strong().color(text)).fill(accent)
}

/// A tick box that adds `path` to or removes it from the selection.
fn select_box(ui: &mut egui::Ui, path: &PathBuf, selected: &mut HashSet<PathBuf>, label: impl Into<egui::WidgetText>) -> egui::Response {
    let mut checked = selected.contains(path);
    let response = ui.checkbox(&mut checked, label);
    if response.changed() {
        if checked {
            selected.insert(path.clone());
        } else {
            selected.remove(path);
        }
    }
    response
}

/// What the Library tree reads and fills in while it's drawn.
struct Tree<'a> {
    selected: &'a mut HashSet<PathBuf>,
    stems: bool,
    action: Option<LibraryAction>,
    details: &'a HashMap<PathBuf, backend::AudioInfo>,
    probing: &'a HashSet<PathBuf>,
    /// Shown songs whose details haven't been read yet.
    wanted: Vec<PathBuf>,
}

/// One folder in the Library tree, then its subfolders and songs, each with a tick box.
/// Double-click plays a song; right-click opens a menu.
fn folder_row(ui: &mut egui::Ui, folder: &Folder, tree: &mut Tree, default_open: bool) {
    let header = |ui: &mut egui::Ui, tree: &mut Tree| {
        select_box(ui, &folder.path, tree.selected, &folder.name).context_menu(|ui| {
            if ui.button("Open in file manager").clicked() {
                tree.action = Some(LibraryAction::Reveal(folder.path.clone()));
                ui.close();
            }
        });
    };
    if folder.children.is_empty() && folder.files.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(ui.spacing().indent);
            header(ui, tree);
        });
        return;
    }
    let id = ui.make_persistent_id(&folder.path);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open)
        .show_header(ui, |ui| header(ui, tree))
        .body(|ui| {
            for child in &folder.children {
                folder_row(ui, child, tree, false);
            }
            for file in &folder.files {
                let name = file.file_stem().unwrap_or_default().to_string_lossy();
                let stems = tree.stems;
                let action = &mut tree.action;
                ui.horizontal(|ui| {
                    select_box(ui, file, tree.selected, "");
                    let row = ui.add(egui::Label::new(format!("♪  {name}")).sense(egui::Sense::click()));
                    if row.double_clicked() {
                        *action = Some(LibraryAction::Play(file.clone()));
                    }
                    row.on_hover_text("Double-click to play, right-click for more").context_menu(|ui| {
                        if ui.button("Play").clicked() {
                            *action = Some(LibraryAction::Play(file.clone()));
                            ui.close();
                        }
                        if stems && ui.button("Open in Stems").clicked() {
                            *action = Some(LibraryAction::Stems(file.clone()));
                            ui.close();
                        }
                        if ui.button("Show in file manager").clicked() {
                            *action = Some(LibraryAction::Reveal(folder.path.clone()));
                            ui.close();
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match tree.details.get(file) {
                        Some(info) => {
                            ui.weak(info.describe());
                        }
                        None => {
                            if !tree.probing.contains(file) {
                                tree.wanted.push(file.clone());
                            }
                        }
                    });
                });
            }
        });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.theme.update(&ctx);
        self.zoom_keys(&ctx);
        self.preview.poll();
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(&ctx, msg);
        }
        self.start_downloads(&ctx);
        const DAY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
        if self.settings.check_updates && self.update_checked.is_none_or(|t| t.elapsed() > DAY) {
            self.update_checked = Some(std::time::Instant::now());
            self.spawn(&ctx, |_| Msg::Release(backend::latest_release()));
        }
        egui::Panel::bottom("footer").show(ui, |ui| self.footer(ui));
        egui::Panel::bottom("downloads").resizable(true).default_size(180.0).min_size(80.0).show(ui, |ui| {
            ui.add_space(6.0);
            self.downloads_panel(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Library, "Library");
                ui.selectable_value(&mut self.tab, Tab::Search, "Search");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let gear = egui::RichText::new("⚙").size(20.0);
                    ui.selectable_value(&mut self.tab, Tab::Settings, gear).on_hover_text("Settings");
                });
            });
            if let Some(s) = self.status.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, s);
                    if ui.small_button("✖").clicked() {
                        self.status = None;
                    }
                });
            }
            ui.separator();
            match self.tab {
                Tab::Search => self.search_tab(ui),
                Tab::Library => self.library_tab(ui),
                Tab::Settings => self.settings_tab(ui),
            }
        });
        self.confirm_dialog(&ctx);
        self.trash_dialog(&ctx);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([820.0, 620.0]).with_app_id("mixtape").with_title("Mixtape"),
        ..Default::default()
    };
    eframe::run_native("Mixtape", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))))
}
