//! Everything that is not UI: settings, search, downloads, the download log and library scanning.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const YTMUSIC_PY: &str = include_str!("ytmusic.py");
const AUDIO_EXTS: [&str; 8] = ["mp3", "m4a", "flac", "ogg", "opus", "wav", "aac", "wma"];

/// Players offered in Settings when installed: (label, command).
pub const PLAYERS: [(&str, &str); 3] =
    [("Amberol", "amberol"), ("VLC", "vlc"), ("mpv", "mpv --player-operation-mode=pseudo-gui")];

#[derive(Deserialize, Clone)]
pub struct Song {
    pub id: String,
    pub title: String,
    /// Primary artist, used for the folder name and the library match.
    pub artist: String,
    /// All artists, for display.
    pub artists: String,
    pub album: String,
    pub duration: String,
}

#[derive(Deserialize, Clone)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub year: String,
}

#[derive(Deserialize)]
pub struct SearchResult {
    pub songs: Vec<Song>,
    pub albums: Vec<Album>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

pub fn state_home() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
}

fn settings_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("mixtape/settings.json")
}

fn log_file() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("mixtape/downloads.tsv")
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub music_dir: PathBuf,
    pub player: String,
    pub format: Format,
    /// yt-dlp name of the browser whose YouTube login to use (for Premium audio); empty = none.
    pub cookies_browser: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Converted to MP3 (VBR V0): plays everywhere.
    #[default]
    Mp3,
    /// The audio exactly as YouTube sends it (Opus, or AAC with Premium): best quality, no conversion.
    Original,
}

/// Browsers yt-dlp can read a YouTube login from: (label, yt-dlp name, binary).
pub const BROWSERS: [(&str, &str, &str); 6] = [
    ("Chromium", "chromium", "chromium"),
    ("Firefox", "firefox", "firefox"),
    ("Chrome", "chrome", "google-chrome-stable"),
    ("Brave", "brave", "brave"),
    ("Vivaldi", "vivaldi", "vivaldi-stable"),
    ("Edge", "edge", "microsoft-edge-stable"),
];

pub fn detected_browsers() -> Vec<(&'static str, &'static str)> {
    BROWSERS.into_iter().filter(|b| on_path(b.2)).map(|b| (b.0, b.1)).collect()
}

impl Default for Settings {
    fn default() -> Self {
        let player = detected_players().first().map_or("xdg-open", |p| p.1).to_string();
        Self { music_dir: home().join("Music"), player, format: Format::Mp3, cookies_browser: String::new() }
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(settings_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = settings_file();
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&path, serde_json::to_string_pretty(self).unwrap()).map_err(|e| e.to_string())
    }
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

pub fn detected_players() -> Vec<(&'static str, &'static str)> {
    PLAYERS.into_iter().filter(|(_, cmd)| on_path(cmd.split(' ').next().unwrap())).collect()
}

/// Command that opens a song in the Stems app (splits a song into instruments), when installed.
pub const STEMS: &str = "stems open";

pub fn stems_installed() -> bool {
    on_path("stems")
}

/// Runs `command` with the songs or folders appended. shortcut: the command is split on spaces, so no quoted arguments; add shell-style parsing if someone needs them.
pub fn open_with(command: &str, paths: &[PathBuf]) -> Result<(), String> {
    let mut words = command.split_whitespace();
    let bin = words.next().ok_or("no player set in Settings")?;
    let mut child = Command::new(bin).args(words).args(paths).spawn().map_err(|e| format!("could not start {bin}: {e}"))?;
    std::thread::spawn(move || child.wait()); // reap it so it doesn't linger as a zombie
    Ok(())
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let bin = cmd.get_program().to_string_lossy().into_owned();
    let out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{bin} is not installed (see README)"),
        _ => format!("could not start {bin}: {e}"),
    })?;
    if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(last_line(&out.stderr)) }
}

fn last_line(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string()
}

fn ytmusic(mode: &str, arg: &str) -> Result<String, String> {
    run(Command::new("uv").args(["run", "-q", "--no-project", "--with", "ytmusicapi", "python", "-I", "-c", YTMUSIC_PY, mode, arg]))
}

pub fn search(query: &str) -> Result<SearchResult, String> {
    serde_json::from_str(&ytmusic("search", query)?).map_err(|e| e.to_string())
}

pub fn album_tracks(browse_id: &str) -> Result<Vec<Song>, String> {
    if !safe_id(browse_id, 64) {
        return Err(format!("bad album id {browse_id:?}"));
    }
    serde_json::from_str(&ytmusic("album", browse_id)?).map_err(|e| e.to_string())
}

/// YouTube ids are [A-Za-z0-9_-]; anything else never reaches yt-dlp.
fn safe_id(id: &str, max: usize) -> bool {
    !id.is_empty() && id.len() <= max && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A path component that can't escape its folder or hide itself.
fn safe_name(s: &str, empty: &str) -> String {
    let s: String = s.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).take(120).collect();
    let s = s.trim().trim_start_matches('.').trim();
    if s.is_empty() { empty.to_string() } else { s.to_string() }
}

fn parts(s: &Song) -> [String; 3] {
    [safe_name(&s.artist, "Unknown Artist"), safe_name(&s.album, "Singles"), safe_name(&s.title, s.id.as_str())]
}

pub struct Downloaded {
    pub path: PathBuf,
    /// What YouTube sent, before any conversion, e.g. "130k Opus" or "256k AAC".
    pub source: String,
}

/// Downloads one song to <music_dir>/Artist/Album/Title.<ext>, reporting progress 0..1.
pub fn download(settings: &Settings, s: &Song, progress: impl Fn(f32)) -> Result<Downloaded, String> {
    if !safe_id(&s.id, 11) {
        return Err(format!("bad video id {:?}", s.id));
    }
    let [artist, album, title] = parts(s).map(|p| p.replace('%', "%%")); // yt-dlp template escaping
    let mut cmd = Command::new("yt-dlp");
    cmd.args(["-x", "--no-playlist", "--embed-metadata", "--convert-thumbnails", "jpg"]);
    match settings.format {
        Format::Mp3 => cmd.args(["--audio-format", "mp3", "--audio-quality", "0", "--embed-thumbnail"]),
        // Embedding art in Opus needs Python's mutagen, which many installs lack; a cover.jpg beside the songs works in every player.
        Format::Original => cmd.args(["--write-thumbnail", "-o"]).arg(format!("thumbnail:{artist}/{album}/cover.%(ext)s")),
    };
    if !settings.cookies_browser.is_empty() {
        if !BROWSERS.iter().any(|b| b.1 == settings.cookies_browser) {
            return Err(format!("unknown browser {:?} in Settings", settings.cookies_browser));
        }
        cmd.args(["--cookies-from-browser", &settings.cookies_browser]);
    }
    let mut child = cmd
        .args(["--quiet", "--progress", "--newline", "--progress-template"])
        .arg("download:MIXTAPE %(progress.downloaded_bytes)s %(progress.total_bytes,progress.total_bytes_estimate)s")
        .args(["--print", "video:MIXTAPE_SOURCE %(acodec)s %(abr)s"])
        .args(["--print", "after_move:MIXTAPE_FILE %(filepath)s"])
        .arg("-P")
        .arg(&settings.music_dir)
        .arg("-o")
        .arg(format!("{artist}/{album}/{title}.%(ext)s"))
        .arg(format!("https://music.youtube.com/watch?v={}", s.id))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start yt-dlp: {e}"))?;
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let (mut saved, mut source) = (None, String::new());
    for line in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) {
        if let Some(f) = parse_progress(&line) {
            progress(f);
        } else if let Some(s) = line.strip_prefix("MIXTAPE_SOURCE ") {
            source = describe_source(s);
        } else if let Some(path) = line.strip_prefix("MIXTAPE_FILE ") {
            saved = Some(PathBuf::from(path));
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr = errors.join().unwrap_or_default();
    match (status.success(), saved) {
        (true, Some(path)) => Ok(Downloaded { path, source }),
        (true, None) => Err("yt-dlp finished without saving a file".into()),
        (false, _) => Err(last_line(&stderr)),
    }
}

/// "opus 129.055" -> "129k Opus"; "mp4a.40.2 256.0" -> "256k AAC".
fn describe_source(s: &str) -> String {
    let (codec, abr) = s.split_once(' ').unwrap_or((s, ""));
    let name = match codec {
        "opus" => "Opus".to_string(),
        c if c.starts_with("mp4a") => "AAC".to_string(),
        c => c.to_uppercase(),
    };
    match abr.parse::<f64>() {
        Ok(k) => format!("{}k {name}", k.round()),
        Err(_) => name,
    }
}

fn parse_progress(line: &str) -> Option<f32> {
    let mut it = line.strip_prefix("MIXTAPE ")?.split(' ');
    let done: f64 = it.next()?.parse().ok()?;
    let total: f64 = it.next()?.parse().ok()?;
    (total > 0.0).then(|| (done / total).clamp(0.0, 1.0) as f32)
}

/// Moves files and folders to the desktop Trash with `gio trash`, so they can be restored.
pub fn trash(paths: &[PathBuf]) -> Result<(), String> {
    run(Command::new("gio").arg("trash").arg("--").args(paths)).map(|_| ())
}

/// Drops log lines for songs under `paths`, so trashed songs no longer count as downloaded.
pub fn forget(paths: &[PathBuf]) -> Result<(), String> {
    let file = log_file();
    let Ok(text) = std::fs::read_to_string(&file) else { return Ok(()) };
    std::fs::write(&file, forget_lines(&text, paths)).map_err(|e| e.to_string())
}

fn forget_lines(text: &str, paths: &[PathBuf]) -> String {
    let gone = |line: &str| line.split_once('\t').is_some_and(|(_, p)| paths.iter().any(|d| Path::new(p).starts_with(d)));
    text.lines().filter(|l| !gone(l)).map(|l| format!("{l}\n")).collect()
}

/// Ids Mixtape has downloaded before.
pub fn read_log() -> HashSet<String> {
    let text = std::fs::read_to_string(log_file()).unwrap_or_default();
    text.lines().filter_map(|l| l.split('\t').next()).filter(|id| !id.is_empty()).map(String::from).collect()
}

pub fn append_log(id: &str, path: &Path) -> Result<(), String> {
    let file = log_file();
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file).map_err(|e| e.to_string())?;
    writeln!(f, "{id}\t{}", path.display()).map_err(|e| e.to_string())
}

/// Lowercase letters and digits only, so "AC/DC" and "ac-dc" match.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub fn key(artist: &str, title: &str) -> String {
    format!("{}|{}", norm(artist), norm(title))
}

pub struct Folder {
    pub name: String,
    pub path: PathBuf,
    pub children: Vec<Folder>,
    /// Audio files directly in this folder, sorted by name.
    pub files: Vec<PathBuf>,
}

pub struct Scan {
    /// artist|title keys for every audio file, from its path.
    pub keys: HashSet<String>,
    pub root: Folder,
    pub songs: usize,
}

/// Walks the music folder. shortcut: songs are matched by path (Artist/Album/Title, Artist/Title or "Artist - Title"), not by reading tags; read ID3 tags if files from other tools are named differently.
pub fn scan(dir: &Path) -> Scan {
    let (mut keys, mut songs) = (HashSet::new(), 0);
    let root = walk(dir, dir.display().to_string(), &mut keys, &mut songs);
    Scan { keys, root, songs }
}

fn walk(dir: &Path, name: String, keys: &mut HashSet<String>, songs: &mut usize) -> Folder {
    let mut folder = Folder { name, path: dir.to_path_buf(), children: vec![], files: vec![] };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // file_type() does not follow symlinks, so a symlink loop can't trap the walk.
        let Ok(kind) = entry.file_type() else { continue };
        if name.starts_with('.') {
            continue;
        }
        if kind.is_dir() {
            folder.children.push(walk(&path, name, keys, songs));
        } else if path.extension().is_some_and(|x| AUDIO_EXTS.iter().any(|a| x.eq_ignore_ascii_case(a))) {
            *songs += 1;
            folder.files.push(path.clone());
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let title = stem.trim_start_matches(|c: char| c.is_ascii_digit() || " -.".contains(c)); // "01 - Title"
            let dir_name = |p: Option<&Path>| p.and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let parent = path.parent();
            for artist in [dir_name(parent), dir_name(parent.and_then(Path::parent))] {
                keys.insert(key(&artist, &stem));
                keys.insert(key(&artist, title));
            }
            if let Some((artist, t)) = stem.split_once(" - ") {
                keys.insert(key(artist, t));
            }
        }
    }
    folder.children.sort_by_key(|f| f.name.to_lowercase());
    folder.files.sort_by_key(|f| f.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    folder
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(artist: &str, album: &str, title: &str) -> Song {
        Song { id: "dQw4w9WgXcQ".into(), title: title.into(), artist: artist.into(), artists: artist.into(), album: album.into(), duration: String::new() }
    }

    #[test]
    fn ids() {
        assert!(safe_id("dQw4w9WgXcQ", 11));
        assert!(safe_id("-abc_DEF123", 11));
        assert!(!safe_id("--exec=evil", 11));
        assert!(!safe_id("", 11));
    }

    #[test]
    fn paths_stay_inside_the_music_folder() {
        assert_eq!(parts(&song("AC/DC", "../..", ".hidden")), ["AC_DC", "_..", "hidden"]);
        assert_eq!(parts(&song("", "", "T")), ["Unknown Artist", "Singles", "T"]);
    }

    #[test]
    fn old_settings_files_still_load() {
        let s: Settings = serde_json::from_str(r#"{"music_dir":"/m","player":"vlc"}"#).unwrap();
        assert!(s.format == Format::Mp3 && s.cookies_browser.is_empty());
        let s: Settings = serde_json::from_str(r#"{"format":"original","cookies_browser":"firefox"}"#).unwrap();
        assert!(s.format == Format::Original && s.cookies_browser == "firefox");
    }

    #[test]
    fn forgetting_a_folder_forgets_its_songs() {
        let log = "a\t/m/X/One.mp3\nb\t/m/X/Y/Two.mp3\nc\t/m/Xylo/Three.mp3\n";
        assert_eq!(forget_lines(log, &[PathBuf::from("/m/X")]), "c\t/m/Xylo/Three.mp3\n");
        assert_eq!(forget_lines(log, &[PathBuf::from("/m/Xylo/Three.mp3")]), "a\t/m/X/One.mp3\nb\t/m/X/Y/Two.mp3\n");
    }

    #[test]
    fn source_quality() {
        assert_eq!(describe_source("opus 129.055"), "129k Opus");
        assert_eq!(describe_source("mp4a.40.2 256.0"), "256k AAC");
        assert_eq!(describe_source("opus NA"), "Opus");
    }

    #[test]
    fn progress() {
        assert_eq!(parse_progress("MIXTAPE 50 200"), Some(0.25));
        assert_eq!(parse_progress("MIXTAPE 50 NA"), None);
        assert_eq!(parse_progress("other"), None);
    }

    #[test]
    fn scan_matches_common_layouts() {
        let dir = std::env::temp_dir().join(format!("mixtape-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Rick Astley/Whenever You Need Somebody")).unwrap();
        std::fs::write(dir.join("Rick Astley/Whenever You Need Somebody/01 - Never Gonna Give You Up.mp3"), "").unwrap();
        std::fs::write(dir.join("AC-DC - Thunderstruck.mp3"), "").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let s = scan(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(s.songs, 2);
        assert!(s.keys.contains(&key("rick astley", "Never Gonna Give You Up")));
        assert!(s.keys.contains(&key("AC/DC", "thunderstruck")));
        assert_eq!(s.root.children[0].name, "Rick Astley");
        assert_eq!(s.root.children[0].children.len(), 1);
        assert_eq!(s.root.children[0].children[0].files.len(), 1);
        assert_eq!(s.root.files.len(), 1);
    }
}
