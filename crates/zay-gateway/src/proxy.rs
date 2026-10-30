//! Upstream forwarding: `/rest/*` to Navidrome and `/api/fetch/*` to ZayFetch.

use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::Response,
};
use md5::{Digest, Md5};
use url::{form_urlencoded, Url};

use crate::{error, guard};

/// Client-supplied auth is untrusted; the gateway is the only authenticator.
const AUTH_FIELDS: &[&str] = &["u", "s", "t", "p", "v", "c", "f", "apiKey", "jwt"];
const STREAM_ENDPOINTS: &[&str] = &["stream", "download", "getCoverArt", "hls", "hls.m3u8"];
const REST_METHODS: &[Method] = &[Method::GET, Method::HEAD, Method::POST];
const FETCH_METHODS: &[Method] = &[Method::GET, Method::HEAD, Method::POST, Method::DELETE];
const REST_REQUEST_HEADERS: &[HeaderName] = &[
    header::RANGE,
    header::IF_RANGE,
    header::IF_NONE_MATCH,
    header::IF_MODIFIED_SINCE,
    header::ACCEPT,
    header::USER_AGENT,
    header::CONTENT_TYPE,
];
const FETCH_REQUEST_HEADERS: &[HeaderName] = &[
    header::RANGE,
    header::IF_RANGE,
    header::ACCEPT,
    header::CONTENT_TYPE,
];
const RESPONSE_HEADERS: &[HeaderName] = &[
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_ENCODING,
    header::CONTENT_RANGE,
    header::ACCEPT_RANGES,
    header::CACHE_CONTROL,
    header::ETAG,
    header::LAST_MODIFIED,
    header::CONTENT_DISPOSITION,
];
/// Playlist mutations are small forms; cookie pastes are the largest fetch bodies.
const REST_BODY_LIMIT: usize = 4 << 20;
const FETCH_BODY_LIMIT: usize = 16 << 20;
const MAX_REDIRECTS: usize = 5;

struct Incoming {
    method: Method,
    path: String,
    query: String,
    headers: HeaderMap,
    body: Bytes,
}

pub async fn rest(State(gw): State<crate::State>, request: Request) -> Response {
    let incoming = match accept(request, "/rest/", REST_METHODS, REST_BODY_LIMIT).await {
        Ok(incoming) => incoming,
        Err(response) => return response,
    };
    let endpoint = incoming.path.split('/').next().unwrap_or_default();
    let streaming = STREAM_ENDPOINTS.contains(&endpoint.strip_suffix(".view").unwrap_or(endpoint));

    let config = &gw.config;
    let mut query = without_auth(incoming.query.as_bytes());
    query.push(subsonic_auth(&config.navidrome_user, &config.navidrome_pass).into_bytes());
    let mut url = join(&config.navidrome_url, &format!("rest/{}", incoming.path));
    url.set_query(Some(&String::from_utf8_lossy(&query.join(&b'&'))));

    // Navidrome reads form bodies as parameters, so they are filtered too.
    let body = if is_form(&incoming.headers) {
        Bytes::from(without_auth(&incoming.body).join(&b'&'))
    } else {
        incoming.body
    };
    let headers = allowed(&incoming.headers, REST_REQUEST_HEADERS);
    let upstream = match send(&gw, "navidrome", &incoming.method, url, headers, body).await {
        Ok(upstream) => upstream,
        Err(response) => return response,
    };
    let mut response = relay(upstream, &incoming.method);
    if !response.headers().contains_key(header::CACHE_CONTROL) {
        let policy = if streaming {
            "private, max-age=3600"
        } else {
            "no-store"
        };
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    }
    response
}

pub async fn fetch(State(gw): State<crate::State>, request: Request) -> Response {
    let incoming = match accept(request, "/api/fetch/", FETCH_METHODS, FETCH_BODY_LIMIT).await {
        Ok(incoming) => incoming,
        Err(response) => return response,
    };
    let mut url = join(&gw.config.zayfetch_url, &incoming.path);
    url.set_query(
        Some(&incoming.query)
            .filter(|q| !q.is_empty())
            .map(String::as_str),
    );

    let mut headers = allowed(&incoming.headers, FETCH_REQUEST_HEADERS);
    if !gw.config.zayfetch_token.is_empty() {
        let Ok(mut token) = HeaderValue::from_str(&gw.config.zayfetch_token) else {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Gateway token is misconfigured",
            );
        };
        token.set_sensitive(true);
        headers.insert("x-zay-token", token);
    }
    match send(
        &gw,
        "zayfetch",
        &incoming.method,
        url,
        headers,
        incoming.body,
    )
    .await
    {
        Ok(upstream) => relay(upstream, &incoming.method),
        Err(response) => response,
    }
}

/// Applies the method, browser-origin and path guards before reading a body.
async fn accept(
    request: Request,
    prefix: &str,
    methods: &[Method],
    limit: usize,
) -> Result<Incoming, Response> {
    let (parts, body) = request.into_parts();
    if !methods.contains(&parts.method) {
        return Err(error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed"));
    }
    guard::same_origin(&parts.headers)?;
    let path = parts
        .uri
        .path()
        .strip_prefix(prefix)
        .and_then(guard::upstream_path)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Invalid API path"))?
        .to_owned();
    let body = if has_body(&parts.method) {
        to_bytes(body, limit).await.map_err(|_| {
            error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body too large or unreadable",
            )
        })?
    } else {
        Bytes::new()
    };
    Ok(Incoming {
        method: parts.method,
        path,
        query: parts.uri.query().unwrap_or_default().to_owned(),
        headers: parts.headers,
        body,
    })
}

async fn send(
    gw: &crate::Gateway,
    service: &str,
    method: &Method,
    url: Url,
    mut headers: HeaderMap,
    body: Bytes,
) -> Result<reqwest::Response, Response> {
    // Never let upstream compress: the relayed bytes, Content-Encoding and
    // Content-Length must describe the same body (the old Node fetch decoded
    // gzip but kept the compressed length, truncating JSON).
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    let mut request = gw.http.request(method.clone(), url).headers(headers);
    if has_body(method) {
        request = request.body(body);
    }
    request.send().await.map_err(|failure| {
        // reqwest errors carry the credentialed URL; log only the class.
        let class = if failure.is_timeout() {
            "timeout"
        } else if failure.is_connect() {
            "connect"
        } else if failure.is_redirect() {
            "redirect refused"
        } else {
            "request"
        };
        eprintln!("zay-gateway: {service} upstream {class} failure");
        if failure.is_timeout() {
            error(StatusCode::GATEWAY_TIMEOUT, "Upstream timed out")
        } else {
            error(StatusCode::BAD_GATEWAY, "Upstream unavailable")
        }
    })
}

/// Streams the upstream body chunk by chunk; audio is never buffered.
fn relay(upstream: reqwest::Response, method: &Method) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = upstream.status();
    *response.headers_mut() = allowed(upstream.headers(), RESPONSE_HEADERS);
    if method != Method::HEAD {
        *response.body_mut() = Body::from_stream(upstream.bytes_stream());
    }
    response
}

/// Follows only same-origin upstream redirects (e.g. trailing-slash fixes)
/// so credentials and tokens never travel to another host.
pub fn redirect_policy(upstreams: [Url; 2]) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let same = upstreams
            .iter()
            .any(|base| base.origin() == attempt.url().origin());
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many upstream redirects")
        } else if same {
            attempt.follow()
        } else {
            attempt.error("cross-origin upstream redirect")
        }
    })
}

fn allowed(source: &HeaderMap, names: &[HeaderName]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in names {
        for value in source.get_all(name) {
            headers.append(name.clone(), value.clone());
        }
    }
    headers
}

fn has_body(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD)
}

fn is_form(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| {
            v.trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
}

/// Drops auth pairs while keeping every other pair, its order, repeats and
/// original encoding (playlist calls repeat `songIdToAdd` and friends).
fn without_auth(raw: &[u8]) -> Vec<Vec<u8>> {
    raw.split(|b| *b == b'&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            form_urlencoded::parse(pair)
                .next()
                .is_none_or(|(key, _)| !AUTH_FIELDS.contains(&&*key))
        })
        .map(<[u8]>::to_vec)
        .collect()
}

fn join(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    url.set_path(&format!("{}/{path}", base.path().trim_end_matches('/')));
    url
}

/// Subsonic token auth: `t = md5(password + salt)` with a fresh random salt.
fn subsonic_auth(user: &str, password: &str) -> String {
    let salt = hex(&rand::random::<[u8; 16]>());
    let token = hex(&Md5::digest(format!("{password}{salt}")));
    form_urlencoded::Serializer::new(String::new())
        .append_pair("u", user)
        .append_pair("s", &salt)
        .append_pair("t", &token)
        .append_pair("v", "1.16.1")
        .append_pair("c", "ZayMusic")
        .append_pair("f", "json")
        .finish()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_fields_are_dropped_and_others_kept_in_order() {
        let kept = without_auth(
            b"u=x&id=1&%75=y&songIdToAdd=a&t=&songIdToAdd=b&p=enc:1&apiKey=k&q=a+b&&f=xml",
        );
        assert_eq!(kept.join(&b'&'), b"id=1&songIdToAdd=a&songIdToAdd=b&q=a+b");
    }

    #[test]
    fn join_keeps_base_prefix() {
        let base = Url::parse("http://zf:8080/prefix").unwrap();
        assert_eq!(
            join(&base, "jobs/1").as_str(),
            "http://zf:8080/prefix/jobs/1"
        );
        let root = Url::parse("http://nd:4533").unwrap();
        assert_eq!(
            join(&root, "rest/ping").as_str(),
            "http://nd:4533/rest/ping"
        );
    }
}
