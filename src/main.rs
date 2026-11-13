//! Mixtape: search YouTube Music, download songs as MP3 into an Artist/Album library, play folders.

mod backend;
mod chrome;
mod downloads;
mod library;
mod search;
mod settings;
mod theme;
mod widgets;

#[cfg(test)]
mod ui_tests;

use backend::{Album, Artist, Folder, Format, Scan, SearchResult, Settings, Song};
use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use downloads::*;
use library::*;
use search::*;
use widgets::*;

const PARALLEL_DOWNLOADS: usize = 2;

enum Msg {
    Search(u64, Result<SearchResult, String>),
    Album(String, Result<Vec<Song>, String>),
    /// A download thread took this song from the queue.
    Started(String),
    Progress(String, f32),
    /// A song finished (or failed); true when the user had cancelled it.
    Done(Song, Result<backend::Downloaded, String>, bool),
    /// The queue ran out (or paused): songs downloaded and failed in this batch.
    BatchDone(usize, usize),
    Scanned(Scan),
    /// Details and tags for every song in the folder at `PathBuf`, and the duplicate groups among them.
    Indexed(PathBuf, HashMap<PathBuf, backend::AudioInfo>, Vec<Vec<PathBuf>>),
    /// Installed version, newest version, and the command that updates it.
    Ytdlp(Option<(String, String, String)>),
    Picked(Option<PathBuf>),
    Release(Result<Option<backend::Version>, String>),
    Updated(Result<(), String>),
    Trashed(Vec<PathBuf>, Result<(), String>),
    Deleted(Vec<PathBuf>, Result<(), String>),
    /// Fix tags run, an old file, and the YouTube Music song it came from.
    Resolved(u64, PathBuf, Result<Song, String>),
    /// That Fix tags run finished looking up songs.
    ResolveDone(u64),
    Probed(Vec<(PathBuf, backend::AudioInfo)>),
    /// Album folders and their cover picture, if they have one.
    Covers(Vec<(PathBuf, Option<String>)>),
    /// Each dependency's installed version and path, for Settings.
    Versions(Vec<(&'static str, Option<String>, Option<PathBuf>)>),
    /// The JavaScript runtime yt-dlp can use, and whether its challenge solver is installed.
    Challenges(Option<(&'static str, &'static str)>, bool),
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
    /// Every song queued this session, oldest first, for the Downloads list.
    history: Vec<Song>,

    logged: HashSet<String>,
    scan: Option<Scan>,
    scanning: bool,
    selected: HashSet<PathBuf>,
    stems: bool,
    /// A short confirmation ("Settings saved") and when it was shown; it fades after a few seconds.
    notice: Option<(String, std::time::Instant)>,
    /// Tools that weren't found at startup or at the last "Check again".
    missing: Vec<&'static backend::Tool>,
    /// Selected songs and folders waiting on "Move to Trash?".
    trash_confirm: Option<Vec<PathBuf>>,
    /// Fix tags: looking up the songs, then the preview waiting on "Fix".
    fix: Option<Fix>,
    /// Counts Fix tags runs, so a cancelled run's late answers are ignored.
    fix_runs: u64,
    /// Items the Trash couldn't take (the drive has none), waiting on "Delete permanently?".
    delete_confirm: Option<Vec<PathBuf>>,
    /// Old copies Fix tags couldn't trash for the same reason, asked about when the queue finishes.
    untrashable: Vec<PathBuf>,
    /// Song id -> the old file it replaces; trashed once the new download is in.
    replaces: HashMap<String, PathBuf>,
    update: Update,
    preview: Preview,
    downloads: Downloads,
    /// When GitHub was last asked for a newer release.
    update_checked: Option<std::time::Instant>,
    /// Format, bitrate, length and size of songs shown in the Library, read in the background.
    details: HashMap<PathBuf, backend::AudioInfo>,
    probing: HashSet<PathBuf>,
    /// Cover picture for each album folder shown in the Library (None: it has none).
    /// Folder -> its picture's file:// or https:// address, if it has one.
    covers: HashMap<PathBuf, Option<String>>,
    covers_pending: HashSet<PathBuf>,
    /// Dependency versions and paths for Settings, read in the background.
    versions: HashMap<&'static str, (Option<String>, Option<PathBuf>)>,
    /// What YouTube's download challenges need: (runtime, solver installed); None until checked.
    challenges: Option<(Option<(&'static str, &'static str)>, bool)>,
    /// Downloads panel opened while nothing is downloading.
    downloads_open: bool,

    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let settings = Settings::load();
        theme::install_fonts(ctx);
        egui_extras::install_image_loaders(ctx); // cover thumbnails in search results, fetched over https
        ctx.options_mut(|o| o.zoom_with_keyboard = false); // handled in zoom_keys so the size is saved
        ctx.set_zoom_factor(settings.text_scale.clamp(*backend::TEXT_SCALES.start(), *backend::TEXT_SCALES.end()));
        let mut app = Self::with_settings(settings);
        app.logged = backend::read_log();
        app.rescan(ctx);
        app.spawn(ctx, |_| Msg::Ytdlp(backend::ytdlp_outdated()));
        app.read_versions(ctx);
        let resumed = backend::load_queue();
        if !resumed.is_empty() {
            let n = resumed.len();
            resumed.into_iter().for_each(|song| app.enqueue(song));
            app.notify(if n == 1 { "Resuming 1 download from last time".into() } else { format!("Resuming {n} downloads from last time") });
        }
        app
    }

    /// The app's state with nothing started: no scan, no network, no download log, no resumed
    /// queue. `new` starts those; the window tests use this directly.
    fn with_settings(settings: Settings) -> Self {
        let (tx, rx) = channel();
        Self {
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
            history: vec![],
            logged: HashSet::new(),
            scan: None,
            scanning: false,
            selected: HashSet::new(),
            stems: backend::stems_installed(),
            missing: backend::missing_tools(),
            notice: None,
            trash_confirm: None,
            fix: None,
            fix_runs: 0,
            delete_confirm: None,
            untrashable: vec![],
            replaces: HashMap::new(),
            update: Update::None,
            downloads: Downloads(Default::default()),
            preview: Preview { mpv: backend::mpv_installed(), playing: None, clicked: None, hovered: None },
            update_checked: None,
            details: HashMap::new(),
            probing: HashSet::new(),
            covers: HashMap::new(),
            covers_pending: HashSet::new(),
            downloads_open: false,
            versions: HashMap::new(),
            challenges: None,
            tx,
            rx,
        }
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
                let (logged, dir) = (&self.logged, &self.settings.music_dir);
                if let Some(a) = self.albums.iter_mut().find(|a| a.album.id == id) {
                    a.loading = false;
                    match result {
                        Ok(mut tracks) => {
                            // The album's own length can be longer than the search limit. Show the first tracks only.
                            backend::apply_search_limit(&mut tracks, self.settings.search_limit);
                            let checked = a.checked;
                            a.tracks = Some(
                                tracks.into_iter().map(|song| Row { checked: checked && !in_album(logged, dir, &song), song }).collect(),
                            );
                        }
                        Err(e) => a.error = Some(e),
                    }
                }
            }
            Msg::Started(id) => {
                self.jobs.insert(id, Job::Running(0.0));
            }
            Msg::Progress(id, f) => {
                self.jobs.insert(id, Job::Running(f));
            }
            Msg::Done(song, result, cancelled) => {
                if cancelled {
                    self.jobs.remove(&song.id);
                    self.history.retain(|s| s.id != song.id);
                    return;
                }
                // The download thread already wrote the log and the saved queue.
                let job = match result {
                    Ok(backend::Downloaded { path, source }) => {
                        // Fix tags: the fresh copy is in, so the old one goes (unless it was overwritten in place).
                        if let Some(old) = self.replaces.remove(&song.id)
                            && old != path
                            && let Err(e) =
                                backend::trash(std::slice::from_ref(&old)).and_then(|_| backend::forget(std::slice::from_ref(&old)))
                        {
                            if backend::no_trash(&e) {
                                self.untrashable.push(old);
                            } else {
                                self.status = Some(format!("Could not move the old copy to the Trash: {e}"));
                            }
                        }
                        self.logged.insert(song.id.clone());
                        Job::Done(source)
                    }
                    Err(e) => Job::Failed(e),
                };
                self.jobs.insert(song.id, job);
            }
            Msg::BatchDone(ok, failed) => {
                if !self.untrashable.is_empty() {
                    self.delete_confirm = Some(std::mem::take(&mut self.untrashable));
                }
                let summary = batch_summary(ok, failed);
                self.notify(if failed > 0 { format!("{summary} (see Downloads)") } else { summary });
                self.rescan(ctx);
            }
            Msg::Scanned(scan) => {
                self.scanning = false;
                self.selected.retain(|p| p.starts_with(&scan.root.path));
                self.details.clear(); // files may have changed; read again when shown
                self.covers.clear();
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
                    for (path, info) in &index {
                        for k in backend::tag_keys(info) {
                            scan.keys.entry(k).or_insert_with(|| path.clone());
                        }
                    }
                    self.details.extend(index);
                    self.duplicates = groups;
                    self.indexing = false;
                }
            }
            Msg::Ytdlp(outdated) => self.ytdlp_outdated = outdated,
            Msg::Challenges(runtime, solver) => self.challenges = Some((runtime, solver)),
            Msg::Versions(found) => {
                self.versions = found.into_iter().map(|(bin, v, p)| (bin, (v, p))).collect();
            }
            Msg::Covers(found) => {
                for (folder, cover) in found {
                    self.covers_pending.remove(&folder);
                    self.covers.insert(folder, cover);
                }
            }
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
                    Err(e) if backend::no_trash(&e) => self.delete_confirm = Some(paths.clone()),
                    Err(e) => self.status = Some(format!("Could not move to the Trash: {e}")),
                }
                self.logged = backend::read_log();
                self.selected.clear();
                self.rescan(ctx);
            }
            Msg::Deleted(paths, result) => {
                match result {
                    Ok(()) => {
                        let n = paths.len();
                        self.notify(if n == 1 { "Deleted 1 item".into() } else { format!("Deleted {n} items") });
                    }
                    Err(e) => self.status = Some(format!("Could not delete: {e}")),
                }
                self.logged = backend::read_log();
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
            Msg::Resolved(run, old, song) => {
                if let Some(fix) = self.fix.as_mut().filter(|f| f.run == run) {
                    let ticked = song.is_ok();
                    fix.rows.push(FixRow { old, song, ticked });
                }
            }
            Msg::ResolveDone(run) => {
                if let Some(fix) = self.fix.as_mut().filter(|f| f.run == run) {
                    fix.done = true;
                }
            }
            Msg::Picked(dir) => {
                if let Some(dir) = dir {
                    self.draft.music_dir = dir;
                    self.apply_music_dir(ctx);
                }
            }
        }
    }

    fn notify(&mut self, text: String) {
        self.notice = Some((text, std::time::Instant::now()));
    }
}

impl eframe::App for App {
    /// Background work, separate from drawing: eframe keeps calling this while the window is hidden
    /// (another workspace, minimized), when `ui` doesn't run. Finished downloads get handled, the
    /// queue keeps moving and the "finished" notification goes out even when you're elsewhere.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.preview.poll();
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(ctx, msg);
        }
        self.start_downloads(ctx);
        const DAY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
        if self.settings.check_updates && self.update_checked.is_none_or(|t| t.elapsed() > DAY) {
            self.update_checked = Some(std::time::Instant::now());
            self.spawn(ctx, |_| Msg::Release(backend::latest_release()));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // For the download threads: whether you're looking at Mixtape (see Shared::away).
        let shared = &self.downloads.0;
        shared.focused.store(ctx.input(|i| i.viewport().focused) != Some(false), std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut t) = shared.last_drawn.lock() {
            *t = std::time::Instant::now();
        }
        self.theme.update(&ctx);
        self.zoom_keys(&ctx);
        self.shortcuts(&ctx);
        egui::Panel::bottom("footer").show(ui, |ui| self.footer(ui));
        // The Downloads list opens while something is downloading; otherwise it's one line.
        let (running, waiting) = self.queue_counts();
        let busy = running + waiting > 0 || self.paused().is_some();
        if busy || self.downloads_open {
            egui::Panel::bottom("downloads").resizable(true).default_size(180.0).min_size(80.0).show(ui, |ui| {
                ui.add_space(6.0);
                self.downloads_panel(ui);
            });
        } else {
            egui::Panel::bottom("downloads-line").resizable(false).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Downloads");
                    let done = self.history.iter().filter(|s| matches!(self.jobs.get(&s.id), Some(Job::Done(_)))).count();
                    let failed = self.history.iter().filter(|s| matches!(self.jobs.get(&s.id), Some(Job::Failed(_)))).count();
                    match (done, failed) {
                        (0, 0) => ui.weak("Nothing downloading"),
                        (d, 0) => ui.weak(format!("{d} done")),
                        (d, f) => ui.colored_label(ui.visuals().error_fg_color, format!("{d} done, {f} failed")),
                    };
                    if !self.history.is_empty() && ui.small_button("Show").clicked() {
                        self.downloads_open = true;
                    }
                });
            });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Search, "Search");
                ui.selectable_value(&mut self.tab, Tab::Library, "Library");
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
            if self.tab != Tab::Search {
                self.preview.stop(); // a preview belongs to the Search tab
            }
            match self.tab {
                Tab::Search => self.search_tab(ui),
                Tab::Library => self.library_tab(ui),
                Tab::Settings => {
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.settings_tab(ui));
                }
            }
        });
        self.confirm_dialog(&ctx);
        self.trash_dialog(&ctx);
        self.fix_dialog(&ctx);
        self.delete_dialog(&ctx);
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
