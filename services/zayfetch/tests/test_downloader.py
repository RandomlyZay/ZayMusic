"""yt-dlp command and metadata parsing regression tests."""

import json
import subprocess
from pathlib import Path

import pytest

from zayfetch.downloader import (
    DownloadError,
    DownloadedMeta,
    _parse_info_json,
    build_cmd,
    normalize_for_library,
)


def test_download_keeps_native_audio_without_thumbnail_postprocessor(tmp_path: Path):
    cmd = build_cmd("video-id", tmp_path)

    assert "bestaudio/best" in cmd
    assert "--embed-thumbnail" not in cmd
    assert "--embed-metadata" in cmd
    assert "--write-info-json" in cmd
    assert "--no-progress" not in cmd
    assert "--newline" in cmd
    assert "https://www.youtube.com/watch?v=video-id" in cmd


def test_parse_info_json_preserves_full_metadata_and_thumbnail(tmp_path: Path):
    info_path = tmp_path / "video-id.info.json"
    info_path.write_text(
        json.dumps(
            {
                "id": "video-id",
                "track": "Part 1 | Part 2",
                "title": "A title with a | delimiter",
                "artist": "Artist",
                "album": "Album",
                "duration": 183.5,
                "ext": "webm",
                "acodec": "opus",
                "thumbnails": [{"url": "https://i.ytimg.com/cover.jpg"}],
            }
        ),
        encoding="utf-8",
    )

    meta = _parse_info_json(info_path)

    assert meta is not None
    assert meta.artist == "Artist"
    assert meta.title == "Part 1 | Part 2"
    assert meta.album == "Album"
    assert meta.duration == 183.5
    assert meta.ext == "webm"
    assert meta.thumbnail == "https://i.ytimg.com/cover.jpg"
    assert meta.extra["acodec"] == "opus"


@pytest.mark.parametrize(("codec", "extension"), [("opus", "opus"), ("vorbis", "ogg")])
def test_webm_is_remuxed_without_reencoding(
    tmp_path: Path, monkeypatch, codec: str, extension: str
):
    source = tmp_path / "video-id.webm"
    source.write_bytes(b"native stream")
    commands: list[list[str]] = []

    def fake_ffmpeg(command, **kwargs):
        commands.append(command)
        Path(command[-1]).write_bytes(b"remuxed native stream")
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr("zayfetch.downloader.subprocess.run", fake_ffmpeg)
    meta = DownloadedMeta(
        video_id="video-id",
        title="Track",
        ext="webm",
        path=source,
        extra={"acodec": codec},
    )

    result = normalize_for_library(meta)

    assert result.path == tmp_path / f"video-id.{extension}"
    assert result.ext == extension
    assert result.path.read_bytes() == b"remuxed native stream"
    assert not source.exists()
    assert "-c:a" in commands[0]
    assert commands[0][commands[0].index("-c:a") + 1] == "copy"


def test_webm_with_unknown_codec_fails_before_discarding_download(tmp_path: Path):
    source = tmp_path / "video-id.webm"
    source.write_bytes(b"native stream")
    meta = DownloadedMeta(video_id="video-id", title="Track", ext="webm", path=source)

    with pytest.raises(DownloadError, match="unsupported WebM audio codec"):
        normalize_for_library(meta)

    assert source.read_bytes() == b"native stream"
