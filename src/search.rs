//! The Search tab: results, albums and artists, previews, and whether a song is already in the library.

use crate::*;

/// Plays one search result at a time through mpv, without saving it.
pub(crate) struct Preview {
    pub(crate) mpv: bool,
    pub(crate) playing: Option<(String, std::process::Child)>,
    /// The result under the pointer last frame, for Space.
    pub(crate) hovered: Option<String>,
    /// The ▶ or ⏹ clicked this frame.
    pub(crate) clicked: Option<String>,
}

impl Preview {
    pub(crate) fn current(&self) -> Option<&str> {
        self.playing.as_ref().map(|(id, _)| id.as_str())
    }

    /// Starts this song, or stops it if it's the one playing.
    pub(crate) fn toggle(&mut self, id: String) -> Result<(), String> {
        let same = self.current() == Some(id.as_str());
        self.stop();
        if !same {
            self.playing = Some((id.clone(), backend::preview(&id)?));
        }
        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        if let Some((_, mut child)) = self.playing.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Forgets a preview that has played to the end.
    pub(crate) fn poll(&mut self) {
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

pub(crate) struct Row {
    pub(crate) song: Song,
    pub(crate) checked: bool,
}

pub(crate) struct AlbumRow {
    pub(crate) album: Album,
    pub(crate) open: bool,
    pub(crate) checked: bool,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) tracks: Option<Vec<Row>>,
}

/// A song from search counts as had when Mixtape downloaded it, or any file has its artist and title.
pub(crate) fn in_library(logged: &HashSet<String>, scan: &Option<Scan>, s: &Song) -> bool {
    logged.contains(&s.id) || scan.as_ref().is_some_and(|sc| sc.keys.contains_key(&backend::key(&s.artist, &s.title)))
}

/// An album's track only counts as had when it's in that album's folder (or was downloaded from
/// that album), so downloading an album gets all of it.
pub(crate) fn in_album(logged: &HashSet<String>, music_dir: &std::path::Path, s: &Song) -> bool {
    logged.contains(&s.id) || backend::already_there(music_dir, s)
}

/// The album folder of another copy of this song, for "also on Greatest Hits".
pub(crate) fn elsewhere(scan: &Option<Scan>, s: &Song) -> Option<String> {
    let path = scan.as_ref()?.keys.get(&backend::key(&s.artist, &s.title))?;
    Some(path.parent()?.file_name()?.to_string_lossy().into_owned())
}

/// What a result row needs to know about the library and downloads.
pub(crate) struct Lib<'a> {
    pub(crate) jobs: &'a HashMap<String, Job>,
    pub(crate) logged: &'a HashSet<String>,
    pub(crate) scan: &'a Option<Scan>,
    pub(crate) music_dir: &'a std::path::Path,
}

/// One search result. `from_album` is a track in an opened album's list: it counts as had only
/// when it's in that album's folder, and another copy elsewhere is mentioned instead.
pub(crate) fn song_row(
    ui: &mut egui::Ui,
    row_: &mut Row,
    lib: &Lib,
    from_album: bool,
    wants_confirm: &mut Option<Song>,
    preview: &mut Preview,
) {
    let have = if from_album { in_album(lib.logged, lib.music_dir, &row_.song) } else { in_library(lib.logged, lib.scan, &row_.song) };
    let other_copy = if from_album && !have { elsewhere(lib.scan, &row_.song) } else { None };
    let id = row_.song.id.clone();
    let row_duration = row_.song.duration.clone();
    let owned = have || matches!(lib.jobs.get(&id), Some(Job::Done(_)));
    // The status on the far right is easy to miss when scanning a list, so mark the title too.
    let title = |ui: &mut egui::Ui, text: &str| {
        ui.horizontal(|ui| {
            if owned {
                ui.colored_label(ui.visuals().hyperlink_color, "✔");
            }
            fit(ui, egui::RichText::new(text).strong());
        });
    };
    let hovered = row(
        ui,
        |ui| {
            if from_album {
                ui.weak(&row_duration);
            }
            job_status(ui, lib.jobs.get(&id), have);
            if let Some(album) = &other_copy
                && lib.jobs.get(&id).is_none()
            {
                ui.weak(format!("also on {album}")).on_hover_text("You have this song in another album's folder");
            }
        },
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
            // Inside an open album the album row already says who and what: just number and title.
            if from_album {
                let number = row_.song.track.map(|n| format!("{n:02}")).unwrap_or_default();
                ui.add_sized([22.0, 18.0], egui::Label::new(egui::RichText::new(number).weak()));
                title(ui, &row_.song.title);
                return;
            }
            cover(ui, &row_.song.thumb, 40.0);
            ui.vertical(|ui| {
                title(ui, &row_.song.title);
                let album = if row_.song.album.is_empty() {
                    " · no album (goes to Singles)".to_string()
                } else {
                    format!(" · {}", row_.song.album)
                };
                fit(ui, format!("{}{album} · {}", row_.song.artists, row_.song.duration));
            });
        },
    );
    if hovered {
        preview.hovered = Some(id);
    }
}

impl App {
    pub(crate) fn search_tab(&mut self, ui: &mut egui::Ui) {
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
        // A ticked album counts once its track list has loaded; until then downloading would leave it out.
        let loading = self.albums.iter().any(|a| a.checked && a.tracks.is_none() && a.error.is_none());
        ui.horizontal(|ui| {
            let label = if loading { "Loading tracks…".to_string() } else { format!("Download selected ({})", selected.len()) };
            let ready = !selected.is_empty() && !loading;
            let clicked = ui.add_enabled(ready, accent_button(ui, label)).on_hover_text("Ctrl+D").clicked();
            if (clicked || std::mem::take(&mut self.download_now)) && ready {
                selected.into_iter().for_each(|song| self.enqueue(song));
                self.rows_mut().for_each(|r| r.checked = false);
                self.albums.iter_mut().for_each(|a| a.checked = false);
            }
            let (running, queued) = self.queue_counts();
            if running + queued > 0 {
                ui.spinner();
                ui.label(format!("Downloading {running}, {queued} waiting"));
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
        let Self { songs, albums, artists, page_title, jobs, logged, scan, preview, settings, .. } = self;
        let lib = Lib { jobs, logged, scan, music_dir: &settings.music_dir };
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
                                    t.checked = a.checked && !in_album(lib.logged, lib.music_dir, &t.song);
                                }
                            }
                            cover(ui, &a.album.thumb, 48.0);
                            ui.vertical(|ui| {
                                fit(ui, egui::RichText::new(&a.album.title).strong());
                                let count = match &a.tracks {
                                    Some(t) if t.len() == 1 => "  ·  1 song".to_string(),
                                    Some(t) => format!("  ·  {} songs", t.len()),
                                    None => String::new(),
                                };
                                fit(ui, format!("{}  ·  {} {}{count}", a.album.artists, a.album.kind, a.album.year));
                            });
                        },
                    );
                    // Every album's track list loads in the background (for its song count and an instant
                    // open); opened and ticked ones go first.
                    if a.tracks.is_none() && !a.loading && a.error.is_none() {
                        load_album.push((!(a.open || a.checked), a.album.id.clone()));
                    }
                    if let Some(e) = &a.error {
                        error_line(ui, e);
                    }
                    if a.open
                        && let Some(tracks) = &mut a.tracks
                    {
                        ui.indent(&a.album.id, |ui| {
                            for row in tracks.iter_mut() {
                                song_row(ui, row, &lib, true, &mut wants_confirm, preview);
                            }
                        });
                    }
                }
                ui.add_space(8.0);
            }
            if !songs.is_empty() {
                ui.heading("Songs");
                for row in songs.iter_mut() {
                    song_row(ui, row, &lib, false, &mut wants_confirm, preview);
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
        load_album.sort_by_key(|(later, _)| *later); // stable: otherwise in display order
        for (_, id) in load_album.into_iter().take(3usize.saturating_sub(self.album_loads)) {
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

    pub(crate) fn confirm_dialog(&mut self, ctx: &egui::Context) {
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
                self.downloads.0.lock().overwrite.insert(song.id.clone());
            }
            self.confirm = None;
        }
    }
}
