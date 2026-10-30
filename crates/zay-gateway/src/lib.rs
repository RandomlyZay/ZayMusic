//! Credential-injecting HTTP gateway between the native clients and the
//! internal services.
//!
//! Native clients speak plain OpenSubsonic (`/rest/*`) and ZayFetch
//! (`/api/fetch/*`) without holding any server credential. The gateway adds
//! Navidrome token auth and the ZayFetch service token on the way upstream,
//! streams media through without buffering, and only lets an allowlist of
//! headers cross in either direction. There is no app-level login: the
//! gateway is published on loopback and reached over the Tailnet only.

// Guards return a ready `Response` as their error; it is built once per
// rejected request, so boxing it would only add noise.
#![allow(clippy::result_large_err)]

mod guard;
mod proxy;

use std::{
    fmt,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Router,
};
use serde_json::json;
use url::Url;

/// Runtime settings. Secrets are never printed: `Debug` redacts them.
#[derive(Clone)]
pub struct Config {
    pub navidrome_url: Url,
    pub navidrome_user: String,
    pub navidrome_pass: String,
    pub zayfetch_url: Url,
    pub zayfetch_token: String,
    pub build_sha: String,
    /// Optional public noVNC address shown by the native settings screens.
    pub remote_browser_url: Option<String>,
}

impl Config {
    /// Reads the same environment keys the web proxy used.
    pub fn from_env() -> Result<Self, String> {
        let var = |key: &str| std::env::var(key).unwrap_or_default();
        let or = |key: &str, fallback: &str| {
            Some(var(key))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| fallback.into())
        };
        let remote = var("PUBLIC_REMOTE_BROWSER_URL");
        let remote_browser_url = public_url(&remote);
        if !remote.trim().is_empty() && remote_browser_url.is_none() {
            eprintln!("zay-gateway: ignoring PUBLIC_REMOTE_BROWSER_URL (not a plain http(s) URL)");
        }
        Ok(Self {
            navidrome_url: base_url(
                "NAVIDROME_URL",
                &or("NAVIDROME_URL", "http://navidrome:4533"),
            )?,
            navidrome_user: or("NAVIDROME_USER", "admin"),
            navidrome_pass: var("NAVIDROME_PASS"),
            zayfetch_url: base_url("ZAYFETCH_URL", &or("ZAYFETCH_URL", "http://zayfetch:8080"))?,
            zayfetch_token: var("ZAYFETCH_TOKEN"),
            build_sha: build_sha(&var("BUILD_SHA")),
            remote_browser_url,
        })
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("navidrome_url", &self.navidrome_url.as_str())
            .field("navidrome_user", &"[REDACTED]")
            .field("navidrome_pass", &"[REDACTED]")
            .field("zayfetch_url", &self.zayfetch_url.as_str())
            .field("zayfetch_token", &"[REDACTED]")
            .field("build_sha", &self.build_sha)
            .field("remote_browser_url", &self.remote_browser_url)
            .finish()
    }
}

fn base_url(key: &str, value: &str) -> Result<Url, String> {
    let url = Url::parse(value.trim_end_matches('/')).map_err(|_| format!("{key} is not a URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.query().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(format!("{key} must be a plain http(s) base URL"));
    }
    Ok(url)
}

fn public_url(value: &str) -> Option<String> {
    let url = Url::parse(value.trim()).ok()?;
    let plain = matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none();
    plain.then(|| url.into())
}

/// `BUILD_SHA` wins unless it is the `dev` placeholder; otherwise the deploy
/// bundle's `app-manifest.json` (baked or mounted) supplies it.
fn build_sha(env: &str) -> String {
    if !env.is_empty() && env != "dev" {
        return env.into();
    }
    for path in ["/app-manifest.json", "app-manifest.json"] {
        let sha = std::fs::read(Path::new(path))
            .ok()
            .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
            .and_then(|manifest| manifest["sha"].as_str().map(str::to_owned))
            .filter(|sha| !sha.is_empty());
        if let Some(sha) = sha {
            return sha;
        }
    }
    if env.is_empty() {
        "dev".into()
    } else {
        env.into()
    }
}

struct Gateway {
    config: Config,
    http: reqwest::Client,
    started: Instant,
}

type State = Arc<Gateway>;

/// Builds the gateway application.
pub fn router(config: Config) -> Router {
    let upstreams = [config.navidrome_url.clone(), config.zayfetch_url.clone()];
    let http = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        // Idle limit between body reads; long playback streams stay open.
        .read_timeout(Duration::from_secs(300))
        .redirect(proxy::redirect_policy(upstreams))
        .build()
        .expect("HTTP client configuration is static");
    let state = Arc::new(Gateway {
        config,
        http,
        started: Instant::now(),
    });
    Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/api/native/config", get(native_config))
        .route("/rest/{*path}", any(proxy::rest))
        .route("/api/fetch/{*path}", any(proxy::fetch))
        .fallback(|| async { error(StatusCode::NOT_FOUND, "Not found") })
        .with_state(state)
}

async fn root() -> Response {
    json_response(
        StatusCode::OK,
        json!({
            "name": "ZayMusic gateway",
            "clients": "native",
            "endpoints": ["/healthz", "/rest/*", "/api/fetch/*", "/api/native/config"],
        }),
    )
}

async fn healthz(axum::extract::State(state): axum::extract::State<State>) -> Response {
    json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "sha": state.config.build_sha,
            "uptimeSec": state.started.elapsed().as_secs(),
        }),
    )
}

async fn native_config(
    axum::extract::State(state): axum::extract::State<State>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(response) = guard::same_origin(&headers) {
        return response;
    }
    json_response(
        StatusCode::OK,
        json!({ "remoteBrowserUrl": state.config.remote_browser_url }),
    )
}

fn json_response(status: StatusCode, body: serde_json::Value) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body.to_string(),
    )
        .into_response()
}

/// Gateway-generated errors never echo upstream URLs, queries or credentials.
fn error(status: StatusCode, message: &str) -> Response {
    json_response(status, json!({ "error": message }))
}
