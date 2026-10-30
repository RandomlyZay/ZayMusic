//! A routing loopback server for shared-core tests. Fake data only; requests are recorded in order.
#![allow(dead_code)]
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use zay_core::NativeCore;

static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: String,
}
impl Request {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
    pub fn route(&self) -> String {
        format!("{} {}", self.method, self.path)
    }
}
pub enum Reply {
    Json(Value),
    Status(u16),
}
pub fn rest(value: Value) -> Reply {
    let mut inner = value;
    inner["status"] = json!("ok");
    Reply::Json(json!({ "subsonic-response": inner }))
}
pub fn fetch(value: Value) -> Reply {
    Reply::Json(value)
}

pub struct Server {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    /// `route` returning `None` answers 404, which core treats as an ordinary server failure.
    pub fn new(route: impl Fn(&Request) -> Option<Reply> + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let stop = Arc::new(AtomicBool::new(false));
        let (captured, done) = (requests.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() || first.is_empty() {
                    continue;
                }
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let mut parts = first.split_whitespace();
                let method = parts.next().unwrap().to_owned();
                let target = parts.next().unwrap();
                let parsed = url::Url::parse(&format!("http://fixture.invalid{target}")).unwrap();
                let mut query: Vec<(String, String)> = parsed
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                let body = String::from_utf8(body).unwrap();
                if parsed.path().starts_with("/rest/") && method == "POST" {
                    // Subsonic POSTs are form bodies; tests read them like query parameters.
                    query.extend(
                        url::form_urlencoded::parse(body.as_bytes())
                            .map(|(k, v)| (k.into_owned(), v.into_owned())),
                    );
                }
                let request = Request {
                    method,
                    path: parsed.path().to_owned(),
                    query,
                    body,
                };
                let reply = route(&request);
                captured.lock().unwrap().push(request);
                let (status, text) = match reply {
                    Some(Reply::Json(v)) => (200, v.to_string()),
                    Some(Reply::Status(code)) => (code, "{}".to_owned()),
                    None => (404, "{}".to_owned()),
                };
                let _ = write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    pub fn routes(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(Request::route)
            .collect()
    }
    pub fn take(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct Root(pub PathBuf);
impl Root {
    pub fn new(tag: &str) -> Self {
        Self(std::env::temp_dir().join(format!(
            "zay-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    pub fn core(&self, url: &str) -> Arc<NativeCore> {
        let core = NativeCore::new(url.into(), self.0.to_string_lossy().into()).unwrap();
        core.set_autoplay(false).unwrap();
        core
    }
    /// Seeds listening history directly, newest `played_at` first in `recent_songs()`.
    pub fn played(&self, songs: &[(&str, i64)]) {
        let db = rusqlite::Connection::open(self.0.join("catalog.sqlite")).unwrap();
        for (id, at) in songs {
            db.execute(
                "INSERT OR REPLACE INTO recent_songs(id,played_at,song) VALUES(?1,?2,?3)",
                rusqlite::params![
                    id,
                    at,
                    json!({"id":id,"title":format!("Recent {id}")}).to_string()
                ],
            )
            .unwrap();
        }
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}
pub fn ids(rows: &Value) -> Vec<String> {
    rows.as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["id"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default()
}
pub fn sections(page: &Value) -> Vec<(String, u64, u64)> {
    page["extra"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["title"].as_str().unwrap().to_owned(),
                s["start"].as_u64().unwrap(),
                s["count"].as_u64().unwrap(),
            )
        })
        .collect()
}
