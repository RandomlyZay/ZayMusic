"""Managed browser session + cookie-jar helpers (§7).

These guard the pieces that make the session self-sustaining: the CDP cookie
export round-trips through yt-dlp's own jar, sign-in detection mirrors
yt-dlp, and the freshness guarantee actually rewrites the stored file.
"""

from contextlib import contextmanager
import sys
from types import ModuleType, SimpleNamespace
from unittest.mock import Mock

import pytest

from zayfetch import downloader, main, session

CDP_PAYLOAD = [
    {"name": "LOGIN_INFO", "value": "login", "domain": ".youtube.com", "path": "/",
     "expires": -1, "httpOnly": True, "secure": True},
    {"name": "SAPISID", "value": "sap", "domain": ".youtube.com", "path": "/",
     "expires": 0, "httpOnly": True, "secure": True},
    {"name": "SID", "value": "sid", "domain": ".youtube.com", "path": "/",
     "expires": 0, "httpOnly": False, "secure": False},
    {"name": "OTHER", "value": "x", "domain": ".example.com", "path": "/",
     "expires": 123, "httpOnly": False, "secure": False},
]


@pytest.fixture(autouse=True)
def isolated_data_dir(tmp_path, monkeypatch):
    monkeypatch.delenv("YTDLP_COOKIES_FILE", raising=False)
    monkeypatch.setenv("DATA_DIR", str(tmp_path))
    monkeypatch.delenv("YTM_OAUTH_CLIENT_ID", raising=False)
    monkeypatch.delenv("YTM_OAUTH_CLIENT_SECRET", raising=False)


class FakeLoop:
    def __init__(self) -> None:
        self.tasks = []

    def create_task(self, coro):
        self.tasks.append(coro)
        coro.close()
        return object()


def test_cdp_payload_roundtrips_through_cookiejar():
    text = downloader.cookies_from_cdp(CDP_PAYLOAD)
    assert "#HttpOnly_.youtube.com" in text
    path = downloader.write_netscape(text)
    assert path.stat().st_mode & 0o777 == 0o600

    jar = downloader.load_cookiejar()
    by_name = {c.name: c for c in jar}
    assert set(by_name) == {"LOGIN_INFO", "SAPISID", "SID", "OTHER"}
    # expires=0 session cookies come back as session cookies, not 1970.
    assert by_name["SAPISID"].expires is None
    assert by_name["SAPISID"].discard is True

    again = downloader.cookiejar_to_netscape(jar)
    assert "\t0\tSAPISID\t" in again
    assert "\t0\tSID\t" in again
    assert "LOGIN_INFO" in again


def test_has_login_session_mirrors_yt_dlp():
    assert downloader.has_login_session({"LOGIN_INFO": "x", "SAPISID": "y"}) is True
    assert downloader.has_login_session(
        {"LOGIN_INFO": "x", "__Secure-1PSAPISID": "y"}
    ) is True
    assert downloader.has_login_session(
        {"LOGIN_INFO": "x", "__Secure-3PSAPISID": "y"}
    ) is True
    # LOGIN_INFO alone is not enough (yt-dlp requires a SAPISID too).
    assert downloader.has_login_session({"LOGIN_INFO": "x"}) is False
    # SID/HSID only (the legacy paste fixture) is not a login either.
    assert downloader.has_login_session({"SID": "a", "HSID": "b", "LOGIN_INFO": "c"}) is False
    assert downloader.has_login_session({}) is False


def test_session_expiry_info_uses_soonest(monkeypatch):
    now = 1_000_000.0
    downloader.write_netscape(
        "# Netscape HTTP Cookie File\n"
        f".youtube.com\tTRUE\t/\tTRUE\t{int(now + 3 * 86400)}\tSAPISID\ta\n"
        f".youtube.com\tTRUE\t/\tTRUE\t{int(now + 10 * 86400)}\tLOGIN_INFO\tb\n"
    )
    monkeypatch.setattr(downloader.time, "time", lambda: now)
    info = downloader.session_expiry_info()
    assert info["expiresAt"] == now + 3 * 86400
    assert info["expiresInDays"] == pytest.approx(3.0)


def test_session_expiry_info_none_when_session_only():
    downloader.write_netscape(
        "# Netscape HTTP Cookie File\n"
        ".youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\ta\n"
    )
    assert downloader.session_expiry_info() == {"expiresAt": None, "expiresInDays": None}


def test_ensure_session_keeper_starts_once(monkeypatch):
    monkeypatch.setattr(session, "CDP_URL", "http://browser:9222")
    monkeypatch.setattr(main, "_session_keeper_started", False)
    loop = FakeLoop()
    monkeypatch.setattr(main.asyncio, "get_running_loop", lambda: loop)
    main.ensure_session_keeper()
    main.ensure_session_keeper()
    assert len(loop.tasks) == 1


def test_ensure_session_keeper_skips_without_browser(monkeypatch):
    monkeypatch.setattr(session, "CDP_URL", "")
    monkeypatch.setattr(main, "_session_keeper_started", False)
    loop = FakeLoop()
    monkeypatch.setattr(main.asyncio, "get_running_loop", lambda: loop)
    main.ensure_session_keeper()
    assert loop.tasks == []
    assert main._session_keeper_started is False


def test_export_cookies_none_when_unreachable(monkeypatch):
    monkeypatch.setattr(session, "CDP_URL", "http://127.0.0.1:1")
    assert session.export_cookies_from_browser() is None
    assert session.browser_logged_in() is None


def test_session_sync_disconnected_when_unreachable(monkeypatch):
    monkeypatch.setattr(session, "CDP_URL", "http://127.0.0.1:1")
    assert session.sync_session() == (None, "disconnected")


@pytest.mark.parametrize("logged_in, expected", [
    (True, "connected"), (False, "signed_out"), (None, "disconnected"),
])
def test_status_checks_live_browser_after_sign_in(monkeypatch, logged_in, expected):
    monkeypatch.setattr(session, "CDP_URL", "http://browser:9222")
    downloader.write_cookies_status({"browser": "signed_out"})
    monkeypatch.setattr(session, "browser_logged_in", lambda: logged_in)

    assert main.youtube_cookies_status()["browser"] == expected


def test_status_does_not_connect_when_browser_disabled(monkeypatch):
    monkeypatch.setattr(session, "CDP_URL", "")

    def unexpected_connect():
        pytest.fail("Manual-cookie mode must not connect to the browser")

    monkeypatch.setattr(session, "browser_logged_in", unexpected_connect)
    assert main.youtube_cookies_status()["browser"] is None


def test_build_cmd_refreshes_cookies_when_browser_enabled(monkeypatch, tmp_path):
    monkeypatch.setattr(session, "CDP_URL", "http://browser:9222")
    downloader.write_netscape(
        "# Netscape HTTP Cookie File\n"
        ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tstale\n"
    )
    calls = []

    def fake_export():
        calls.append(1)
        return (
            "# Netscape HTTP Cookie File\n"
            ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tfresh\n"
        )

    monkeypatch.setattr(session, "export_cookies_from_browser", fake_export)
    downloader.build_cmd("abcdefghijk", tmp_path)
    assert calls == [1]
    assert "fresh" in downloader.cookies_path().read_text()


def test_build_cmd_skips_refresh_without_browser(monkeypatch, tmp_path):
    monkeypatch.setattr(session, "CDP_URL", "")
    downloader.write_netscape(
        "# Netscape HTTP Cookie File\n"
        ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tstale\n"
    )
    downloader.build_cmd("abcdefghijk", tmp_path)
    assert "stale" in downloader.cookies_path().read_text()


def mock_browser(monkeypatch, contexts):
    """A remote browser, without requiring a local Playwright installation."""
    monkeypatch.setattr(session, "CDP_URL", "http://127.0.0.1:9222")
    browser = SimpleNamespace(contexts=contexts)

    @contextmanager
    def sync_playwright():
        yield SimpleNamespace(chromium=SimpleNamespace(connect_over_cdp=lambda *a, **kw: browser))

    package = ModuleType("playwright")
    api = ModuleType("playwright.sync_api")
    api.sync_playwright = sync_playwright
    monkeypatch.setitem(sys.modules, "playwright", package)
    monkeypatch.setitem(sys.modules, "playwright.sync_api", api)


@pytest.mark.parametrize("urls", [
    ["https://accounts.google.com/v3/signin/challenge/pwd"],
    ["https://music.youtube.com/", "https://accounts.google.com/v3/signin/challenge/totp"],
])
def test_keeper_leaves_interactive_sign_in_alone(monkeypatch, urls):
    pages = [SimpleNamespace(url=url, goto=Mock()) for url in urls]
    context = SimpleNamespace(pages=pages, new_page=Mock())
    mock_browser(monkeypatch, [context])

    assert session.touch_session() is False
    context.new_page.assert_not_called()
    for page in pages:
        page.goto.assert_not_called()


def test_keeper_refreshes_music_tab_without_touching_other_tabs(monkeypatch):
    other = SimpleNamespace(url="https://www.google.com/", goto=Mock())
    music = SimpleNamespace(url="https://music.youtube.com/", goto=Mock())
    context = SimpleNamespace(pages=[other, music], new_page=Mock())
    mock_browser(monkeypatch, [context])

    assert session.touch_session() is True
    other.goto.assert_not_called()
    context.new_page.assert_not_called()
    music.goto.assert_called_once_with("https://music.youtube.com/", wait_until="load",
                                       timeout=session._CONNECT_TIMEOUT_MS)


def test_keeper_creates_music_tab_in_persistent_profile(monkeypatch):
    other = SimpleNamespace(url="https://www.youtube.com/", goto=Mock())
    music = SimpleNamespace(url="about:blank", goto=Mock())
    context = SimpleNamespace(pages=[other], new_page=Mock(return_value=music))
    mock_browser(monkeypatch, [context])

    assert session.touch_session() is True
    other.goto.assert_not_called()
    context.new_page.assert_called_once()
    music.goto.assert_called_once()


def test_missing_persistent_context_is_unavailable(monkeypatch):
    mock_browser(monkeypatch, [])
    assert session.touch_session() is False
    assert session.sync_session() == (None, "disconnected")


def test_cookie_read_failure_is_unavailable_not_signed_out(monkeypatch):
    context = SimpleNamespace(cookies=Mock(side_effect=RuntimeError("CDP interrupted")))
    mock_browser(monkeypatch, [context])
    assert session.sync_session() == (None, "disconnected")
    assert session.browser_logged_in() is None


def test_incognito_cookies_do_not_masquerade_as_persistent_sign_in(monkeypatch):
    persistent = SimpleNamespace(cookies=Mock(return_value=[]))
    incognito = SimpleNamespace(cookies=Mock(return_value=CDP_PAYLOAD))
    mock_browser(monkeypatch, [persistent, incognito])
    assert session.sync_session() == (None, "signed_out")
    incognito.cookies.assert_not_called()


def test_authenticated_browser_exports_current_cookies(monkeypatch):
    monkeypatch.setattr(session, "_cdp_cookies", lambda: CDP_PAYLOAD[:3])
    text, state = session.sync_session()
    assert state == "connected"
    assert text == session.export_cookies_from_browser()
    assert "LOGIN_INFO" in text and "SAPISID" in text


def test_anonymous_browser_preserves_last_authenticated_jar(monkeypatch, tmp_path):
    monkeypatch.setattr(session, "CDP_URL", "http://browser:9222")
    original = downloader.cookies_from_cdp(CDP_PAYLOAD[:3])
    downloader.write_netscape(original)
    downloader.write_cookies_status({"verified": True, "lastSyncAt": 123.0})
    anonymous = [{"name": "VISITOR_INFO1_LIVE", "value": "visitor",
                  "domain": ".youtube.com", "path": "/"}]
    monkeypatch.setattr(session, "_cdp_cookies", lambda: anonymous)

    assert main._session_keeper_tick() == "signed_out"
    downloader.build_cmd("abcdefghijk", tmp_path)
    assert downloader.cookies_path().read_text() == original
    status = downloader.read_cookies_status()
    assert status["browser"] == "signed_out"
    assert status["lastSyncAt"] == 123.0


# --- automatic re-login -----------------------------------------------------


class FakeLocator:
    first = None  # set in __init__; the driver only ever uses .first

    def __init__(self, page, selector: str) -> None:
        self.page = page
        self.selector = selector
        type(self).first = self

    def is_visible(self) -> bool:
        return self.page.step in self.page.visible_steps(self.selector)

    def count(self) -> int:
        return 0

    def is_editable(self) -> bool:
        return True

    def fill(self, value: str, timeout=None) -> None:
        self.page.typed.append((self.selector, value))

    def press(self, key: str, timeout=None) -> None:
        self.page.typed.append((f"press:{key}", ""))
        self.page.advance()

    def click(self, timeout=None) -> None:
        self.page.typed.append((self.selector, ""))
        self.page.advance()


class FakePage:
    """Scripted Google sign-in: one step per page, Enter moves to the next."""

    def __init__(self, url: str = "", step: str = "email") -> None:
        self.url = url or session._SIGNIN_URL
        self.step = step
        self.typed: list[tuple[str, str]] = []
        self.closed = False
        self.body_text = "Sign in to continue"

    def goto(self, url, wait_until=None, timeout=None) -> None:
        pass

    def locator(self, selector: str) -> FakeLocator:
        return FakeLocator(self, selector)

    def wait_for_load_state(self, timeout=None) -> None:
        pass

    def inner_text(self, selector: str) -> str:
        texts = {
            "captcha": "Type the text you hear or see",
            "options": "Select the way you want to sign in",
            "selection": "Choose how you want to sign in",
        }
        return texts.get(self.step, self.body_text)

    def close(self) -> None:
        self.closed = True

    def visible_steps(self, selector: str) -> set[str]:
        if "Try another way" in selector:
            return {"captcha", "selection"}
        if ("authenticator" in selector.casefold()
                or "verification code from" in selector):
            return {"options"}
        if "data-identifier" in selector:
            return {"chooser"}
        if "email" in selector:
            return {"email"}
        if "password" in selector:
            return {"password"}
        if "Pin" in selector or "one-time-code" in selector:
            return {"totp"}
        return set()

    def advance(self) -> None:
        following = {"chooser": "password", "email": "password",
                     "captcha": "options", "selection": "options",
                     "options": "totp", "password": "totp", "totp": "done"}
        self.step = following.get(self.step, self.step)
        if self.step in ("password", "options", "totp"):
            self.url = self.url.replace(
                "ServiceLogin", f"v3/signin/challenge/{self.step}")
        elif self.step == "done":
            self.url = "https://music.youtube.com/"
            self.step = "signed_in"


def relogin_context(monkeypatch, page: FakePage, cookies=CDP_PAYLOAD):
    context = SimpleNamespace(
        pages=[], new_page=Mock(return_value=page),
        cookies=Mock(return_value=cookies))
    mock_browser(monkeypatch, [context])
    monkeypatch.setattr(session, "GOOGLE_EMAIL", "me@gmail.com")
    monkeypatch.setattr(session, "GOOGLE_PASSWORD", "hunter2")
    monkeypatch.setattr(session, "_totp_now", lambda: "123456")
    monkeypatch.setattr(session.time, "sleep", lambda s: None)
    return context


def test_relogin_disabled_without_credentials(monkeypatch):
    mock_browser(monkeypatch, [SimpleNamespace(pages=[], new_page=Mock())])
    monkeypatch.setattr(session, "GOOGLE_EMAIL", "")
    monkeypatch.setattr(session, "GOOGLE_PASSWORD", "hunter2")
    assert session.attempt_relogin() == "disabled"


@pytest.mark.parametrize("email, password", [
    ("", "hunter2"), ("me@gmail.com", ""),
])
def test_relogin_disabled_with_partial_credentials(monkeypatch, email, password):
    mock_browser(monkeypatch, [SimpleNamespace(pages=[], new_page=Mock())])
    monkeypatch.setattr(session, "GOOGLE_EMAIL", email)
    monkeypatch.setattr(session, "GOOGLE_PASSWORD", password)
    assert session.attempt_relogin() == "disabled"


def test_relogin_skips_when_human_is_signing_in(monkeypatch):
    page = SimpleNamespace(url="https://accounts.google.com/v3/signin/identifier")
    context = SimpleNamespace(pages=[page], new_page=Mock())
    mock_browser(monkeypatch, [context])
    monkeypatch.setattr(session, "GOOGLE_EMAIL", "me@gmail.com")
    monkeypatch.setattr(session, "GOOGLE_PASSWORD", "hunter2")

    assert session.attempt_relogin() == "busy"
    context.new_page.assert_not_called()


def test_relogin_completes_email_password_and_totp(monkeypatch):
    page = FakePage()
    context = relogin_context(monkeypatch, page)

    assert session.attempt_relogin() == "reconnected"
    assert page.closed is True
    typed = " ".join(sel for sel, _ in page.typed)
    assert 'input[type="email"]' in typed
    assert "password" in typed
    assert "Pin" in typed or "one-time-code" in typed
    values = {v for sel, v in page.typed if v}
    assert values == {"me@gmail.com", "hunter2", "123456"}


def test_relogin_succeeds_through_account_chooser(monkeypatch):
    # Google often remembers the address even after revoking the session:
    # the flow must pick the matching tile, fill the password and TOTP code.
    page = FakePage(step="chooser")
    context = relogin_context(monkeypatch, page)

    assert session.attempt_relogin() == "reconnected"
    tiles = [sel for sel, v in page.typed if "data-identifier" in sel and v == ""]
    assert tiles, "expected the account tile to be clicked"
    assert "password" in " ".join(sel for sel, _ in page.typed)
    assert "hunter2" in {v for _, v in page.typed if v}


def test_relogin_ditches_captcha_for_authenticator(monkeypatch):
    # Google injects a puzzle CAPTCHA on the password page. The driver must
    # never attempt it: switch to "Try another way" → Authenticator → TOTP.
    page = FakePage(step="captcha")
    context = relogin_context(monkeypatch, page)

    assert session.attempt_relogin() == "reconnected"
    kinds = " ".join(sel for sel, _ in page.typed)
    assert "Try another way" in kinds
    assert "Authenticator" in kinds
    # Nothing filled the captcha itself (its input has no known selector
    # here) and nothing was typed but the TOTP code.
    assert {v for _, v in page.typed if v} == {"123456"}


def test_relogin_survives_challenge_selection_page(monkeypatch):
    # Google's "Choose how you want to sign in" page has no inputs at all:
    # the driver must open "Try another way" from there too.
    page = FakePage(step="selection")
    context = relogin_context(monkeypatch, page)

    assert session.attempt_relogin() == "reconnected"
    kinds = " ".join(sel for sel, _ in page.typed)
    assert "Try another way" in kinds and "Authenticator" in kinds


class WrongPasswordPage(FakePage):
    """Google rejects the password and re-renders the same page silently."""

    def advance(self) -> None:
        if self.step == "password":
            return
        super().advance()

    def inner_text(self, selector: str) -> str:
        if self.step == "password":
            return 'Welcome ... Enter your password ... Wrong password. Try again'
        return super().inner_text(selector)


def test_relogin_never_hammers_a_rejected_password(monkeypatch):
    page = WrongPasswordPage()
    relogin_context(monkeypatch, page)

    assert session.attempt_relogin() == "needs_human"
    # Exactly one password attempt was made, then the driver stopped.
    password_fills = [t for t in page.typed if "password" in t[0]
                      and t[0].startswith("input")]
    assert len(password_fills) == 1


def test_relogin_stops_when_captcha_has_no_alternative(monkeypatch):
    page = FakePage(step="captcha")
    relogin_context(monkeypatch, page)
    # Remove the "Try another way" affordance: nothing left to automate.

    import unittest.mock as mock
    real_click = session._click_first

    def no_alt(page_arg, selectors):
        for selector in selectors:
            if "Try another way" in selector:
                return False
        return real_click(page_arg, selectors)

    with mock.patch.object(session, "_click_first", no_alt):
        assert session.attempt_relogin() == "needs_human"


def test_relogin_stops_on_human_only_challenge(monkeypatch):
    page = FakePage(step="phone")
    page.body_text = "Check your phone to verify it's you"
    relogin_context(monkeypatch, page)

    assert session.attempt_relogin() == "needs_human"
    assert page.typed == []


def test_relogin_fails_on_unknown_page(monkeypatch):
    page = FakePage(step="mystery")
    relogin_context(monkeypatch, page)

    assert session.attempt_relogin() == "failed"


def test_relogin_verifies_login_cookies_after_flow(monkeypatch):
    # The script can reach music.youtube.com without real auth cookies; the
    # caller must trust has_login_session, not the URL alone.
    page = FakePage()
    relogin_context(monkeypatch, page, cookies=[
        {"name": "SID", "value": "x", "domain": ".google.com"}])

    assert session.attempt_relogin() == "failed"


def test_totp_code_skipped_without_secret(monkeypatch):
    monkeypatch.setattr(session, "GOOGLE_TOTP_SECRET", "")
    assert session._totp_now() is None


def test_totp_accepts_bare_key_and_vaultwarden_uri(monkeypatch):
    monkeypatch.setattr(session, "GOOGLE_TOTP_SECRET", "JBSWY3DPEHPK3PXP")
    monkeypatch.setattr(session.time, "time", lambda: 0)
    code = session._totp_now()
    assert code and code.isdigit() and len(code) == 6

    # The provisioning URI form Vaultwarden/Bitwarden sometimes stores.
    monkeypatch.setattr(session.time, "time", lambda: 0)
    uri = "otpauth://totp/Google:me%40gmail.com?secret=JBSWY3DPEHPK3PXP&issuer=Google"
    monkeypatch.setattr(session, "GOOGLE_TOTP_SECRET", uri)
    assert session._totp_now() == code


def test_keeper_relogin_featured_in_status(monkeypatch):
    monkeypatch.setattr(session, "CDP_URL", "http://browser:9222")
    monkeypatch.delenv("YTDLP_COOKIES_FILE", raising=False)
    downloader.write_cookies_status({"browser": "signed_out"})
    monkeypatch.setattr(session, "relogin_configured", lambda: True)
    status = main._youtube_status()
    assert status["selfRelogin"] is True
    assert status["lastReloginState"] is None


def test_maybe_relogin_attempts_and_records_when_signed_out(monkeypatch):
    monkeypatch.setenv("YOUTUBE_RELOGIN_MIN_MINUTES", "5")
    downloader.write_cookies_status({"browser": "signed_out"})
    monkeypatch.setattr(session, "relogin_configured", lambda: True)
    events = []
    monkeypatch.setattr(session, "attempt_relogin",
                        lambda: events.append("attempt") or "needs_human")
    monkeypatch.setattr(main, "_session_keeper_tick",
                        lambda: events.append("tick") or "connected")

    main._maybe_relogin()
    assert events == ["attempt"]
    status = downloader.read_cookies_status()
    assert status["lastReloginState"] == "needs_human"
    assert status["lastReloginAt"] > 0


def test_maybe_relogin_tick_and_reverify_after_reconnect(monkeypatch):
    monkeypatch.setenv("YOUTUBE_RELOGIN_MIN_MINUTES", "5")
    downloader.write_cookies_status({"browser": "signed_out"})
    monkeypatch.setattr(session, "relogin_configured", lambda: True)
    monkeypatch.setattr(session, "attempt_relogin", lambda: "reconnected")
    events = []
    monkeypatch.setattr(main, "_session_keeper_tick",
                        lambda: events.append("tick") or "connected")
    monkeypatch.setattr(main, "_reverify_youtube",
                        lambda *, force: events.append(("verify", force)))

    main._maybe_relogin()
    assert events == ["tick", ("verify", True)]


def test_maybe_relogin_respects_throttle(monkeypatch):
    monkeypatch.setenv("YOUTUBE_RELOGIN_MIN_MINUTES", "60")
    downloader.write_cookies_status({
        "browser": "signed_out",
        "lastReloginAt": main.time.time(),
        "lastReloginState": "needs_human",
    })
    monkeypatch.setattr(session, "relogin_configured", lambda: True)
    monkeypatch.setattr(session, "attempt_relogin",
                        lambda: pytest.fail("too soon to retry"))

    main._maybe_relogin()
    assert downloader.read_cookies_status()["lastReloginState"] == "needs_human"


def test_maybe_relogin_skips_when_not_configured(monkeypatch):
    monkeypatch.setattr(session, "relogin_configured", lambda: False)
    monkeypatch.setattr(session, "attempt_relogin",
                        lambda: pytest.fail("must not run without credentials"))

    main._maybe_relogin()
