# Changelog

Mixtape uses [semantic versioning](https://semver.org). The version is in
`Cargo.toml`, shows in the window's footer, and each release is tagged
`vX.Y.Z`.

## 0.5.0

- Settings: the list of programs moved to a **Dependencies** section at the
  bottom, with each one's installed version (the path on hover). The page
  scrolls when the window is short.
- Album results show how many songs they have; their track lists load in
  the background (three at a time), so opening one is instant.
- The Downloads panel is one line when nothing is downloading, with **Show**;
  it opens by itself when downloads start.
- Inside an open album, tracks show their number, title and length; the
  artist and album are already on the album row.
- Album folders in the Library show their cover (cover.jpg, or the art
  inside the first song, cached in ~/.cache/mixtape/covers).
- Cleaner files: no more YouTube description, link, upload date or "Music"
  genre in the tags; the date is the album's year instead.
- Video endings come off titles: (Official Audio), (Official Video),
  (Official Music Video), (Lyric Video), (Lyrics), (Visualizer), (Audio).
  Real parts of a title, like (In the Air), stay.
- Songs picked from search get their album's track number and year too
  (Mixtape looks the album up when it downloads them).
- A result with no album says "no album (goes to Singles)".
- Downloading an album gets every track that isn't already in that album's
  folder. A song you have on another album says "also on <album>" instead of
  being skipped.
- **Fix tags** in the Library re-downloads the ticked songs with clean tags,
  track numbers, year and a square cover, finding each one on YouTube Music
  by its artist folder, album folder and title, then moves the old file to
  the Trash.
- **Download selected** waits ("Loading tracks…") until every ticked album's
  track list has loaded, so an album still loading is no longer dropped.
- Closing Mixtape stops its downloads (yt-dlp and the ffmpeg it runs); they
  resume from the partial file next time instead of a second copy writing
  the same file. Running downloads have **Cancel** too.
- A bot check or rate limit from YouTube pauses the queue, with **Resume**,
  instead of failing every song after it.
- **Download again** replaces the file on disk (it used to keep the old one).
- Stems folders (vocals, drums, bass, other, guitar, piano) stay in the
  Library tree but no longer count as songs, match "In library", or show up
  as duplicates.
- The search helper asks uv for ytmusicapi 1.x (`>=1.12,<2`), so a future 2.0
  can't break search before Mixtape is checked against it.
- Keyboard shortcuts: Ctrl+1 / Ctrl+2 / Ctrl+, for the tabs, Ctrl+F for the
  search box or Library filter, Space to preview the result under the
  pointer, Ctrl+D to download, Delete to move to the Trash, F5 to rescan, Esc
  to stop the preview and clear the filter. **?**, F1 or **⌨ Shortcuts** in
  the footer lists them.
- Search results show album covers.
- Long titles end in "…" (the full title shows on hover) instead of running
  into the details on the right; details line up at the right edge.
- The Library toolbars wrap onto a second line in a narrow window.
- The download queue survives a restart: waiting and unfinished songs resume
  the next time Mixtape opens. Waiting songs have **Cancel**, and
  **Cancel waiting** drops them all.
- The footer warns when a newer yt-dlp has been out for over a week and yours
  is over a month old (YouTube changes often and old versions stop working),
  with **Copy update command**. The command fits how yt-dlp is installed:
  `omarchy update` or `sudo pacman -Syu` when your distribution already has
  the new release, otherwise `uv tool install yt-dlp` (no sudo), and
  `uv tool upgrade yt-dlp` for a copy installed that way.
- After each scan Mixtape reads every song's tags in the background (cached
  in `~/.cache/mixtape/songs.json`, so later scans are instant). The "In
  library" check also matches by tags, and the Library shows song details
  without waiting.
- **Duplicates** view in the Library: songs that share an artist and title
  (from tags or from the folder and file name) and are within 3 seconds of
  each other's length, grouped so you can tick and trash the extras.

## 0.4.0

- GitHub Actions checks formatting, lints, tests, the search helper and the
  shell scripts on every push and pull request. The README shows the status.
- The music folder's own row in the Library has no tick box, so it can't be
  selected for **Move to Trash**.
- Settings apply and save as you change them; there's no Save button. The
  music folder applies when you leave its box or pick one with **Browse**,
  and only if it exists. The text size applies when you let go of the
  slider.
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
