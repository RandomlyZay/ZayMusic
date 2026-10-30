//! Request guards: proxied path validation and browser cross-site rejection.

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use percent_encoding::percent_decode_str;
use url::Url;

/// Accepts a still-encoded upstream path only when every segment is a plain
/// name, so `..`, `.`, encoded separators and empty segments cannot walk the
/// upstream path tree. One trailing slash is kept for upstream redirects.
pub fn upstream_path(raw: &str) -> Option<&str> {
    let plain = raw
        .strip_suffix('/')
        .unwrap_or(raw)
        .split('/')
        .all(|segment| {
            let decoded: Vec<u8> = percent_decode_str(segment).collect();
            !decoded.is_empty()
                && decoded != b"."
                && decoded != b".."
                && !decoded
                    .iter()
                    .any(|b| matches!(b, b'/' | b'\\') || b.is_ascii_control())
        });
    plain.then_some(raw)
}

/// Native HTTP clients send no `Origin`, or their own server origin on writes.
/// Browsers attach `Origin`/`Sec-Fetch-Site`; any page other than this server
/// is refused so a visited site cannot drive library or download requests.
pub fn same_origin(headers: &HeaderMap) -> Result<(), Response> {
    if let Some(site) = headers.get("sec-fetch-site") {
        if matches!(site.as_bytes(), b"cross-site" | b"same-site") {
            return Err(forbidden());
        }
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let origin = origin
        .to_str()
        .ok()
        .and_then(|o| Url::parse(o).ok())
        .ok_or_else(forbidden)?;
    if origin.host().is_none() {
        return Err(forbidden());
    }
    // Tailscale Serve keeps the public Host; X-Forwarded-Host covers other
    // reverse proxies. Browsers cannot forge either cross-origin without a
    // preflight, which the gateway never grants.
    let served = [header::HOST.as_str(), "x-forwarded-host"]
        .into_iter()
        .any(|name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .is_some_and(|host| same_authority(&origin, host.trim()))
        });
    if served {
        Ok(())
    } else {
        Err(forbidden())
    }
}

fn same_authority(origin: &Url, host: &str) -> bool {
    Url::parse(&format!("{}://{host}", origin.scheme())).is_ok_and(|served| {
        served.username().is_empty()
            && served.host() == origin.host()
            && served.port_or_known_default() == origin.port_or_known_default()
            && served.path() == "/"
    })
}

fn forbidden() -> Response {
    crate::error(StatusCode::FORBIDDEN, "Cross-site request refused")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_reject_traversal_and_encoded_separators() {
        for ok in [
            "ping",
            "getCoverArt.view",
            "preview/abc-_~",
            "jobs/42",
            "jobs/",
            "a%20b",
        ] {
            assert_eq!(upstream_path(ok), Some(ok), "{ok}");
        }
        for bad in [
            "", "..", ".", "a/../b", "%2e%2E/x", "a%2fb", "a%5Cb", "a\\b", "a//b", "/", "a//",
            "%00", "x/%2e",
        ] {
            assert_eq!(upstream_path(bad), None, "{bad}");
        }
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(k, v)| (header::HeaderName::from_static(k), v.parse().unwrap()))
            .collect()
    }

    #[test]
    fn origin_must_match_served_host() {
        assert!(same_origin(&headers(&[])).is_ok());
        assert!(same_origin(&headers(&[
            ("host", "127.0.0.1:3847"),
            ("origin", "http://127.0.0.1:3847")
        ]))
        .is_ok());
        assert!(same_origin(&headers(&[
            ("host", "box.ts.net"),
            ("origin", "https://box.ts.net")
        ]))
        .is_ok());
        assert!(same_origin(&headers(&[
            ("host", "box.ts.net:443"),
            ("origin", "https://box.ts.net")
        ]))
        .is_ok());
        assert!(same_origin(&headers(&[
            ("host", "127.0.0.1:3847"),
            ("x-forwarded-host", "box.ts.net:8445"),
            ("origin", "https://box.ts.net:8445"),
        ]))
        .is_ok());
        assert!(same_origin(&headers(&[
            ("host", "box.ts.net"),
            ("origin", "https://evil.example")
        ]))
        .is_err());
        assert!(same_origin(&headers(&[
            ("host", "box.ts.net:8445"),
            ("origin", "https://box.ts.net")
        ]))
        .is_err());
        assert!(same_origin(&headers(&[("host", "box.ts.net"), ("origin", "null")])).is_err());
        assert!(same_origin(&headers(&[("origin", "https://box.ts.net")])).is_err());
        assert!(same_origin(&headers(&[("sec-fetch-site", "cross-site")])).is_err());
        assert!(same_origin(&headers(&[("sec-fetch-site", "same-origin")])).is_ok());
    }
}
