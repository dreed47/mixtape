# Changelog

Mixtape uses [semantic versioning](https://semver.org). The version is in
`Cargo.toml`, shows in the window's footer, and each release is tagged
`vX.Y.Z`.

## Unreleased

- A cassette app icon, installed by `bin/install.sh`.
- A setup check: when uv, yt-dlp, ffmpeg or ffprobe is missing, a banner
  says what each is for and shows the install command for your distribution
  (Arch, Debian/Ubuntu, Fedora), with **Copy** and **Check again**.
  **Settings** lists every program Mixtape uses and whether it's installed.
- Errors are in plain words: YouTube's bot check, songs that are unavailable,
  private or blocked in your country, an outdated yt-dlp, no internet, a full
  disk, a missing Trash. **Copy details** (or right-click a failed download)
  copies the full error for a bug report.
- Short confirmations: "Settings saved", "Moved 3 items to the Trash", and
  "Downloaded 12 songs" when the queue finishes.
- Library: a filter box (every word must match the artist, album or song
  name) and a **Recently added** view of the newest 200 songs.
- Search: paste a YouTube or YouTube Music link to a song, album, playlist or
  artist. Artists appear in results; **Albums and singles ›** lists all of
  theirs, with **Select all**. A playlist page has **Select all songs**.
- Albums show their type (Album, Single, EP). Track lists load three at a
  time.

## 0.3.0

- Album downloads keep album order: files start with the track number
  (`01 Bohemian Rhapsody.mp3`) and get a track number tag.
- Cover art is cropped to a square instead of a letterboxed video frame.
- Title, artist and album tags come from YouTube Music's song data, so music
  videos no longer get titles like "Artist - Song (Official Video)".
- A ▶ button on each search result plays it through mpv without downloading;
  ⏹ stops it. It shows when mpv is installed.
- Songs opened in Amberol start playing. Amberol only loads them, so Mixtape
  presses Play over MPRIS.
- The footer says when a newer release is on GitHub and offers an **Update**
  button that installs it, then **Restart to finish**. The check runs at
  startup and once a day, and can be turned off in Settings.
- `bin/update.sh` installs the newest tagged release instead of the newest
  code on `main`.
- Text uses the desktop's interface font (Adwaita Sans on Omarchy and GNOME)
  and is about 20% larger. **Settings > Text size** (80% to 200%) and
  Ctrl +, Ctrl −, Ctrl 0 change it, and the size is remembered.
- The Library shows each song's format, bitrate, length, and file size, for
  example `MP3 · 236 kbps · 3:34 · 7.6 MB`.

## 0.2.0

- Library: tick songs as well as folders, **Move to Trash** (with a confirm
  step; the music folder itself can't be trashed), **Open in file manager**,
  and **Show in file manager** on a song's right-click menu.
- Settings: **Format** (MP3, or Original with no conversion) and an optional
  YouTube Music Premium login through your browser for 256 kbps audio.
- Finished downloads show the source quality, for example `256k AAC`.
- Songs open in Stems from the right-click menu when Stems is installed.
- The search box has the cursor when the window opens.
- Scripts moved to `bin/`. The README has a screenshot.
- The version shows in the footer.

## 0.1.0

- First release: YouTube Music search, multi-song downloads into
  `Artist/Album/`, an "In library" check, the Library tree, Settings, and
  the Omarchy theme.
