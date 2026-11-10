//! The Library tab: the folder tree, flat lists, duplicates, and the Trash / delete / Fix tags dialogs.

use crate::*;

/// What a click in the Library tree asks for.
pub(crate) enum LibraryAction {
    Play(PathBuf),
    Stems(PathBuf),
    /// Open a folder in the file manager.
    Reveal(PathBuf),
}

#[derive(PartialEq, Clone, Copy)]
pub(crate) enum View {
    Folders,
    Recent,
    Duplicates,
}

/// A tick box that adds `path` to or removes it from the selection.
pub(crate) fn select_box(
    ui: &mut egui::Ui,
    path: &PathBuf,
    selected: &mut HashSet<PathBuf>,
    label: impl Into<egui::WidgetText>,
) -> egui::Response {
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
pub(crate) fn file_row(ui: &mut egui::Ui, file: &PathBuf, label: &str, tree: &mut Tree) {
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
pub(crate) struct Tree<'a> {
    pub(crate) selected: &'a mut HashSet<PathBuf>,
    pub(crate) stems: bool,
    pub(crate) action: Option<LibraryAction>,
    pub(crate) details: &'a HashMap<PathBuf, backend::AudioInfo>,
    pub(crate) probing: &'a HashSet<PathBuf>,
    /// Shown songs whose details haven't been read yet.
    pub(crate) wanted: Vec<PathBuf>,
    pub(crate) covers: &'a HashMap<PathBuf, Option<PathBuf>>,
    pub(crate) covers_pending: &'a HashSet<PathBuf>,
    /// Album folders shown whose cover hasn't been looked for yet.
    pub(crate) wanted_covers: Vec<(PathBuf, Vec<PathBuf>)>,
}

/// One folder in the Library tree, then its subfolders and songs, each with a tick box.
/// Double-click plays a song; right-click opens a menu.
pub(crate) fn folder_row(ui: &mut egui::Ui, folder: &Folder, tree: &mut Tree, root: bool) {
    let header = |ui: &mut egui::Ui, tree: &mut Tree| {
        // The music folder itself has no tick box, so it can never be selected for the Trash.
        // Tick box, then (for an album folder, one with songs in it) its cover, then the name.
        let row = if root {
            ui.strong(&folder.name)
        } else if folder.files.is_empty() {
            select_box(ui, &folder.path, tree.selected, &folder.name)
        } else {
            select_box(ui, &folder.path, tree.selected, "");
            match tree.covers.get(&folder.path) {
                Some(Some(file)) => {
                    ui.add(
                        egui::Image::new(format!("file://{}", file.display())).fit_to_exact_size(egui::vec2(24.0, 24.0)).corner_radius(2.0),
                    );
                }
                Some(None) => {}
                None if !tree.covers_pending.contains(&folder.path) => tree.wanted_covers.push((folder.path.clone(), folder.files.clone())),
                None => {}
            }
            ui.add(egui::Label::new(&folder.name).sense(egui::Sense::click()))
        };
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

impl App {
    pub(crate) fn library_tab(&mut self, ui: &mut egui::Ui) {
        let dir = self.settings.music_dir.clone();
        ui.horizontal_wrapped(|ui| {
            let n = self.selected.len();
            let mut picked: Vec<PathBuf> = self.selected.iter().cloned().collect();
            picked.sort();
            if ui.add_enabled(n > 0, egui::Button::new(format!("Play selected ({n})"))).clicked() {
                if let Err(e) = backend::open_with(&self.settings.player, &picked) {
                    self.status = Some(e);
                }
                // Done with them: ticks left behind could later go to the Trash by surprise.
                self.selected.clear();
            }
            // Never the music folder itself, and nothing inside a folder that's also going.
            let trash: Vec<PathBuf> =
                picked.iter().filter(|p| **p != dir && !picked.iter().any(|q| q != *p && p.starts_with(q))).cloned().collect();
            let clicked =
                ui.add_enabled(!trash.is_empty(), egui::Button::new(format!("Move to Trash ({})", trash.len()))).on_hover_text("Delete");
            if (clicked.clicked() || std::mem::take(&mut self.trash_now)) && !trash.is_empty() {
                self.trash_confirm = Some(trash);
            }
            // Every song under the ticked songs and folders.
            let songs: Vec<PathBuf> = self
                .scan
                .as_ref()
                .map(|s| s.all.iter().map(|(p, _)| p).filter(|p| picked.iter().any(|q| p.starts_with(q))).cloned().collect())
                .unwrap_or_default();
            let fix = ui.add_enabled(!songs.is_empty(), egui::Button::new(format!("Fix tags ({})", songs.len()))).on_hover_text(
                "Download these songs again with clean tags, track numbers and a square cover, then move the old files to the Trash",
            );
            if fix.clicked() {
                self.fix_confirm = Some(songs);
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
            let view = self.view;
            ui.selectable_value(&mut self.view, View::Folders, "Folders");
            ui.selectable_value(&mut self.view, View::Recent, "Recently added");
            let dups = if self.indexing { "Duplicates…".to_string() } else { format!("Duplicates ({})", self.duplicates.len()) };
            ui.selectable_value(&mut self.view, View::Duplicates, dups)
                .on_hover_text("The same song in more than one place: same artist and title, and lengths within 3 seconds");
            if self.view != view {
                self.selected.clear(); // ticks you can no longer see shouldn't stay selected
            }
        });
        let Some(scan) = &self.scan else { return };
        let mut tree = Tree {
            selected: &mut self.selected,
            stems: self.stems,
            action: None,
            details: &self.details,
            probing: &self.probing,
            wanted: vec![],
            covers: &self.covers,
            covers_pending: &self.covers_pending,
            wanted_covers: vec![],
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
        let (action, wanted, wanted_covers) = (tree.action, tree.wanted, tree.wanted_covers);
        if !wanted_covers.is_empty() {
            self.covers_pending.extend(wanted_covers.iter().map(|(f, _)| f.clone()));
            self.spawn(ui.ctx(), move |_| {
                Msg::Covers(
                    wanted_covers
                        .into_iter()
                        .map(|(folder, files)| {
                            let cover = backend::folder_cover(&folder, &files);
                            (folder, cover)
                        })
                        .collect(),
                )
            });
        }
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

    pub(crate) fn fix_dialog(&mut self, ctx: &egui::Context) {
        let Some(files) = &self.fix_confirm else { return };
        let mut answer = None;
        egui::Modal::new(egui::Id::new("fix")).show(ctx, |ui| {
            let n = files.len();
            ui.heading(if n == 1 { "Fix tags on 1 song?".to_string() } else { format!("Fix tags on {n} songs?") });
            ui.label("Mixtape downloads each song again from YouTube Music, with its album's title, track number, year and a square cover, into Artist/Album. The old file then goes to the Trash.");
            ui.weak("Each song is found by its artist folder and file name. Songs that can't be found are left alone.");
            ui.horizontal(|ui| {
                if ui.button("Fix tags").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            let files = self.fix_confirm.take().unwrap();
            if yes {
                self.selected.clear();
                self.notify(format!("Looking up {} songs…", files.len()));
                let root = self.settings.music_dir.clone();
                self.spawn(ctx, move |tx| {
                    let mut missed = 0;
                    for old in files {
                        let song = backend::resolve(&root, &old);
                        missed += usize::from(song.is_err());
                        let _ = tx.send(Msg::Resolved(old, song));
                    }
                    Msg::ResolveDone(missed)
                });
            }
        }
    }

    /// Asked only when the Trash failed because the drive has none.
    pub(crate) fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(paths) = &self.delete_confirm else { return };
        let mut answer = None;
        egui::Modal::new(egui::Id::new("delete")).show(ctx, |ui| {
            let n = paths.len();
            ui.heading("This drive has no Trash");
            ui.label(if n == 1 {
                "The item can't be moved to the Trash. Delete it permanently instead?".to_string()
            } else {
                format!("These {n} items can't be moved to the Trash. Delete them permanently instead?")
            });
            for p in paths.iter().take(8) {
                ui.label(format!("•  {}", p.file_name().unwrap_or_default().to_string_lossy()));
            }
            if n > 8 {
                ui.label(format!("…and {} more", n - 8));
            }
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().error_fg_color, "Deleted files can't be restored.");
            ui.horizontal(|ui| {
                let red = egui::Button::new(egui::RichText::new("Delete permanently").color(egui::Color32::WHITE))
                    .fill(ui.visuals().error_fg_color);
                if ui.add(red).clicked() {
                    answer = Some(true);
                }
                if ui.button("Keep them").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            let paths = self.delete_confirm.take().unwrap();
            if yes {
                let dir = self.settings.music_dir.clone();
                self.spawn(ctx, move |_| {
                    let r = backend::delete_permanently(&dir, &paths);
                    Msg::Deleted(paths, r)
                });
            }
        }
    }

    pub(crate) fn trash_dialog(&mut self, ctx: &egui::Context) {
        let Some(paths) = &self.trash_confirm else { return };
        let mut answer = None;
        // Song counts per item, so a whole folder can't pass for one song.
        let count = |p: &PathBuf| -> usize { self.scan.as_ref().map_or(0, |s| s.all.iter().filter(|(f, _)| f.starts_with(p)).count()) };
        let folders: Vec<(&PathBuf, usize)> = paths.iter().filter(|p| p.is_dir()).map(|p| (p, count(p))).collect();
        let songs = paths.len() - folders.len() + folders.iter().map(|(_, n)| n).sum::<usize>();
        egui::Modal::new(egui::Id::new("trash")).show(ctx, |ui| {
            let n = paths.len();
            let what = if songs == 1 { "1 song".to_string() } else { format!("{songs} songs") };
            ui.heading(format!("Move {what} to the Trash?"));
            if !folders.is_empty() {
                ui.colored_label(ui.visuals().warn_fg_color, "This includes whole folders, with every song in them:");
            }
            for p in paths.iter().take(8) {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                match folders.iter().find(|(f, _)| *f == p) {
                    Some((_, k)) => ui.strong(format!("•  Whole folder: {name} ({k} {})", if *k == 1 { "song" } else { "songs" })),
                    None => ui.label(format!("•  {name}")),
                };
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
}
