//! Gateway behaviour against in-process fake upstreams. Only fake credentials.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use md5::{Digest, Md5};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use url::{form_urlencoded, Url};
use zay_gateway::Config;

const USER: &str = "fake-user";
const PASS: &str = "fake-pass";
const TOKEN: &str = "fake-token";

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    query: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

fn audio() -> Vec<u8> {
    (0..=255u8).cycle().take(4096).collect()
}

async fn fake_upstream(State(log): State<Log>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, usize::MAX).await.unwrap().to_vec();
    log.lock().unwrap().push(Seen {
        method: parts.method.to_string(),
        path: parts.uri.path().into(),
        query: parts.uri.query().unwrap_or_default().into(),
        headers: parts.headers.clone(),
        body,
    });
    let subsonic_ok = || {
        (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::SET_COOKIE, "nd=session"),
                (header::SERVER, "navidrome"),
            ],
            [("x-internal", "secret-ish")],
            r#"{"subsonic-response":{"status":"ok"}}"#,
        )
            .into_response()
    };
    match parts.uri.path() {
        "/rest/ping" | "/rest/updatePlaylist" => subsonic_ok(),
        "/rest/stream" | "/rest/stream.view" => {
            let audio = audio();
            let range = parts
                .headers
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok());
            let (status, start, end) = match range.and_then(|r| r.strip_prefix("bytes=")) {
                Some(spec) => {
                    let (a, b) = spec.split_once('-').unwrap();
                    (
                        StatusCode::PARTIAL_CONTENT,
                        a.parse().unwrap(),
                        b.parse::<usize>().unwrap(),
                    )
                }
                None => (StatusCode::OK, 0, audio.len() - 1),
            };
            let mut response = Response::new(Body::from(audio[start..=end].to_vec()));
            *response.status_mut() = status;
            let h = response.headers_mut();
            h.insert(header::CONTENT_TYPE, "audio/ogg".parse().unwrap());
            h.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
            h.insert(header::ETAG, "\"song-1\"".parse().unwrap());
            h.insert(header::CONTENT_LENGTH, (end - start + 1).into());
            if status == StatusCode::PARTIAL_CONTENT {
                h.insert(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{}", audio.len())
                        .parse()
                        .unwrap(),
                );
            }
            response
        }
        // Deliberately ignores Accept-Encoding: the bytes must still arrive intact.
        "/rest/compressed" => (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CONTENT_ENCODING, "gzip"),
            ],
            vec![0x1f, 0x8b, 0x08, 0x00, 1, 2, 3, 4, 5],
        )
            .into_response(),
        "/rest/redirect" => (
            StatusCode::FOUND,
            [(header::LOCATION, "http://127.0.0.2:9/elsewhere")],
        )
            .into_response(),
        "/jobs/" => (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, "/jobs")],
        )
            .into_response(),
        "/jobs" | "/youtube/cookies" | "/radio" => (
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"ok":true}"#,
        )
            .into_response(),
        "/fail" => (StatusCode::INTERNAL_SERVER_ERROR, r#"{"detail":"boom"}"#).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

fn config(navidrome: &str, zayfetch: &str) -> Config {
    Config {
        navidrome_url: Url::parse(navidrome).unwrap(),
        navidrome_user: USER.into(),
        navidrome_pass: PASS.into(),
        zayfetch_url: Url::parse(zayfetch).unwrap(),
        zayfetch_token: TOKEN.into(),
        build_sha: "test-sha".into(),
        remote_browser_url: Some("https://box.example:8446/vnc.html".into()),
    }
}

/// Gateway in front of one fake upstream serving both services.
async fn stack() -> (String, Log) {
    let log = Log::default();
    let upstream = serve(
        Router::new()
            .fallback(any(fake_upstream))
            .with_state(log.clone()),
    )
    .await;
    (
        serve(zay_gateway::router(config(&upstream, &upstream))).await,
        log,
    )
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn pairs(raw: &str) -> Vec<(String, String)> {
    form_urlencoded::parse(raw.as_bytes())
        .into_owned()
        .collect()
}

fn value<'a>(pairs: &'a [(String, String)], key: &str) -> Vec<&'a str> {
    pairs
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .collect()
}

fn assert_server_auth(query: &str) {
    let q = pairs(query);
    assert_eq!(value(&q, "u"), [USER]);
    assert_eq!(value(&q, "v"), ["1.16.1"]);
    assert_eq!(value(&q, "c"), ["ZayMusic"]);
    assert_eq!(value(&q, "f"), ["json"]);
    let salt = value(&q, "s");
    assert_eq!(salt.len(), 1);
    assert_eq!(salt[0].len(), 32);
    let expected: String = Md5::digest(format!("{PASS}{}", salt[0]))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(value(&q, "t"), [expected.as_str()]);
    for forbidden in ["p", "apiKey", "jwt"] {
        assert!(
            value(&q, forbidden).is_empty(),
            "{forbidden} leaked upstream"
        );
    }
}

async fn json(request: reqwest::RequestBuilder) -> serde_json::Value {
    serde_json::from_slice(&request.send().await.unwrap().bytes().await.unwrap()).unwrap()
}

/// Sends a request line verbatim so path normalisation in HTTP clients
/// cannot hide traversal attempts.
async fn raw_status(base: &str, method: &str, path: &str) -> u16 {
    let address = base.trim_start_matches("http://");
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request =
        format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response.split(' ').nth(1).unwrap().parse().unwrap()
}

#[tokio::test]
async fn rest_injects_server_auth_and_drops_client_auth() {
    let (gateway, log) = stack().await;
    let url = format!(
        "{gateway}/rest/ping?u=evil&s=x&t=y&p=enc:00&apiKey=k&jwt=j&v=1.0&c=evil&f=xml&id=1&id=2&q=a%20b"
    );
    let response = client().get(url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let seen = log.lock().unwrap()[0].clone();
    assert_eq!(seen.path, "/rest/ping");
    let q = pairs(&seen.query);
    assert_eq!(value(&q, "id"), ["1", "2"]);
    assert_eq!(value(&q, "q"), ["a b"]);
    assert!(
        seen.query.starts_with("id=1&id=2&q=a%20b&u="),
        "client order kept: {}",
        seen.query
    );
    assert_server_auth(&seen.query);
}

#[tokio::test]
async fn rest_post_form_keeps_repeated_fields_and_strips_auth() {
    let (gateway, log) = stack().await;
    let form = "playlistId=pl1&u=evil&songIdToAdd=a&songIdToAdd=b%2Bc&t=forged&songIndexToRemove=0&songIndexToRemove=3";
    let response = client()
        .post(format!("{gateway}/rest/updatePlaylist"))
        .header(
            header::CONTENT_TYPE,
            "application/x-www-form-urlencoded; charset=utf-8",
        )
        .header(header::ORIGIN, gateway.clone())
        .body(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let seen = log.lock().unwrap()[0].clone();
    assert_eq!(seen.method, "POST");
    assert_eq!(
        String::from_utf8(seen.body).unwrap(),
        "playlistId=pl1&songIdToAdd=a&songIdToAdd=b%2Bc&songIndexToRemove=0&songIndexToRemove=3"
    );
    assert_eq!(
        seen.headers[header::CONTENT_TYPE],
        "application/x-www-form-urlencoded; charset=utf-8"
    );
    assert_server_auth(&seen.query);
}

#[tokio::test]
async fn fetch_injects_service_token_and_relays_query_and_body() {
    let (gateway, log) = stack().await;
    let http = client();
    let get = http
        .get(format!(
            "{gateway}/api/fetch/radio?seed=a&seed=b&x-zay-token=q"
        ))
        .header("x-zay-token", "client-forged")
        .header(header::AUTHORIZATION, "Bearer client")
        .header(header::COOKIE, "c=1")
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 200);
    assert!(get.headers().get("x-zay-token").is_none());
    let post = http
        .post(format!("{gateway}/api/fetch/youtube/cookies"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(r#"{"cookies":"fake"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), 200);
    let delete = http
        .delete(format!("{gateway}/api/fetch/youtube/cookies"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete.status(), 200);

    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    for request in &seen {
        assert_eq!(
            request
                .headers
                .get_all("x-zay-token")
                .iter()
                .collect::<Vec<_>>(),
            [TOKEN]
        );
        assert!(request.headers.get(header::AUTHORIZATION).is_none());
        assert!(request.headers.get(header::COOKIE).is_none());
    }
    assert_eq!(
        (seen[0].path.as_str(), seen[0].query.as_str()),
        ("/radio", "seed=a&seed=b&x-zay-token=q")
    );
    assert_eq!(
        (seen[1].method.as_str(), seen[1].body.as_slice()),
        ("POST", br#"{"cookies":"fake"}"#.as_slice())
    );
    assert_eq!(seen[1].headers[header::CONTENT_TYPE], "application/json");
    assert_eq!(
        (seen[2].method.as_str(), seen[2].path.as_str()),
        ("DELETE", "/youtube/cookies")
    );
}

#[tokio::test]
async fn ranges_and_head_are_relayed_with_media_caching() {
    let (gateway, log) = stack().await;
    let http = client();
    let partial = http
        .get(format!("{gateway}/rest/stream?id=song-1"))
        .header(header::RANGE, "bytes=10-19")
        .header(header::IF_RANGE, "\"song-1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(partial.status(), 206);
    let h = partial.headers().clone();
    assert_eq!(h[header::CONTENT_RANGE], "bytes 10-19/4096");
    assert_eq!(h[header::CONTENT_LENGTH], "10");
    assert_eq!(h[header::ACCEPT_RANGES], "bytes");
    assert_eq!(h[header::ETAG], "\"song-1\"");
    assert_eq!(h[header::CACHE_CONTROL], "private, max-age=3600");
    assert_eq!(partial.bytes().await.unwrap(), audio()[10..20]);

    let head = http
        .head(format!("{gateway}/rest/stream.view?id=song-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()[header::CONTENT_LENGTH], "4096");
    assert_eq!(
        head.headers()[header::CACHE_CONTROL],
        "private, max-age=3600"
    );
    assert!(head.bytes().await.unwrap().is_empty());

    let seen = log.lock().unwrap().clone();
    assert_eq!(seen[0].headers[header::RANGE], "bytes=10-19");
    assert_eq!(seen[0].headers[header::IF_RANGE], "\"song-1\"");
    assert_eq!(
        seen[1].method, "HEAD",
        "HEAD must not fetch the audio upstream"
    );
}

#[tokio::test]
async fn media_streams_through_without_buffering() {
    // Raw upstream that sends half the body, then waits for the test.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 4096];
        let _ = socket.read(&mut buffer).await.unwrap();
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: audio/ogg\r\ncontent-length: 8\r\n\r\nABCD",
            )
            .await
            .unwrap();
        released.await.unwrap();
        socket.write_all(b"EFGH").await.unwrap();
    });
    let gateway = serve(zay_gateway::router(config(&upstream, &upstream))).await;
    let mut response = client()
        .get(format!("{gateway}/rest/stream?id=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "8");
    let first = tokio::time::timeout(Duration::from_secs(5), response.chunk())
        .await
        .expect("first chunk relayed early");
    assert_eq!(first.unwrap().unwrap(), "ABCD");
    release.send(()).unwrap();
    let mut rest = Vec::new();
    while let Some(chunk) = response.chunk().await.unwrap() {
        rest.extend_from_slice(&chunk);
    }
    assert_eq!(rest, b"EFGH");
}

#[tokio::test]
async fn compressed_bodies_keep_matching_encoding_and_length() {
    let (gateway, log) = stack().await;
    let response = client()
        .get(format!("{gateway}/rest/compressed"))
        .header(header::ACCEPT_ENCODING, "gzip, br")
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "9");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        [0x1f, 0x8b, 0x08, 0x00, 1, 2, 3, 4, 5]
    );
    assert_eq!(
        log.lock().unwrap()[0].headers[header::ACCEPT_ENCODING],
        "identity"
    );
}

#[tokio::test]
async fn only_allowlisted_response_headers_are_returned() {
    let (gateway, _) = stack().await;
    let response = client()
        .get(format!("{gateway}/rest/ping"))
        .send()
        .await
        .unwrap();
    let h = response.headers();
    for hidden in [
        "set-cookie",
        "server",
        "x-internal",
        "access-control-allow-origin",
    ] {
        assert!(h.get(hidden).is_none(), "{hidden} relayed");
    }
    assert_eq!(h[header::CONTENT_TYPE], "application/json");
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.text().await.unwrap(),
        r#"{"subsonic-response":{"status":"ok"}}"#
    );
}

#[tokio::test]
async fn upstream_failures_are_reported_without_internals() {
    let (gateway, log) = stack().await;
    let http = client();
    let failed = http
        .get(format!("{gateway}/api/fetch/fail"))
        .send()
        .await
        .unwrap();
    assert_eq!(failed.status(), 500);
    assert_eq!(failed.text().await.unwrap(), r#"{"detail":"boom"}"#);

    let refused = http
        .get(format!("{gateway}/rest/redirect"))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 502);
    assert!(refused.headers().get(header::LOCATION).is_none());
    let followed = http
        .get(format!("{gateway}/api/fetch/jobs/"))
        .send()
        .await
        .unwrap();
    assert_eq!(followed.status(), 200);
    assert_eq!(
        log.lock().unwrap().last().unwrap().headers["x-zay-token"],
        TOKEN
    );

    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let gateway = serve(zay_gateway::router(config(&dead, &dead))).await;
    for path in ["rest/ping", "api/fetch/jobs"] {
        let down = http.get(format!("{gateway}/{path}")).send().await.unwrap();
        assert_eq!(down.status(), 502);
        let body = down.text().await.unwrap();
        assert_eq!(body, r#"{"error":"Upstream unavailable"}"#);
        for secret in [PASS, TOKEN, USER, dead.as_str()] {
            assert!(!body.contains(secret));
        }
    }
}

#[tokio::test]
async fn traversal_and_malformed_paths_never_reach_upstream() {
    let (gateway, log) = stack().await;
    for path in [
        "/rest/../healthz",
        "/rest/..%2fhealthz",
        "/rest/%2e%2e/%2e%2e/etc",
        "/api/fetch/../rest/ping",
        "/api/fetch/jobs/%2E%2E/x",
        "/api/fetch/a%2Fb",
        "/api/fetch/a%5Cb",
        "/api/fetch//jobs",
        "/api/fetch/jobs/./1",
        "/api/fetch/a%00b",
    ] {
        assert_eq!(raw_status(&gateway, "GET", path).await, 400, "{path}");
    }
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cross_site_browser_requests_are_refused() {
    let (gateway, log) = stack().await;
    let http = client();
    let evil = http
        .post(format!("{gateway}/rest/updatePlaylist"))
        .header(header::ORIGIN, "https://evil.example")
        .body("playlistId=1")
        .send()
        .await
        .unwrap();
    assert_eq!(evil.status(), 403);
    let sneaky = http
        .get(format!("{gateway}/api/fetch/jobs"))
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(sneaky.status(), 403);
    let config = http
        .get(format!("{gateway}/api/native/config"))
        .header(header::ORIGIN, "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(config.status(), 403);
    assert!(log.lock().unwrap().is_empty());

    // Native clients send their own server origin on writes, or none at all.
    for origin in [Some(gateway.as_str()), None] {
        let mut request = http.post(format!("{gateway}/api/fetch/jobs")).body("{}");
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        assert_eq!(request.send().await.unwrap().status(), 200);
    }
}

#[tokio::test]
async fn gateway_endpoints_and_unknown_routes() {
    let (gateway, log) = stack().await;
    let http = client();
    let health = json(http.get(format!("{gateway}/healthz"))).await;
    assert_eq!(
        (health["ok"].as_bool(), health["sha"].as_str()),
        (Some(true), Some("test-sha"))
    );
    assert!(health["uptimeSec"].is_u64());

    let root = http.get(format!("{gateway}/")).send().await.unwrap();
    assert_eq!(root.status(), 200);
    assert!(root.text().await.unwrap().contains("/rest/*"));
    assert_eq!(
        http.head(format!("{gateway}/"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    let native = json(http.get(format!("{gateway}/api/native/config"))).await;
    assert_eq!(
        native,
        serde_json::json!({"remoteBrowserUrl": "https://box.example:8446/vnc.html"})
    );

    for (method, path, status) in [
        ("GET", "/settings", 404),
        ("GET", "/rest", 404),
        ("GET", "/api/fetch", 404),
        ("PUT", "/rest/ping", 405),
        ("DELETE", "/rest/ping", 405),
        ("OPTIONS", "/api/fetch/jobs", 405),
    ] {
        assert_eq!(
            raw_status(&gateway, method, path).await,
            status,
            "{method} {path}"
        );
    }
    assert!(log.lock().unwrap().is_empty());
}
