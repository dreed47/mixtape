//! Window tests: draw a tab headless with egui_kittest, find widgets by their label, click them,
//! and check the app's state. Nothing here scans a real folder or touches the network.

use crate::*;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

fn song(id: &str, title: &str) -> Song {
    Song {
        id: id.into(),
        title: title.into(),
        artist: "Queen".into(),
        artists: "Queen".into(),
        album: "Greatest Hits".into(),
        duration: "3:00".into(),
        track: None,
        thumb: String::new(),
        album_id: String::new(),
        year: String::new(),
    }
}

/// An app whose music folder is an empty scratch folder.
fn app(name: &str) -> (App, PathBuf) {
    let dir = std::env::temp_dir().join(format!("mixtape-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let settings = Settings { music_dir: dir.clone(), ..Settings::default() };
    (App::with_settings(settings), dir)
}

fn album(checked: bool, tracks: Option<Vec<Row>>) -> AlbumRow {
    let album = backend::Album {
        id: "MPREb_test".into(),
        title: "Greatest Hits".into(),
        artists: "Queen".into(),
        year: "1981".into(),
        kind: "Album".into(),
        thumb: String::new(),
    };
    // `loading` keeps the tab from fetching the track list for real.
    AlbumRow { album, open: false, checked, loading: tracks.is_none(), error: None, tracks }
}

#[test]
fn settings_page_has_its_sections_and_settings() {
    let (state, dir) = app("settings");
    let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.settings_tab(ui), state);
    harness.run();
    for label in ["Settings", "Library", "Downloads", "App", "Dependencies", "Music folder", "Player", "Format", "Premium", "Notifications"]
    {
        harness.get_by_label(label);
    }
    // The notification setting is on by default and turns off with a click.
    assert!(harness.state().settings.notify);
    harness.get_by_label("Tell me when downloads finish").click();
    harness.run();
    assert!(!harness.state().settings.notify);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn download_waits_until_ticked_albums_have_loaded() {
    let (mut state, dir) = app("albums");
    state.albums = vec![album(true, None)];
    let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.search_tab(ui), state);
    harness.run_steps(3); // the album's spinner keeps repainting, so step instead of run
    assert!(harness.get_by_label("Loading tracks…").accesskit_node().is_disabled());

    // The track list arrives with its track ticked: now it can download.
    let mut s = song("abcdefghijk", "Bohemian Rhapsody");
    s.track = Some(1);
    harness.state_mut().albums = vec![album(true, Some(vec![Row { song: s, checked: true }]))];
    harness.run();
    let button = harness.get_by_label("Download selected (1)");
    assert!(!button.accesskit_node().is_disabled());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ticking_a_song_you_have_asks_first() {
    let (mut state, dir) = app("have");
    state.songs = vec![Row { song: song("abcdefghijk", "Bicycle Race"), checked: false }];
    state.logged.insert("abcdefghijk".into());
    state.preview.mpv = false; // no ▶ buttons, so the only check box is the song's
    let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.search_tab(ui), state);
    harness.run();
    harness.get_by_label("✔ In library");
    harness.get_by_role(Role::CheckBox).click();
    harness.run();
    assert!(harness.state().confirm.is_some(), "asks \"Download again?\"");
    assert!(!harness.state().songs[0].checked, "not ticked until you say yes");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_music_folder_itself_has_no_tick_box() {
    let (mut state, dir) = app("root");
    std::fs::create_dir_all(dir.join("Queen/Greatest Hits")).unwrap();
    std::fs::write(dir.join("Queen/Greatest Hits/01 Bohemian Rhapsody.mp3"), "").unwrap();
    state.scan = Some(backend::scan(&dir));
    let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.library_tab(ui), state);
    harness.run();
    harness.get_by_label(&dir.display().to_string());
    // Only "Queen" gets a tick box; the music folder above it doesn't.
    assert_eq!(harness.get_all_by_role(Role::CheckBox).count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn select_all_ticks_only_the_songs_shown_and_clears_again() {
    let (mut state, dir) = app("select-all");
    std::fs::create_dir_all(dir.join("Queen/Greatest Hits")).unwrap();
    std::fs::create_dir_all(dir.join("Abba/Gold")).unwrap();
    for song in
        ["Queen/Greatest Hits/01 Bohemian Rhapsody.mp3", "Queen/Greatest Hits/02 Under Pressure.mp3", "Abba/Gold/01 Dancing Queen.mp3"]
    {
        std::fs::write(dir.join(song), "").unwrap();
    }
    state.scan = Some(backend::scan(&dir));
    state.library_filter = "rhapsody".into();
    let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.library_tab(ui), state);
    harness.run();
    harness.get_by_label("Select all (1)").click();
    harness.run();
    let want: HashSet<PathBuf> = [dir.join("Queen/Greatest Hits/01 Bohemian Rhapsody.mp3")].into();
    assert_eq!(harness.state().selected, want);
    harness.get_by_label("1 selected").click();
    harness.run();
    assert!(harness.state().selected.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn trashing_a_folder_says_it_is_a_whole_folder() {
    let (mut state, dir) = app("trash");
    let album = dir.join("Queen/Greatest Hits");
    std::fs::create_dir_all(&album).unwrap();
    for song in ["01 Bohemian Rhapsody.mp3", "02 Another One Bites the Dust.mp3"] {
        std::fs::write(album.join(song), "").unwrap();
    }
    state.scan = Some(backend::scan(&dir));
    state.trash_confirm = Some(vec![album.clone()]);
    let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.trash_dialog(ui.ctx()), state);
    harness.run();
    harness.get_by_label("Move 2 songs to the Trash?");
    harness.get_by_label("This includes whole folders, with every song in them:");
    harness.get_by_label("•  Whole folder: Greatest Hits (2 songs)");
    std::fs::remove_dir_all(dir).unwrap();
}
