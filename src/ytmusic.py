# Mixtape search helper: talks to YouTube Music through ytmusicapi and prints JSON.
# Usage: ytmusic.py search <query> | ytmusic.py album <browseId>
import json
import sys

from ytmusicapi import YTMusic

yt = YTMusic()


def names(item):
    return [a["name"] for a in item.get("artists") or [] if a.get("name")]


def song(s, album=None, album_artists=()):
    artists = names(s) or list(album_artists)
    return {
        "id": s["videoId"],
        "title": s.get("title") or "",
        "artist": artists[0] if artists else "",
        "artists": ", ".join(artists),
        "album": album if album is not None else (s.get("album") or {}).get("name") or "",
        "duration": s.get("duration") or "",
    }


mode, arg = sys.argv[1], sys.argv[2]
if mode == "search":
    songs = [song(s) for s in yt.search(arg, filter="songs", limit=20) if s.get("videoId")]
    albums = [
        {"id": a["browseId"], "title": a.get("title") or "", "artists": ", ".join(names(a)), "year": str(a.get("year") or "")}
        for a in yt.search(arg, filter="albums", limit=6)
        if a.get("browseId")
    ]
    print(json.dumps({"songs": songs, "albums": albums}))
elif mode == "album":
    a = yt.get_album(arg)
    print(json.dumps([song(t, a.get("title") or "", names(a)) for t in a.get("tracks") or [] if t.get("videoId")]))
else:
    sys.exit(f"unknown mode {mode}")
