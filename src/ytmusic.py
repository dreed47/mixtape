# Mixtape search helper: talks to YouTube Music through ytmusicapi and prints JSON.
# Usage: ytmusic.py search <query> | album <browseId> | artist <channelId> | photo <artist name> | link <url>
import json
import sys
from urllib.parse import parse_qs, urlparse

from ytmusicapi import YTMusic

yt = YTMusic()


def names(item):
    return [a["name"] for a in item.get("artists") or [] if a.get("name")]


def thumb(item, size=120):
    """The smallest cover at least `size` pixels wide (they're square), or the biggest there is."""
    covers = item.get("thumbnails") or []
    fit = [t for t in covers if (t.get("width") or 0) >= size]
    cover = fit[0] if fit else (covers[-1] if covers else None)
    url = (cover or {}).get("url") or ""
    return url if url.startswith("https://") else ""


def song(s, album=None, album_artists=(), cover="", year=""):
    artists = names(s) or list(album_artists)
    album_info = s.get("album") if isinstance(s.get("album"), dict) else {}  # a plain name in album track lists
    return {
        "id": s["videoId"],
        "title": s.get("title") or "",
        "artist": artists[0] if artists else "",
        "artists": ", ".join(artists),
        "album": album if album is not None else album_info.get("name") or "",
        "duration": s.get("duration") or s.get("length") or "",
        "track": s.get("trackNumber") if album is not None else None,
        "thumb": thumb(s) or cover,
        # Lets Mixtape look up the track number and year of a song found by search.
        "album_id": album_info.get("id") or "",
        "year": str(year or s.get("year") or ""),
    }


def album_item(a, artist=""):
    return {
        "id": a["browseId"],
        "title": a.get("title") or "",
        "artists": ", ".join(names(a)) or artist,
        "year": str(a.get("year") or ""),
        "kind": a.get("type") or "",
        "thumb": thumb(a),
    }


def page(songs=(), albums=(), artists=(), title=None):
    return {"songs": list(songs), "albums": list(albums), "artists": list(artists), "title": title}


# Same bounds as SEARCH_LIMITS in backend.rs.
SONG_LIMIT_MIN = 5
SONG_LIMIT_MAX = 50
SONG_LIMIT_DEFAULT = 20


def song_limit(raw):
    """A song-count a search is allowed to ask for."""
    try:
        n = int(raw)
    except (TypeError, ValueError):
        return SONG_LIMIT_DEFAULT
    return max(SONG_LIMIT_MIN, min(n, SONG_LIMIT_MAX))


def search(query, limit=SONG_LIMIT_DEFAULT):
    # ytmusicapi only uses `limit` to fetch further pages. The first page is
    # about 20 items even when you ask for fewer, so cut it here.
    n = song_limit(limit)
    songs = [song(s) for s in yt.search(query, filter="songs", limit=n) if s.get("videoId")][:n]
    albums = [album_item(a) for a in yt.search(query, filter="albums", limit=n) if a.get("browseId")][:n]
    artists = [
        {"id": a["browseId"], "name": a.get("artist") or ""}
        for a in yt.search(query, filter="artists", limit=n)
        if a.get("browseId")
    ][:n]
    return page(songs, albums, artists)


def album_tracks(browse_id):
    a = yt.get_album(browse_id)
    cover, year = thumb(a), a.get("year") or ""
    return [
        {**song(t, a.get("title") or "", names(a), cover, year), "album_id": browse_id}
        for t in a.get("tracks") or []
        if t.get("videoId")
    ]


def album_by_id(browse_id):
    a = yt.get_album(browse_id)
    return {**album_item({**a, "browseId": browse_id}), "kind": a.get("type") or ""}


def artist(channel_id):
    r = yt.get_artist(channel_id)
    name = r.get("name") or ""
    albums = []
    for section in ("albums", "singles"):
        part = r.get(section) or {}
        items = part.get("results") or []
        # The artist page shows a few; the full list is one more request.
        if part.get("browseId") and part.get("params"):
            try:
                items = yt.get_artist_albums(part["browseId"], part["params"], limit=None)
            except Exception:
                pass
        albums += [album_item(a, name) for a in items if a.get("browseId")]
    songs = [song(s) for s in (r.get("songs") or {}).get("results") or [] if s.get("videoId")]
    return page(songs, albums, title=name)


def link(url):
    u = urlparse(url.strip())
    host = (u.hostname or "").lower()
    if not (host == "youtu.be" or host == "youtube.com" or host.endswith(".youtube.com")):
        sys.exit("That isn't a YouTube or YouTube Music link.")
    parts = [p for p in u.path.split("/") if p]
    query = parse_qs(u.query)
    video = query.get("v", [None])[0] or (parts[0] if host == "youtu.be" and parts else None)
    playlist = query.get("list", [None])[0]
    if len(parts) == 2 and parts[0] == "browse" and parts[1].startswith("MPREb_"):
        return page(albums=[album_by_id(parts[1])])
    if len(parts) == 2 and parts[0] == "channel":
        return artist(parts[1])
    if playlist and playlist.startswith("OLAK5uy_"):
        return page(albums=[album_by_id(yt.get_album_browse_id(playlist))])
    if video:
        track = yt.get_watch_playlist(video, limit=1)["tracks"][0]
        return page(songs=[song(track)])
    if playlist:
        p = yt.get_playlist(playlist, limit=None)
        return page(songs=[song(t) for t in p.get("tracks") or [] if t.get("videoId")], title=p.get("title"))
    sys.exit("Mixtape can't open that kind of YouTube link.")


def artist_photo(name):
    """The photo of the artist called exactly `name` (any case), or "" when there's no such artist."""
    for a in yt.search(name, filter="artists", limit=5):
        if (a.get("artist") or "").casefold() == name.casefold():
            return thumb(a)
    return ""


mode, arg = sys.argv[1], sys.argv[2]
if mode == "search":
    print(json.dumps(search(arg, sys.argv[3] if len(sys.argv) > 3 else SONG_LIMIT_DEFAULT)))
elif mode == "album":
    print(json.dumps(album_tracks(arg)))
elif mode == "artist":
    print(json.dumps(artist(arg)))
elif mode == "photo":
    print(json.dumps(artist_photo(arg)))
elif mode == "link":
    print(json.dumps(link(arg)))
else:
    sys.exit(f"unknown mode {mode}")
