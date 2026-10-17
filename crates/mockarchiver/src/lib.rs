//! A mock EPICS Archiver Appliance for tests, CI and demos.
//!
//! It serves deterministic synthetic history for a fixed set of PVs (see
//! [`signals`]) with the real appliance's wire format and quirks:
//!
//! - `GET /retrieval/data/getData.json?pv=..&from=..&to=..` — raw (with the
//!   sample before `from`), `optimized_N` (raw when few samples, else
//!   `[mean, std, min, max, count]` bins at bin centres, filled), `mean_N`,
//!   `min_N`, `max_N`, `std_N`, `variance_N`, `median_N`, `count_N`,
//!   `firstSample_N`, `lastSample_N`, `nth_N`, `ncount` and the
//!   `<op>Sample_N` no-fill variants; `op(PV,N)` and `pp=` forms too;
//!   bare `NaN` tokens for `MOCK:NAN`; 404 (HTML body) for unknown PVs;
//!   400 for a bad time, `to < from` or unbalanced parentheses. `to`
//!   defaults to now + 1h, `from` to `to` − 1 day. Summaries of strings and
//!   waveforms return raw data.
//! - `GET /retrieval/bpl/getMatchingPVs?pv=<glob>|regex=<re>&limit=N` —
//!   the appliance's naive glob (`*`→`.*`, `?`→`.`), whole-name match,
//!   sorted; 404 with neither parameter.
//! - `GET /retrieval/bpl/getMetadata?pv=..` — `DBRType`, `elementCount`,
//!   `units`, `precision`, ... as strings.
//! - `GET /retrieval/bpl/getVersion` — `{"version": "Archiver Appliance
//!   Version <mock version>"}`.
//! - `GET /mgmt/bpl/getAllPVs?pv=<glob>[,<glob>]&regex=..&limit=N` — unsorted.
//!
//! Every endpoint is served on one port (like the single-container
//! appliance), so `--archiver http://127.0.0.1:PORT` works for the client.
//! Data begins [`signals::HISTORY_DAYS`] days before the server started
//! and grows with the clock; large responses are streamed (chunked).

use std::io::{self, Read};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

mod bpl;
mod data;
pub mod signals;

/// The appliance's retrieval port, the default for the binary.
pub const DEFAULT_PORT: u16 = 17668;

/// Request-handling threads.
const WORKERS: usize = 8;

/// A running mock appliance. Serves on background threads until dropped.
pub struct MockArchiver {
    addr: SocketAddr,
    server: Arc<tiny_http::Server>,
    stop: Arc<AtomicBool>,
    metadata: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl MockArchiver {
    /// Bind `addr` (port 0 picks a free port: use [`MockArchiver::url`])
    /// and start serving.
    pub fn start(addr: SocketAddr) -> io::Result<Self> {
        let server = Arc::new(tiny_http::Server::http(addr).map_err(io::Error::other)?);
        let addr = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| io::Error::other("not an IP listener"))?;
        let clock = Arc::new(Clock::started_now());
        let stop = Arc::new(AtomicBool::new(false));
        let metadata = Arc::new(AtomicBool::new(true));
        let threads = (0..WORKERS)
            .map(|i| {
                let (server, clock, stop, metadata) = (
                    server.clone(),
                    clock.clone(),
                    stop.clone(),
                    metadata.clone(),
                );
                std::thread::Builder::new()
                    .name(format!("mockarchiver-{i}"))
                    .spawn(move || serve(&server, &clock, &stop, &metadata))
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self {
            addr,
            server,
            stop,
            metadata,
            threads,
        })
    }

    /// Start on 127.0.0.1 with a free port: the usual test setup.
    pub fn start_local() -> io::Result<Self> {
        Self::start(SocketAddr::from(([127, 0, 0, 1], 0)))
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:PORT`, what to pass as `--archiver`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Whether `bpl/getMetadata` is served (the default) or answers 404,
    /// like a proxy that passes on only data and search requests.
    pub fn serve_metadata(&self, on: bool) {
        self.metadata.store(on, Ordering::SeqCst);
    }

    /// Serve until the process is killed (the binary).
    pub fn wait(mut self) {
        for t in std::mem::take(&mut self.threads) {
            let _ = t.join();
        }
    }
}

impl Drop for MockArchiver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for _ in 0..self.threads.len() {
            self.server.unblock();
        }
        for t in std::mem::take(&mut self.threads) {
            let _ = t.join();
        }
    }
}

fn serve(server: &tiny_http::Server, clock: &Clock, stop: &AtomicBool, metadata: &AtomicBool) {
    loop {
        match server.recv() {
            Ok(rq) => {
                let reply = if !metadata.load(Ordering::SeqCst)
                    && rq.url().starts_with("/retrieval/bpl/getMetadata")
                {
                    Reply::error(404, "Not Found", "getMetadata is not served here")
                } else {
                    route(rq.url(), clock)
                };
                log::debug!("{} {} -> {}", rq.method(), rq.url(), reply.status);
                let _ = rq.respond(reply.into_response());
            }
            Err(_) if stop.load(Ordering::SeqCst) => return,
            Err(e) => log::warn!("mockarchiver: {e}"),
        }
    }
}

/// The server's notion of time: history starts at `origin_ns`, data ends
/// at the current time.
#[derive(Clone, Debug)]
pub(crate) struct Clock {
    pub(crate) origin_ns: i64,
    /// A fixed "now" for tests; the wall clock when `None`.
    fixed_now: Option<i64>,
}

impl Clock {
    fn started_now() -> Self {
        Self::starting_at(wall_ns())
    }

    /// History from `HISTORY_DAYS` before `start_ns`, aligned to 10
    /// minutes (a multiple of every PV's period).
    fn starting_at(start_ns: i64) -> Self {
        const ALIGN: i64 = 600_000_000_000;
        let origin = start_ns - signals::HISTORY_DAYS as i64 * 86_400_000_000_000;
        Self {
            origin_ns: origin.div_euclid(ALIGN) * ALIGN,
            fixed_now: None,
        }
    }

    /// A clock whose history starts `HISTORY_DAYS` before `now_ns`, which
    /// stays fixed (tests).
    #[cfg(test)]
    pub(crate) fn fixed(now_ns: i64) -> Self {
        Self {
            fixed_now: Some(now_ns),
            ..Self::starting_at(now_ns)
        }
    }

    pub(crate) fn now_ns(&self) -> i64 {
        self.fixed_now.unwrap_or_else(wall_ns)
    }
}

fn wall_ns() -> i64 {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or(i64::MAX / 2)
}

/// Decoded query parameters, in order.
pub(crate) struct Params(Vec<(String, String)>);

impl Params {
    fn parse(query: &str) -> Self {
        Self(
            query
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    (decode(k), decode(v))
                })
                .collect(),
        )
    }

    /// The first value of `key`.
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// `application/x-www-form-urlencoded` decoding, as the servlet does it.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A response body: small text, or streamed pieces.
pub(crate) enum Body {
    Text(String),
    Stream(Box<dyn Iterator<Item = String> + Send>),
}

pub(crate) struct Reply {
    pub(crate) status: u16,
    content_type: &'static str,
    body: Body,
}

impl Reply {
    pub(crate) fn json(body: String) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: Body::Text(body),
        }
    }

    pub(crate) fn json_stream(body: Box<dyn Iterator<Item = String> + Send>) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: Body::Stream(body),
        }
    }

    /// A Tomcat-style HTML error page.
    pub(crate) fn error(status: u16, reason: &str, message: &str) -> Self {
        let message = message
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        Self {
            status,
            content_type: "text/html;charset=utf-8",
            body: Body::Text(format!(
                "<!doctype html><html lang=\"en\"><head><title>HTTP Status {status} \u{2013} \
                 {reason}</title><style type=\"text/css\">body {{font-family:Tahoma,Arial,\
                 sans-serif;}}</style></head><body><h1>HTTP Status {status} \u{2013} {reason}\
                 </h1><hr class=\"line\" /><p><b>Type</b> Status Report</p><p><b>Message</b> \
                 {message}</p><hr class=\"line\" /><h3>Apache Tomcat (mock)</h3></body></html>"
            )),
        }
    }

    /// The whole body as text (tests).
    #[cfg(test)]
    pub(crate) fn text(self) -> String {
        match self.body {
            Body::Text(s) => s,
            Body::Stream(it) => it.collect(),
        }
    }

    fn into_response(self) -> tiny_http::Response<Box<dyn Read + Send>> {
        let header =
            tiny_http::Header::from_bytes("Content-Type", self.content_type).expect("valid header");
        let (reader, len): (Box<dyn Read + Send>, Option<usize>) = match self.body {
            Body::Text(s) => {
                let len = s.len();
                (Box::new(io::Cursor::new(s.into_bytes())), Some(len))
            }
            Body::Stream(it) => (Box::new(IterReader::new(it)), None),
        };
        tiny_http::Response::new(self.status.into(), vec![header], reader, len, None)
    }
}

/// Reads the concatenation of an iterator's strings.
struct IterReader {
    iter: Box<dyn Iterator<Item = String> + Send>,
    buf: Vec<u8>,
    pos: usize,
}

impl IterReader {
    fn new(iter: Box<dyn Iterator<Item = String> + Send>) -> Self {
        Self {
            iter,
            buf: Vec::new(),
            pos: 0,
        }
    }
}

impl Read for IterReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        while n < out.len() {
            if self.pos == self.buf.len() {
                match self.iter.next() {
                    Some(s) => {
                        self.buf = s.into_bytes();
                        self.pos = 0;
                        continue;
                    }
                    None => break,
                }
            }
            let k = (self.buf.len() - self.pos).min(out.len() - n);
            out[n..n + k].copy_from_slice(&self.buf[self.pos..self.pos + k]);
            self.pos += k;
            n += k;
        }
        Ok(n)
    }
}

/// Dispatch one request URL (path and query).
pub(crate) fn route(url: &str, clock: &Clock) -> Reply {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let params = Params::parse(query);
    match path.trim_end_matches('/') {
        "/retrieval/data/getData.json" => data::get_data(&params, clock),
        "/retrieval/bpl/getMatchingPVs" => bpl::get_matching_pvs(&params),
        "/retrieval/bpl/getMetadata" => bpl::get_metadata(&params),
        "/retrieval/bpl/getVersion" => bpl::get_version(),
        "/mgmt/bpl/getAllPVs" => bpl::get_all_pvs(&params),
        _ => Reply::error(404, "Not Found", &format!("{path} is not served here")),
    }
}

#[cfg(test)]
mod tests;
