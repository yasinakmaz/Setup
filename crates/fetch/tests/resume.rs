//! Download tests against a real local HTTP/1.1 server driven by `ureq`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use inst_fetch::cache::{Cache, ItemInfo};
use inst_fetch::download::{DownloadError, DownloadObserver, NoopObserver, Progress, SourceFailure};
use inst_fetch::hash::ContentHash;
use inst_fetch::transport::{ProxySetting, TransportConfig, UreqTransport};
use inst_fetch::{DownloadRequest, Downloader, RetryPolicy};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Behavior {
    /// Normal server with Range + If-Range support.
    Good,
    /// First request: send `n` bytes then drop the connection.
    DropAfter(usize),
    /// Ignores Range; always 200 with the full body.
    NoRange,
    /// Different bytes with the same size.
    Tampered,
    NotFound,
    /// 503 for the first `n` requests.
    Busy(usize),
    /// ETag changes on every request.
    ChangingEtag,
}

#[derive(Default)]
struct State {
    requests: Vec<(String, Option<String>, Option<String>)>, // path, Range, If-Range
    per_path: HashMap<String, usize>,
}

struct Server {
    base: String,
    state: Arc<Mutex<State>>,
}

fn serve(data: Arc<Vec<u8>>, routes: HashMap<&'static str, Behavior>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let state = Arc::new(Mutex::new(State::default()));
    let st = state.clone();
    let routes = Arc::new(routes);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let data = data.clone();
            let st = st.clone();
            let routes = routes.clone();
            std::thread::spawn(move || handle(stream, &data, &st, &routes));
        }
    });
    Server { base, state }
}

fn handle(mut stream: TcpStream, data: &[u8], st: &Mutex<State>, routes: &HashMap<&'static str, Behavior>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut range = None;
    let mut if_range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        let (k, v) = line.split_once(':').unwrap_or(("", ""));
        match k.to_ascii_lowercase().as_str() {
            "range" => range = Some(v.trim().to_owned()),
            "if-range" => if_range = Some(v.trim().to_owned()),
            _ => {}
        }
    }
    let nth = {
        let mut s = st.lock().expect("lock");
        s.requests.push((path.clone(), range.clone(), if_range.clone()));
        let c = s.per_path.entry(path.clone()).or_default();
        *c += 1;
        *c
    };
    let behavior = routes.get(path.as_str()).copied().unwrap_or(Behavior::NotFound);
    let etag = match behavior {
        Behavior::ChangingEtag => format!("\"v{nth}\""),
        _ => "\"v1\"".to_owned(),
    };
    let body: Vec<u8> = match behavior {
        Behavior::Tampered => data.iter().map(|b| b ^ 1).collect(),
        _ => data.to_vec(),
    };
    let write_resp = |stream: &mut TcpStream, status: &str, headers: &str, body: &[u8]| {
        let _ = write!(stream, "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n", body.len());
        let _ = stream.write_all(body);
    };
    match behavior {
        Behavior::NotFound => return write_resp(&mut stream, "404 Not Found", "", b""),
        Behavior::Busy(n) if nth <= n => return write_resp(&mut stream, "503 Service Unavailable", "", b""),
        _ => {}
    }
    let start = range
        .as_deref()
        .filter(|_| behavior != Behavior::NoRange)
        .filter(|_| if_range.as_deref().is_none_or(|v| v == etag))
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok());
    match start {
        Some(start) if start < body.len() => {
            let headers = format!(
                "ETag: {etag}\r\nContent-Range: bytes {start}-{}/{}\r\n",
                body.len() - 1,
                body.len()
            );
            write_resp(&mut stream, "206 Partial Content", &headers, &body[start..]);
        }
        Some(_) => write_resp(&mut stream, "416 Range Not Satisfiable", "", b""),
        None => {
            let headers = format!("ETag: {etag}\r\nAccept-Ranges: bytes\r\n");
            if let Behavior::DropAfter(n) = behavior
                && nth == 1
            {
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{headers}\r\n", body.len());
                let _ = stream.write_all(&body[..n]);
                let _ = stream.flush();
                return; // drop mid-body
            }
            write_resp(&mut stream, "200 OK", &headers, &body);
        }
    }
}

fn data() -> Arc<Vec<u8>> {
    Arc::new((0..3_000_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect())
}

fn sha256(data: &[u8]) -> ContentHash {
    let mut h = ContentHash::Sha256([0; 32]).hasher();
    h.update(data);
    h.finalize()
}

fn transport() -> UreqTransport {
    UreqTransport::new(&TransportConfig {
        allow_insecure_http: true,
        proxy: ProxySetting::None,
        body_budget: Duration::from_secs(10),
        ..TransportConfig::default()
    })
    .expect("transport")
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        attempts_per_source: 3,
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
    }
}

struct Recorder(Vec<Progress>);
impl DownloadObserver for Recorder {
    fn progress(&mut self, p: Progress) {
        self.0.push(p);
    }
}

#[test]
fn resumes_after_connection_drop_without_restarting() {
    let data = data();
    let server = serve(data.clone(), [("/drop", Behavior::DropAfter(1_000_000))].into());
    let tmp = tempfile::tempdir().expect("tmp");
    let url = format!("{}/drop", server.base);
    let sources = [url.as_str()];
    let req = DownloadRequest {
        sources: &sources,
        expected_hash: sha256(&data),
        expected_size: data.len() as u64,
    };
    let t = transport();
    let mut d = Downloader::new(&t, fast_retry());
    let dest = tmp.path().join("out.bin");
    d.download(&req, &tmp.path().join("partial"), &dest, &mut NoopObserver).expect("download");
    assert_eq!(std::fs::read(&dest).expect("read"), *data);
    let reqs = &server.state.lock().expect("lock").requests;
    assert_eq!(reqs.len(), 2, "{reqs:?}");
    assert_eq!(reqs[1].1.as_deref(), Some("bytes=1000000-"), "second request must resume");
    assert_eq!(reqs[1].2.as_deref(), Some("\"v1\""), "must send If-Range");
    // Partial files are gone after atomic completion.
    assert_eq!(std::fs::read_dir(tmp.path().join("partial")).expect("ls").count(), 0);
}

#[test]
fn resumes_across_processes_from_partial_file() {
    let data = data();
    let server = serve(data.clone(), [("/f", Behavior::Good)].into());
    let tmp = tempfile::tempdir().expect("tmp");
    let url = format!("{}/f", server.base);
    let sources = [url.as_str()];
    let hash = sha256(&data);
    let req = DownloadRequest {
        sources: &sources,
        expected_hash: hash,
        expected_size: data.len() as u64,
    };
    // Simulate an earlier run that downloaded 40% (no metadata validator).
    let partial = tmp.path().join("partial");
    std::fs::create_dir_all(&partial).expect("mkdir");
    let (part, _meta) = inst_fetch::download::partial_paths(&partial, &hash);
    // Without matching metadata the partial must be discarded, not trusted.
    std::fs::write(&part, &data[..1_200_000]).expect("write");
    let t = transport();
    let mut d = Downloader::new(&t, fast_retry());
    let mut rec = Recorder(Vec::new());
    d.download(&req, &partial, &tmp.path().join("a"), &mut rec).expect("download");
    assert_eq!(rec.0.first().map(|p| p.resumed_from), Some(0));

    // Now a proper interrupted state: run with a dropping server first.
    let server2 = serve(data.clone(), [("/drop", Behavior::DropAfter(2_000_000))].into());
    let url2 = format!("{}/drop", server2.base);
    let sources2 = [url2.as_str()];
    let req2 = DownloadRequest { sources: &sources2, ..req };
    let one_shot = RetryPolicy {
        attempts_per_source: 1,
        ..fast_retry()
    };
    // A downloader that gives up immediately after the drop.
    struct CancelAfterDrop(bool);
    impl DownloadObserver for CancelAfterDrop {
        fn progress(&mut self, p: Progress) {
            if p.downloaded >= 2_000_000 {
                self.0 = true;
            }
        }
        fn cancelled(&self) -> bool {
            self.0
        }
    }
    let mut d1 = Downloader::new(&t, one_shot);
    let err = d1
        .download(&req2, &partial, &tmp.path().join("b"), &mut CancelAfterDrop(false))
        .expect_err("cancelled");
    assert!(matches!(err, DownloadError::Cancelled));
    // "New process": fresh downloader resumes at 2 MB.
    let mut d2 = Downloader::new(&t, fast_retry());
    let mut rec = Recorder(Vec::new());
    d2.download(&req2, &partial, &tmp.path().join("b"), &mut rec).expect("resume");
    assert_eq!(rec.0.first().map(|p| p.resumed_from), Some(2_000_000));
    assert_eq!(std::fs::read(tmp.path().join("b")).expect("read"), *data);
}

#[test]
fn restarts_when_server_ignores_range_or_content_changes() {
    let data = data();
    let server = serve(
        data.clone(),
        [("/norange", Behavior::NoRange), ("/etag", Behavior::ChangingEtag)].into(),
    );
    for path in ["/norange", "/etag"] {
        let tmp = tempfile::tempdir().expect("tmp");
        let url = format!("{}{path}", server.base);
        let sources = [url.as_str()];
        let hash = sha256(&data);
        let req = DownloadRequest {
            sources: &sources,
            expected_hash: hash,
            expected_size: data.len() as u64,
        };
        let t = transport();
        let mut d = Downloader::new(&t, fast_retry());
        d.download(&req, &tmp.path().join("p"), &tmp.path().join("o"), &mut NoopObserver)
            .expect("download");
        assert_eq!(std::fs::read(tmp.path().join("o")).expect("read"), *data, "{path}");
    }
}

#[test]
fn falls_back_across_mirrors_and_never_returns_tampered_bytes() {
    let data = data();
    let server = serve(
        data.clone(),
        [
            ("/missing", Behavior::NotFound),
            ("/tampered", Behavior::Tampered),
            ("/busy", Behavior::Busy(2)),
        ]
        .into(),
    );
    let tmp = tempfile::tempdir().expect("tmp");
    let urls: Vec<String> = ["/missing", "/tampered", "/busy"]
        .iter()
        .map(|p| format!("{}{p}", server.base))
        .collect();
    let sources: Vec<&str> = urls.iter().map(String::as_str).collect();
    let req = DownloadRequest {
        sources: &sources,
        expected_hash: sha256(&data),
        expected_size: data.len() as u64,
    };
    let t = transport();
    let mut d = Downloader::new(&t, fast_retry());
    d.download(&req, &tmp.path().join("p"), &tmp.path().join("o"), &mut NoopObserver)
        .expect("third mirror succeeds after retries");
    assert_eq!(std::fs::read(tmp.path().join("o")).expect("read"), *data);

    // Only a tampered mirror: must fail with an integrity error and leave
    // nothing behind.
    let only_bad = [urls[1].as_str()];
    let req = DownloadRequest { sources: &only_bad, ..req };
    let out = tmp.path().join("bad");
    let err = d
        .download(&req, &tmp.path().join("p2"), &out, &mut NoopObserver)
        .expect_err("tampered");
    assert!(err.is_integrity_failure(), "{err}");
    assert!(matches!(&err, DownloadError::AllSourcesFailed(v) if matches!(v[0].1, SourceFailure::Integrity { .. })));
    assert!(!out.exists());
}

#[test]
fn rejects_plain_http_by_default() {
    let t = UreqTransport::new(&TransportConfig::default()).expect("transport");
    let sources = ["http://127.0.0.1:9/x"];
    let req = DownloadRequest {
        sources: &sources,
        expected_hash: sha256(b"0123456789"),
        expected_size: 10,
    };
    let tmp = tempfile::tempdir().expect("tmp");
    let err = Downloader::new(&t, fast_retry())
        .download(&req, tmp.path(), &tmp.path().join("o"), &mut NoopObserver)
        .expect_err("http must be refused");
    assert!(matches!(&err, DownloadError::AllSourcesFailed(v) if matches!(v[0].1, SourceFailure::Forbidden(_))), "{err}");
}

#[test]
fn cache_hits_verify_and_heal_corruption() {
    let data = data();
    let server = serve(data.clone(), [("/f", Behavior::Good)].into());
    let tmp = tempfile::tempdir().expect("tmp");
    let cache = Cache::open(tmp.path().join("cache")).expect("cache");
    let url = format!("{}/f", server.base);
    let sources = [url.as_str()];
    let req = DownloadRequest {
        sources: &sources,
        expected_hash: sha256(&data),
        expected_size: data.len() as u64,
    };
    let info = ItemInfo {
        vendor: "Acme".into(),
        product: "Runtime".into(),
        version: "1.0".into(),
        arch: "x64".into(),
        platform: "windows".into(),
        file_name: "runtime.exe".into(),
    };
    let t = transport();
    let mut d = Downloader::new(&t, fast_retry());
    let p1 = cache.fetch(&mut d, &req, &info, &mut NoopObserver).expect("fetch");
    let p2 = cache.fetch(&mut d, &req, &info, &mut NoopObserver).expect("hit");
    assert_eq!(p1, p2);
    assert_eq!(server.state.lock().expect("lock").requests.len(), 1, "second fetch is a cache hit");

    let entries = cache.entries(true).expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].info, info);
    assert_eq!(entries[0].integrity, inst_fetch::cache::Integrity::Valid);

    // Corrupt the cached object: next fetch detects it and re-downloads.
    let mut bytes = std::fs::read(&p1).expect("read");
    bytes[10] ^= 0xff;
    std::fs::write(&p1, &bytes).expect("write");
    cache.fetch(&mut d, &req, &info, &mut NoopObserver).expect("healed");
    assert_eq!(std::fs::read(&p1).expect("read"), *data);
    assert_eq!(server.state.lock().expect("lock").requests.len(), 2);

    // Locked entries survive cleaning.
    let lock = cache.lock_shared(&req.expected_hash).expect("lock");
    assert_eq!(cache.clear_all().expect("clear"), 0);
    drop(lock);
    assert_eq!(cache.clear_all().expect("clear"), data.len() as u64);
    assert_eq!(cache.stats().expect("stats").entries, 0);
}
