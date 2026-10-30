"""Candidate ranking — sort order only, NEVER a filter (§9.2, §3.4)."""

from __future__ import annotations

import math
import re

from rapidfuzz import fuzz

from .models import Candidate

_MV_RE = re.compile(r"music video|official video|official mv", re.I)
_AUDIO_RE = re.compile(r"official audio|\(\s*audio\s*\)|\baudio\s+(only|version)\b", re.I)
_VIDEO_INTENT_RE = re.compile(r"video|\bmv\b|visuali[sz]er|\blive\b|concert|performance", re.I)
_VERSION_WORDS = {"remix", "cover", "instrumental", "karaoke", "live", "demo", "slowed", "sped", "nightcore", "8bit", "8-bit", "mashup"}
_CORP_SUFFIX_RE = re.compile(
    r"\s+(media|vevo|records|music|official|tv|hd|hq|productions|entertainment)$",
    re.I,
)


def _alnum(text: str) -> str:
    return re.sub(r"[^a-z0-9]", "", text.casefold())


def _core_channel(channel: str) -> str:
    """Uploader name without "- Topic" or a corporate suffix."""
    c = (channel or "").removesuffix(" - Topic").strip()
    return _CORP_SUFFIX_RE.sub("", c).strip()


# Label / aggregator / official-channel words: uploads from these are
# authoritative about the recording even when they aren't the artist's
# own channel.
_LABEL_RE = re.compile(
    r"media|music|tv|lyrics|audio|video|tunes|records|official|vevo|"
    r"nation|empire|songs|beats|sounds|sound|label|entertainment|distribution",
    re.I,
)


def _authoritative_upload(channel: str, top_artist: str) -> bool:
    """True when this uploader may legitimately carry "Official Audio".

    The artist's own channel (or a Topic/ATV channel), a label, or an
    aggregator; a plain re-uploader wearing the artist's name in the
    title does not qualify."""
    if not top_artist:
        return True
    if _alnum(_core_channel(channel)) == _alnum(top_artist):
        return True
    return bool(_LABEL_RE.search(channel or ""))


def _fuzzy_ratio(title: str, query: str) -> float:
    if not query:
        return 0.0
    return fuzz.token_sort_ratio(title, query) / 100.0


def is_topic(channel: str) -> bool:
    return channel.endswith(" - Topic")


def has_embedded_metadata(c: Candidate) -> bool:
    return "has-metadata" in c.badges


def _normalize(text: str) -> str:
    text = text.casefold().strip()
    text = re.sub(r"\s+", " ", text)
    return text.strip(" -–—:|\"'()[]")


def _title_variants(title: str) -> list[str]:
    """The raw title plus de-prefixed forms ("Drake - Hotline Bling" →
    "Hotline Bling") plus de-suffixed audio forms ("Hotline Bling
    (Official Audio)" → "Hotline Bling"). YouTube decorates titles;
    matching should see through the decorations."""
    base = title.strip()
    variants = [title]
    stripped = re.sub(r"^.+?\s[-–—:|]\s+", "", base, count=1)
    if stripped and stripped != base:
        variants.append(stripped)
    for v in list(variants):
        audio_stripped = re.sub(
            r"\s*[\(\[]?\s*official audio\s*[\)\]]?\s*$", "", v.strip(), flags=re.I
        )
        audio_stripped = re.sub(r"\s*-\s*audio\s*$", "", audio_stripped, flags=re.I)
        if audio_stripped and audio_stripped != v.strip():
            variants.append(audio_stripped)
    return variants


def parse_views(raw: object) -> int | None:
    """Parse YTM's formatted view counts ("24M", "5.9K", "2.5B", "1,234")."""
    if raw is None:
        return None
    if isinstance(raw, (int, float)):
        return int(raw) if raw >= 0 else None
    if not isinstance(raw, str):
        return None
    s = raw.strip().upper().replace(",", "")
    if not s:
        return None
    mult = 1
    if s.endswith("K"):
        mult, s = 1_000, s[:-1]
    elif s.endswith("M"):
        mult, s = 1_000_000, s[:-1]
    elif s.endswith("B"):
        mult, s = 1_000_000_000, s[:-1]
    try:
        return int(float(s) * mult)
    except ValueError:
        return None


def score(
    c: Candidate,
    query: str = "",
    expected_duration: float | None = None,
    top_artist: str = "",
    pinned_album: str = "",
    catalog_match: bool = False,
    catalog_loose_match: bool = False,
    catalog_active: bool = False,
    catalog_other_match: bool = False,
    catalog_loose_other_match: bool = False,
) -> float:
    """§9.2 scoring formula. Pure sort key — never used to drop rows."""
    s = 0.0
    if is_topic(c.channel):
        s += 12  # clean audio + static art
    if c.source == "ytmusic":
        s += 6
    # Match the complete artist + title query as well as either field alone.
    # Token-sort similarity alone ranks a vaguely related title too highly.
    combined = f"{c.channel.removesuffix(' - Topic')} {c.title}"
    if c.album:
        combined += f" {c.album}"
    if query:
        variants = _title_variants(c.title)
        title_match = max(fuzz.WRatio(query, v) / 100 for v in variants)
        s += 20 * max(
            title_match,
            fuzz.WRatio(query, combined) / 100,
        )
        # Whole-word matching: "cole" must not match via "Keyshia Cole"
        # when the query means J. Cole, and single letters match everything.
        terms = [t for t in re.findall(r"\w+", query.casefold()) if len(t) > 1]
        haywords = set(re.findall(r"\w+", combined.casefold()))
        # Explicit version requests must beat an unrelated canonical studio
        # recording. Versions stay neutral when the query doesn't ask for one.
        requested_versions = set(terms) & _VERSION_WORDS
        if requested_versions:
            s += 45 if requested_versions <= haywords else -35
        s += 2 * sum(1 for t in terms if t in haywords)
        substantive = terms
        # Exact multi-word title match wins over longer fuzzy cousins
        # ("Hotline Bling" should beat "Hotline Bling (1950s Motown Choir Gospel)").
        # Single-word queries skip this so live/cover/remix variants stay neutral.
        query_norm = _normalize(query)
        if len(substantive) > 1 and any(_normalize(v) == query_norm for v in variants):
            s += 12
        # Exact intent: every substantive query word covered by artist/title
        # (e.g. "J Cole Change" → "J. Cole - Change"). Without this, same-artist
        # junk outranks the exact match and it gets truncated by limit.
        if substantive and all(t in haywords for t in substantive):
            s += 10
        # Popularity supersedes source tidiness: a 2B-view official upload
        # beats a 10K-view mislabeled re-upload. Gated behind title match so
        # popular-but-irrelevant songs can't hijack album queries. Uncapped:
        # among same-titled uploads the most-watched one is usually the real
        # song, and caps let mid-size covers tie it.
        # Once the catalog has an opinion on the query, identity is settled
        # and popularity only breaks ties among verified recordings —
        # unverified same-titled uploads keep it at half weight so a
        # popular cover can't outrank the canonical recording. Rank-only.
        if c.views and title_match >= 0.6:
            popularity = 5 * math.log10(c.views + 1) * title_match
            if catalog_active and not (catalog_match or catalog_loose_match
                                       or catalog_other_match
                                       or catalog_loose_other_match):
                popularity *= 0.5
            s += popularity
        # Officialdom: the query's top artist on the upload wins over
        # same-titled covers. Matched as a whole ("jcole" is not in
        # "keyshiacole"), never as loose substrings.
        if top_artist and _alnum(top_artist) in _alnum(combined):
            s += 15
        # Library intent: for acquisition the clean audio beats the video.
        # Rank-only — videos stay in the list, just below their audio.
        # The clean-audio bonus only applies to uploads that are
        # authoritative for the artist: a random re-upload titled
        # "Artist - Title (Official Audio)" must not outrank the artist's
        # own upload just by wearing the tag.
        if not _VIDEO_INTENT_RE.search(query):
            if _MV_RE.search(c.title):
                s -= 6
            elif _AUDIO_RE.search(c.title) and _authoritative_upload(c.channel, top_artist):
                s += 14
        # A 25-minute "song" is a playlist video, a 99-minute one a rant or
        # podcast. Rank-only demotion; real mixes stay findable below.
        if c.durationSec > 1200:
            s -= 10
        # Pinned album's tracks cluster right under the album card.
        if pinned_album and c.album and _normalize(c.album) == _normalize(pinned_album):
            s += 25
        # Catalog authority: this upload IS the canonical recording (exact
        # title + artist + duration per the Deezer lookup). Decisive over
        # popularity so mislabeled re-uploads and covers can't win on
        # views — but rank-only, like everything else here. A verified
        # same-titled recording by a different artist (the catalog lists
        # covers too) is real but not the requested one: smaller boost.
        if catalog_match:
            s += 30
        elif catalog_other_match:
            s += 10
        elif catalog_loose_match:
            # Same song on the artist's own channel, video-cut duration.
            # Verified identity, weaker recording equivalence.
            s += 18
        elif catalog_loose_other_match:
            s += 8
    if has_embedded_metadata(c):
        s += 3
    if _MV_RE.search(c.title):
        s -= 1  # soft demotion only
        # Album queries ("Drake Views") never match track titles ("9"),
        # so album-expanded tracks need their own bonus or they drown
        # under unrelated same-artist songs and get truncated by limit.
        if c.album:
            album_norm = c.album.casefold().strip()
            q_norm = query.casefold().strip()
            if album_norm and (album_norm in q_norm or q_norm in album_norm):
                s += 16
            else:
                album_terms = set(re.findall(r"\w+", album_norm))
                query_terms = set(re.findall(r"\w+", q_norm))
                if album_terms and album_terms <= query_terms:
                    s += 16
                elif album_terms and query_terms:
                    overlap = len(album_terms & query_terms) / len(album_terms)
                    if overlap >= 0.5:
                        s += 8
    if has_embedded_metadata(c):
        s += 3
    if _MV_RE.search(c.title):
        s -= 1  # soft demotion only
    # live / cover / remix / instrumental → 0 (NEUTRAL — see §3.4)
    if expected_duration and c.durationSec and abs(c.durationSec - expected_duration) > 90:
        s -= 4
    return s


def compute_badges(c: Candidate) -> list[str]:
    badges: list[str] = []
    if is_topic(c.channel):
        badges.append("topic")
    if c.source == "ytmusic" and is_topic(c.channel):
        badges.append("audio-only")
    if _MV_RE.search(c.title):
        badges.append("likely-mv")
    if "has-metadata" in c.badges:
        badges.append("has-metadata")
    # de-dupe preserving order
    seen: set[str] = set()
    out: list[str] = []
    for b in badges:
        if b not in seen:
            seen.add(b)
            out.append(b)
    return out


def rank(
    candidates: list[Candidate],
    query: str = "",
    expected_duration: float | None = None,
    top_artist: str = "",
    pinned_album: str = "",
    catalog_matches: set[str] | None = None,
    catalog_loose_matches: set[str] | None = None,
    catalog_active: bool = False,
    catalog_other_matches: set[str] | None = None,
    catalog_loose_other_matches: set[str] | None = None,
) -> list[Candidate]:
    """Score and sort. Returns ALL candidates — nothing is removed."""
    matched = catalog_matches or set()
    other = (catalog_other_matches or set()) - matched
    loose = (catalog_loose_matches or set()) - matched - other
    loose_other = (catalog_loose_other_matches or set()) - matched - other - loose
    active = catalog_active or bool(matched or other or loose or loose_other)
    for c in candidates:
        c.badges = compute_badges(c)
        c.score = round(
            score(c, query, expected_duration, top_artist, pinned_album,
                  c.videoId in matched, c.videoId in loose, active,
                  c.videoId in other, c.videoId in loose_other),
            4,
        )
    return sorted(candidates, key=lambda c: c.score, reverse=True)


def is_album_intent(query: str, album_title: str, album_artist: str) -> bool:
    """Does the query explicitly name this album? Same strict gates as
    expansion: full title containment plus artist match. Used to pin the
    album card first (deterministic, not fuzzy)."""
    title = (album_title or "").strip()
    if not title:
        return False
    title_terms = set(re.findall(r"\w+", title.casefold()))
    query_terms = set(re.findall(r"\w+", query.casefold()))
    if not (title.casefold() in query.casefold() or (title_terms and title_terms <= query_terms)):
        return False
    artist = (album_artist or "").strip()
    if artist:
        artist_terms = set(re.findall(r"\w+", artist.casefold()))
        if (
            artist.casefold() not in query.casefold()
            and not (artist_terms and artist_terms <= query_terms)
            # Knockoff albums ("Drakee", "Sadmethod") must never pin on
            # fuzzy resemblance alone — the artist name itself has to
            # appear in the query. Fuzzy WRatio let "Drakee" pin over
            # the real record for "One Dance Drake".
            and _alnum(artist) not in _alnum(query)
            and _alnum(query) not in _alnum(artist)
        ):
            return False
    return True


def album_score(title: str, artist: str, query: str = "") -> float:
    """Score an album on the same scale as track candidates so the UI can
    interleave albums and songs in a single results list."""
    if not query:
        return 0.0
    combined = f"{artist} {title}".strip()
    s = 20 * max(
        fuzz.WRatio(query, title) / 100,
        fuzz.WRatio(query, combined) / 100,
    )
    terms = re.findall(r"\w+", query.casefold())
    haystack = combined.casefold()
    s += 2 * sum(term in haystack for term in terms)
    substantive = [t for t in terms if len(t) > 1]
    if len(substantive) > 1 and _normalize(title) == _normalize(query):
        s += 12
    if substantive and all(t in haystack for t in substantive):
        s += 10
    return round(s, 4)
