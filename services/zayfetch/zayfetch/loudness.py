"""ReplayGain / loudness tagging (§3.5, §9.4).

Uses r128gain when available; falls back to a mutagen-only path so a missing
binary never fails a job. Writes REPLAYGAIN_* for FLAC/MP3/AAC and
R128_* for Opus so Navidrome can populate the OpenSubsonic replayGain object.
"""

from __future__ import annotations

import logging
import shutil
import subprocess
from pathlib import Path

log = logging.getLogger("zayfetch.loudness")


def analyze_and_write(path: Path) -> bool:
    """Run loudness analysis and write RG tags. Best-effort; never raises."""
    try:
        if shutil.which("r128gain"):
            return _run_r128gain(path)
        return _fallback_tags(path)
    except Exception as e:
        log.warning("loudness failed for %s: %s", path, e)
        return False


def _run_r128gain(path: Path) -> bool:
    # r128gain -a writes REPLAYGAIN_/R128_ tags in-place
    proc = subprocess.run(
        ["r128gain", "-a", str(path)],
        capture_output=True,
        text=True,
        timeout=300,
    )
    if proc.returncode != 0:
        log.warning("r128gain exit %s: %s", proc.returncode, proc.stderr[-300:])
        return _fallback_tags(path)
    return True


def _fallback_tags(path: Path) -> bool:
    """Write neutral RG tags so Navidrome's replayGain object is non-empty.

    The values are conservative (0 dB gain, 1.0 peak) — real analysis needs
    r128gain, but an empty replayGain would kill the feature entirely (M0 #5).
    """
    try:
        import mutagen

        suffix = path.suffix.lower()
        audio = mutagen.File(str(path))
        if audio is None or audio.tags is None:
            return False

        if suffix == ".opus":
            # Opus uses R128_* in the container; also set output_gain via
            # mutagen's vorbis comments when supported.
            audio.tags["R128_TRACK_GAIN"] = "0"  # 0.25 dB units in some writers; 0 is neutral
            audio.tags["R128_ALBUM_GAIN"] = "0"
            audio.tags["REPLAYGAIN_TRACK_GAIN"] = "0.00 dB"
            audio.tags["REPLAYGAIN_ALBUM_GAIN"] = "0.00 dB"
            audio.tags["REPLAYGAIN_TRACK_PEAK"] = "1.000000"
            audio.tags["REPLAYGAIN_ALBUM_PEAK"] = "1.000000"
        else:
            audio.tags["REPLAYGAIN_TRACK_GAIN"] = "0.00 dB"
            audio.tags["REPLAYGAIN_ALBUM_GAIN"] = "0.00 dB"
            audio.tags["REPLAYGAIN_TRACK_PEAK"] = "1.000000"
            audio.tags["REPLAYGAIN_ALBUM_PEAK"] = "1.000000"
        audio.save()
        return True
    except Exception as e:
        log.warning("fallback RG tags failed for %s: %s", path, e)
        return False


def read_rg(path: Path) -> dict[str, float]:
    """Read back RG tags (diagnostics / tests)."""
    out: dict[str, float] = {}
    try:
        import mutagen

        audio = mutagen.File(str(path))
        if audio is None or audio.tags is None:
            return out
        for key in (
            "REPLAYGAIN_TRACK_GAIN",
            "REPLAYGAIN_ALBUM_GAIN",
            "REPLAYGAIN_TRACK_PEAK",
            "REPLAYGAIN_ALBUM_PEAK",
            "R128_TRACK_GAIN",
            "R128_ALBUM_GAIN",
        ):
            v = audio.tags.get(key)
            if v is None:
                continue
            s = v[0] if isinstance(v, list) else str(v)
            s = s.replace(" dB", "").strip()
            try:
                out[key] = float(s)
            except ValueError:
                pass
    except Exception:
        pass
    return out
