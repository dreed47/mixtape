# Mixtape

A small desktop app for Linux that searches YouTube Music, downloads songs as
MP3 into an `Artist/Album/` folder structure, and opens folders of your music
in the player you choose. It follows the [Omarchy](https://omarchy.org) theme
when Omarchy is installed and works on any Linux desktop otherwise.

![Mixtape searching for Queen, with an album open and two songs downloading](docs/screenshot.png)

## Features

- **Search** by artist, song, album, or a mix. Partial and misspelled searches
  work, because YouTube Music does the matching.
- **Albums** show up as their own rows. Open one to see its tracks, or tick it
  to select every track you don't already have.
- **Download several songs at once.** Tick songs, then click
  **Download selected**. Two songs download at a time and the rest wait in a
  queue, each with a progress bar.
- **Already-downloaded check.** Songs you already have are marked
  **In library**. Mixtape knows a song is in your library when it downloaded
  that song before, or when a file in your music folder matches its artist and
  title. Ticking one asks before downloading it again.
- **Library.** Browse your music folder down to each song. Tick one or more
  folders and click **Play selected** to open them in your player.
  Double-click a song to play it, or right-click it to play it or open it in
  the Stems app when it is installed.
- **Settings.** Choose the music folder (local or a network share) and the
  player: Amberol, VLC, mpv, or any command.

Files are saved as `<music folder>/<Artist>/<Album>/<Title>.mp3` at the best
available quality, with artist, album, title, and cover art embedded.

## Requirements

- Linux with a Wayland or X11 desktop
- [Rust](https://www.rust-lang.org/tools/install) (to build)
- [uv](https://docs.astral.sh/uv/) (runs the YouTube Music search through
  [ytmusicapi](https://github.com/sigma67/ytmusicapi); uv fetches it on first
  search)
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [FFmpeg](https://ffmpeg.org)
- A music player, for example Amberol, VLC, or mpv

On Arch Linux and Omarchy:

```bash
sudo pacman -S rust uv yt-dlp ffmpeg
```

## Install

```bash
git clone https://github.com/dreed47/mixtape.git
cd mixtape
bin/install.sh
```

This builds Mixtape, installs the `mixtape` binary to `~/.local/bin`, and adds
Mixtape to your app launcher. Make sure `~/.local/bin` is on your `PATH`.

## Update

From the folder you cloned:

```bash
bin/update.sh
```

This pulls the latest version from GitHub and reinstalls it. Restart Mixtape
afterwards.

## Uninstall

```bash
bin/uninstall.sh
```

This removes the app and its launcher entry. Your music, settings, and
download log stay where they are.

## Files Mixtape uses

| Path | What's in it |
| --- | --- |
| `~/.config/mixtape/settings.json` | Music folder and player |
| `~/.local/share/mixtape/downloads.tsv` | Log of songs Mixtape downloaded |

Mixtape respects `XDG_CONFIG_HOME` and `XDG_DATA_HOME`.

## Development

```bash
cargo run          # run from source
cargo test         # unit tests
```

The code is in `src/`:

- `main.rs`: the window and its three tabs
- `backend.rs`: settings, search, downloads, the download log, and library
  scanning
- `theme.rs`: follows the Omarchy theme
- `ytmusic.py`: the search helper that calls ytmusicapi

## Legal

Download only music you have the right to download. Respect the terms of
service of YouTube Music and the copyright of the artists.

## License

[MIT](LICENSE)
