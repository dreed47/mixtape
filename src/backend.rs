//! Everything that is not UI: settings, search, downloads, the download log and library scanning.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const YTMUSIC_PY: &str = include_str!("ytmusic.py");
/// Any 1.x keeps getting YouTube Music fixes; a 2.0 could rename what ytmusic.py uses, so it's
/// only allowed after the helper is checked against it.
const YTMUSICAPI: &str = "ytmusicapi>=1.12,<2";
const AUDIO_EXTS: [&str; 8] = ["mp3", "m4a", "flac", "ogg", "opus", "wav", "aac", "wma"];

/// Players offered in Settings when installed: (label, command).
pub const PLAYERS: [(&str, &str); 3] = [("Amberol", "amberol"), ("VLC", "vlc"), ("mpv", "mpv --player-operation-mode=pseudo-gui")];

#[derive(Serialize, Deserialize, Clone)]
pub struct Song {
    pub id: String,
    pub title: String,
    /// Primary artist, used for the folder name and the library match.
    pub artist: String,
    /// All artists, for display.
    pub artists: String,
    pub album: String,
    pub duration: String,
    /// Position on the album, when the song came from an album's track list.
    #[serde(default)]
    pub track: Option<u32>,
    /// Cover thumbnail (https), shown in search results.
    #[serde(default)]
    pub thumb: String,
}

#[derive(Deserialize, Clone)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub year: String,
    /// "Album", "Single" or "EP".
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub thumb: String,
}

#[derive(Deserialize, Clone)]
pub struct Artist {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct SearchResult {
    pub songs: Vec<Song>,
    pub albums: Vec<Album>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    /// Set for an artist's page or a playlist: its name.
    #[serde(default)]
    pub title: Option<String>,
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

fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("mixtape")
}

fn log_file() -> PathBuf {
    data_dir().join("downloads.tsv")
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub music_dir: PathBuf,
    pub player: String,
    pub format: Format,
    /// yt-dlp name of the browser whose YouTube login to use (for Premium audio); empty = none.
    pub cookies_browser: String,
    /// Ask GitHub for a newer release at startup and once a day.
    pub check_updates: bool,
    /// Size of text and controls, 1.0 = normal. Ctrl + and Ctrl − change it too.
    pub text_scale: f32,
}

pub const TEXT_SCALES: std::ops::RangeInclusive<f32> = 0.8..=2.0;

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
        Self {
            music_dir: home().join("Music"),
            player,
            format: Format::Mp3,
            cookies_browser: String::new(),
            check_updates: true,
            text_scale: 1.0,
        }
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

/// A command Mixtape runs, and the package that provides it on each family of distributions.
pub struct Tool {
    pub bin: &'static str,
    pub needed_for: &'static str,
    pub required: bool,
    arch: &'static str,
    debian: Option<&'static str>,
    fedora: &'static str,
}

pub const TOOLS: [Tool; 6] = [
    Tool { bin: "uv", needed_for: "searching YouTube Music", required: true, arch: "uv", debian: None, fedora: "uv" },
    Tool { bin: "yt-dlp", needed_for: "downloading", required: true, arch: "yt-dlp", debian: Some("yt-dlp"), fedora: "yt-dlp" },
    Tool {
        bin: "ffmpeg",
        needed_for: "converting audio and cover art",
        required: true,
        arch: "ffmpeg",
        debian: Some("ffmpeg"),
        fedora: "ffmpeg",
    },
    Tool {
        bin: "ffprobe",
        needed_for: "song details in the Library",
        required: true,
        arch: "ffmpeg",
        debian: Some("ffmpeg"),
        fedora: "ffmpeg",
    },
    Tool { bin: "mpv", needed_for: "previews", required: false, arch: "mpv", debian: Some("mpv"), fedora: "mpv" },
    Tool { bin: "gio", needed_for: "Move to Trash", required: false, arch: "glib2", debian: Some("libglib2.0-bin"), fedora: "glib2" },
];

/// Tools that aren't installed, required ones first.
pub fn missing_tools() -> Vec<&'static Tool> {
    let mut missing: Vec<_> = TOOLS.iter().filter(|t| !on_path(t.bin)).collect();
    missing.sort_by_key(|t| !t.required);
    missing
}

pub fn tool_installed(t: &Tool) -> bool {
    on_path(t.bin)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Distro {
    Arch,
    Debian,
    Fedora,
    Other,
}

fn distro(os_release: &str) -> Distro {
    let ids: Vec<&str> = os_release
        .lines()
        .filter_map(|l| l.strip_prefix("ID=").or_else(|| l.strip_prefix("ID_LIKE=")))
        .flat_map(|v| v.trim_matches('"').split_whitespace())
        .collect();
    let has = |names: &[&str]| ids.iter().any(|id| names.contains(id));
    if has(&["arch"]) {
        Distro::Arch
    } else if has(&["debian", "ubuntu"]) {
        Distro::Debian
    } else if has(&["fedora", "rhel"]) {
        Distro::Fedora
    } else {
        Distro::Other
    }
}

/// The command that installs these tools here, and anything it can't cover.
pub fn install_hint(tools: &[&Tool]) -> (Option<String>, Vec<String>) {
    install_hint_for(distro(&std::fs::read_to_string("/etc/os-release").unwrap_or_default()), tools)
}

fn install_hint_for(distro: Distro, tools: &[&Tool]) -> (Option<String>, Vec<String>) {
    let mut packages: Vec<&str> = Vec::new();
    let mut notes = Vec::new();
    for t in tools {
        let package = match distro {
            Distro::Arch => Some(t.arch),
            Distro::Debian => t.debian,
            Distro::Fedora => Some(t.fedora),
            Distro::Other => None,
        };
        match package {
            Some(p) if !packages.contains(&p) => packages.push(p),
            Some(_) => {}
            None if t.bin == "uv" => notes.push("Install uv from https://docs.astral.sh/uv/".to_string()),
            None => notes.push(format!("Install {} with your package manager", t.bin)),
        }
    }
    let tool = match distro {
        Distro::Arch => "sudo pacman -S --needed",
        Distro::Debian => "sudo apt install",
        Distro::Fedora => "sudo dnf install",
        Distro::Other => "",
    };
    ((!packages.is_empty()).then(|| format!("{tool} {}", packages.join(" "))), notes)
}

pub fn mpv_installed() -> bool {
    on_path("mpv")
}

/// Streams a song's audio through mpv, saving nothing.
pub fn preview(id: &str) -> Result<std::process::Child, String> {
    if !safe_id(id, 11) {
        return Err(format!("bad video id {id:?}"));
    }
    Command::new("mpv")
        .args(["--no-video", "--really-quiet", "--no-terminal", "--ytdl-format=bestaudio"])
        .arg(format!("https://music.youtube.com/watch?v={id}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start mpv: {e}"))
}

pub fn stems_installed() -> bool {
    on_path("stems")
}

/// Runs `command` with the songs or folders appended. shortcut: the command is split on spaces, so no quoted arguments; add shell-style parsing if someone needs them.
pub fn open_with(command: &str, paths: &[PathBuf]) -> Result<(), String> {
    let mut words = command.split_whitespace();
    let bin = words.next().ok_or("no player set in Settings")?;
    let mut child = Command::new(bin).args(words).args(paths).spawn().map_err(|e| format!("could not start {bin}: {e}"))?;
    std::thread::spawn(move || child.wait()); // reap it so it doesn't linger as a zombie
    if let Some((_, bus)) = AUTOPLAY.iter().find(|(b, _)| *b == bin) {
        std::thread::spawn(move || press_play(bus));
    }
    Ok(())
}

/// Players that load songs without playing them, and their MPRIS name on D-Bus.
const AUTOPLAY: [(&str, &str); 1] = [("amberol", "org.mpris.MediaPlayer2.io.bassi.Amberol")];

/// Presses Play over MPRIS until the player reports Playing, for up to 10 seconds
/// (it needs a moment to start and load the songs).
fn press_play(bus: &str) {
    let gdbus = |args: &[&str]| {
        Command::new("gdbus")
            .args(["call", "--session", "--dest", bus, "--object-path", "/org/mpris/MediaPlayer2", "--method"])
            .args(args)
            .stderr(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    for _ in 0..33 {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let status = gdbus(&["org.freedesktop.DBus.Properties.Get", "org.mpris.MediaPlayer2.Player", "PlaybackStatus"]);
        if status.contains("Playing") {
            return;
        }
        if !status.is_empty() {
            gdbus(&["org.mpris.MediaPlayer2.Player.Play"]);
        }
    }
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let bin = cmd.get_program().to_string_lossy().into_owned();
    let out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{bin} is not installed (see README)"),
        _ => format!("could not start {bin}: {e}"),
    })?;
    if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(tail(&out.stderr)) }
}

/// The end of a program's error output: the details behind a failure, for "Copy details".
fn tail(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = s.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let text = lines[lines.len().saturating_sub(40)..].join("\n");
    if text.is_empty() { "failed with no message".into() } else { text }
}

/// Stops a download started by `download`: yt-dlp and the ffmpeg it runs. The .part file stays,
/// so downloading the song again continues it.
pub fn stop(pid: u32) {
    let _ = Command::new("kill").args(["-TERM", "--"]).arg(format!("-{pid}")).stderr(Stdio::null()).status();
}

/// YouTube is rate-limiting or asking for a bot check: more downloads right now would fail too.
pub fn is_blocked(raw: &str) -> bool {
    let raw = raw.to_lowercase();
    ["not a bot", "http error 429", "too many requests"].iter().any(|n| raw.contains(n))
}

/// A plain-language version of an error from yt-dlp, ytmusicapi, uv or gio. The raw text stays
/// available for "Copy details".
pub fn friendly(raw: &str) -> String {
    let has = |needles: &[&str]| needles.iter().any(|n| raw.to_lowercase().contains(&n.to_lowercase()));
    let known = if has(&["not a bot"]) {
        Some(
            "YouTube wants to confirm you're not a bot. Wait a while and try again, or turn on the Premium login in Settings so yt-dlp uses your browser's sign-in.",
        )
    } else if has(&["in your country", "not made this video available"]) {
        Some("This song isn't available in your country.")
    } else if has(&["private video"]) {
        Some("This song is private on YouTube.")
    } else if has(&["video unavailable", "video is unavailable", "video is not available", "has been removed"]) {
        Some("This song isn't available on YouTube anymore.")
    } else if has(&["confirm your age", "age-restricted"]) {
        Some("YouTube needs a signed-in adult account for this song. Turn on the Premium login in Settings to use your browser's sign-in.")
    } else if has(&["cookie", "failed to decrypt"]) {
        Some(
            "yt-dlp couldn't read your YouTube login from the browser chosen in Settings. Check that you're signed in to music.youtube.com there, or turn off the Premium login.",
        )
    } else if has(&["http error 403", "signature extraction", "nsig extraction", "requested format is not available", "unable to extract"])
    {
        return format!(
            "YouTube changed something this version of yt-dlp can't handle yet. Updating yt-dlp usually fixes it; {}",
            "when a newer one is out, the bottom of the window shows how."
        );
    } else if has(&[
        "name resolution",
        "name or service not known",
        "network is unreachable",
        "connectionerror",
        "timed out",
        "connection refused",
        "failed to fetch",
    ]) {
        Some("Couldn't reach YouTube. Check your internet connection.")
    } else if has(&["no space left"]) {
        Some("The disk with your music folder is full.")
    } else if has(&["permission denied", "read-only file system"]) {
        Some("Mixtape isn't allowed to write to that folder.")
    } else if has(&["trash directory", "trashing on system internal mounts is not supported"]) {
        Some("That folder has no Trash (common on network shares), so nothing was moved.")
    } else {
        None
    };
    known.map(String::from).unwrap_or_else(|| {
        let last = raw.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(raw).trim();
        let last = last.strip_prefix("ERROR: ").unwrap_or(last);
        // "[youtube] abc123: message" -> "message"
        let last = if last.starts_with('[') { last.split_once("]: ").or(last.split_once(": ")).map_or(last, |p| p.1) } else { last };
        last.to_string()
    })
}

fn ytmusic(mode: &str, arg: &str) -> Result<String, String> {
    run(Command::new("uv").args(["run", "-q", "--no-project", "--with", YTMUSICAPI, "python", "-I", "-c", YTMUSIC_PY, mode, arg]))
}

/// A search, or a pasted YouTube / YouTube Music link to a song, album, playlist or artist.
pub fn search(query: &str) -> Result<SearchResult, String> {
    let mode = if is_link(query) { "link" } else { "search" };
    serde_json::from_str(&ytmusic(mode, query)?).map_err(|e| e.to_string())
}

pub fn is_link(query: &str) -> bool {
    let q = query.trim();
    q.starts_with("https://") || q.starts_with("http://")
}

/// An artist's albums, singles and top songs.
pub fn artist(channel_id: &str) -> Result<SearchResult, String> {
    if !safe_id(channel_id, 64) {
        return Err(format!("bad artist id {channel_id:?}"));
    }
    serde_json::from_str(&ytmusic("artist", channel_id)?).map_err(|e| e.to_string())
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

/// Artist, album and file name (without extension). Album tracks start with their number so players keep album order.
fn parts(s: &Song) -> [String; 3] {
    let title = safe_name(&s.title, s.id.as_str());
    let file = match s.track {
        Some(n) => format!("{n:02} {title}"),
        None => title,
    };
    [safe_name(&s.artist, "Unknown Artist"), safe_name(&s.album, "Singles"), file]
}

/// yt-dlp --parse-metadata arguments that set a tag to a fixed value. The value is a
/// template, so % is doubled and the first unescaped ':' ends it.
fn set_tag(value: &str, field: &str) -> Option<String> {
    let value = value.replace('\\', "").replace('%', "%%").replace(':', "\\:");
    (!value.trim().is_empty()).then(|| format!("{value}:%({field})s"))
}

pub struct Downloaded {
    pub path: PathBuf,
    /// What YouTube sent, before any conversion, e.g. "130k Opus" or "256k AAC".
    pub source: String,
}

/// Downloads one song to <music_dir>/Artist/Album/Title.<ext>, reporting progress 0..1.
/// `overwrite` replaces a file that's already there ("Download again"); yt-dlp keeps it otherwise.
/// `started` gets yt-dlp's process id, which is also its process group, for `stop`.
pub fn download(
    settings: &Settings,
    s: &Song,
    overwrite: bool,
    started: impl FnOnce(u32),
    progress: impl Fn(f32),
) -> Result<Downloaded, String> {
    if !safe_id(&s.id, 11) {
        return Err(format!("bad video id {:?}", s.id));
    }
    let [artist, album, title] = parts(s).map(|p| p.replace('%', "%%")); // yt-dlp template escaping
    let mut cmd = Command::new("yt-dlp");
    cmd.args(["-x", "--no-playlist", "--embed-metadata", "--convert-thumbnails", "jpg"]);
    // YouTube's thumbnails are video frames; the album art is the centred square.
    cmd.args(["--ppa", r#"ThumbnailsConvertor+ffmpeg_o:-c:v mjpeg -vf crop="'if(gt(ih,iw),iw,ih)':'if(gt(iw,ih),ih,iw)'""#]);
    // Tags from YouTube Music's song data; a music video's own title reads like "Artist - Song (Official Video)".
    let track = s.track.map(|n| n.to_string()).unwrap_or_default();
    for (value, field) in [(&s.title, "title"), (&s.artists, "artist"), (&s.album, "album"), (&track, "track_number")] {
        if let Some(arg) = set_tag(value, field) {
            cmd.arg("--parse-metadata").arg(arg);
        }
    }
    match settings.format {
        Format::Mp3 => cmd.args(["--audio-format", "mp3", "--audio-quality", "0", "--embed-thumbnail"]),
        // Embedding art in Opus needs Python's mutagen, which many installs lack; a cover.jpg beside the songs works in every player.
        Format::Original => cmd.args(["--write-thumbnail", "-o"]).arg(format!("thumbnail:{artist}/{album}/cover.%(ext)s")),
    };
    if overwrite {
        cmd.arg("--force-overwrites");
    }
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
        .process_group(0) // its own group, so stopping it also stops the ffmpeg it runs
        .spawn()
        .map_err(|e| format!("could not start yt-dlp: {e}"))?;
    started(child.id());
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
        (false, _) => Err(tail(&stderr)),
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

pub type Version = (u64, u64, u64);

pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

pub fn parse_version(s: &str) -> Option<Version> {
    let mut it = s.strip_prefix('v').unwrap_or(s).split('.');
    let v = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    it.next().is_none().then_some(v)
}

pub fn current_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).expect("Cargo.toml version is X.Y.Z")
}

pub fn version_text(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// The newest vX.Y.Z tag in `git ls-remote --tags` output.
fn newest_tag(ls_remote: &str) -> Option<Version> {
    ls_remote.lines().filter_map(|l| l.split_whitespace().nth(1)?.strip_prefix("refs/tags/")).filter_map(parse_version).max()
}

/// The newest tagged release on GitHub.
pub fn latest_release() -> Result<Option<Version>, String> {
    let out = run(Command::new("git").env("GIT_TERMINAL_PROMPT", "0").args(["ls-remote", "--tags", "--refs", REPOSITORY]))?;
    Ok(newest_tag(&out))
}

/// Where Mixtape was installed from, recorded by bin/install.sh.
pub enum Source {
    /// Not recorded, or the folder has moved.
    Unknown,
    /// A development copy (has a .mixtape-dev file); updated by hand with git.
    Development,
    Clone(PathBuf),
}

pub fn source() -> Source {
    let Ok(text) = std::fs::read_to_string(data_dir().join("source-dir")) else { return Source::Unknown };
    let dir = PathBuf::from(text.trim());
    if !dir.is_absolute() || !dir.join("bin/update.sh").is_file() {
        Source::Unknown
    } else if dir.join(".mixtape-dev").exists() {
        Source::Development
    } else {
        Source::Clone(dir)
    }
}

/// Runs bin/update.sh in the clone: checks out the newest release and reinstalls.
pub fn self_update(dir: &Path) -> Result<(), String> {
    run(&mut Command::new(dir.join("bin/update.sh"))).map(|_| ())
}

/// Starts the installed Mixtape (the new version after an update).
pub fn relaunch() -> Result<(), String> {
    open_with(&home().join(".local/bin/mixtape").to_string_lossy(), &[])
}

/// What the Library shows next to a song, plus the tags used to match it.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct AudioInfo {
    pub codec: String,
    pub kbps: Option<u32>,
    pub seconds: Option<f64>,
    pub bytes: u64,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub title: String,
}

impl AudioInfo {
    /// "MP3 · 236 kbps · 3:34 · 7.6 MB"
    pub fn describe(&self) -> String {
        let mut parts = vec![self.codec.clone()];
        if let Some(k) = self.kbps {
            parts.push(format!("{k} kbps"));
        }
        if let Some(s) = self.seconds {
            let s = s.round() as u64;
            parts.push(format!("{}:{:02}", s / 60, s % 60));
        }
        parts.push(format!("{:.1} MB", self.bytes as f64 / 1_000_000.0));
        parts.join(" · ")
    }
}

/// Reads format, bitrate and length with ffprobe (about 50 ms a file), and the size from the file system.
pub fn probe(path: &Path) -> AudioInfo {
    let bytes = path.metadata().map_or(0, |m| m.len());
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_uppercase();
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-of", "json"])
        .args(["-show_entries", "format=duration,bit_rate:stream=codec_name,bit_rate:format_tags=artist,title:stream_tags=artist,title"])
        .arg(path)
        .output();
    let json = out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    parse_probe(json.as_deref().unwrap_or(""), bytes, &ext)
}

fn parse_probe(json: &str, bytes: u64, ext: &str) -> AudioInfo {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let stream = &v["streams"][0];
    let number = |x: &serde_json::Value| x.as_str().and_then(|s| s.parse::<f64>().ok());
    let codec = match stream["codec_name"].as_str() {
        Some("mp3") => "MP3".to_string(),
        Some("opus") => "Opus".to_string(),
        Some("aac") => "AAC".to_string(),
        Some("vorbis") => "Vorbis".to_string(),
        Some(c) if c.starts_with("pcm") => "WAV".to_string(),
        Some(c) => c.to_uppercase(),
        None => ext.to_string(),
    };
    // The stream's own bitrate leaves out embedded cover art; Opus only has the file's.
    let bps = number(&stream["bit_rate"]).or_else(|| number(&v["format"]["bit_rate"]));
    // MP3 keeps tags on the file, Opus on the stream, and Vorbis-style tags may be upper case.
    let tag = |name: &str| {
        [&v["format"]["tags"], &stream["tags"]]
            .into_iter()
            .filter_map(|t| t.as_object())
            .flat_map(|t| t.iter())
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    AudioInfo {
        codec,
        kbps: bps.map(|b| (b / 1000.0).round() as u32),
        seconds: number(&v["format"]["duration"]),
        bytes,
        artist: tag("artist"),
        title: tag("title"),
    }
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    path: String,
    modified: u64,
    size: u64,
    info: AudioInfo,
}

fn cache_file() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("mixtape/songs.json")
}

fn stamp(changed: std::time::SystemTime) -> u64 {
    changed.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Details and tags for every song, from a cache in ~/.cache/mixtape where the file hasn't
/// changed and ffprobe (four at a time) where it has. The first run on a big library takes a while.
pub fn index(all: &[(PathBuf, std::time::SystemTime)]) -> HashMap<PathBuf, AudioInfo> {
    let cached: Vec<CacheEntry> =
        std::fs::read_to_string(cache_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut cached: HashMap<String, CacheEntry> = cached.into_iter().map(|e| (e.path.clone(), e)).collect();
    let mut found = HashMap::new();
    let mut todo = Vec::new();
    for (path, changed) in all {
        let size = path.metadata().map_or(0, |m| m.len());
        match path.to_str().and_then(|p| cached.remove(p)) {
            Some(e) if e.modified == stamp(*changed) && e.size == size => {
                found.insert(path.clone(), e.info);
            }
            _ => todo.push(path.clone()),
        }
    }
    let chunk = todo.len().div_ceil(4).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> =
            todo.chunks(chunk).map(|part| scope.spawn(move || part.iter().map(|p| (p.clone(), probe(p))).collect::<Vec<_>>())).collect();
        for w in workers {
            found.extend(w.join().unwrap_or_default());
        }
    });
    let entries: Vec<CacheEntry> = all
        .iter()
        .filter_map(|(path, changed)| {
            let info = found.get(path)?.clone();
            Some(CacheEntry { path: path.to_str()?.to_string(), modified: stamp(*changed), size: info.bytes, info })
        })
        .collect();
    let file = cache_file();
    if std::fs::create_dir_all(file.parent().unwrap()).is_ok() {
        let _ = std::fs::write(&file, serde_json::to_string(&entries).unwrap_or_default());
    }
    found
}

/// The artist|title keys a song's tags give, for the library check (also the first of several artists).
pub fn tag_keys(info: &AudioInfo) -> Vec<String> {
    if info.artist.is_empty() || info.title.is_empty() {
        return vec![];
    }
    let mut keys = vec![key(&info.artist, &info.title)];
    if let Some((first, _)) = info.artist.split_once(", ") {
        keys.push(key(first, &info.title));
    }
    keys
}

/// Groups of files that look like the same song: they share an artist|title key (from tags, or
/// from the top folder and file name) and their lengths are within 3 seconds, so a live version
/// or a remix of a different length doesn't count.
pub fn duplicates(root: &Path, index: &HashMap<PathBuf, AudioInfo>) -> Vec<Vec<PathBuf>> {
    let files: Vec<(&PathBuf, &AudioInfo)> = index.iter().filter(|(_, i)| i.seconds.is_some()).collect();
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (n, (path, info)) in files.iter().enumerate() {
        let mut keys = tag_keys(info);
        let below = path.strip_prefix(root).unwrap_or(path);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let title = stem.trim_start_matches(|c: char| c.is_ascii_digit() || " -.".contains(c));
        if below.components().count() > 1 {
            let top = below.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
            keys.push(key(&top, title));
        } else if let Some((artist, t)) = stem.split_once(" - ") {
            keys.push(key(artist, t));
        }
        keys.sort();
        keys.dedup();
        for k in keys {
            buckets.entry(k).or_default().push(n);
        }
    }
    let mut parent: Vec<usize> = (0..files.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for members in buckets.values() {
        for (i, &a) in members.iter().enumerate() {
            for &b in &members[i + 1..] {
                let (sa, sb) = (files[a].1.seconds.unwrap_or(0.0), files[b].1.seconds.unwrap_or(0.0));
                if (sa - sb).abs() <= 3.0 {
                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                    parent[ra] = rb;
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<PathBuf>> = HashMap::new();
    for (n, (path, _)) in files.iter().enumerate() {
        let r = find(&mut parent, n);
        groups.entry(r).or_default().push((*path).clone());
    }
    let mut groups: Vec<Vec<PathBuf>> = groups.into_values().filter(|g| g.len() > 1).collect();
    groups.iter_mut().for_each(|g| g.sort());
    groups.sort();
    groups
}

fn queue_file() -> PathBuf {
    data_dir().join("queue.json")
}

/// Remembers the songs still waiting or downloading, so they resume after a restart.
pub fn save_queue(songs: &[Song]) -> Result<(), String> {
    let file = queue_file();
    if songs.is_empty() {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&file, serde_json::to_string(songs).unwrap_or_default()).map_err(|e| e.to_string())
}

pub fn load_queue() -> Vec<Song> {
    std::fs::read_to_string(queue_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Days since 1970-01-01 for a calendar date (Howard Hinnant's days_from_civil).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// yt-dlp versions are release dates: "2026.08.19" (sometimes with a ".1" after).
fn version_days(version: &str) -> Option<i64> {
    let mut it = version.trim().split('.').map(|p| p.parse::<i64>().ok());
    Some(days_from_civil(it.next()??, it.next()??, it.next()??))
}

/// The installed yt-dlp and the newest release, when a newer one has been out for a while and the
/// installed one is over a month old. Distributions take a few days to package a release, so a
/// brand-new one isn't reported.
pub fn ytdlp_outdated() -> Option<(String, String, String)> {
    let out = Command::new("yt-dlp").arg("--version").output().ok()?;
    let installed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let tags = run(Command::new("git").env("GIT_TERMINAL_PROMPT", "0").args([
        "ls-remote",
        "--tags",
        "--refs",
        "https://github.com/yt-dlp/yt-dlp",
    ]))
    .ok()?;
    let today = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64 / 86_400;
    let (installed, latest) = newer_ytdlp(&installed, &tags, today)?;
    let command = ytdlp_update_command(&installed, &latest);
    Some((installed, latest, command))
}

fn newer_ytdlp(installed: &str, ls_remote: &str, today: i64) -> Option<(String, String)> {
    let have = version_days(installed)?;
    let (latest_days, latest) = ls_remote
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1)?.strip_prefix("refs/tags/"))
        .filter_map(|t| Some((version_days(t)?, t.to_string())))
        .max()?;
    (latest_days > have && today - have > 30 && today - latest_days > 7).then(|| (installed.to_string(), latest))
}

/// The command that gets a newer yt-dlp here. A copy installed with uv upgrades with uv. A
/// system package updates with the system only when the distribution already has the newer
/// release; distributions often lag weeks behind, so otherwise it's `uv tool install yt-dlp`
/// (uv is already required), which puts the newest yt-dlp in ~/.local/bin, ahead of the system one.
fn ytdlp_update_command(installed: &str, latest: &str) -> String {
    let path = std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).map(|d| d.join("yt-dlp")).find(|f| f.is_file()))
        .unwrap_or_default();
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let packaged = match distro(&os_release) {
        // "Version : 2026.10.26-1" from the local package database; no root or network needed.
        Distro::Arch => Command::new("pacman").args(["-Si", "yt-dlp"]).output().ok().and_then(|o| {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            text.lines()
                .find_map(|l| l.strip_prefix("Version"))
                .and_then(|v| v.trim_start_matches([' ', ':']).split('-').next().map(String::from))
        }),
        _ => None,
    };
    update_command_for(&path, &home(), &os_release, installed, latest, packaged.as_deref())
}

fn update_command_for(path: &Path, home: &Path, os_release: &str, installed: &str, latest: &str, packaged: Option<&str>) -> String {
    if path.starts_with(home) {
        return "uv tool upgrade yt-dlp".into();
    }
    // The system update helps only if the distribution has (nearly) the newest release.
    let packaged_days = packaged.and_then(version_days);
    let helps = packaged_days > version_days(installed) && packaged_days >= version_days(latest).map(|d| d - 14);
    if helps {
        let omarchy = os_release.lines().any(|l| l.trim() == "ID=omarchy");
        return if omarchy { "omarchy update".into() } else { "sudo pacman -Syu".into() };
    }
    "uv tool install yt-dlp".into()
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
    /// Every song with when it was last changed, newest first.
    pub all: Vec<(PathBuf, std::time::SystemTime)>,
}

impl Scan {
    /// Songs whose path below the music folder contains every word of `query`, ignoring case.
    pub fn matching(&self, query: &str) -> Vec<&PathBuf> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.all
            .iter()
            .map(|(p, _)| p)
            .filter(|p| {
                let text = p.strip_prefix(&self.root.path).unwrap_or(p).to_string_lossy().to_lowercase();
                words.iter().all(|w| text.contains(w))
            })
            .collect()
    }
}

/// Walks the music folder. shortcut: songs are matched by path (Artist/Album/Title, Artist/Title or "Artist - Title"), not by reading tags; read ID3 tags if files from other tools are named differently.
pub fn scan(dir: &Path) -> Scan {
    let (mut keys, mut all) = (HashSet::new(), Vec::new());
    let root = walk(dir, dir.display().to_string(), &mut keys, &mut all);
    all.sort_by_key(|f| std::cmp::Reverse(f.1));
    Scan { keys, root, all }
}

/// Demucs (and Stems) name the parts of a split song like this.
const STEM_NAMES: [&str; 6] = ["vocals", "drums", "bass", "other", "guitar", "piano"];

fn is_stems_folder(files: &[PathBuf]) -> bool {
    files.len() >= 2 && files.iter().all(|f| f.file_stem().is_some_and(|s| STEM_NAMES.iter().any(|n| s.eq_ignore_ascii_case(n))))
}

fn walk(dir: &Path, name: String, keys: &mut HashSet<String>, songs: &mut Vec<(PathBuf, std::time::SystemTime)>) -> Folder {
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
            folder.files.push(path.clone());
        }
    }
    // A folder of stems (vocals.wav, drums.wav, …) is one song split apart, not songs: it stays in
    // the tree but isn't counted, matched or checked for duplicates.
    if !is_stems_folder(&folder.files) {
        for path in &folder.files {
            let changed = path.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            songs.push((path.clone(), changed));
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
        Song {
            id: "dQw4w9WgXcQ".into(),
            title: title.into(),
            artist: artist.into(),
            artists: artist.into(),
            album: album.into(),
            duration: String::new(),
            track: None,
            thumb: String::new(),
        }
    }

    #[test]
    fn ids() {
        assert!(safe_id("dQw4w9WgXcQ", 11));
        assert!(safe_id("-abc_DEF123", 11));
        assert!(!safe_id("--exec=evil", 11));
        assert!(!safe_id("", 11));
    }

    #[test]
    fn album_tracks_keep_their_order() {
        let mut s = song("Queen", "Greatest Hits", "Bohemian Rhapsody");
        s.track = Some(1);
        assert_eq!(parts(&s)[2], "01 Bohemian Rhapsody");
        s.track = Some(12);
        assert_eq!(parts(&s)[2], "12 Bohemian Rhapsody");
    }

    #[test]
    fn tags_are_escaped() {
        assert_eq!(set_tag("Part 2: 100%", "title").unwrap(), "Part 2\\: 100%%:%(title)s");
        assert_eq!(set_tag("AC\\DC", "artist").unwrap(), "ACDC:%(artist)s");
        assert_eq!(set_tag("", "album"), None);
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
    fn versions() {
        assert_eq!(parse_version("v0.10.2"), Some((0, 10, 2)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("v1.2.3-rc1"), None);
        let ls = "abc\trefs/tags/v0.2.0\ndef\trefs/tags/v0.10.0\n123\trefs/tags/v0.9.9\n456\trefs/tags/nightly\n";
        assert_eq!(newest_tag(ls), Some((0, 10, 0)));
        assert_eq!(newest_tag(""), None);
        assert!(parse_version(env!("CARGO_PKG_VERSION")).is_some());
    }

    #[test]
    fn song_details() {
        let mp3 = r#"{"streams":[{"codec_name":"mp3","bit_rate":"235608"}],"format":{"duration":"214.4","bit_rate":"268618"}}"#;
        assert_eq!(parse_probe(mp3, 7_600_000, "MP3").describe(), "MP3 · 236 kbps · 3:34 · 7.6 MB");
        let opus = r#"{"streams":[{"codec_name":"opus"}],"format":{"duration":"61","bit_rate":"127304"}}"#;
        assert_eq!(parse_probe(opus, 3_400_000, "OPUS").describe(), "Opus · 127 kbps · 1:01 · 3.4 MB");
        assert_eq!(parse_probe("", 1_000, "FLAC").describe(), "FLAC · 0.0 MB");
    }

    #[test]
    fn install_hints() {
        assert_eq!(distro("ID=omarchy\nID_LIKE=arch\n"), Distro::Arch);
        assert_eq!(distro("ID=ubuntu\nID_LIKE=\"ubuntu debian\"\n"), Distro::Debian);
        assert_eq!(distro("ID=fedora\n"), Distro::Fedora);
        assert_eq!(distro("ID=gentoo\n"), Distro::Other);
        let [uv, ytdlp, ffmpeg, ffprobe, ..] = &TOOLS;
        let all = [uv, ytdlp, ffmpeg, ffprobe];
        assert_eq!(install_hint_for(Distro::Arch, &all), (Some("sudo pacman -S --needed uv yt-dlp ffmpeg".into()), vec![]));
        let (cmd, notes) = install_hint_for(Distro::Debian, &all);
        assert_eq!(cmd.as_deref(), Some("sudo apt install yt-dlp ffmpeg"));
        assert!(notes[0].contains("docs.astral.sh/uv"));
        assert_eq!(install_hint_for(Distro::Other, &[ytdlp]).0, None);
    }

    #[test]
    fn friendly_errors() {
        assert!(friendly("ERROR: [youtube] abc: Sign in to confirm you’re not a bot.").contains("not a bot"));
        assert!(friendly("WARNING: x\nERROR: [youtube] abc: Video unavailable").starts_with("This song isn't available on YouTube"));
        assert!(friendly("requests.exceptions.ConnectionError: Max retries").contains("internet"));
        assert!(friendly("ERROR: unable to download video data: HTTP Error 403: Forbidden").contains("Updating yt-dlp"));
        assert!(friendly("ERROR: [youtube] aaaaaaaaaaa: This video is unavailable").starts_with("This song isn't available"));
        assert_eq!(friendly("ERROR: [youtube] abc123: Something new"), "Something new");
        assert_eq!(friendly("That isn't a YouTube or YouTube Music link."), "That isn't a YouTube or YouTube Music link.");
        assert_eq!(tail(b""), "failed with no message");
    }

    #[test]
    fn reads_tags_from_file_or_stream() {
        let mp3 = r#"{"streams":[{"codec_name":"mp3"}],"format":{"duration":"1","tags":{"artist":"Queen","title":"Bicycle Race"}}}"#;
        let i = parse_probe(mp3, 1, "MP3");
        assert_eq!((i.artist.as_str(), i.title.as_str()), ("Queen", "Bicycle Race"));
        let opus = r#"{"streams":[{"codec_name":"opus","tags":{"ARTIST":"Rick Astley","TITLE":"Never"}}],"format":{"duration":"1"}}"#;
        assert_eq!(parse_probe(opus, 1, "OPUS").artist, "Rick Astley");
        assert_eq!(
            tag_keys(&AudioInfo { artist: "A, B".into(), title: "T".into(), ..Default::default() }),
            vec![key("A, B", "T"), key("A", "T")]
        );
    }

    #[test]
    fn finds_duplicates_by_name_and_length() {
        let info = |artist: &str, title: &str, seconds: f64| AudioInfo {
            seconds: Some(seconds),
            artist: artist.into(),
            title: title.into(),
            ..Default::default()
        };
        let mut index = HashMap::new();
        index.insert(
            PathBuf::from("/m/Queen/Greatest Hits/Bicycle Race.mp3"),
            info("Queen Official", "Queen - Bicycle Race (Official Video)", 181.0),
        );
        index.insert(PathBuf::from("/m/Queen/Jazz/05 Bicycle Race.opus"), info("Queen", "Bicycle Race", 182.5));
        index.insert(PathBuf::from("/m/Queen/Live/Bicycle Race.mp3"), info("Queen", "Bicycle Race", 240.0));
        index.insert(PathBuf::from("/m/Stems/Song A/vocals.wav"), info("", "", 200.0));
        index.insert(PathBuf::from("/m/Stems/Song B/vocals.wav"), info("", "", 150.0));
        let groups = duplicates(Path::new("/m"), &index);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0],
            vec![PathBuf::from("/m/Queen/Greatest Hits/Bicycle Race.mp3"), PathBuf::from("/m/Queen/Jazz/05 Bicycle Race.opus")]
        );
    }

    #[test]
    fn ytdlp_update_commands() {
        let (home, sys) = (Path::new("/home/me"), Path::new("/usr/bin/yt-dlp"));
        let omarchy = "ID=omarchy\nID_LIKE=arch\n";
        let uv_copy = Path::new("/home/me/.local/bin/yt-dlp");
        // Installed with uv: upgrade with uv.
        assert_eq!(update_command_for(uv_copy, home, omarchy, "2026.08.19", "2026.10.26", None), "uv tool upgrade yt-dlp");
        // Arch hasn't packaged the new release, so a system update wouldn't help.
        assert_eq!(update_command_for(sys, home, omarchy, "2026.08.19", "2026.10.26", Some("2026.08.19")), "uv tool install yt-dlp");
        // It has: update the system, Omarchy's way on Omarchy.
        assert_eq!(update_command_for(sys, home, omarchy, "2026.08.19", "2026.10.26", Some("2026.10.26")), "omarchy update");
        assert_eq!(update_command_for(sys, home, "ID=arch\n", "2026.08.19", "2026.10.26", Some("2026.10.21")), "sudo pacman -Syu");
        assert_eq!(update_command_for(sys, home, "ID=fedora\n", "2026.08.19", "2026.10.26", None), "uv tool install yt-dlp");
    }

    #[test]
    fn ytdlp_age() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(version_days("2026.08.19\n").unwrap() - version_days("2026.06.20").unwrap(), 60);
        assert_eq!(version_days("2026.08.19.1"), version_days("2026.08.19"));
        assert_eq!(version_days("nightly"), None);
        let tags = "a\trefs/tags/2026.08.19\nb\trefs/tags/2026.10.26\nc\trefs/tags/nightly\n";
        let day = |v| version_days(v).unwrap();
        assert_eq!(newer_ytdlp("2026.08.19", tags, day("2026.11.10")), Some(("2026.08.19".into(), "2026.10.26".into())));
        assert_eq!(newer_ytdlp("2026.10.26", tags, day("2026.11.10")), None); // up to date
        assert_eq!(newer_ytdlp("2026.08.19", tags, day("2026.09.01")), None); // only 13 days old
        assert_eq!(newer_ytdlp("2026.08.19", tags, day("2026.10.30")), None); // newest only 4 days out
    }

    #[test]
    fn stems_folders_are_not_songs() {
        let dir = std::env::temp_dir().join(format!("mixtape-stems-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Stems/Heaven Is (6 stems)")).unwrap();
        std::fs::create_dir_all(dir.join("Band/Album")).unwrap();
        for n in ["vocals", "drums", "bass", "other", "guitar", "piano"] {
            std::fs::write(dir.join(format!("Stems/Heaven Is (6 stems)/{n}.wav")), "").unwrap();
        }
        std::fs::write(dir.join("Band/Album/Bass.mp3"), "").unwrap(); // a song called "Bass" alone isn't a stems folder
        let s = scan(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(s.all.len(), 1);
        assert!(!s.keys.contains(&key("stems", "vocals")));
        assert_eq!(s.root.children[1].children[0].files.len(), 6); // still shown in the tree
        assert!(is_blocked("ERROR: [youtube] x: Sign in to confirm you're not a bot"));
        assert!(!is_blocked("ERROR: Video unavailable"));
    }

    #[test]
    fn stop_ends_the_whole_process_group() {
        // A shell that starts a child, like yt-dlp starting ffmpeg.
        let mut child = Command::new("sh").args(["-c", "sleep 30 & wait"]).process_group(0).spawn().unwrap();
        stop(child.id());
        let started = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(started.elapsed() < std::time::Duration::from_secs(5), "still running");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Nothing from that group is left (pgrep -g exits 1 when it finds none).
        let left = Command::new("pgrep").arg("-g").arg(child.id().to_string()).output().unwrap();
        assert!(!left.status.success(), "left behind: {}", String::from_utf8_lossy(&left.stdout));
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
        assert_eq!(s.all.len(), 2);
        assert_eq!(s.matching("rick NEVER").len(), 1);
        assert_eq!(s.matching("thunder").len(), 1);
        assert_eq!(s.matching("rick thunder").len(), 0);
        assert_eq!(s.matching("").len(), 2);
        assert!(s.keys.contains(&key("rick astley", "Never Gonna Give You Up")));
        assert!(s.keys.contains(&key("AC/DC", "thunderstruck")));
        assert_eq!(s.root.children[0].name, "Rick Astley");
        assert_eq!(s.root.children[0].children.len(), 1);
        assert_eq!(s.root.children[0].children[0].files.len(), 1);
        assert_eq!(s.root.files.len(), 1);
    }
}
