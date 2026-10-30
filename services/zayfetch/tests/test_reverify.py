"""A stale "rejected" verdict must not outlive a fresh sign-in."""

import pytest
from fastapi import HTTPException
from fastapi.testclient import TestClient

from zayfetch import downloader, main, session

GOOD = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tsap\n"


@pytest.fixture(autouse=True)
def isolated(tmp_path, monkeypatch):
    monkeypatch.delenv("YTDLP_COOKIES_FILE", raising=False)
    monkeypatch.setenv("DATA_DIR", str(tmp_path))
    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr(main, "_last_reverify", 0.0)
    downloader.write_netscape(GOOD)
    downloader.write_cookies_status({"verified": False, "message": "YouTube rejected these cookies"})


def probes(monkeypatch, result=(True, True, "YouTube Music Premium connected")):
    calls = []
    monkeypatch.setattr(main, "_probe_youtube_auth", lambda: calls.append(1) or result)
    return calls


def test_import_guard_reprobes_a_stale_rejection(monkeypatch):
    calls = probes(monkeypatch)
    main._require_youtube()
    assert calls == [1]
    assert downloader.read_cookies_status()["verified"] is True


def test_import_guard_still_refuses_when_probe_fails(monkeypatch):
    probes(monkeypatch, (False, False, "rejected"))
    with pytest.raises(HTTPException) as err:
        main._require_youtube()
    assert err.value.status_code == 412


def test_reprobe_is_throttled(monkeypatch):
    calls = probes(monkeypatch, (False, False, "rejected"))
    for _ in range(3):
        with pytest.raises(HTTPException):
            main._require_youtube()
    assert calls == [1]


def test_keeper_reprobes_after_browser_sign_in(monkeypatch):
    calls = probes(monkeypatch)
    monkeypatch.setattr(session, "sync_session", lambda: (GOOD, "connected"))
    assert main._session_keeper_tick() == "connected"
    assert calls == [1]
    status = downloader.read_cookies_status()
    assert status["verified"] is True and status["browser"] == "connected"


def test_keeper_does_not_reprobe_when_signed_out_or_verified(monkeypatch):
    calls = probes(monkeypatch)
    monkeypatch.setattr(session, "sync_session", lambda: (None, "signed_out"))
    main._session_keeper_tick()
    downloader.write_cookies_status({"verified": True})
    monkeypatch.setattr(session, "sync_session", lambda: (GOOD, "connected"))
    main._session_keeper_tick()
    assert calls == []


def test_check_connection_forces_a_probe(monkeypatch):
    calls = probes(monkeypatch)
    monkeypatch.setattr(session, "browser_enabled", lambda: False)
    client = TestClient(main.app)
    assert client.get("/youtube/cookies").json()["verified"] is False
    assert client.get("/youtube/cookies?verify=true").json()["verified"] is True
    assert client.get("/youtube/cookies?verify=true").status_code == 200
    assert calls == [1, 1]
