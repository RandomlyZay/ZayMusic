"""Pydantic schemas for ZayFetch (§8.2 / §8.3)."""

from __future__ import annotations

import re
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, ValidationInfo, field_validator, model_validator


Source = Literal["ytmusic", "youtube"]
JobStatus = Literal["queued", "running", "done", "failed", "cancelled"]
Stage = Literal["download", "tag", "loudness", "move", "scan", "string"]


class Thumbnail(BaseModel):
    url: str
    width: int = 0
    height: int = 0


Badge = Literal["topic", "audio-only", "likely-mv", "has-metadata"]


class Candidate(BaseModel):
    videoId: str
    source: Source
    title: str
    channel: str
    album: str | None = None
    track: int | None = None
    durationSec: float = 0.0
    views: int | None = None
    thumbnails: list[Thumbnail] = Field(default_factory=list)
    badges: list[Badge] = Field(default_factory=list)
    score: float = 0.0
    explicit: bool | None = None
    url: str = ""

    def model_post_init(self, __context: Any) -> None:
        if not self.url:
            self.url = f"https://www.youtube.com/watch?v={self.videoId}"


class Overrides(BaseModel):
    title: str | None = None
    artist: str | None = None
    album: str | None = None
    track: int | None = None
    year: int | None = None


class JobResult(BaseModel):
    path: str = ""
    songId: str | None = None
    title: str = ""
    artist: str = ""
    album: str = ""
    durationSec: float = 0.0
    codec: str = ""


class FetchJob(BaseModel):
    id: str
    status: JobStatus
    videoId: str
    url: str | None = None
    requestedAt: str
    startedAt: str | None = None
    finishedAt: str | None = None
    progress: float = 0.0
    stage: str = "queued"
    error: str | None = None
    overrides: Overrides | None = None
    result: JobResult | None = None


class SearchResponse(BaseModel):
    candidates: list[Candidate]
    albums: list["AlbumResult"] = Field(default_factory=list)
    correction: "Correction | None" = None
    suggestions: list[str] = Field(default_factory=list)


class Correction(BaseModel):
    artist: str
    title: str


class SuggestResponse(BaseModel):
    suggestions: list[str] = Field(default_factory=list)
    correction: Correction | None = None


class RadioResponse(BaseModel):
    candidates: list[Candidate]
    sources: list[str] = Field(default_factory=list)


class MetadataCandidate(BaseModel):
    """Canonical metadata for manually fixing one song's tags."""

    source: str = "deezer"
    title: str
    artist: str
    album: str = ""
    year: int | None = None
    artwork: str = ""
    durationSec: float = 0.0
    explicit: bool | None = None
    isrc: str = ""


class MetadataLookupResponse(BaseModel):
    candidates: list[MetadataCandidate] = Field(default_factory=list)


_TAG_CONTROL_RE = re.compile(r"[\x00-\x1f\x7f]")
_TAG_TEXT_LIMITS = {"title": 512, "artist": 512, "album": 512, "genre": 256}
_TAG_NUMBER_RANGES = {"track": (1, 999), "year": (1000, 9999)}


class SongTagsRequest(BaseModel):
    """Partial tag edit for one library file.

    Omitted fields stay untouched; ``null`` or ``""`` clears a field
    (``0`` also clears track/year, which never write a literal 0). The
    title may change but never be cleared."""

    model_config = ConfigDict(extra="forbid")

    title: str | None = None
    artist: str | None = None
    album: str | None = None
    track: int | None = None
    year: int | None = None
    genre: str | None = None

    @field_validator("title", "artist", "album", "genre", mode="before")
    @classmethod
    def _text(cls, value: Any, info: ValidationInfo) -> str | None:
        if value is None:
            return None
        if not isinstance(value, str):
            raise ValueError(f"{info.field_name} must be text")
        value = value.strip()
        if _TAG_CONTROL_RE.search(value):
            raise ValueError(f"{info.field_name} contains control characters")
        if len(value) > _TAG_TEXT_LIMITS[info.field_name]:
            raise ValueError(f"{info.field_name} is too long")
        return value

    @field_validator("track", "year", mode="before")
    @classmethod
    def _number(cls, value: Any, info: ValidationInfo) -> int | None:
        if isinstance(value, str):
            value = value.strip()
            if not value:
                return None
            if not value.isascii() or not value.isdigit():
                raise ValueError(f"{info.field_name} must be a whole number")
            value = int(value)
        if value is None:
            return None
        if isinstance(value, bool) or not isinstance(value, int):
            raise ValueError(f"{info.field_name} must be a whole number")
        if value == 0:
            return None
        low, high = _TAG_NUMBER_RANGES[info.field_name]
        if not low <= value <= high:
            raise ValueError(f"{info.field_name} must be between {low} and {high}")
        return value

    @model_validator(mode="after")
    def _sensible(self) -> SongTagsRequest:
        if not self.model_fields_set:
            raise ValueError("No tag changes were provided")
        if "title" in self.model_fields_set and not self.title:
            raise ValueError("Title cannot be empty")
        return self

    def edits(self) -> dict[str, str | int | None]:
        """Only the fields the client sent; cleared text becomes ``""``."""
        out: dict[str, str | int | None] = {}
        for name in self.model_fields_set:
            value = getattr(self, name)
            out[name] = (value or "") if name in _TAG_TEXT_LIMITS else value
        return out


class SongTagsResponse(BaseModel):
    """The tags read back from the rewritten file."""

    ok: bool = True
    id: str
    changed: bool = True
    title: str = ""
    artist: str = ""
    album: str = ""
    track: int | None = None
    year: int | None = None
    genre: str = ""


class AlbumResult(BaseModel):
    browseId: str
    title: str
    artist: str = ""
    year: int | None = None
    explicit: bool | None = None
    thumbnails: list[Thumbnail] = Field(default_factory=list)
    score: float = 0.0
    pinned: bool = False


class AlbumResponse(BaseModel):
    album: AlbumResult
    candidates: list[Candidate]


class ResolveResponse(BaseModel):
    kind: Literal["video", "playlist", "album"]
    candidates: list[Candidate]
    title: str | None = None


class JobResponse(BaseModel):
    job: FetchJob


class JobsResponse(BaseModel):
    jobs: list[FetchJob]


class OkResponse(BaseModel):
    ok: bool = True


class HealthResponse(BaseModel):
    ok: bool = True
    yt_dlp: str = ""


# ---------------------------------------------------------------------------
# One-way YouTube Music playlist import
# ---------------------------------------------------------------------------

ImportItemStatus = Literal["pending", "downloading", "done", "failed", "skipped"]


class YouTubePlaylist(BaseModel):
    playlistId: str
    title: str
    count: int = 0
    owned: bool | None = None
    thumbnails: list[Thumbnail] = Field(default_factory=list)


class YouTubePlaylistsResponse(BaseModel):
    playlists: list[YouTubePlaylist] = Field(default_factory=list)


class ImportTrack(BaseModel):
    videoId: str
    title: str
    artist: str = ""
    album: str = ""
    durationSec: float = 0.0
    position: int = 0
    thumbnails: list[Thumbnail] = Field(default_factory=list)


class YouTubePlaylistTracksResponse(BaseModel):
    id: str
    title: str
    tracks: list[ImportTrack] = Field(default_factory=list)


class ImportItem(BaseModel):
    videoId: str
    title: str
    artist: str = ""
    album: str = ""
    durationSec: float = 0.0
    position: int = 0
    status: ImportItemStatus = "pending"
    songId: str | None = None
    jobId: str | None = None
    error: str | None = None


class PlaylistImport(BaseModel):
    id: str
    ytmPlaylistId: str
    name: str
    playlistId: str | None = None
    createdAt: str
    updatedAt: str
    total: int = 0
    done: int = 0
    pending: int = 0
    failed: int = 0
    items: list[ImportItem] = Field(default_factory=list)


class PlaylistImportsResponse(BaseModel):
    imports: list[PlaylistImport] = Field(default_factory=list)


class PreviewWarmRequest(BaseModel):
    videoIds: list[str] = Field(default_factory=list, max_length=5)


class AlbumSaveRequest(BaseModel):
    title: str = Field(min_length=1, max_length=300)
    artist: str = Field(default="", max_length=300)
