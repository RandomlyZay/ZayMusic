"""MusicBrainz metadata repair — fills gaps, never overwrites good data.

Uploads from random channels ("J.Cole Change Official Audio" by "Kal")
carry no usable tags. This module recovers (artist, title, album) from
MusicBrainz with strict acceptance: a pair is only accepted when the
recording actually exists under that artist AND the artist is named in
the upload's own text. Anything uncertain is left alone (Unknown).
"""

from __future__ import annotations

import logging
import os
import re
import time

log = logging.getLogger("zayfetch.repair")

_init = False


def _ensure_init() -> None:
    global _init
    if _init:
        return
    import musicbrainzngs

    musicbrainzngs.set_useragent(
        "ZayFetch",
        os.environ.get("ZAYFETCH_VERSION", "0.1.0"),
        os.environ.get("ZAYFETCH_CONTACT", "https://github.com/zaymusic"),
    )
    _init = True

_JUNK_SUFFIX_RE = re.compile(
    r"\s*[\(\[]?\s*(official audio|official lyric video|official lyrics|official video"
    r"|official mv|audio|lyrics|lyric video|music video|\bmv\b|visualizer|visualiser"
    r"|hq|hd|4k)\s*[\)\]]?\s*$",
    re.I,
)
_SPLIT_RE = re.compile(r"\s[-–—:|]\s+")


def clean_title(title: str) -> str:
    """Strip upload decorations ("(Official Audio)", "[Lyrics]", ...).

    Version words that change the performance (live, cover, remix,
    slowed, acoustic, instrumental, karaoke) are meaningful — keep them.
    """
    t = (title or "").strip()
    prev = None
    while prev != t:
        prev = t
        t = _JUNK_SUFFIX_RE.sub("", t).strip()
    return re.sub(r"\s+", " ", t).strip(" -–—:|\"'")


def normalize_channel(channel: str) -> str:
    """Drop auto-generated suffixes ("X - Topic"). VEVO/Media stay — the
    repair below resolves them against MusicBrainz."""
    return (channel or "").removesuffix(" - Topic").strip()


def _alnum(text: str) -> str:
    return re.sub(r"[^a-z0-9]", "", (text or "").casefold())


def _norm_words(text: str) -> str:
    return re.sub(r"\s+", " ", re.sub(r"[._]+", " ", text or "").strip().lower())


def _candidate_pairs(title: str, channel: str) -> list[tuple[str, str]]:
    """(artist, title) guesses, best first. Never invents names — every
    artist guess comes from the upload's own text."""
    t = clean_title(title)
    c = normalize_channel(channel)
    pairs: list[tuple[str, str]] = []
    if c:
        pairs.append((c, t))
        corp = re.sub(
            r"\s+(media|vevo|records|music|official|tv|hd|hq|productions|entertainment)$",
            "",
            c,
            flags=re.I,
        ).strip()
        if corp and corp != c:
            pairs.append((corp, t))
    m = _SPLIT_RE.split(t, maxsplit=1)
    if len(m) == 2 and m[0].strip() and m[1].strip():
        pairs.append((m[0].strip(), m[1].strip()))
    words = _norm_words(t).split()
    for n in (3, 2, 1):
        if len(words) > n:
            pairs.append((" ".join(words[:n]), " ".join(words[n:])))
    # de-dupe preserving order
    seen: set[tuple[str, str]] = set()
    out: list[tuple[str, str]] = []
    for p in pairs:
        key = (_alnum(p[0]), _alnum(p[1]))
        if key[0] and key[1] and key not in seen:
            seen.add(key)
            out.append(p)
    return out


def _recording_hit(title: str, artist: str) -> dict | None:
    """The release + canonical artist for a recording, or None.

    Accepted only when MusicBrainz has this exact title under an
    artist matching the guess — score alone is not trustworthy.
    """
    try:
        from rapidfuzz import fuzz

        import musicbrainzngs

        _ensure_init()
        res = musicbrainzngs.search_recordings(recording=title, artist=artist, limit=5)
        for rec in res.get("recording-list") or []:
            rec_title = rec.get("title") or ""
            credits = rec.get("artist-credit") or []
            credit = credits[0].get("name", "") if credits else ""
            if not credit:
                continue
            # casefolded: WRatio penalizes case/punctuation noise that
            # doesn't change identity ("Change"/"Change", "J. Cole"/"J Cole")
            if fuzz.WRatio(title.casefold(), rec_title.casefold()) < 80:
                continue
            if fuzz.WRatio(artist.casefold(), credit.casefold()) < 80:
                continue
            releases = rec.get("release-list") or []
            return {
                "artist": str(credit),
                "title": str(rec_title),
                "album": str(releases[0].get("title")) if releases else "",
            }
    except Exception as e:
        log.info("musicbrainz lookup failed for %r - %r: %s", artist, title, e)
    return None


def repair_metadata(*, title: str, artist: str, album: str = "") -> dict:
    """Return fixed {title?, artist?, album?} fields (only what improves).

    Never raises; empty dict means "leave everything as-is".
    """
    try:
        return _repair_metadata_inner(title=title, artist=artist, album=album)
    except Exception as e:
        log.info("repair skipped for %r - %r: %s", artist, title, e)
        return {}


def _repair_metadata_inner(*, title: str, artist: str, album: str = "") -> dict:
    upload_text = f"{title} {artist}"
    fixed: dict = {}
    for cand_artist, cand_title in _candidate_pairs(title, artist):
        # The credited artist must be named in the upload itself —
        # otherwise any same-titled song by anyone would match.
        if _alnum(cand_artist) not in _alnum(upload_text):
            continue
        hit = _recording_hit(cand_title, cand_artist)
        if not hit:
            time.sleep(1)  # MusicBrainz rate limit: 1 req/sec
            continue
        if hit["artist"] and hit["artist"] != artist:
            fixed["artist"] = hit["artist"]
        if hit["title"] and _norm_words(hit["title"]) != _norm_words(title):
            # only shorten toward the canonical title, never sideways
            if _alnum(hit["title"]) in _alnum(title):
                fixed["title"] = hit["title"]
        if not album and hit.get("album"):
            fixed["album"] = hit["album"]
        break
    return fixed


def repair_artist(*, title: str, artist: str, limit: int = 1) -> str | None:
    """Legacy entry point (kept for compatibility)."""
    fixed = repair_metadata(title=title, artist=artist)
    return fixed.get("artist")
