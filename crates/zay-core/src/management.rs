//! Shared settings, connection and acquisition-management behavior.
use crate::{err, NativeCore, Result};
use serde_json::{json, Value};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

pub(crate) struct OAuthFlow {
    pub value: Value,
    pub expires: Instant,
    pub poll_after: Instant,
}
fn segment(id: &str) -> Result<String> {
    if id.is_empty() {
        return Err(err("Choose an item"));
    }
    Ok(url::form_urlencoded::byte_serialize(id.as_bytes()).collect())
}
#[uniffi::export]
impl NativeCore {
    pub fn preferences(&self) -> Result<String> {
        let remote = self.setting("remoteBrowserUrl".into(), None)?;
        let mut default_remote = self.base.clone();
        let _ = default_remote.set_port(Some(8446));
        default_remote.set_path("/vnc.html");
        default_remote.set_query(None);
        Ok(json!({"server":self.base.as_str().trim_end_matches('/'),"autoplay":self.autoplay.load(Ordering::SeqCst),"volume":self.volume(),"remoteBrowserUrl":if remote.is_empty() || remote == "null"{default_remote.to_string()}else{remote}}).to_string())
    }
    pub fn volume(&self) -> f64 {
        f64::from_bits(self.user_volume.load(Ordering::SeqCst))
    }
    pub fn set_volume(&self, volume: f64) -> Result<()> {
        if !volume.is_finite() {
            return Err(err("Invalid volume"));
        }
        let volume = volume.clamp(0., 1.);
        self.setting("volume".into(), Some(volume.to_string()))?;
        self.user_volume.store(volume.to_bits(), Ordering::SeqCst);
        Ok(())
    }
    pub fn set_preference(&self, key: String, value: String) -> Result<()> {
        if !["remoteBrowserUrl", "server"].contains(&key.as_str()) {
            return Err(err("Unknown preference"));
        }
        let parsed = url::Url::parse(&value).map_err(|_| err("Use an HTTP(S) URL"))?;
        if !["http", "https"].contains(&parsed.scheme())
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(err(
                "Use an HTTP(S) URL without credentials, query or fragment",
            ));
        }
        self.setting(key, Some(value))?;
        Ok(())
    }
    /// Status never includes account identities or saved credentials. OAuth codes are memory-only.
    pub fn management_status(&self) -> Result<String> {
        let poll = {
            let mut flow = self
                .oauth_flow
                .lock()
                .map_err(|_| err("Connection unavailable"))?;
            if flow.as_ref().is_some_and(|f| Instant::now() >= f.expires) {
                *flow = None;
                *self
                    .oauth_error
                    .lock()
                    .map_err(|_| err("Connection unavailable"))? =
                    "The Google code expired. Get a new code.".into();
            }
            flow.as_mut()
                .filter(|f| Instant::now() >= f.poll_after)
                .map(|f| {
                    f.poll_after = Instant::now() + Duration::from_secs(5);
                    f.value.clone()
                })
        };
        if let Some(expected) = poll {
            let result = self.fetch_management("youtube/oauth/poll", "POST", json!({}));
            if result.as_ref().is_err()
                || result
                    .as_ref()
                    .is_ok_and(|v| v["connected"] == true || v["pending"] != true)
            {
                let mut flow = self
                    .oauth_flow
                    .lock()
                    .map_err(|_| err("Connection unavailable"))?;
                if flow.as_ref().is_some_and(|f| f.value == expected) {
                    *flow = None;
                    *self
                        .oauth_error
                        .lock()
                        .map_err(|_| err("Connection unavailable"))? =
                        if result.as_ref().is_ok_and(|v| v["connected"] == true) {
                            String::new()
                        } else {
                            "Google authorization ended. Get a new code.".into()
                        };
                }
            }
        }
        let raw = self.fetch_management("youtube/cookies", "GET", json!({}))?;
        let mut status = json!({});
        for key in [
            "configured",
            "verified",
            "premium",
            "message",
            "checkedAt",
            "updatedAt",
            "browser",
            "lastSyncAt",
            "selfRelogin",
            "lastReloginAt",
            "lastReloginState",
            "expiresAt",
            "expiresInDays",
            "oauth",
        ] {
            status[key] = raw[key].clone();
        }
        let flow = self
            .oauth_flow
            .lock()
            .map_err(|_| err("Connection unavailable"))?
            .as_ref()
            .map(|f| f.value.clone());
        Ok(json!({"preferences":serde_json::from_str::<Value>(&self.preferences()?).map_err(|_|err("Invalid preferences"))?,"youtube":status,"oauthFlow":flow,"oauthError":self.oauth_error.lock().map_err(|_|err("Connection unavailable"))?.clone(),"storage":serde_json::from_str::<Value>(&self.storage_stats()?).map_err(|_|err("Invalid storage totals"))?}).to_string())
    }
    /// Commands contain user input; callers must never log or persist their envelopes.
    pub fn management_command(&self, command: String) -> Result<String> {
        let v: Value =
            serde_json::from_str(&command).map_err(|_| err("Invalid management action"))?;
        let op = v["op"].as_str().unwrap_or("");
        let id = v["id"].as_str().unwrap_or("");
        let result = match op {
            "verifyConnection" => {
                self.fetch_management("youtube/cookies", "GET", json!({"verify":true}))?
            }
            "saveCookies" => {
                let cookies = v["cookies"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| err("Paste cookies.txt contents"))?;
                self.fetch_management("youtube/cookies", "POST", json!({"cookies":cookies}))?
            }
            "disconnectCookies" => self.fetch_management("youtube/cookies", "DELETE", json!({}))?,
            "oauthStart" => {
                self.oauth_error
                    .lock()
                    .map_err(|_| err("Connection unavailable"))?
                    .clear();
                let result = self.fetch_management("youtube/oauth/start", "POST", json!({}))?;
                let url = result["verification_url"]
                    .as_str()
                    .and_then(|s| url::Url::parse(s).ok())
                    .filter(|u| {
                        u.scheme() == "https" && u.username().is_empty() && u.password().is_none()
                    })
                    .ok_or_else(|| err("Invalid verification URL"))?;
                let flow = json!({"user_code":result["user_code"],"verification_url":url.to_string(),"expires_in":result["expires_in"]});
                *self
                    .oauth_flow
                    .lock()
                    .map_err(|_| err("Connection unavailable"))? = Some(OAuthFlow {
                    value: flow,
                    expires: Instant::now()
                        + Duration::from_secs(
                            result["expires_in"].as_u64().unwrap_or(600).min(3600),
                        ),
                    poll_after: Instant::now() + Duration::from_secs(5),
                });
                json!({"started":true})
            }
            "oauthCancel" => {
                *self
                    .oauth_flow
                    .lock()
                    .map_err(|_| err("Connection unavailable"))? = None;
                self.oauth_error
                    .lock()
                    .map_err(|_| err("Connection unavailable"))?
                    .clear();
                json!({"ok":true})
            }
            "oauthDisconnect" => {
                *self
                    .oauth_flow
                    .lock()
                    .map_err(|_| err("Connection unavailable"))? = None;
                self.fetch_management("youtube/oauth", "DELETE", json!({}))?
            }
            "importPlaylist" => {
                self.fetch_management("youtube/imports", "POST", json!({"playlistId":id}))?
            }
            "pullImport" | "syncImport" => self.fetch_management(
                &format!(
                    "youtube/imports/{}/{}",
                    segment(id)?,
                    if op == "pullImport" { "pull" } else { "sync" }
                ),
                "POST",
                json!({}),
            )?,
            "retryJob" if id.starts_with("queue:") => {
                let text = self.setting(format!("acquisition:{id}"), None)?;
                let saved: Value =
                    serde_json::from_str(&text).map_err(|_| err("Invalid saved request"))?;
                if saved["request"].is_object() {
                    return self.queue_acquisition(saved["song"].clone(), saved["request"].clone());
                }
                return self.acquire_song(text);
            }
            "retryJob" | "cancelJob" => self.fetch_management(
                &format!(
                    "jobs/{}/{}",
                    segment(id)?,
                    if op == "retryJob" { "retry" } else { "cancel" }
                ),
                "POST",
                json!({}),
            )?,
            "deleteJob" if id.starts_with("queue:") => {
                self.db
                    .lock()
                    .map_err(|_| err("Catalog unavailable"))?
                    .execute("DELETE FROM kv WHERE key=?1", [format!("acquisition:{id}")])
                    .map_err(|_| err("Cannot dismiss failure"))?;
                json!({"ok":true})
            }
            "deleteJob" => {
                self.fetch_management(&format!("jobs/{}", segment(id)?), "DELETE", json!({}))?
            }
            _ => return Err(err("Unknown management action")),
        };
        Ok(result.to_string())
    }
    /// Counts for the Jobs badge: failed downloads, unsent requests and failed server jobs.
    /// With `refresh`, failed server jobs are re-read; an unreachable server keeps the last count.
    pub fn attention_summary(&self, refresh: bool) -> Result<String> {
        if refresh {
            if let Ok(v) = self.fetch_management("jobs", "GET", json!({"limit":200})) {
                self.record_server_jobs(&v["jobs"]);
            }
        }
        let downloads = serde_json::from_str::<Value>(&self.downloads()?)
            .map(|v| crate::catalog::list(&v).iter().filter(|d| d["state"] == "failed").count())
            .unwrap_or(0);
        let local = crate::catalog::list(
            &serde_json::from_str::<Value>(&self.acquisition_failures()?).unwrap_or_default(),
        )
        .len();
        let server = self.server_job_failures.load(Ordering::SeqCst) as usize;
        Ok(json!({"downloads":downloads,"acquisitions":local,"server":server,"total":downloads+local+server}).to_string())
    }
    pub fn acquisition_failures(&self) -> Result<String> {
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        let mut stmt=db.prepare("SELECT substr(key,13),value FROM kv WHERE key LIKE 'acquisition:%' ORDER BY rowid DESC LIMIT 200").map_err(|_|err("Cannot read failures"))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|_| err("Cannot read failures"))?;
        let values:Vec<_>=rows.filter_map(|r|r.ok()).filter_map(|(id,s)|serde_json::from_str::<Value>(&s).ok().map(|stored|{let song=if stored["song"].is_object(){&stored["song"]}else{&stored};json!({"id":id,"name":song["title"],"status":"failed","stage":"Queue request failed; retry when connected","local":true,"overrides":song})})).collect();
        Ok(json!(values).to_string())
    }
}
/// ZayFetch job states grouped as the Jobs filters show them.
pub fn job_bucket(status: &str) -> &'static str {
    match status {
        "queued" | "pending" | "running" | "downloading" => "active",
        "failed" | "cancelled" | "error" => "attention",
        _ => "done",
    }
}
impl NativeCore {
    pub(crate) fn record_server_jobs(&self, jobs: &Value) {
        let failed = crate::catalog::list(jobs)
            .iter()
            .filter(|j| job_bucket(j["status"].as_str().unwrap_or("")) == "attention")
            .count();
        self.server_job_failures.store(failed as u64, Ordering::SeqCst);
    }
    fn fetch_management(&self, endpoint: &str, method: &str, value: Value) -> Result<Value> {
        serde_json::from_str(&self.api(
            "fetch".into(),
            endpoint.into(),
            method.into(),
            value.to_string(),
        )?)
        .map_err(|_| err("Invalid management response"))
    }
}

impl NativeCore {
    /// Persist the complete acquisition request so retry keeps playlist targets and tag overrides.
    pub(crate) fn queue_acquisition(&self, song: Value, mut request: Value) -> Result<String> {
        if let Some(overrides) = request["overrides"].as_object_mut() {
            overrides.retain(|_, value| !value.is_null());
        }
        let id = song["id"]
            .as_str()
            .ok_or_else(|| err("Invalid acquisition song"))?;
        let key = format!("acquisition:queue:{id}");
        if let Ok(old) = serde_json::from_str::<Value>(&self.setting(key.clone(), None)?) {
            let mut targets = crate::catalog::list(&old["request"]["playlistIds"]);
            for target in crate::catalog::list(&request["playlistIds"]) {
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
            if !targets.is_empty() {
                request["playlistIds"] = json!(targets);
            }
        }
        let result = self.api(
            "fetch".into(),
            "jobs".into(),
            "POST".into(),
            request.to_string(),
        );
        match &result {
            Ok(_) => {
                self.db
                    .lock()
                    .map_err(|_| err("Catalog unavailable"))?
                    .execute("DELETE FROM kv WHERE key=?1", [key])
                    .map_err(|_| err("Cannot clear failure"))?;
            }
            Err(_) => {
                self.setting(
                    key,
                    Some(json!({"song":song,"request":request}).to_string()),
                )?;
            }
        }
        result
    }
}
