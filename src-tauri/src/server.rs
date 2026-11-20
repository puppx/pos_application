//! Optional local network server: other devices (tablets, laptops, phones)
//! open `http://<this PC>:<port>` in a browser and use the same POS, with
//! the same database and printers. Each browser logs in with its own PIN.
//!
//! The browser gets the same UI files as the desktop window and calls the
//! same commands (`POST /api/<command>` with JSON arguments), identified by
//! a random terminal id it generates and sends in the `X-Terminal` header.
//! A custom header also means other web pages cannot call the API (the
//! browser would need a CORS preflight, which this server never allows).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tiny_http::{Header, Method, Request, Response};

use crate::commands::{backup_download, dispatch, App};

const WORKERS: usize = 4;
const MAX_BODY: u64 = 1 << 20;

/// A file of the UI, built into the app.
struct Asset {
    body: &'static [u8],
    kind: &'static str,
    cache: &'static str,
    tag: OnceLock<String>,
}

impl Asset {
    const fn new(body: &'static [u8], kind: &'static str) -> Asset {
        // The browser checks for a newer one each time (a 304 when there is none).
        Asset { body, kind, cache: "no-cache", tag: OnceLock::new() }
    }

    /// Fonts never change within a version, so browsers may keep them for a day.
    const fn font(body: &'static [u8]) -> Asset {
        Asset { body, kind: "font/ttf", cache: "max-age=86400", tag: OnceLock::new() }
    }

    /// `"<app version>-<hash of the contents>"`: a new build with other
    /// files never matches what a browser kept from an older one.
    fn etag(&self) -> &str {
        self.tag.get_or_init(|| format!("\"{}-{:016x}\"", env!("CARGO_PKG_VERSION"), fnv1a(self.body)))
    }
}

/// FNV-1a, 64 bits: a fixed, quick hash of a file's contents.
fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

static INDEX_HTML: Asset = Asset::new(include_bytes!("../../ui/index.html"), "text/html; charset=utf-8");
static APP_JS: Asset = Asset::new(include_bytes!("../../ui/app.js"), "text/javascript; charset=utf-8");
static STYLE_CSS: Asset = Asset::new(include_bytes!("../../ui/style.css"), "text/css; charset=utf-8");
static ICON_PNG: Asset = Asset::new(include_bytes!("../../ui/icon.png"), "image/png");
static KEYBOARD_JS: Asset = Asset::new(include_bytes!("../../ui/keyboard.js"), "text/javascript; charset=utf-8");
// Sarabun (SIL Open Font License, see ui/fonts/OFL.txt): Latin and Thai in one family.
static FONT_REGULAR: Asset = Asset::font(include_bytes!("../../ui/fonts/Sarabun-Regular.ttf"));
static FONT_SEMIBOLD: Asset = Asset::font(include_bytes!("../../ui/fonts/Sarabun-SemiBold.ttf"));
static FONT_BOLD: Asset = Asset::font(include_bytes!("../../ui/fonts/Sarabun-Bold.ttf"));

/// Set when accepted connections did not get TCP_NODELAY from the listening
/// socket (see [`listen`]): API answers then close the connection instead.
static CLOSE_API_CONNECTIONS: AtomicBool = AtomicBool::new(false);

/// Listen on `port` with TCP_NODELAY on, so every answer goes out at once.
/// Without it a reply sent in two writes (tiny_http writes the headers,
/// then the body) waits for the tablet's delayed ACK: ~40 ms per API call
/// on a kept-alive connection. tiny_http never sets it on the connections
/// it accepts, but they inherit it from the listening socket (Linux,
/// Windows, macOS); that is checked here once with a loopback connection.
/// Returns the listener and whether accepted connections have it.
fn listen(port: u16) -> std::io::Result<(TcpListener, bool)> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    socket2::SockRef::from(&listener).set_tcp_nodelay(true)?;
    let inherited = accepted_have_nodelay(&listener);
    Ok((listener, inherited))
}

/// Connect to `listener` from this PC and see whether the accepted end has
/// TCP_NODELAY. False when that cannot be told.
fn accepted_have_nodelay(listener: &TcpListener) -> bool {
    let Ok(port) = listener.local_addr().map(|a| a.port()) else { return false };
    let Ok(probe) = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(1)) else {
        return false;
    };
    let Ok(me) = probe.local_addr() else { return false };
    if listener.set_nonblocking(true).is_err() {
        return false;
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    let accepted = loop {
        match listener.accept() {
            Ok((conn, from)) if from == me => break Some(conn),
            // Someone else, this very moment: they retry.
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => break None,
        }
    };
    let blocking = listener.set_nonblocking(false).is_ok();
    blocking && accepted.is_some_and(|c| c.nodelay().unwrap_or(false))
}

pub struct Server {
    http: Arc<tiny_http::Server>,
    workers: Vec<JoinHandle<()>>,
    pub port: u16,
}

impl Server {
    pub fn start(handle: AppHandle, port: u16) -> Result<Server, String> {
        let busy = |e: &dyn std::fmt::Display| format!("cannot start the server on port {port} ({e}); is another program using it?");
        let (listener, nodelay) = listen(port).map_err(|e| busy(&e))?;
        if !nodelay {
            eprintln!("server: connections do not get TCP_NODELAY here; API answers close the connection instead");
        }
        CLOSE_API_CONNECTIONS.store(!nodelay, Ordering::Relaxed);
        let http = tiny_http::Server::from_listener(listener, None).map_err(|e| busy(&e))?;
        let http = Arc::new(http);
        let workers = (0..WORKERS)
            .map(|_| {
                let (http, handle) = (http.clone(), handle.clone());
                std::thread::spawn(move || {
                    while let Ok(rq) = http.recv() {
                        // A bug in one request must not take this worker down.
                        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| respond(&handle, rq)));
                        if ok.is_err() {
                            eprintln!("server: a request failed unexpectedly");
                        }
                    }
                })
            })
            .collect();
        Ok(Server { http, workers, port })
    }

    /// Stop accepting requests and free the port.
    pub fn stop(self) {
        for _ in 0..self.workers.len() {
            self.http.unblock();
        }
        for w in self.workers {
            let _ = w.join();
        }
    }
}

/// Addresses other devices can use, best guess first.
pub fn urls(port: u16) -> Vec<String> {
    let mut out = Vec::new();
    // Connecting a UDP socket sends nothing; it just picks the LAN interface.
    for probe in ["8.8.8.8:80", "192.168.1.1:80", "10.0.0.1:80"] {
        let ip = UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect(probe).and_then(|_| s.local_addr()));
        if let Ok(addr) = ip {
            let url = format!("http://{}:{port}", addr.ip());
            if !addr.ip().is_unspecified() && !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out.push(format!("http://localhost:{port}"));
    out
}

/// Adding headers never panics: a value is made plain ASCII first, and a
/// header that still cannot be built is left out.
trait Headers: Sized {
    fn header(self, name: &str, value: &str) -> Self;
}

impl<R: Read> Headers for Response<R> {
    fn header(self, name: &str, value: &str) -> Self {
        let value: String = value.chars().map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '_' }).collect();
        match Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            Ok(h) => self.with_header(h),
            Err(_) => self,
        }
    }
}

/// `attachment` with the file name: an ASCII fallback and the exact UTF-8
/// name (RFC 6266), e.g. for "pos-café.db".
fn attachment(name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    let exact: String = name
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{exact}")
}

/// Whether the browser's `If-None-Match` names `tag` (its copy is current).
fn still_fresh(rq: &Request, tag: &str) -> bool {
    rq.headers().iter().filter(|h| h.field.equiv("If-None-Match")).any(|h| {
        h.value.as_str().split(',').map(str::trim).any(|t| t == "*" || t.strip_prefix("W/").unwrap_or(t) == tag)
    })
}

/// A UI file, or 304 Not Modified when the browser already has this one
/// (app.js alone is over 300 KB: tablets only ask, not fetch it again).
fn file(rq: &Request, a: &Asset) -> Response<std::io::Cursor<Vec<u8>>> {
    let fresh = still_fresh(rq, a.etag());
    let r = if fresh { Response::from_data(Vec::new()).with_status_code(304) } else { Response::from_data(a.body).header("Content-Type", a.kind) };
    r.header("ETag", a.etag()).header("Cache-Control", a.cache).header("X-Content-Type-Options", "nosniff")
}

fn json(status: u16, body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Cache-Control", "no-store")
}

fn error(status: u16, msg: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    json(status, serde_json::to_string(msg).unwrap_or_default())
}

/// A terminal id is the browser's own random secret: 32 to 64 hex digits.
fn terminal_id(rq: &Request) -> Option<String> {
    let h = rq.headers().iter().find(|h| h.field.equiv("X-Terminal"))?;
    let id = h.value.as_str();
    ((32..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("web-{id}"))
}

fn respond(handle: &AppHandle, mut rq: Request) {
    let path = rq.url().split('?').next().unwrap_or("/").to_string();
    let result = match (rq.method(), path.as_str()) {
        (Method::Get, p) if asset(p).is_some() => {
            let response = asset(p).map(|a| file(&rq, a)).unwrap_or_else(|| error(404, "not found"));
            rq.respond(response)
        }
        (Method::Post, "/backup-file") => match download(handle, &mut rq) {
            Ok(r) => rq.respond(r),
            Err(r) => rq.respond(r),
        },
        (Method::Post, p) if p.starts_with("/api/") => {
            let cmd = p["/api/".len()..].to_string();
            let (status, body) = api(handle, &mut rq, &cmd);
            if CLOSE_API_CONNECTIONS.load(Ordering::Relaxed) {
                respond_and_close(rq, status, &body)
            } else {
                rq.respond(json(status, body))
            }
        }
        _ => rq.respond(error(404, "not found")),
    };
    if let Err(e) = result {
        eprintln!("server: {e}");
    }
}

/// The `/api/<cmd>` answer: status and JSON body.
fn api(handle: &AppHandle, rq: &mut Request, cmd: &str) -> (u16, String) {
    let fail = |status: u16, msg: &str| (status, serde_json::to_string(msg).unwrap_or_default());
    let Some(terminal) = terminal_id(rq) else {
        return fail(400, "missing terminal id; reload the page");
    };
    let mut body = Vec::new();
    if rq.as_reader().take(MAX_BODY + 1).read_to_end(&mut body).is_err() || body.len() as u64 > MAX_BODY {
        return fail(413, "request too large");
    }
    let args = if body.is_empty() {
        serde_json::Value::Null
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return fail(400, "invalid JSON"),
        }
    };
    let app = handle.state::<App>();
    match dispatch(&app, &terminal, cmd, args) {
        Ok(v) => (200, v.to_string()),
        Err(e) => fail(400, &e.to_string()),
    }
}

/// The JSON answer as [`json`] makes it, written in one go with
/// `Connection: close` (tiny_http leaves that header out of its own
/// responses), so the browser takes a new connection for the next call:
/// for when connections lack TCP_NODELAY (see [`listen`]).
fn respond_and_close(rq: Request, status: u16, body: &str) -> std::io::Result<()> {
    let reason = tiny_http::StatusCode(status).default_reason_phrase();
    let mut out = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body.as_bytes());
    let mut w = rq.into_writer();
    w.write_all(&out)?;
    w.flush()
}

fn asset(path: &str) -> Option<&'static Asset> {
    Some(match path {
        "/" | "/index.html" => &INDEX_HTML,
        "/keyboard.js" => &KEYBOARD_JS,
        "/app.js" => &APP_JS,
        "/style.css" => &STYLE_CSS,
        "/icon.png" | "/favicon.ico" => &ICON_PNG,
        "/fonts/Sarabun-Regular.ttf" => &FONT_REGULAR,
        "/fonts/Sarabun-SemiBold.ttf" => &FONT_SEMIBOLD,
        "/fonts/Sarabun-Bold.ttf" => &FONT_BOLD,
        _ => return None,
    })
}

/// A full backup as a file download (Settings → Backup & data → Download),
/// e.g. to copy it to a USB stick from a laptop. `POST {"name": …}` with
/// the terminal header, like the API, so other web pages cannot fetch it.
fn download(handle: &AppHandle, rq: &mut Request) -> Result<Response<std::fs::File>, Response<std::io::Cursor<Vec<u8>>>> {
    let terminal = terminal_id(rq).ok_or_else(|| error(400, "missing terminal id; reload the page"))?;
    let mut body = Vec::new();
    if rq.as_reader().take(4096).read_to_end(&mut body).is_err() {
        return Err(error(400, "bad request"));
    }
    let name = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("name")?.as_str().map(str::to_string))
        .ok_or_else(|| error(400, "which backup?"))?;
    let app = handle.state::<App>();
    let path = backup_download(&app, &terminal, &name).map_err(|e| error(400, &e.to_string()))?;
    let file = std::fs::File::open(&path).map_err(|_| error(404, "not found"))?;
    Ok(Response::from_file(file)
        .header("Content-Type", "application/vnd.sqlite3")
        .header("Content-Disposition", &attachment(&name))
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny_http server on a free port as [`Server::start`] makes it,
    /// answering each request with `handle`.
    fn serve(handle: fn(Request)) -> (u16, bool) {
        let (listener, nodelay) = listen(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let http = tiny_http::Server::from_listener(listener, None).unwrap();
        std::thread::spawn(move || {
            while let Ok(rq) = http.recv() {
                handle(rq);
            }
        });
        (port, nodelay)
    }

    /// Send `request` and read the answer (the request ends the connection).
    fn ask(port: u16, request: &str) -> String {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(request.as_bytes()).unwrap();
        let mut out = Vec::new();
        c.read_to_end(&mut out).unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn connections_get_nodelay_from_the_listening_socket() {
        let (port, nodelay) = serve(|rq| {
            let _ = rq.respond(Response::from_string("ok"));
        });
        // Linux, Windows and macOS all pass it on to accepted connections.
        if cfg!(any(target_os = "linux", windows, target_os = "macos")) {
            assert!(nodelay);
        }
        assert!(ask(port, "GET / HTTP/1.1\r\nConnection: close\r\n\r\n").ends_with("ok"));
        // The port is taken now: a second server says so.
        assert!(listen(port).is_err());
    }

    #[test]
    fn ui_files_carry_an_etag_and_a_current_copy_is_not_sent_again() {
        let (port, _) = serve(|rq| {
            let a = asset(rq.url()).unwrap();
            let r = file(&rq, a);
            let _ = rq.respond(r);
        });
        let tag = APP_JS.etag().to_string();
        assert!(tag.starts_with(&format!("\"{}-", env!("CARGO_PKG_VERSION"))) && tag.ends_with('"') && tag.len() == 2 + env!("CARGO_PKG_VERSION").len() + 17);
        assert_ne!(tag, STYLE_CSS.etag());
        let full = ask(port, "GET /app.js HTTP/1.1\r\nConnection: close\r\n\r\n");
        assert!(full.starts_with("HTTP/1.1 200"));
        assert!(full.contains(&format!("ETag: {tag}\r\n")) && full.contains("Cache-Control: no-cache\r\n"));
        assert!(full.len() > APP_JS.body.len());
        for header in [tag.clone(), format!("W/{tag}"), format!("\"x\", {tag}"), "*".into()] {
            let again = ask(port, &format!("GET /app.js HTTP/1.1\r\nIf-None-Match: {header}\r\nConnection: close\r\n\r\n"));
            assert!(again.starts_with("HTTP/1.1 304"), "{header}: {again}");
            assert!(again.contains(&format!("ETag: {tag}\r\n")) && again.contains("Cache-Control: no-cache\r\n"));
            assert!(again.ends_with("\r\n\r\n"), "no body");
        }
        // Another version's copy: sent in full.
        let old = ask(port, "GET /app.js HTTP/1.1\r\nIf-None-Match: \"0.0.1-0000000000000000\"\r\nConnection: close\r\n\r\n");
        assert!(old.starts_with("HTTP/1.1 200") && old.len() > APP_JS.body.len());
        // Fonts keep their day of caching.
        let font = ask(port, "GET /fonts/Sarabun-Bold.ttf HTTP/1.1\r\nConnection: close\r\n\r\n");
        assert!(font.contains("Cache-Control: max-age=86400\r\n") && font.contains("ETag: \""));
    }

    #[test]
    fn without_nodelay_api_answers_close_the_connection() {
        let (port, _) = serve(|rq| {
            let _ = respond_and_close(rq, 400, "\"wrong PIN\"");
        });
        // Even when the browser would keep the connection (it closes it on
        // reading the answer; tiny_http then sees the end).
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"POST /api/login HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        let mut r = Vec::new();
        while !r.ends_with(b"\"wrong PIN\"") {
            let mut buf = [0u8; 512];
            let n = c.read(&mut buf).unwrap();
            assert!(n > 0);
            r.extend_from_slice(&buf[..n]);
        }
        let r = String::from_utf8(r).unwrap();
        assert_eq!(
            r,
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: 11\r\nConnection: close\r\n\r\n\"wrong PIN\""
        );
    }

    #[test]
    fn any_backup_name_makes_a_safe_download_header() {
        assert_eq!(attachment("pos-1.db"), "attachment; filename=\"pos-1.db\"; filename*=UTF-8''pos-1.db");
        assert_eq!(attachment("pos-café.db"), "attachment; filename=\"pos-caf_.db\"; filename*=UTF-8''pos-caf%C3%A9.db");
        // Odd values never panic: they are made ASCII (or the header is left out).
        let r = Response::from_string("x").header("Content-Disposition", "a\r\nb é").header("Bad Name\u{e9}", "v");
        assert!(r.headers().iter().any(|h| h.field.equiv("Content-Disposition") && h.value.as_str() == "a__b _"));
    }
}
