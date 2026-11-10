//! Mixtape: search YouTube Music, download songs as MP3 into an Artist/Album library, play folders.

mod backend;
mod theme;

use backend::{Album, Folder, Scan, SearchResult, Settings, Song};
use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

const PARALLEL_DOWNLOADS: usize = 2;

enum Msg {
    Search(u64, Result<SearchResult, String>),
    Album(String, Result<Vec<Song>, String>),
    Progress(String, f32),
    Done(Song, Result<PathBuf, String>),
    Scanned(Scan),
    Picked(Option<PathBuf>),
}

enum Job {
    Queued,
    Running(f32),
    Done,
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
    /// Put the cursor in the search box when the window opens.
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
        let mut app = Self {
            tab: Tab::Search,
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
                    Ok(path) => {
                        if let Err(e) = backend::append_log(&song.id, &path) {
                            self.status = Some(format!("Could not write the download log: {e}"));
                        }
                        self.logged.insert(song.id.clone());
                        Job::Done
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
            let finished = |j: Option<&Job>| matches!(j, Some(Job::Done | Job::Failed(_)));
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
            let dir = self.settings.music_dir.clone();
            self.spawn(ctx, move |tx| {
                let id = song.id.clone();
                let result = backend::download(&dir, &song, |f| {
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
        let Self { songs, albums, jobs, logged, scan, .. } = self;
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
                                    song_row(ui, row, jobs, logged, scan, &mut wants_confirm);
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
                    song_row(ui, row, jobs, logged, scan, &mut wants_confirm);
                }
            }
        });
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
            if ui.add_enabled(n > 0, egui::Button::new(format!("Play selected ({n})"))).clicked() {
                let mut folders: Vec<PathBuf> = self.selected.iter().cloned().collect();
                folders.sort();
                if let Err(e) = backend::open_with(&self.settings.player, &folders) {
                    self.status = Some(e);
                }
            }
            if ui.add_enabled(n > 0, egui::Button::new("Clear selection")).clicked() {
                self.selected.clear();
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
        let mut open = None;
        let (selected, stems) = (&mut self.selected, self.stems);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| folder_row(ui, &scan.root, selected, stems, &mut open, true));
        if let Some((with_stems, song)) = open {
            let command = if with_stems { backend::STEMS } else { self.settings.player.as_str() };
            if let Err(e) = backend::open_with(command, &[song]) {
                self.status = Some(e);
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
        });
        ui.add_space(12.0);
        let changed = self.draft != self.settings;
        if ui.add_enabled(changed, egui::Button::new("Save")).clicked() {
            let folder_changed = self.draft.music_dir != self.settings.music_dir;
            self.settings = self.draft.clone();
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
) {
    let have = in_library(logged, scan, &row.song);
    ui.horizontal(|ui| {
        if ui.checkbox(&mut row.checked, "").changed() && row.checked && have {
            row.checked = false;
            *wants_confirm = Some(row.song.clone());
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
        Some(Job::Done) => {
            ui.colored_label(ui.visuals().hyperlink_color, "✔ Downloaded");
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

/// One folder in the Library tree: a checkbox to select it, then its subfolders and songs.
/// A song is played by double-click or from its right-click menu; `open` gets (in Stems?, song).
fn folder_row(
    ui: &mut egui::Ui,
    folder: &Folder,
    selected: &mut HashSet<PathBuf>,
    stems: bool,
    open: &mut Option<(bool, PathBuf)>,
    default_open: bool,
) {
    let mut checked = selected.contains(&folder.path);
    let check = |ui: &mut egui::Ui, checked: &mut bool, selected: &mut HashSet<PathBuf>| {
        if ui.checkbox(checked, &folder.name).changed() {
            if *checked {
                selected.insert(folder.path.clone());
            } else {
                selected.remove(&folder.path);
            }
        }
    };
    if folder.children.is_empty() && folder.files.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(ui.spacing().indent);
            check(ui, &mut checked, selected);
        });
        return;
    }
    let id = ui.make_persistent_id(&folder.path);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open)
        .show_header(ui, |ui| check(ui, &mut checked, selected))
        .body(|ui| {
            for child in &folder.children {
                folder_row(ui, child, selected, stems, open, false);
            }
            for file in &folder.files {
                let name = file.file_stem().unwrap_or_default().to_string_lossy();
                let row = ui.add(egui::Label::new(format!("♪  {name}")).sense(egui::Sense::click()));
                if row.double_clicked() {
                    *open = Some((false, file.clone()));
                }
                row.on_hover_text("Double-click to play, right-click for more").context_menu(|ui| {
                    if ui.button("Play").clicked() {
                        *open = Some((false, file.clone()));
                        ui.close();
                    }
                    if stems && ui.button("Open in Stems").clicked() {
                        *open = Some((true, file.clone()));
                        ui.close();
                    }
                });
            }
        });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.theme.update(&ctx);
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(&ctx, msg);
        }
        self.start_downloads(&ctx);
        egui::Panel::bottom("downloads").resizable(true).default_size(180.0).min_size(80.0).show(ui, |ui| {
            ui.add_space(6.0);
            self.downloads_panel(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Search, "Search");
                ui.selectable_value(&mut self.tab, Tab::Library, "Library");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
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
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([820.0, 620.0]).with_app_id("mixtape").with_title("Mixtape"),
        ..Default::default()
    };
    eframe::run_native("Mixtape", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))))
}
