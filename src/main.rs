//! Mixtape: search YouTube Music, download songs as MP3 into an Artist/Album library, play folders.

mod backend;
mod theme;

use backend::{Album, Artist, Folder, Format, Scan, SearchResult, Settings, Song};
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
    /// Details and tags for every song in the folder at `PathBuf`, and the duplicate groups among them.
    Indexed(PathBuf, HashMap<PathBuf, backend::AudioInfo>, Vec<Vec<PathBuf>>),
    /// Installed version, newest version, and the command that updates it.
    Ytdlp(Option<(String, String, String)>),
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
    /// The result under the pointer last frame, for Space.
    hovered: Option<String>,
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
        if let Some((_, child)) = &mut self.playing
            && !matches!(child.try_wait(), Ok(None))
        {
            self.playing = None;
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    Folders,
    Recent,
    Duplicates,
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
    /// Put the cursor in the Library filter box (Ctrl+F on the Library).
    focus_filter: bool,
    /// Requests from keyboard shortcuts, handled where the button is drawn.
    download_now: bool,
    trash_now: bool,
    show_keys: bool,
    rescan_requested: bool,
    /// Put the cursor in the search box the first time the Search tab shows.
    autofocus: bool,
    search_gen: u64,
    searching: bool,
    search_error: Option<String>,
    songs: Vec<Row>,
    albums: Vec<AlbumRow>,
    artists: Vec<Artist>,
    /// The artist or playlist the results come from.
    page_title: Option<String>,
    /// Album track lists being fetched; a few at a time so "Select all" on a big artist doesn't start dozens.
    album_loads: usize,
    library_filter: String,
    view: View,
    /// Reading every song's tags in the background after a scan.
    indexing: bool,
    duplicates: Vec<Vec<PathBuf>>,
    /// The installed and newest yt-dlp, when the installed one is out of date.
    ytdlp_outdated: Option<(String, String, String)>,
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
    /// A short confirmation ("Settings saved") and when it was shown; it fades after a few seconds.
    notice: Option<(String, std::time::Instant)>,
    /// Songs finished and failed since the queue was last empty.
    batch: (usize, usize),
    /// Tools that weren't found at startup or at the last "Check again".
    missing: Vec<&'static backend::Tool>,
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
        egui_extras::install_image_loaders(ctx); // cover thumbnails in search results, fetched over https
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
            focus_filter: false,
            download_now: false,
            trash_now: false,
            show_keys: false,
            rescan_requested: false,
            search_gen: 0,
            searching: false,
            search_error: None,
            songs: vec![],
            albums: vec![],
            artists: vec![],
            page_title: None,
            album_loads: 0,
            library_filter: String::new(),
            view: View::Folders,
            indexing: false,
            duplicates: vec![],
            ytdlp_outdated: None,
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
            missing: backend::missing_tools(),
            notice: None,
            batch: (0, 0),
            trash_confirm: None,
            update: Update::None,
            preview: Preview { mpv: backend::mpv_installed(), playing: None, clicked: None, hovered: None },
            update_checked: None,
            details: HashMap::new(),
            probing: HashSet::new(),
            tx,
            rx,
        };
        app.rescan(ctx);
        app.spawn(ctx, |_| Msg::Ytdlp(backend::ytdlp_outdated()));
        let resumed = backend::load_queue();
        if !resumed.is_empty() {
            let n = resumed.len();
            resumed.into_iter().for_each(|song| app.enqueue(song));
            app.notify(if n == 1 { "Resuming 1 download from last time".into() } else { format!("Resuming {n} downloads from last time") });
        }
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
                        self.artists = r.artists;
                        self.page_title = r.title;
                    }
                    Err(e) => self.search_error = Some(e),
                }
            }
            Msg::Search(..) => {} // results of an older search
            Msg::Album(id, result) => {
                self.album_loads = self.album_loads.saturating_sub(1);
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
                        self.batch.0 += 1;
                        Job::Done(source)
                    }
                    Err(e) => {
                        self.batch.1 += 1;
                        Job::Failed(e)
                    }
                };
                self.jobs.insert(song.id, job);
                self.save_queue();
                if self.queue.is_empty() && self.running == 0 {
                    let (ok, failed) = std::mem::take(&mut self.batch);
                    let songs = |n| if n == 1 { "1 song".to_string() } else { format!("{n} songs") };
                    self.notify(match failed {
                        0 => format!("Downloaded {}", songs(ok)),
                        _ => format!("Downloaded {}, {} failed (see Downloads)", songs(ok), failed),
                    });
                    self.rescan(ctx);
                }
            }
            Msg::Scanned(scan) => {
                self.scanning = false;
                self.selected.retain(|p| p.starts_with(&scan.root.path));
                self.details.clear(); // files may have changed; read again when shown
                self.indexing = true;
                let (root, all) = (scan.root.path.clone(), scan.all.clone());
                self.scan = Some(scan);
                self.spawn(ctx, move |_| {
                    let index = backend::index(&all);
                    let groups = backend::duplicates(&root, &index);
                    Msg::Indexed(root, index, groups)
                });
            }
            Msg::Indexed(root, index, groups) => {
                // Ignore the result for a folder that's no longer the music folder.
                if let Some(scan) = self.scan.as_mut().filter(|s| s.root.path == root) {
                    for info in index.values() {
                        scan.keys.extend(backend::tag_keys(info));
                    }
                    self.details.extend(index);
                    self.duplicates = groups;
                    self.indexing = false;
                }
            }
            Msg::Ytdlp(outdated) => self.ytdlp_outdated = outdated,
            Msg::Probed(found) => {
                for (path, info) in found {
                    self.probing.remove(&path);
                    self.details.insert(path, info);
                }
            }
            Msg::Trashed(paths, result) => {
                match result.and_then(|_| backend::forget(&paths)) {
                    Ok(()) => {
                        self.status = None;
                        let n = paths.len();
                        self.notify(if n == 1 { "Moved 1 item to the Trash".into() } else { format!("Moved {n} items to the Trash") });
                    }
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
            Msg::Picked(dir) => {
                if let Some(dir) = dir {
                    self.draft.music_dir = dir;
                    self.apply_music_dir(ctx);
                }
            }
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
        self.save_queue();
    }

    /// Writes the waiting and running songs to disk, so they resume after a restart.
    fn save_queue(&mut self) {
        let pending: Vec<Song> =
            self.history.iter().filter(|s| matches!(self.jobs.get(&s.id), Some(Job::Queued | Job::Running(_)))).cloned().collect();
        if let Err(e) = backend::save_queue(&pending) {
            self.status = Some(format!("Could not save the download queue: {e}"));
        }
    }

    /// Drops songs that haven't started downloading: one, or all of them.
    fn cancel_waiting(&mut self, only: Option<&str>) {
        let cancel: Vec<String> = self.queue.iter().filter(|s| only.is_none_or(|id| s.id == id)).map(|s| s.id.clone()).collect();
        self.queue.retain(|s| !cancel.contains(&s.id));
        self.history.retain(|s| !cancel.contains(&s.id));
        for id in &cancel {
            self.jobs.remove(id);
        }
        self.save_queue();
    }

    fn downloads_panel(&mut self, ui: &mut egui::Ui) {
        let (mut retry, mut cancel) = (None, None);
        ui.horizontal(|ui| {
            ui.heading("Downloads");
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
                    .hint_text("Artist, song or album, or paste a YouTube link")
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
            let clicked = ui.add_enabled(!selected.is_empty(), accent_button(ui, label)).on_hover_text("Ctrl+D").clicked();
            if (clicked || std::mem::take(&mut self.download_now)) && !selected.is_empty() {
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
            error_line(ui, e);
        }
        ui.separator();

        let mut wants_confirm = None;
        let mut load_album = Vec::new();
        let mut open_artist = None;
        let Self { songs, albums, artists, page_title, jobs, logged, scan, preview, .. } = self;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if let Some(title) = page_title {
                ui.horizontal(|ui| {
                    ui.heading(title.as_str());
                    if !albums.is_empty() && ui.button(format!("Select all {} albums and singles", albums.len())).clicked() {
                        albums.iter_mut().for_each(|a| a.checked = true);
                    } else if albums.is_empty() && !songs.is_empty() && ui.button(format!("Select all {} songs", songs.len())).clicked() {
                        songs.iter_mut().filter(|r| !in_library(logged, scan, &r.song)).for_each(|r| r.checked = true);
                    }
                });
                ui.add_space(4.0);
            }
            if !artists.is_empty() {
                ui.heading("Artists");
                for a in artists.iter() {
                    ui.horizontal(|ui| {
                        ui.strong(&a.name);
                        if ui.button("Albums and singles ›").clicked() {
                            open_artist = Some(a.id.clone());
                        }
                    });
                }
                ui.add_space(8.0);
            }
            if !albums.is_empty() {
                ui.heading("Albums");
                for a in albums.iter_mut() {
                    let loading = a.loading;
                    row(
                        ui,
                        |ui| {
                            if loading {
                                ui.spinner();
                            }
                        },
                        |ui| {
                            let arrow = if a.open { "⏷" } else { "⏵" };
                            if ui.button(arrow).on_hover_text("Show tracks").clicked() {
                                a.open = !a.open;
                            }
                            if ui.checkbox(&mut a.checked, "").on_hover_text("Select every track not already in your library").changed()
                                && let Some(tracks) = &mut a.tracks
                            {
                                for t in tracks {
                                    t.checked = a.checked && !in_library(logged, scan, &t.song);
                                }
                            }
                            cover(ui, &a.album.thumb, 48.0);
                            ui.vertical(|ui| {
                                fit(ui, egui::RichText::new(&a.album.title).strong());
                                fit(ui, format!("{}  ·  {} {}", a.album.artists, a.album.kind, a.album.year));
                            });
                        },
                    );
                    if (a.open || a.checked) && a.tracks.is_none() && !a.loading && a.error.is_none() {
                        load_album.push(a.album.id.clone());
                    }
                    if let Some(e) = &a.error {
                        error_line(ui, e);
                    }
                    if a.open
                        && let Some(tracks) = &mut a.tracks
                    {
                        ui.indent(&a.album.id, |ui| {
                            for row in tracks.iter_mut() {
                                song_row(ui, row, jobs, logged, scan, &mut wants_confirm, preview);
                            }
                        });
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
        if let Some(id) = self.preview.clicked.take()
            && let Err(e) = self.preview.toggle(id)
        {
            self.status = Some(e);
        }
        if let Some(song) = wants_confirm {
            self.confirm = Some(song);
        }
        for id in load_album.into_iter().take(3usize.saturating_sub(self.album_loads)) {
            if let Some(a) = self.albums.iter_mut().find(|a| a.album.id == id) {
                a.loading = true;
            }
            self.album_loads += 1;
            self.spawn(ui.ctx(), move |_| {
                let r = backend::album_tracks(&id);
                Msg::Album(id, r)
            });
        }
        if let Some(id) = open_artist {
            self.search_gen += 1;
            self.searching = true;
            self.search_error = None;
            let generation = self.search_gen;
            self.spawn(ui.ctx(), move |_| Msg::Search(generation, backend::artist(&id)));
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
        ui.horizontal_wrapped(|ui| {
            let n = self.selected.len();
            let mut picked: Vec<PathBuf> = self.selected.iter().cloned().collect();
            picked.sort();
            if ui.add_enabled(n > 0, egui::Button::new(format!("Play selected ({n})"))).clicked()
                && let Err(e) = backend::open_with(&self.settings.player, &picked)
            {
                self.status = Some(e);
            }
            // Never the music folder itself, and nothing inside a folder that's also going.
            let trash: Vec<PathBuf> =
                picked.iter().filter(|p| **p != dir && !picked.iter().any(|q| q != *p && p.starts_with(q))).cloned().collect();
            let clicked =
                ui.add_enabled(!trash.is_empty(), egui::Button::new(format!("Move to Trash ({})", trash.len()))).on_hover_text("Delete");
            if (clicked.clicked() || std::mem::take(&mut self.trash_now)) && !trash.is_empty() {
                self.trash_confirm = Some(trash);
            }
            if ui.add_enabled(n > 0, egui::Button::new("Clear selection")).clicked() {
                self.selected.clear();
            }
            if ui.button("Open in file manager").on_hover_text("Open the music folder").clicked()
                && let Err(e) = backend::open_with("xdg-open", std::slice::from_ref(&dir))
            {
                self.status = Some(e);
            }
            if ui.button("Refresh").clicked() {
                self.rescan(ui.ctx());
            }
            if self.scanning {
                ui.spinner();
            }
            if let Some(scan) = &self.scan {
                ui.label(format!("{} songs", scan.all.len()));
            }
        });
        ui.separator();
        if !dir.is_dir() {
            ui.colored_label(ui.visuals().error_fg_color, format!("Music folder not found: {}. Set it in Settings.", dir.display()));
            return;
        }
        ui.horizontal_wrapped(|ui| {
            let filter = ui.add(
                egui::TextEdit::singleline(&mut self.library_filter)
                    .hint_text("Filter by artist, album or song (Ctrl+F)")
                    .desired_width(320.0),
            );
            if std::mem::take(&mut self.focus_filter) {
                filter.request_focus();
            }
            if !self.library_filter.is_empty() && ui.small_button("✖").on_hover_text("Clear the filter").clicked() {
                self.library_filter.clear();
            }
            ui.separator();
            ui.selectable_value(&mut self.view, View::Folders, "Folders");
            ui.selectable_value(&mut self.view, View::Recent, "Recently added");
            let dups = if self.indexing { "Duplicates…".to_string() } else { format!("Duplicates ({})", self.duplicates.len()) };
            ui.selectable_value(&mut self.view, View::Duplicates, dups)
                .on_hover_text("The same song in more than one place: same artist and title, and lengths within 3 seconds");
        });
        let Some(scan) = &self.scan else { return };
        let mut tree = Tree {
            selected: &mut self.selected,
            stems: self.stems,
            action: None,
            details: &self.details,
            probing: &self.probing,
            wanted: vec![],
        };
        let filter = self.library_filter.trim();
        let flat: Option<Vec<&PathBuf>> = if !filter.is_empty() {
            Some(scan.matching(filter))
        } else if self.view == View::Recent {
            Some(scan.all.iter().map(|(p, _)| p).take(200).collect())
        } else {
            None
        };
        let label = |file: &PathBuf| {
            // "Queen / Greatest Hits / Bohemian Rhapsody"
            let shown = file.strip_prefix(&scan.root.path).unwrap_or(file).with_extension("");
            shown.iter().map(|c| c.to_string_lossy()).collect::<Vec<_>>().join(" / ")
        };
        let (duplicates, indexing) = (&self.duplicates, self.indexing);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match &flat {
            None if self.view == View::Duplicates => {
                if indexing {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading song tags… The first time takes a while on a big library.");
                    });
                } else if duplicates.is_empty() {
                    ui.weak("No duplicates found.");
                } else {
                    ui.weak("Each group looks like the same song. Tick the copies you don't want, then use Move to Trash.");
                    for group in duplicates {
                        ui.add_space(6.0);
                        let name = group[0].file_stem().unwrap_or_default().to_string_lossy();
                        ui.strong(format!(
                            "{} — {} copies",
                            name.trim_start_matches(|c: char| c.is_ascii_digit() || " -.".contains(c)),
                            group.len()
                        ));
                        for file in group {
                            file_row(ui, file, &label(file), &mut tree);
                        }
                    }
                }
            }
            None => folder_row(ui, &scan.root, &mut tree, true),
            Some(files) if files.is_empty() => {
                ui.weak("No songs match.");
            }
            Some(files) => {
                const MAX: usize = 500;
                for file in files.iter().take(MAX) {
                    file_row(ui, file, &label(file), &mut tree);
                }
                if files.len() > MAX {
                    ui.weak(format!("…and {} more. Type more of the name to narrow it down.", files.len() - MAX));
                }
            }
        });
        let (action, wanted) = (tree.action, tree.wanted);
        if !wanted.is_empty() {
            self.probing.extend(wanted.iter().cloned());
            self.spawn(ui.ctx(), move |_| {
                Msg::Probed(
                    wanted
                        .into_iter()
                        .map(|p| {
                            let i = backend::probe(&p);
                            (p, i)
                        })
                        .collect(),
                )
            });
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

    /// Keyboard shortcuts. Single keys (Space, Delete, ?) only count when no text box has the cursor.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let ctrl = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let typing = ctx.egui_wants_keyboard_input();
        let hovered = self.preview.hovered.take();
        let mut toggle_preview = None;
        ctx.input_mut(|i| {
            if i.consume_shortcut(&ctrl(Key::Num1)) {
                self.tab = Tab::Library;
            }
            if i.consume_shortcut(&ctrl(Key::Num2)) {
                self.tab = Tab::Search;
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
            if typing || self.confirm.is_some() || self.trash_confirm.is_some() {
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

    fn keys_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_keys {
            return;
        }
        const KEYS: [(&str, &str); 12] = [
            ("Ctrl+1 / Ctrl+2", "Library / Search"),
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

    /// Shown while a required tool is missing: what it's for and the command that installs it.
    fn setup_banner(&mut self, ui: &mut egui::Ui) {
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
            }
        });
        ui.add_space(6.0);
    }

    fn notify(&mut self, text: String) {
        self.notice = Some((text, std::time::Instant::now()));
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
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

    /// Settings apply and save as they change. The music folder applies when you leave its box
    /// (or pick one with Browse), and the text size when you let go of the slider.
    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let before = self.settings.clone();
        egui::Grid::new("settings").num_columns(2).spacing([12.0, 12.0]).show(ui, |ui| {
            ui.label("Music folder");
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
            ui.end_row();

            ui.label("Player");
            ui.vertical(|ui| {
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
                    ui.small("Mixtape adds the selected folders after the command.");
                }
            });
            ui.end_row();

            ui.label("Format");
            ui.vertical(|ui| {
                ui.radio_value(&mut self.settings.format, Format::Mp3, "MP3: plays everywhere");
                ui.radio_value(
                    &mut self.settings.format,
                    Format::Original,
                    "Original: best quality, no conversion (Opus, or AAC with Premium)",
                );
                ui.small("Original files are about half the size. Some car stereos and older devices can't play Opus.");
            });
            ui.end_row();

            ui.label("Programs");
            ui.vertical(|ui| {
                for t in &backend::TOOLS {
                    let (mark, color) = if backend::tool_installed(t) {
                        ("✔", ui.visuals().hyperlink_color)
                    } else if t.required {
                        ("✖", ui.visuals().error_fg_color)
                    } else {
                        ("–", ui.visuals().weak_text_color())
                    };
                    let optional = if t.required { "" } else { " (optional)" };
                    ui.horizontal(|ui| {
                        ui.colored_label(color, mark);
                        ui.label(format!("{} for {}{optional}", t.bin, t.needed_for));
                    });
                }
            });
            ui.end_row();

            ui.label("Text size");
            ui.horizontal(|ui| {
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
                ui.small("Ctrl + and Ctrl − work anywhere");
            });
            ui.end_row();

            ui.label("Updates");
            ui.checkbox(&mut self.settings.check_updates, "Check GitHub for a new version at startup and once a day");
            ui.end_row();

            ui.label("Premium");
            ui.vertical(|ui| {
                let browsers = backend::detected_browsers();
                let mut premium = !self.settings.cookies_browser.is_empty();
                if ui.add_enabled(!browsers.is_empty(), egui::Checkbox::new(&mut premium, "Use my YouTube Music Premium login")).changed() {
                    self.settings.cookies_browser = if premium { browsers[0].1.to_string() } else { String::new() };
                }
                if premium {
                    let current = browsers.iter().find(|b| b.1 == self.settings.cookies_browser).map_or("Choose a browser", |b| b.0);
                    egui::ComboBox::from_id_salt("browser").selected_text(current).show_ui(ui, |ui| {
                        for (name, id) in &browsers {
                            ui.selectable_value(&mut self.settings.cookies_browser, id.to_string(), *name);
                        }
                    });
                }
                ui.small(
                    "Premium gets 256 kbps AAC instead of 130 kbps. Sign in to music.youtube.com in that browser first. \
                          Mixtape only passes the browser's name to yt-dlp, which reads the login on this computer.",
                );
            });
            ui.end_row();
        });
        ui.add_space(12.0);
        if self.draft.music_dir != self.settings.music_dir && !self.draft.music_dir.is_dir() {
            ui.colored_label(ui.visuals().warn_fg_color, "That folder doesn't exist, so the music folder hasn't changed.");
        }
        ui.weak("Changes are saved automatically.");
        if self.settings != before
            && let Err(e) = self.settings.save()
        {
            self.status = Some(format!("Could not save settings: {e}"));
        }
    }

    /// Switches to the folder typed or picked in Settings, if it exists.
    fn apply_music_dir(&mut self, ctx: &egui::Context) {
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

fn song_row(
    ui: &mut egui::Ui,
    row_: &mut Row,
    jobs: &HashMap<String, Job>,
    logged: &HashSet<String>,
    scan: &Option<Scan>,
    wants_confirm: &mut Option<Song>,
    preview: &mut Preview,
) {
    let have = in_library(logged, scan, &row_.song);
    let id = row_.song.id.clone();
    let hovered = row(
        ui,
        |ui| job_status(ui, jobs.get(&id), have),
        |ui| {
            if ui.checkbox(&mut row_.checked, "").changed() && row_.checked && have {
                row_.checked = false;
                *wants_confirm = Some(row_.song.clone());
            }
            if preview.mpv {
                let playing = preview.current() == Some(id.as_str());
                let (icon, tip) =
                    if playing { ("⏹", "Stop the preview (Space)") } else { ("▶", "Preview: listen without downloading (Space)") };
                if ui.button(icon).on_hover_text(tip).clicked() {
                    preview.clicked = Some(id.clone());
                }
            }
            cover(ui, &row_.song.thumb, 40.0);
            ui.vertical(|ui| {
                fit(ui, egui::RichText::new(&row_.song.title).strong());
                let album = if row_.song.album.is_empty() { String::new() } else { format!(" · {}", row_.song.album) };
                fit(ui, format!("{}{album} · {}", row_.song.artists, row_.song.duration));
            });
        },
    );
    if hovered {
        preview.hovered = Some(id);
    }
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

/// A row with `right` drawn against the right edge first and `left` in the space left over, so a
/// long name is cut short ("…") instead of running into the details. Returns whether the pointer
/// is over the row.
fn row(ui: &mut egui::Ui, right: impl FnOnce(&mut egui::Ui), left: impl FnOnce(&mut egui::Ui)) -> bool {
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
fn fit(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Label::new(text).truncate())
}

/// A square cover from a URL, or blank space of the same size so rows line up.
fn cover(ui: &mut egui::Ui, url: &str, size: f32) {
    if url.starts_with("https://") {
        ui.add(egui::Image::new(url).fit_to_exact_size(egui::vec2(size, size)).corner_radius(3.0));
    } else {
        ui.add_space(size + ui.spacing().item_spacing.x);
    }
}

/// An error in plain words, with a button that copies the full details for a bug report.
fn error_line(ui: &mut egui::Ui, raw: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(ui.visuals().error_fg_color, backend::friendly(raw));
        if ui.small_button("Copy details").on_hover_text("Copy the full error, for a bug report").clicked() {
            ui.ctx().copy_text(raw.to_string());
        }
    });
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

/// One song in the Library: tick box, name, details; double-click plays, right-click opens a menu.
fn file_row(ui: &mut egui::Ui, file: &PathBuf, label: &str, tree: &mut Tree) {
    let details = match tree.details.get(file) {
        Some(info) => info.describe(),
        None => {
            if !tree.probing.contains(file) {
                tree.wanted.push(file.clone());
            }
            String::new()
        }
    };
    let stems = tree.stems;
    let folder = file.parent().map(PathBuf::from).unwrap_or_default();
    let (selected, action) = (&mut *tree.selected, &mut tree.action);
    row(
        ui,
        |ui| {
            ui.weak(details);
        },
        |ui| {
            select_box(ui, file, selected, "");
            let name = fit(ui, format!("♪  {label}")).interact(egui::Sense::click());
            if name.double_clicked() {
                *action = Some(LibraryAction::Play(file.clone()));
            }
            name.context_menu(|ui| {
                if ui.button("Play").clicked() {
                    *action = Some(LibraryAction::Play(file.clone()));
                    ui.close();
                }
                if stems && ui.button("Open in Stems").clicked() {
                    *action = Some(LibraryAction::Stems(file.clone()));
                    ui.close();
                }
                if ui.button("Show in file manager").clicked() {
                    *action = Some(LibraryAction::Reveal(folder.clone()));
                    ui.close();
                }
            });
        },
    );
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
fn folder_row(ui: &mut egui::Ui, folder: &Folder, tree: &mut Tree, root: bool) {
    let header = |ui: &mut egui::Ui, tree: &mut Tree| {
        // The music folder itself has no tick box, so it can never be selected for the Trash.
        let row = if root { ui.strong(&folder.name) } else { select_box(ui, &folder.path, tree.selected, &folder.name) };
        row.context_menu(|ui| {
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
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, root).show_header(ui, |ui| header(ui, tree)).body(
        |ui| {
            for child in &folder.children {
                folder_row(ui, child, tree, false);
            }
            for file in &folder.files {
                file_row(ui, file, &file.file_stem().unwrap_or_default().to_string_lossy(), tree);
            }
        },
    );
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.theme.update(&ctx);
        self.zoom_keys(&ctx);
        self.shortcuts(&ctx);
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
            self.setup_banner(ui);
            if let Some(s) = self.status.clone() {
                ui.horizontal(|ui| {
                    error_line(ui, &s);
                    if ui.small_button("✖").on_hover_text("Dismiss").clicked() {
                        self.status = None;
                    }
                });
            }
            const NOTICE_TIME: std::time::Duration = std::time::Duration::from_secs(4);
            match &self.notice {
                Some((text, shown)) if shown.elapsed() < NOTICE_TIME => {
                    ui.colored_label(ui.visuals().hyperlink_color, format!("✔ {text}"));
                    ui.ctx().request_repaint_after(NOTICE_TIME - shown.elapsed());
                }
                Some(_) => self.notice = None,
                None => {}
            }
            ui.separator();
            // A folder typed in Settings but not confirmed applies (or is dropped) on leaving the tab.
            if self.tab != Tab::Settings && self.draft.music_dir != self.settings.music_dir {
                self.apply_music_dir(ui.ctx());
                self.draft.music_dir = self.settings.music_dir.clone();
            }
            match self.tab {
                Tab::Search => self.search_tab(ui),
                Tab::Library => self.library_tab(ui),
                Tab::Settings => self.settings_tab(ui),
            }
        });
        self.confirm_dialog(&ctx);
        self.trash_dialog(&ctx);
        self.keys_dialog(&ctx);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([820.0, 620.0]).with_app_id("mixtape").with_title("Mixtape"),
        ..Default::default()
    };
    eframe::run_native("Mixtape", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))))
}
