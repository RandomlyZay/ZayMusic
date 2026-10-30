"""Preview URL extraction: shared in-flight work, expiry-based caching, warm-up."""

import threading
import time

import pytest

from zayfetch import preview


@pytest.fixture(autouse=True)
def clean_state():
    preview._cache.clear()
    preview._inflight.clear()
    yield
    preview._cache.clear()
    preview._inflight.clear()


def test_ttl_follows_expire_param():
    now = 1_000_000
    url = f"https://rr1.googlevideo.com/videoplayback?expire={now + 6 * 3600}&id=x"
    assert preview.url_ttl(url, now=now) == 6 * 3600 - preview.EXPIRY_MARGIN


def test_ttl_path_style_expire_and_fallback():
    now = 1_000_000
    assert preview.url_ttl(f"https://x/videoplayback/expire/{now + 3600}/id/x", now=now) == 3600 - preview.EXPIRY_MARGIN
    assert preview.url_ttl("https://x/audio.m4a", now=now) == preview.FALLBACK_TTL


def test_nearly_expired_url_is_not_cached(monkeypatch):
    soon = int(time.time()) + 60
    monkeypatch.setattr(preview, "_extract", lambda vid: f"https://x/v?expire={soon}")
    preview.audio_url("aaaaaaaaaaa")
    assert preview.cached("aaaaaaaaaaa") is None


def test_concurrent_requests_share_one_extraction(monkeypatch):
    calls = []
    gate = threading.Event()

    def slow(vid):
        calls.append(vid)
        gate.wait(5)
        return f"https://x/{vid}"

    monkeypatch.setattr(preview, "_extract", slow)
    results = []
    threads = [threading.Thread(target=lambda: results.append(preview.audio_url("bbbbbbbbbbb"))) for _ in range(4)]
    for t in threads:
        t.start()
    time.sleep(0.1)
    gate.set()
    for t in threads:
        t.join(5)
    assert calls == ["bbbbbbbbbbb"]
    assert results == ["https://x/bbbbbbbbbbb"] * 4
    # Served from the cache afterwards.
    assert preview.audio_url("bbbbbbbbbbb") == "https://x/bbbbbbbbbbb"
    assert calls == ["bbbbbbbbbbb"]


def test_failure_releases_waiters_and_is_retried(monkeypatch):
    attempts = []

    def flaky(vid):
        attempts.append(vid)
        if len(attempts) == 1:
            raise preview.Unavailable("nope")
        return "https://x/ok"

    monkeypatch.setattr(preview, "_extract", flaky)
    with pytest.raises(preview.Unavailable):
        preview.audio_url("ccccccccccc")
    assert preview.audio_url("ccccccccccc") == "https://x/ok"


def test_warm_runs_in_background_and_feeds_the_player(monkeypatch):
    calls = []
    monkeypatch.setattr(preview, "_extract", lambda vid: calls.append(vid) or f"https://x/{vid}")
    assert preview.warm(["ddddddddddd", "eeeeeeeeeee"]) == 2
    assert preview.audio_url("ddddddddddd") == "https://x/ddddddddddd"
    deadline = time.time() + 5
    while preview.cached("eeeeeeeeeee") is None and time.time() < deadline:
        time.sleep(0.01)
    assert preview.warm(["ddddddddddd", "eeeeeeeeeee"]) == 0
    assert sorted(calls) == ["ddddddddddd", "eeeeeeeeeee"]


def test_cookie_failure_does_not_retry_anonymously(monkeypatch):
    seen = []

    def once(vid, *, with_cookies):
        seen.append(with_cookies)
        return None if with_cookies else "https://x/anon"

    monkeypatch.setattr(preview, "extract_once", once)
    monkeypatch.setattr(preview.downloader, "cookies_file", lambda: "/tmp/cookies.txt")
    with pytest.raises(preview.Unavailable):
        preview._extract("fffffffffff")
    assert seen == [True]


def test_in_process_breakage_falls_back_to_cli(monkeypatch):
    def broken(vid, *, with_cookies):
        raise RuntimeError("plugin import failed")

    monkeypatch.setattr(preview, "_extract_in_process", broken)
    monkeypatch.setattr(preview, "_extract_cli", lambda vid, *, with_cookies: "https://x/cli")
    assert preview.extract_once("ggggggggggg", with_cookies=True) == "https://x/cli"


def test_cache_is_bounded(monkeypatch):
    monkeypatch.setattr(preview, "CACHE_MAX", 3)
    for i in range(5):
        preview._store(f"vid{i}", f"https://x/{i}")
    assert list(preview._cache) == ["vid2", "vid3", "vid4"]
