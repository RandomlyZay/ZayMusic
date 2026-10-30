> Native migration scope: the web client is retired at the owner’s request.
> Web-specific implementation references below are historical; the behavior
> reference is `git show 93f43ec:apps/web/...`. Current clients are Linux and iPhone;
> their API gateway is `crates/zay-gateway`. Authentication ownership remains as
> documented here. See `docs/native/DECISIONS.md` for current decisions.

# Implementation Plan — Self-sustaining Google auth (server browser + OAuth)

> **For the implementing agent:** this file is the spec. Everything below is verified
> against the current tree and the pinned dependencies. Follow the steps in order.
> "Verification" (§8) is mandatory before reporting completion. No secrets in the repo.

---

## 1. Problem

The user's Google sign-in dies and must be manually re-pasted.

**What is actually expiring:** the YouTube/Google session cookies stored at
`$DATA_DIR/cookies/youtube.txt` (host `./zayfetch/data/cookies/youtube.txt`, dev
`./dev-zayfetch/cookies/youtube.txt`), written by the Settings → YouTube Music paste
flow and consumed by both yt-dlp and ytmusicapi.

**Why it expires — this is not a TTL, it is cookie rotation.** yt-dlp says so
explicitly at `yt_dlp/extractor/youtube/_base.py:820-827` (pinned `yt-dlp==2026.8.19`):

> "The provided YouTube account cookies are no longer valid. They have likely been
> rotated in the browser as a security measure."

Google rotates the session cookies when the *browser that owns the session* keeps
using it — and especially when the same session appears from a second client/IP.
A pasted `cookies.txt` is therefore a **snapshot that the user's own browser keeps
invalidating**. That is why it "works for a while, then expires".

**Why it cannot be fixed with OAuth alone.** Verified in the pinned yt-dlp source:

| Path | Status |
|---|---|
| yt-dlp OAuth / email+password | **Dead.** `_base.py:690-694`: `Login with OAuth is no longer supported`, and `Login with password is not supported for YouTube`. Cookies are the only supported auth for media. |
| ytmusicapi OAuth (own GCP client) | **Works, self-renewing** (`RefreshingToken` + a real `refresh_token`). But **API-only** (search/library/radio). Upstream docs: "these OAuth credentials do not work for uploads". Premium audio downloads still need web session cookies. |
| Google session cookies | The only credential that unlocks Premium audio. Google issues no server-renewable equivalent for playback. |

**The fix:** give the Google login exactly one owner — a real Chromium that lives on
the server — and have zayfetch read cookies from that live session immediately before
each download instead of from a static copy. Plus OAuth so the API layer never depends
on cookies at all.

---

## 2. Goal

1. **Zero manual re-paste, indefinitely.** One interactive Google sign-in in a
   persistent remote browser; after that the session is self-sustaining.
2. **API layer cookie-free** via OAuth refresh tokens.
3. **Graceful degradation**: the manual paste flow keeps working as a fallback, and
   public playback keeps working when nothing is configured.

**Non-goals:** multi-account support, moving auth into the web container, changing the
download/PO-token pipeline, reviving app-level login (deleted in `fb8e10d`).

---

## 3. Architecture (after)

| Layer | Today | After |
|---|---|---|
| Downloads (yt-dlp, Premium audio) | static `cookies.txt` snapshot | live Chromium exports cookies via CDP **seconds before** each yt-dlp call |
| Search / radio / library (ytmusicapi) | cookie headers via `discovery._browser_auth()` | **OAuth refresh token**; cookies become fallback |
| Session longevity | dies on Google rotation | the browser *is* the session, so exported values are always current |
| Manual paste | only path | kept under "Advanced" in Settings |

New components:

```
services/browser/                    Chromium + Xvfb + noVNC   (new compose service "browser")
services/zayfetch/zayfetch/session.py   (new) CDP cookie export + session keeper loop
zayfetch OAuth endpoints + token store
Settings UI: managed-session row + OAuth panel
```

---

## 4. Verified code facts (read these before changing anything)

### 4.1 zayfetch

`services/zayfetch/zayfetch/downloader.py`

- `cookies_path()` `:65-73` → `$DATA_DIR/cookies/youtube.txt` (`YTDLP_COOKIES_FILE` overrides)
- `cookies_status_path()` `:77-78` → sibling `youtube.status.json`
- `write_cookies(text)` `:85-97` — atomic write (`.tmp` → `chmod 600` → `os.replace`)
- `normalize_cookies(text)` `:106-140` — accepts Netscape / "Copy as cURL" / raw `Cookie:`
- `_AUTH_COOKIE_RE` `:100-102` — sign-in cookie name matcher
- `read_cookies_dict()` `:232-257` — name→value for youtube/google/googlevideo
- `read_cookies_status()` / `write_cookies_status()` `:200-218`
- `ytdlp_cmd(*args, with_cookies=True)` `:260-290` — adds `--cookies`, `web_music`
  client, PO-token provider, `--impersonate chrome`
- `build_cmd(...)` `:293+`

`services/zayfetch/zayfetch/main.py`

- `ensure_worker()` / `ensure_reconciler()` `:455-465`, `_startup()` `:466-477`
- `GET/POST/DELETE /youtube/cookies` `:515-568`; `_youtube_status()` `:515-533`
- `_require_youtube()` `:584-593` (412 gate)
- `_run_yt_probe()` `:1272-1286`, `_cookies_authenticated()` `:1293-1313`,
  `_probe_youtube_auth()` `:1316-1336`

`services/zayfetch/zayfetch/discovery.py`

- `_browser_auth()` `:248-283` — SAPISIDHASH headers built from `read_cookies_dict()`
- `ytmusic_client()` `:271-284`, `ytmusic_call()` `:287-302` (anonymous fallback)

### 4.2 Web

- `apps/web/src/routes/settings/+page.svelte` `:66-148` — cookie form + status row
- `apps/web/src/routes/library/downloads/+page.svelte` `:42-46` — connect/warn banner
- `apps/web/src/lib/api/subsonic.ts` `:348-357` — `youtubeStatus` / `saveYouTubeCookies` /
  `disconnectYouTube`
- `apps/web/src/lib/api/types.ts:209` — `export interface YouTubeStatus`

### 4.3 Deploy

- `docker-compose.yml` — services `navidrome`, `zayfetch`, `pot-provider`, `zaymusic`;
  network `zay`; `zaymusic` publishes `127.0.0.1:${APP_PORT:-3847}:3000`
- `services/zayfetch/pyproject.toml` — deps incl. `yt-dlp[default]==2026.8.19`,
  `curl_cffi`, `ytmusicapi>=1.9`, `httpx>=0.28`
- `dev.env.example`, `.env.example`, `scripts/gen-secrets.sh`

### 4.4 Pinned yt-dlp internals (reference only — do not vendor)

- `_has_auth_cookies` = `LOGIN_INFO` **and** one of `SAPISID` / `__Secure-1PSAPISID` /
  `__Secure-3PSAPISID` (`_base.py:808-813`) — **mirror this for login detection**
- `YoutubeDL.close()` → `save_cookies()` writes the `--cookies` file back
  (`yt_dlp/YoutubeDL.py:1054-1065`); the CLI wraps `with YoutubeDL(...)`
  (`yt_dlp/__init__.py:982`), so every yt-dlp run already refreshes the jar
- `yt_dlp.cookies.YoutubeDLCookieJar` (`cookies.py:1276-1420`) handles the
  `#HttpOnly_` prefix and `expires=0` session cookies — **use it** for jar I/O

### 4.5 ytmusicapi OAuth surface (`ytmusicapi/auth/oauth/`)

- `OAuthCredentials(client_id, client_secret).get_code() -> AuthCodeDict`
  (`device_code`, `user_code`, `verification_url`, `expires_in`, `interval`)
- `token_from_code(device_code) -> RefreshableTokenDict` (`access_token`,
  `refresh_token`, `expires_in`, `scope`, `token_type`)
- `refresh_token(refresh_token) -> BaseTokenDict`
- `YTMusic("oauth.json", oauth_credentials=OAuthCredentials(id, secret))`;
  `RefreshingToken` auto-renews when the access token is ≤1 min from expiry

---

## 5. Implementation

### 5.1 New `browser` service — `services/browser/`

**`services/browser/Dockerfile`** — Debian slim + `chromium` + `xvfb` + `x11vnc` +
`novnc` + `websockify` + `procps`. No custom Python. An entrypoint script starts, in
order: `Xvfb :99` → `x11vnc` → `websockify 6080 novnc` → Chromium.

Chromium flags (all required):

```
--user-data-dir=/data/profile
--remote-debugging-port=9222
--remote-debugging-address=0.0.0.0
--no-first-run --no-default-browser-check --no-sandbox
--disable-dev-shm-usage --disable-gpu
https://music.youtube.com
```

`--no-sandbox` is mandatory in Docker; `--disable-dev-shm-usage` prevents crashes from
the default 64 MB `/dev/shm`.

**`services/browser/entrypoint.sh`** — plain shell, `set -euo pipefail`, no leaked
background processes. Chromium must stay in the foreground (PID 1) so container
restarts are clean.

**`docker-compose.yml`** — add:

```yaml
  browser:
    build:
      context: ./services/browser
    container_name: zaymusic-browser
    restart: unless-stopped
    volumes:
      - ${BROWSER_DATA_DIR:-./browser/data}:/data:z
    ports:
      - "127.0.0.1:${BROWSER_PORT:-6080}:6080"   # LOOPBACK ONLY (noVNC). Tailscale-only.
    expose: ["9222"]                              # CDP, internal network only
    networks: [zay]
```

And on `zayfetch`: add `BROWSER_CDP_URL: http://browser:9222` and change
`depends_on: [navidrome, pot-provider]` to `depends_on: [navidrome, pot-provider, browser]`.

**`.gitignore`** — add `browser/data/`.

**`dev.env.example` / `.env.example`** — add `BROWSER_DATA_DIR` and `BROWSER_PORT`
(optional, with the defaults above).

### 5.2 Cookie jar helpers — `services/zayfetch/zayfetch/downloader.py`

Add, alongside the existing cookie functions:

```python
def load_cookiejar(path: Path | None = None) -> "YoutubeDLCookieJar": ...
def cookiejar_to_netscape(jar) -> str: ...
def write_netscape(body: str) -> Path: ...          # reuse the atomic 600 write
def cookies_from_cdp(payload: list[dict]) -> str: ...
    # [{name, value, domain, path, expires, httpOnly, secure}] -> Netscape text
def has_login_session(cookies: Mapping[str, str]) -> bool:
    """Mirror yt-dlp's _has_auth_cookies: LOGIN_INFO + one of
    SAPISID / __Secure-1PSAPISID / __Secure-3PSAPISID."""
def session_expiry_info() -> dict:
    """{'expiresAt': epoch|None, 'expiresInDays': float|None} from the soonest
    expiring auth cookie in the stored file."""
def oauth_path() -> Path: ...        # $DATA_DIR/youtube/oauth.json
def oauth_configured() -> bool: ...
```

Use `yt_dlp.cookies.YoutubeDLCookieJar` for read/write so `#HttpOnly_` lines and
`expires=0` session cookies survive round-trips. **Do not change** `write_cookies()` or
`normalize_cookies()` — the manual paste flow must not regress.

### 5.3 Session keeper — `services/zayfetch/zayfetch/session.py` (new)

```python
CDP_URL = os.environ.get("BROWSER_CDP_URL", "").strip()   # "" disables browser mode

def browser_enabled() -> bool: ...

def export_cookies_from_browser() -> str | None:
    """playwright.sync_api chromium.connect_over_cdp(CDP_URL) -> context.cookies()
    -> filter domains youtube.com/google.com/googlevideo.com -> Netscape text.
    Returns None when the browser is unreachable."""

def browser_logged_in() -> bool | None:
    """True/False from has_login_session() over exported cookies; None when unreachable."""

def touch_session() -> None:
    """Navigate the remote browser to https://music.youtube.com/ — this is what keeps
    Google from expiring/rotating the session. Cheap; no scraping."""
```

Add `playwright` to `services/zayfetch/pyproject.toml` **and** to the fallback pip list
in `services/zayfetch/Dockerfile` (the existing `|| pip install ...` line). We only use
`connect_over_cdp` — never `playwright install` (the browser is remote).

**Background loop** in `main.py`, next to `ensure_worker`/`ensure_reconciler`:

```python
_session_keeper_started = False

def ensure_session_keeper() -> None:
    """Idempotent; exactly one loop. Skips entirely when BROWSER_CDP_URL is unset."""
```

Loop body (every `YOUTUBE_SESSION_TOUCH_MINUTES`, default 30, `0` disables):
1. `touch_session()`
2. `export_cookies_from_browser()` → `downloader.write_netscape(...)` (atomic 600)
3. write `youtube.status.json` fields `browser: connected|signed_out|disconnected`,
   `lastSyncAt: time.time()`
4. back off to 120 min after consecutive failures; **never** raise out of the loop.

Call `ensure_session_keeper()` from `_startup()` (`main.py:466-477`) after
`_reprobe_youtube_status()`.

**Freshness guarantee:** call `session.export_cookies_from_browser()` +
`downloader.write_netscape(...)` immediately before every
`ytdlp_cmd(..., with_cookies=True)` invocation — i.e. inside `downloader.build_cmd()`
(`:293`) or at its call sites in `main.py`/`importer.py`. yt-dlp's own write-back of the
cookie file on exit is harmless: we overwrite before the next call. When
`browser_enabled()` is False, skip and use the stored file exactly as today.

### 5.4 OAuth — ytmusicapi device flow

**Env** (user-supplied; *not* generated by `scripts/gen-secrets.sh`):
`YTM_OAUTH_CLIENT_ID`, `YTM_OAUTH_CLIENT_SECRET`. Add as empty/optional entries to
`dev.env.example`, `.env.example`, and a comment block in `scripts/gen-secrets.sh`.

**Token store:** `$DATA_DIR/youtube/oauth.json`, written mode 600 via the same
temp/`os.replace` pattern as `write_cookies`.

**Endpoints** in `main.py`, beside the cookie routes (`:515-568`), all gated by
`dependencies=[Depends(require_token)]`:

| Route | Behaviour |
|---|---|
| `POST /youtube/oauth/start` | `OAuthCredentials(...).get_code()` → persist `device_code` → `{user_code, verification_url, expires_in}` |
| `POST /youtube/oauth/poll` | `token_from_code(device_code)` → write `oauth.json` → `{connected: true}`; map `authorization_pending` to `{connected: false, pending: true}`; 400 on expired code or missing client config |
| `DELETE /youtube/oauth` | unlink `oauth.json` + the device-code cache |
| `GET /youtube/cookies` | extend `_youtube_status()` (`:515-533`) with `oauth: {configured, connected}` |

**Client preference** — rewrite `discovery.ytmusic_client()` (`:271-284`):

1. `YTMusic(downloader.oauth_path(), oauth_credentials=OAuthCredentials(id, secret))`
   when `oauth_configured()`
2. existing `_browser_auth()` cookie headers
3. anonymous

Leave `ytmusic_call()`'s anonymous fallback (`:287-302`) untouched. Wrap construction in
try/except so a revoked refresh token degrades to (2)/(3) instead of raising.

### 5.5 Status shape

`_youtube_status()` (`main.py:515-533`) becomes:

```python
{
  "configured": bool, "verified": bool, "premium": bool, "message": str,
  "checkedAt": float | None, "updatedAt": float | None,
  "browser": "connected" | "signed_out" | "disconnected" | None,
  "lastSyncAt": float | None,
  "expiresAt": float | None, "expiresInDays": float | None,
  "oauth": {"configured": bool, "connected": bool},
}
```

Mirror the new fields in `apps/web/src/lib/api/types.ts` (`YouTubeStatus`, `:209`).

### 5.6 Web UI — `apps/web/src/routes/settings/+page.svelte`

- **Managed session row** (replaces the static status block at `:100-113`):
  - `browser === 'connected'` → "Signed in · session kept alive automatically ·
    last synced {relative}"
  - `browser === 'signed_out'` → warn: "Browser session ended — open the remote
    browser and sign in again", plus an **Open remote browser** link. Do not hardcode
    a hostname; put the URL in the help text as `http://<server>:6080/vnc.html`.
  - `browser` null / `disconnected` → current paste-based wording
- **Google API (OAuth)** panel: Connect (renders `user_code` + `verification_url`,
  polls `POST /youtube/oauth/poll` every ~5 s until `expires_in` elapses) / status /
  Disconnect.
- Move the existing paste form (`:66-89`) under
  `<details><summary>Advanced — manual cookies</summary>`.
- Update the copy at `:124-128`: "One-time setup; the sign-in lasts months." →
  "Sign in once in the app's browser — the session is kept alive for you."
- `apps/web/src/routes/library/downloads/+page.svelte:42-46`: add a case for
  `youtube.browser === 'signed_out'` → "Browser session ended — re-open the remote
  browser" linking to `Settings#youtube`.

Add `zayfetch.oauthStart` / `oauthPoll` / `oauthDisconnect` wrappers next to the existing
YouTube wrappers in `apps/web/src/lib/api/subsonic.ts:348-357`.

---

## 6. Degradation rules (must hold)

| Condition | Behaviour |
|---|---|
| `BROWSER_CDP_URL` unset | no session keeper; stored `youtube.txt` used as today |
| browser unreachable | keeper backs off; stored file still used; `browser: disconnected` |
| browser signed out | `browser: signed_out` + banner; downloads still attempt with stored cookies |
| OAuth env unset | `ytmusic_client()` skips straight to cookie auth |
| OAuth token revoked | catch, fall through to cookies then anonymous |
| no cookies at all | existing `_require_youtube()` 412 + `COOKIES_REQUIRED_MESSAGE` (`downloader.py:56-59`) |
| authenticated attempt fails | existing `with_cookies=False` retry (`downloader.py:260-290`) keeps public playback alive |

---

## 7. Tests

`services/zayfetch/tests/` (pytest, `asyncio_mode = "auto"`; follow the
`isolated_data_dir` fixture pattern at `test_youtube_auth.py:18-21`).

**`test_session.py`** (new)

- CDP cookie payload → Netscape text → `YoutubeDLCookieJar` round-trip, including
  `#HttpOnly_` names and `expires=0` session cookies
- `has_login_session()`: true for `LOGIN_INFO`+`SAPISID`; false without `LOGIN_INFO`;
  false for the existing `NETSCAPE` fixture at `test_youtube_auth.py:12-17` (it has
  `SID`/`HSID` but no `LOGIN_INFO`/`SAPISID`) — **extend that fixture** with a
  `LOGIN_INFO` line rather than weakening the check
- `session_expiry_info()` maths against a fixed clock
- `ensure_session_keeper()` starts exactly once when called twice
- `export_cookies_from_browser()` returns `None` when `BROWSER_CDP_URL` is unreachable
- freshness: `build_cmd()` refreshes `youtube.txt` when browser mode is on (monkeypatch
  the export function; assert content/mtime changed)

**`test_youtube_auth.py`** (extend)

- OAuth `start`/`poll`/`delete` with `OAuthCredentials` mocked; `poll` while pending →
  `{connected: false, pending: true}`; successful `poll` writes `oauth.json` mode 600
- `_youtube_status()` includes the new `browser`/`oauth`/`expiresInDays` fields
- `ytmusic_client()` preference order: OAuth > cookies > anonymous (monkeypatch
  `YTMusic` and assert which constructor was used)

**e2e** — `apps/web/e2e/smoke.spec.ts` (Settings/YouTube area `:421-477`)

- Settings renders the managed-session row and the OAuth panel
- the `Advanced — manual cookies` details element still contains the paste form
- the existing assertion at `:400-410` (`/api/session` etc. are 404) must stay green

---

## 8. Verification (mandatory)

1. `cd services/zayfetch && pytest` — full suite green. Record the count in your
   summary, the way `NOTES.md` does.
2. Typecheck the web app (`npx svelte-check` / whatever `apps/web/package.json`
   defines) and run its vitest suite.
3. Rebuild the reviewed images per `AGENTS.md`:
   ```sh
   docker compose -f docker-compose.yml -f docker-compose.dev.yml --env-file dev.env up -d --build
   ```
   The reviewed app is the Docker one on `127.0.0.1:3847`, **not** the Vite dev server.
4. Live checks (mirror the style of `NOTES.md:46-53`):
   - `GET http://127.0.0.1:8081/healthz` has no new failure modes
   - `GET /youtube/cookies` through the web proxy reports `browser: connected` and an
     `oauth` block
   - noVNC loads on `127.0.0.1:6080`; after a one-time Google sign-in the keeper logs a
     successful sync and `youtube.txt` changes
   - one **gated** preview and one **public** preview both return 200; one job completes
   - stop the `browser` container → the app degrades per §6, and recovers after restart
5. Installed PWA: note in your summary that a close/reopen is needed to pick up the
   updated service worker and assets.

---

## 9. Gotchas

- **GCP OAuth consent screen** must be *Published*, or in *Testing* with the user's
  Google account added as a **test user** — otherwise the device flow silently fails
  with `access_denied`. Put this in the Settings help text.
- The OAuth client type must be **"TVs and Limited Input devices"** and the
  **YouTube Data API** must be enabled on the project (upstream `ytmusicapi` docs:
  `setup/oauth`).
- `--no-sandbox` + `--disable-dev-shm-usage` are required for Chromium in Docker.
- `./browser/data` holds a live Google session — treat it as a credential store. Keep it
  gitignored and out of the deploy bundle, like `./zayfetch/data`.
- `ND_SESSIONTIMEOUT: 720h` at `docker-compose.yml:15` is Navidrome's UI session and is
  unrelated — do not "fix" it.
- Never log cookie values, token contents, or the OAuth `device_code` at info level.
- Exactly one component (`session.py`) may drive the browser. Do not add a second CDP
  client that also navigates, or the session will thrash.
- `apps/web/e2e/smoke.spec.ts:400-410` asserts the deleted app-auth endpoints stay 404.
  This plan does **not** reintroduce app login; keep that test green.

---

## 10. Out of scope

- Multi-account / multiple Google profiles (yt-dlp always authenticates as the browser's
  default account; see `NOTES.md:52`).
- Restoring the deleted app-password login (`fb8e10d`).
- Changing the PO-token / `web_music` / SABR pipeline (`NOTES.md:46-53`).
- A pure HTTP "cookie keep-alive" refresher as a substitute for the browser — it cannot
  survive Google's rotation, which is the root cause.
