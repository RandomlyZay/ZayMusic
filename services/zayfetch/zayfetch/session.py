"""Managed browser session (§5.3): the one owner of the Google sign-in.

zayfetch talks to the Chromium container over CDP and exports its cookies.
Nothing else drives the browser, so the session can't thrash (§9).

Only ``playwright.sync_api`` with ``connect_over_cdp`` is used — the browser
lives in its own container, so we never run ``playwright install``. Every
public function is total: an unreachable browser returns None/False and never
raises.
"""

from __future__ import annotations

import logging
import os
import socket
import threading
import time
from urllib.parse import urlsplit, urlunsplit

from . import downloader

log = logging.getLogger("zayfetch.session")

# Empty disables browser mode entirely (stored cookies.txt is used as before).
CDP_URL = os.environ.get("BROWSER_CDP_URL", "").strip()

# Auto re-login credentials. When set, the keeper signs the managed browser
# back in on its own after Google ends the session (password + optional TOTP).
GOOGLE_EMAIL = os.environ.get("GOOGLE_EMAIL", "").strip()
GOOGLE_PASSWORD = os.environ.get("GOOGLE_PASSWORD", "").strip()
GOOGLE_TOTP_SECRET = os.environ.get("GOOGLE_TOTP_SECRET", "").strip()

_CONNECT_TIMEOUT_MS = 15_000
_CONNECT_LOCK = threading.Lock()

_SIGNIN_URL = (
    "https://accounts.google.com/ServiceLogin"
    "?continue=https%3A%2F%2Fmusic.youtube.com%2F&hl=en"
)
_STEP_TIMEOUT_MS = 20_000
_ACTION_TIMEOUT_MS = 5_000
_RELOGIN_TIMEOUT_S = 180.0

# Challenges the driver cannot complete on its own (device prompts, SMS,
# captchas, rejected passwords). Matched against(lower-cased) page text.
_HUMAN_ONLY_TEXT = (
    "unusual traffic",
    "hear or see",  # audio/image CAPTCHA
    "solve this puzzle",
    "verify it's you",
    "2-step verification sent",
    "check your phone",
    "check your notifications",
    "didn't finish",
    "sent to your phone",
    "confirm the code",
    "wrong password",
    "couldn't sign you in",
)


# "Try another way" appears both on the CAPTCHA-adjacent and on the
# challenge-selection pages; it opens the list that contains the
# Authenticator (TOTP) option the driver can complete.
_TRY_OTHER_SELECTORS = (
    'button:has-text("Try another way")',
    '[role="button"]:has-text("Try another way")',
    'div[role="link"]:has-text("Try another way")',
)
_AUTHENTICATOR_SELECTORS = (
    'div[role="link"]:has-text("Authenticator")',
    'button:has-text("Authenticator")',
    'div[role="link"]:has-text("verification code from")',
)


def browser_enabled() -> bool:
    return bool(CDP_URL)


def relogin_configured() -> bool:
    """True when env hands the keeper a full automated sign-in."""
    return bool(GOOGLE_EMAIL and GOOGLE_PASSWORD)


def _resolved_cdp_url() -> str:
    """CDP_URL with its hostname resolved to an IP.

    Chromium's DevTools HTTP server rejects a non-IP ``Host`` header with a
    500, and the websocket URL it hands back is built from that Host. Docker
    gives the browser container an IP, so resolving ``browser`` -> IP keeps
    the compose URL hostname-based while making CDP reachable."""
    try:
        parts = urlsplit(CDP_URL)
    except ValueError:
        return CDP_URL
    host = parts.hostname or ""
    if not host or _is_ip_literal(host):
        return CDP_URL
    try:
        infos = socket.getaddrinfo(host, parts.port, family=socket.AF_INET,
                                   type=socket.SOCK_STREAM)
        ip = infos[0][4][0]
    except OSError as e:
        log.info("could not resolve CDP host %s: %s", host, e)
        return CDP_URL
    netloc = ip if parts.port is None else f"{ip}:{parts.port}"
    return urlunsplit((parts.scheme, netloc, parts.path, parts.query, parts.fragment))


def _is_ip_literal(host: str) -> bool:
    return all(ch.isdigit() or ch == "." for ch in host)


def _domain_matches(domain: str) -> bool:
    domain = (domain or "").lstrip(".").casefold()
    return any(domain == d or domain.endswith("." + d) for d in downloader._COOKIE_DOMAINS)


def _cdp_cookies() -> list[dict] | None:
    """Return the browser's YouTube/Google cookies, or None when unreachable.

    A reachable browser with no matching cookies returns []."""
    if not CDP_URL:
        return None
    try:
        from playwright.sync_api import sync_playwright
    except Exception as e:  # noqa: BLE001 — playwright is optional at runtime
        log.info("playwright unavailable: %s", e)
        return None
    try:
        # Serialize CDP connects so the single browser is never driven twice.
        with _CONNECT_LOCK:
            with sync_playwright() as pw:
                browser = pw.chromium.connect_over_cdp(_resolved_cdp_url(), timeout=_CONNECT_TIMEOUT_MS)
                # Only the persistent profile owns the sign-in. Never mix in
                # cookies from a temporary/incognito context.
                if not browser.contexts:
                    return None
                cookies = browser.contexts[0].cookies()
        return [c for c in cookies if _domain_matches(str(c.get("domain") or ""))]
    except Exception as e:  # noqa: BLE001 — unreachable browser is not an error
        log.info("browser CDP connect failed: %s", e)
        return None


def export_cookies_from_browser() -> str | None:
    """Live cookies from the managed browser as Netscape text.

    Returns None when the browser is disabled, unreachable, or signed out.
    An anonymous browser must not overwrite the last authenticated jar."""
    cookies = _cdp_cookies()
    if not cookies or not downloader.has_login_session(_cookie_names(cookies)):
        return None
    return downloader.cookies_from_cdp(cookies)


def browser_logged_in() -> bool | None:
    """True/False from the exported cookies; None when the browser is unreachable."""
    cookies = _cdp_cookies()
    if cookies is None:
        return None
    return downloader.has_login_session(_cookie_names(cookies))


def _cookie_names(cookies: list[dict]) -> dict[str, str]:
    return {
        str(c.get("name") or ""): str(c.get("value") or "")
        for c in cookies
        if c.get("name")
    }


def sync_session() -> tuple[str | None, str]:
    """One CDP round-trip for the keeper.

    Returns ``(netscape_text | None, state)`` where state is one of
    ``connected`` / ``signed_out`` / ``disconnected``. A single export is used
    for both the stored cookies and the login check."""
    cookies = _cdp_cookies()
    if cookies is None:
        return None, "disconnected"
    if not downloader.has_login_session(_cookie_names(cookies)):
        return None, "signed_out"
    return downloader.cookies_from_cdp(cookies), "connected"


def touch_session() -> bool:
    """Refresh YouTube Music without interrupting an interactive sign-in.

    Regular browser activity lets Google refresh its cookies, but cannot
    prevent server-side revocation. Returns False if unavailable or signing in.
    """
    if not CDP_URL:
        return False
    try:
        from playwright.sync_api import sync_playwright
    except Exception as e:  # noqa: BLE001
        log.info("playwright unavailable: %s", e)
        return False
    try:
        with _CONNECT_LOCK:
            with sync_playwright() as pw:
                browser = pw.chromium.connect_over_cdp(_resolved_cdp_url(), timeout=_CONNECT_TIMEOUT_MS)
                if not browser.contexts:
                    return False
                context = browser.contexts[0]
                # The human may be midway through password/2FA entry. Do not
                # navigate any tab until they have finished the Google flow.
                if any(urlsplit(p.url or "").hostname == "accounts.google.com"
                       for p in context.pages):
                    return False
                page = None
                for candidate in context.pages:
                    if urlsplit(candidate.url or "").hostname == "music.youtube.com":
                        page = candidate
                        break
                if page is None:
                    page = context.new_page()
                page.goto(
                    "https://music.youtube.com/",
                    wait_until="load",
                    timeout=_CONNECT_TIMEOUT_MS,
                )
        return True
    except Exception as e:  # noqa: BLE001
        log.info("browser session touch failed: %s", e)
        return False


def _totp_now() -> str | None:
    """Current TOTP code, or None when pyotp/secret are unavailable."""
    if not GOOGLE_TOTP_SECRET:
        return None
    try:
        import pyotp
    except Exception as e:  # noqa: BLE001 — pyotp is optional at runtime
        log.info("pyotp unavailable: %s", e)
        return None
    try:
        secret = GOOGLE_TOTP_SECRET
        if secret.startswith("otpauth://"):
            # Vaultwarden/Bitwarden may store the whole provisioning URI
            # instead of the bare base32 key — accept both.
            return pyotp.parse_uri(secret).now()
        return pyotp.TOTP(secret).now()
    except Exception as e:  # noqa: BLE001
        log.info("bad TOTP secret: %s", e)
        return None


def _classify_challenge(text: str) -> str:
    lowered = text.casefold()
    if any(marker in lowered for marker in _HUMAN_ONLY_TEXT):
        return "needs_human"
    return "unknown"


def _captcha_up(page) -> bool:
    """True while a puzzle/word CAPTCHA is embedded in the sign-in flow."""
    try:
        if page.locator('iframe[title*="recaptcha"]').count() > 0:
            return True
        return "hear or see" in page.inner_text("body").casefold()
    except Exception:  # noqa: BLE001 — unreadable page is not a captcha
        return False


def _drive_signin(context) -> str:
    """Run the Google sign-in in the managed browser.

    Returns ``reconnected`` / ``needs_human`` / ``failed``. Cookie
    verification happens in the caller. Never raises; page text snippets are
    logged (never the typed credentials)."""
    page = context.new_page()
    try:
        # Acodeless start: the continue parameter lands on YouTube Music on
        # success, which doubles as the success signal.
        page.goto(_SIGNIN_URL, wait_until="load", timeout=_STEP_TIMEOUT_MS)
    except Exception as e:  # noqa: BLE001
        log.info("relogin: could not open the sign-in page: %s", e)
        page.close()
        return "failed"

    # Steps progress kind-by-kind (chooser -> email -> password -> totp).
    # Repeating the same step without any of the others firing means the
    # page is stuck despite auto-waits reporting success.
    kind_history: list[str] = []
    last_kind: str | None = None
    started = time.monotonic()
    idle_steps = 0
    try:
        while time.monotonic() - started < _RELOGIN_TIMEOUT_S:
            if urlsplit(page.url or "").hostname == "music.youtube.com":
                return "reconnected"

            if last_kind == "password":
                # A rejected password re-renders this page silently: the
                # error appears a second or two after submission, so check
                # one loop cycle after the attempt, never re-fill first.
                text = _page_text(page)
                marker = next((m for m in _HUMAN_ONLY_TEXT
                               if m in text.casefold()), None)
                if marker and marker != "hear or see":
                    log.info("relogin stopped: %s (%s)", marker, _snippet(text))
                    return "needs_human"

            # Account chooser: Google remembers the address even after it
            # invalidates the session, so land on it and pick this account
            # (or open the identifier page when it offers no tile).
            matched = False
            kind = None
            if not matched and _captcha_up(page):
                # Google asks for a puzzle/word CAPTCHA. Never try to solve
                # it — switch the challenge to the Authenticator app instead.
                if _click_first(page, _TRY_OTHER_SELECTORS):
                    kind = "altway"
                    matched = True
                else:
                    text = _page_text(page)
                    log.info("relogin stopped: CAPTCHA cannot be automated (%s)",
                             _snippet(text))
                    return "needs_human"
            if not matched and _click_first(page, _AUTHENTICATOR_SELECTORS):
                # The alternative-challenge list: pick the Authenticator app
                # (TOTP), not SMS/passkey prompts.
                kind = "altoption"
                matched = True
            if not matched and _click_first(page, [
                f'div[data-identifier="{GOOGLE_EMAIL.casefold()}"]',
                f'div[data-identifier="{GOOGLE_EMAIL}"]',
                '[aria-label="Use another account"]',
            ]):
                kind = "chooser"
                matched = True
            if not matched and _fill_first(page, [
                ('input[type="email"]', GOOGLE_EMAIL),
            ]):
                kind = "email"
                matched = True
            if not matched and not _captcha_up(page) and _fill_first(page, [
                ('input[type="password"]:not([aria-hidden="true"])', GOOGLE_PASSWORD),
            ]):
                kind = "password"
                matched = True
            if not matched and _fill_first(page, [
                # TOTP input has gone by both names over the years.
                ('input[name="Pin"]', _totp_now()),
                ('input[autocomplete="one-time-code"]', _totp_now()),
            ]):
                kind = "totp"
                matched = True
            if not matched and _click_first(page, _TRY_OTHER_SELECTORS):
                # Google's challenge-selection page ("Choose how you want to
                # sign in") has no inputs: open the alternatives list.
                kind = "altway"
                matched = True

            if not matched:
                idle_steps += 1
                if idle_steps >= 3:
                    text = _page_text(page)
                    outcome = _classify_challenge(text)
                    if outcome == "needs_human":
                        log.info("relogin stopped: challenge needs the human (%s)",
                                 _snippet(text))
                        return "needs_human"
                    log.info("relogin stalled on unknown page at %s (%s)",
                             _path(page.url), _snippet(text))
                    return "failed"
            else:
                idle_steps = 0
                kind_history.append(kind)
                last_kind = kind
                if len(kind_history) > 6:
                    del kind_history[:-6]
                if len(kind_history) >= 5 and len(set(kind_history)) == 1:
                    text = _page_text(page)
                    log.info("relogin stuck repeating '%s' at %s (%s)",
                             kind, _path(page.url), _snippet(text))
                    return _classify_challenge(text) if _classify_challenge(
                        text) == "needs_human" else "failed"
            time.sleep(2)

            try:
                page.wait_for_load_state("load", timeout=2_000)
            except Exception:  # noqa: BLE001 — SPA pages may never settle
                pass

        log.info("relogin timed out after %ss at %s",
                 int(_RELOGIN_TIMEOUT_S), _path(page.url))
        return "failed"
    finally:
        try:
            page.close()
        except Exception:  # noqa: BLE001
            pass


def _path(url: str) -> str:
    try:
        parts = urlsplit(url or "")
        return f"{parts.hostname}{parts.path}"
    except ValueError:
        return ""


def _fill_first(page, selectors: list[tuple[str, str | None]]) -> bool:
    """Fill the first actionable selector that also has a value to type.

    Submit via Enter on the field itself: the pages carry several "Next"
    buttons (passkey promos, challenge lists) and the first text match is not
    necessarily this form's submit. Bounded timeouts keep a misfired match
    from stalling the next steps."""
    for selector, value in selectors:
        if not value:
            continue
        try:
            field = page.locator(selector).first
            if field.is_visible() and field.is_editable():
                field.fill(value, timeout=_ACTION_TIMEOUT_MS)
                field.press("Enter", timeout=_ACTION_TIMEOUT_MS)
                return True
        except Exception:  # noqa: BLE001 — try the next selector
            continue
    return False


def _click_first(page, selectors: list[str]) -> bool:
    for selector in selectors:
        try:
            target = page.locator(selector).first
            if target.is_visible():
                target.click(timeout=_ACTION_TIMEOUT_MS)
                return True
        except Exception:  # noqa: BLE001 — try the next selector
            continue
    return False


def _page_text(page) -> str:
    try:
        return str(page.inner_text("body") or "")
    except Exception as e:  # noqa: BLE001
        log.info("relogin: could not read the page: %s", e)
        return ""


def _snippet(text: str, limit: int = 120) -> str:
    return " ".join(text.split())[:limit]


def attempt_relogin() -> str:
    """Sign the managed browser back into Google on its own.

    Returns one of ``reconnected`` (fresh login cookies verified),
    ``needs_human`` (a challenge only the person can complete),
    ``busy`` (someone is already interacting with accounts.google.com),
    ``failed`` (browser/flow error), ``disabled`` (no credentials)."""
    if not browser_enabled() or not relogin_configured():
        return "disabled"
    try:
        from playwright.sync_api import sync_playwright
    except Exception as e:  # noqa: BLE001
        log.info("playwright unavailable: %s", e)
        return "failed"
    outcome = "failed"
    try:
        with _CONNECT_LOCK:
            with sync_playwright() as pw:
                browser = pw.chromium.connect_over_cdp(
                    _resolved_cdp_url(), timeout=_CONNECT_TIMEOUT_MS)
                if not browser.contexts:
                    log.info("relogin: browser has no persistent context")
                    return "failed"
                context = browser.contexts[0]
                # The human may be midway through an interactive sign-in in
                # noVNC. Never compete for the account: idle out instead.
                if any(urlsplit(p.url or "").hostname == "accounts.google.com"
                       for p in context.pages):
                    log.info("relogin skipped: an interactive sign-in is open")
                    return "busy"
                outcome = _drive_signin(context)
                if outcome == "reconnected":
                    cookies = context.cookies()
                    if not downloader.has_login_session(_cookie_names(
                            [c for c in cookies if _domain_matches(str(c.get("domain") or ""))])):
                        log.info("relogin: flow ended but no login cookies")
                        outcome = "failed"
    except Exception as e:  # noqa: BLE001 — unreachable browser is not fatal
        log.info("relogin attempt failed: %s", e)
        return "failed"
    if outcome == "reconnected":
        log.info("automatic Google sign-in succeeded")
    return outcome
