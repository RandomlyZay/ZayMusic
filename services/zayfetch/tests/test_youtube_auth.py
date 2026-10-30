"""YouTube Music connection: cookie storage + verification endpoints.

Premium-first acquisition: downloads run only against a connected
account, so these are the tests that guard the connect flow.
"""

import json

import pytest
from fastapi.testclient import TestClient

from zayfetch import discovery, downloader, main

NETSCAPE = (
    "# Netscape HTTP Cookie File\n"
    "# This is a generated file! Do not edit.\n"
    ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\texample-session\n"
    ".youtube.com\tTRUE\t/\tTRUE\t0\tHSID\texample-hsid\n"
    ".youtube.com\tTRUE\t/\tTRUE\t0\tLOGIN_INFO\texample-login\n"
)


@pytest.fixture(autouse=True)
def isolated_data_dir(tmp_path, monkeypatch):
    monkeypatch.delenv("YTDLP_COOKIES_FILE", raising=False)
    monkeypatch.setenv("DATA_DIR", str(tmp_path))


def test_write_cookies_requires_netscape_format():
    with pytest.raises(ValueError):
        downloader.write_cookies("this is not a cookie file")
    assert not downloader.cookies_path().is_file()


def test_write_read_remove_roundtrip():
    path = downloader.write_cookies(NETSCAPE)
    assert path.is_file()
    assert path.stat().st_mode & 0o777 == 0o600
    assert downloader.cookies_file() == str(path)
    assert downloader.remove_cookies() is True
    assert downloader.cookies_file() == ""
    assert downloader.remove_cookies() is False


def test_env_override_wins(tmp_path, monkeypatch):
    custom = tmp_path / "custom" / "yt.txt"
    custom.parent.mkdir()
    custom.write_text(NETSCAPE)
    monkeypatch.setenv("YTDLP_COOKIES_FILE", str(custom))
    assert downloader.cookies_file() == str(custom)


def test_jobs_require_connected_account(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    client = TestClient(main.app)
    r = client.post("/jobs", json={
        "videoId": "abcdefghijk",
        "overrides": {"title": "9", "artist": "Drake"},
    })
    assert r.status_code == 412
    assert "YouTube Music Premium" in r.json()["detail"]


def test_youtube_status_and_connect(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    client = TestClient(main.app)

    assert client.get("/youtube/cookies").json()["configured"] is False

    monkeypatch.setattr(main, "_probe_youtube_auth",
                        lambda: (True, True, "YouTube Music Premium connected"))
    body = client.post("/youtube/cookies", json={"cookies": NETSCAPE}).json()
    assert body["configured"] is True
    assert body["premium"] is True
    assert body["message"] == "YouTube Music Premium connected"

    # The stored status serves later reads without a live probe.
    monkeypatch.setattr(main, "_probe_youtube_auth",
                        lambda: (_ for _ in ()).throw(AssertionError("no probe")))
    again = client.get("/youtube/cookies").json()
    assert again["configured"] is True and again["premium"] is True

    # Jobs now pass the connection gate.
    from zayfetch.models import FetchJob

    monkeypatch.setattr(main.store, "create_job", lambda **k: FetchJob(
        id="j1", status="queued", videoId="abcdefghijk", requestedAt="now"))
    monkeypatch.setattr(main, "ensure_worker", lambda: None)
    r = client.post("/jobs", json={"videoId": "abcdefghijk"})
    assert r.status_code == 200

    removed = client.delete("/youtube/cookies").json()
    assert removed["removed"] is True
    assert client.get("/youtube/cookies").json()["configured"] is False
    assert not downloader.cookies_status_path().is_file()


def test_connect_rejects_junk(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    client = TestClient(main.app)
    r = client.post("/youtube/cookies", json={"cookies": "hello world, not a cookie file"})
    assert r.status_code == 400


def test_normalize_accepts_devtools_copy_as_curl():
    curl = (
        "curl 'https://music.youtube.com/youtubei/v1/browse?key=x' "
        "-H 'cookie: VISITOR_INFO1_LIVE=abc; SID=secret-sid; HSID=secret-hsid; LOGIN_INFO=li' "
        "-H 'user-agent: Mozilla'"
    )
    text = downloader.normalize_cookies(curl)
    assert "# Netscape HTTP Cookie File" in text
    downloader.write_cookies(curl)
    jar = downloader.read_cookies_dict()
    assert jar["SID"] == "secret-sid"
    assert jar["HSID"] == "secret-hsid"


def test_normalize_accepts_raw_cookie_header():
    downloader.write_cookies("PREF=f6=80; __Secure-1PSID=secure-one; SSID=ss; SID=ss")
    jar = downloader.read_cookies_dict()
    assert jar["SSID"] == "ss"
    assert jar["__Secure-1PSID"] == "secure-one"


def test_normalize_rejects_cookies_without_sign_in():
    with pytest.raises(ValueError, match="sign-in"):
        downloader.normalize_cookies("PREF=f6=80; VISITOR_INFO1_LIVE=xyz")


def test_normalize_rejects_garbage():
    with pytest.raises(ValueError):
        downloader.normalize_cookies("hello world, not cookies")


def test_connect_accepts_raw_cookie_header(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr(main, "_probe_youtube_auth",
                        lambda: (True, True, "YouTube Music Premium connected"))
    client = TestClient(main.app)
    body = client.post("/youtube/cookies", json={
        "cookies": "SID=abc123456789; HSID=def123456789; SSID=ghi123456789",
    }).json()
    assert body["configured"] is True
    assert body["premium"] is True
    assert downloader.read_cookies_dict()["SID"] == "abc123456789"


def test_cookie_dict_parses_netscape():
    downloader.write_cookies(NETSCAPE)
    jar = downloader.read_cookies_dict()
    assert jar["SID"] == "example-session"
    assert jar["HSID"] == "example-hsid"


def test_probe_requires_authenticated_session(monkeypatch):
    monkeypatch.setattr(main, "_cookies_authenticated", lambda: False)
    monkeypatch.setattr(main, "_run_yt_probe",
                        lambda vid: (_ for _ in ()).throw(AssertionError("no yt-dlp probe")))
    verified, premium, message = main._probe_youtube_auth()
    assert verified is False and premium is False
    assert "rejected" in message


def test_probe_reports_premium_connected(monkeypatch):
    monkeypatch.setattr(main, "_cookies_authenticated", lambda: True)
    monkeypatch.setattr(main, "_run_yt_probe", lambda vid: (True, ""))
    assert main._probe_youtube_auth() == (True, True, "YouTube Music Premium connected")


def test_probe_reports_missing_entitlement(monkeypatch):
    monkeypatch.setattr(main, "_cookies_authenticated", lambda: True)
    monkeypatch.setattr(main, "_run_yt_probe", lambda vid: (
        False, "ERROR: This video is only available to Music Premium members"))
    verified, premium, message = main._probe_youtube_auth()
    assert verified is True and premium is False
    assert "entitlement" in message


# --- Google API OAuth + managed-session status (§5.4, §5.5) ----------------


class _FakeCreds:
    def __init__(self, code=None, token=None, error=None):
        self._code = code or {}
        self._token = token or {}
        self._error = error

    def get_code(self):
        if self._error:
            raise self._error
        return self._code

    def token_from_code(self, device_code):
        if self._error:
            raise self._error
        return self._token


def _oauth_env(monkeypatch):
    monkeypatch.setenv("YTM_OAUTH_CLIENT_ID", "cid")
    monkeypatch.setenv("YTM_OAUTH_CLIENT_SECRET", "secret")


def test_oauth_requires_configuration(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.delenv("YTM_OAUTH_CLIENT_ID", raising=False)
    monkeypatch.delenv("YTM_OAUTH_CLIENT_SECRET", raising=False)
    client = TestClient(main.app)
    assert client.post("/youtube/oauth/start").status_code == 400
    assert client.post("/youtube/oauth/poll").status_code == 400


def test_oauth_start_poll_delete(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    _oauth_env(monkeypatch)
    client = TestClient(main.app)

    monkeypatch.setattr(main, "_oauth_credentials", lambda: _FakeCreds(code={
        "device_code": "device-1",
        "user_code": "ABCD-EFGH",
        "verification_url": "https://www.google.com/device",
        "expires_in": 900,
    }))
    started = client.post("/youtube/oauth/start").json()
    assert started == {
        "user_code": "ABCD-EFGH",
        "verification_url": "https://www.google.com/device",
        "expires_in": 900,
    }
    assert downloader.read_device_code() == "device-1"

    # Pending sign-in.
    monkeypatch.setattr(main, "_oauth_credentials",
                        lambda: _FakeCreds(token={"error": "authorization_pending"}))
    assert client.post("/youtube/oauth/poll").json() == {"connected": False, "pending": True}
    assert downloader.read_device_code() == "device-1"

    # Success writes a 600 oauth.json and clears the device code.
    monkeypatch.setattr(main, "_oauth_credentials", lambda: _FakeCreds(token={
        "access_token": "at", "refresh_token": "rt", "expires_in": 3600,
        "scope": "https://www.googleapis.com/auth/youtube", "token_type": "Bearer",
    }))
    assert client.post("/youtube/oauth/poll").json() == {"connected": True}
    assert downloader.read_device_code() == ""
    assert downloader.oauth_path().is_file()
    assert downloader.oauth_path().stat().st_mode & 0o777 == 0o600
    token = json.loads(downloader.oauth_path().read_text(encoding="utf-8"))
    assert token["refresh_token"] == "rt"
    assert token["expires_at"] > 0
    assert set(token) >= {"scope", "token_type", "access_token", "refresh_token",
                          "expires_in", "expires_at"}

    status = client.get("/youtube/cookies").json()
    assert status["oauth"] == {"configured": True, "connected": True}

    removed = client.delete("/youtube/oauth").json()
    assert removed["removed"] is True
    assert not downloader.oauth_path().is_file()
    assert client.get("/youtube/cookies").json()["oauth"]["connected"] is False


def test_oauth_poll_rejects_expired_code(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    _oauth_env(monkeypatch)
    downloader.write_device_code("device-1")
    monkeypatch.setattr(main, "_oauth_credentials",
                        lambda: _FakeCreds(token={"error": "expired_token"}))
    client = TestClient(main.app)
    r = client.post("/youtube/oauth/poll")
    assert r.status_code == 400
    assert downloader.read_device_code() == ""


def test_youtube_status_includes_browser_oauth_and_expiry(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.delenv("YTM_OAUTH_CLIENT_ID", raising=False)
    monkeypatch.delenv("YTM_OAUTH_CLIENT_SECRET", raising=False)
    client = TestClient(main.app)
    body = client.get("/youtube/cookies").json()
    for key in ("browser", "lastSyncAt", "expiresAt", "expiresInDays", "oauth"):
        assert key in body
    assert body["browser"] is None
    assert body["oauth"] == {"configured": False, "connected": False}


def test_ytmusic_client_prefers_oauth(monkeypatch, tmp_path):
    created: list[tuple] = []

    class _FakeYTMusic:
        def __init__(self, auth=None, **kwargs):
            created.append((auth, kwargs))

    monkeypatch.setattr("ytmusicapi.YTMusic", _FakeYTMusic)
    _oauth_env(monkeypatch)
    downloader.write_oauth_token({
        "scope": "s", "token_type": "Bearer", "access_token": "a",
        "refresh_token": "r", "expires_in": 3600, "expires_at": 9999999999,
    })
    discovery.ytmusic_client()
    assert created
    assert created[0][0] == str(downloader.oauth_path())
    assert created[0][1].get("oauth_credentials") is not None


def test_ytmusic_client_falls_back_to_cookies(monkeypatch):
    created: list[tuple] = []

    class _FakeYTMusic:
        def __init__(self, auth=None, **kwargs):
            created.append((auth, kwargs))

    monkeypatch.setattr("ytmusicapi.YTMusic", _FakeYTMusic)
    monkeypatch.delenv("YTM_OAUTH_CLIENT_ID", raising=False)
    monkeypatch.delenv("YTM_OAUTH_CLIENT_SECRET", raising=False)
    auth = {"cookie": "c", "authorization": "SAPISIDHASH x",
            "origin": "https://music.youtube.com"}
    monkeypatch.setattr(discovery, "_browser_auth", lambda: auth)
    discovery.ytmusic_client()
    assert created == [(auth, {})]


def test_ytmusic_client_oauth_failure_falls_back(monkeypatch):
    created: list[tuple] = []

    class _FakeYTMusic:
        def __init__(self, auth=None, **kwargs):
            if kwargs.get("oauth_credentials") is not None:
                raise RuntimeError("revoked refresh token")
            created.append((auth, kwargs))

    monkeypatch.setattr("ytmusicapi.YTMusic", _FakeYTMusic)
    _oauth_env(monkeypatch)
    downloader.write_oauth_token({
        "scope": "s", "token_type": "Bearer", "access_token": "a",
        "refresh_token": "r", "expires_in": 3600, "expires_at": 9999999999,
    })
    auth = {"cookie": "c", "authorization": "SAPISIDHASH x",
            "origin": "https://music.youtube.com"}
    monkeypatch.setattr(discovery, "_browser_auth", lambda: auth)
    discovery.ytmusic_client()
    assert created == [(auth, {})]
