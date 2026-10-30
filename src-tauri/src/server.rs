//! Optional local network server: other devices (tablets, laptops, phones)
//! open `http://<this PC>:<port>` in a browser and use the same POS, with
//! the same database and printers. Each browser logs in with its own PIN.
//!
//! The browser gets the same UI files as the desktop window and calls the
//! same commands (`POST /api/<command>` with JSON arguments), identified by
//! a random terminal id it generates and sends in the `X-Terminal` header.
//! A custom header also means other web pages cannot call the API (the
//! browser would need a CORS preflight, which this server never allows).

use std::io::Read;
use std::net::UdpSocket;
use std::sync::Arc;
use std::thread::JoinHandle;

use tauri::{AppHandle, Manager};
use tiny_http::{Header, Method, Request, Response};

use crate::commands::{dispatch, App};

const WORKERS: usize = 4;
const MAX_BODY: u64 = 1 << 20;

const INDEX_HTML: &[u8] = include_bytes!("../../ui/index.html");
const APP_JS: &[u8] = include_bytes!("../../ui/app.js");
const STYLE_CSS: &[u8] = include_bytes!("../../ui/style.css");
const ICON_PNG: &[u8] = include_bytes!("../../ui/icon.png");
const KEYBOARD_JS: &[u8] = include_bytes!("../../ui/keyboard.js");
// Sarabun (SIL Open Font License, see ui/fonts/OFL.txt): Latin and Thai in one family.
const FONT_REGULAR: &[u8] = include_bytes!("../../ui/fonts/Sarabun-Regular.ttf");
const FONT_SEMIBOLD: &[u8] = include_bytes!("../../ui/fonts/Sarabun-SemiBold.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../ui/fonts/Sarabun-Bold.ttf");

pub struct Server {
    http: Arc<tiny_http::Server>,
    workers: Vec<JoinHandle<()>>,
    pub port: u16,
}

impl Server {
    pub fn start(handle: AppHandle, port: u16) -> Result<Server, String> {
        let http = tiny_http::Server::http(("0.0.0.0", port))
            .map_err(|e| format!("cannot start the server on port {port} ({e}); is another program using it?"))?;
        let http = Arc::new(http);
        let workers = (0..WORKERS)
            .map(|_| {
                let (http, handle) = (http.clone(), handle.clone());
                std::thread::spawn(move || {
                    while let Ok(rq) = http.recv() {
                        respond(&handle, rq);
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

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn file(body: &'static [u8], kind: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(body)
        .with_header(header("Content-Type", kind))
        .with_header(header("Cache-Control", "no-cache"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
}

/// Fonts never change within a version, so browsers may keep them for a day.
fn font(body: &'static [u8]) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(body)
        .with_header(header("Content-Type", "font/ttf"))
        .with_header(header("Cache-Control", "max-age=86400"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
}

fn json(status: u16, body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Cache-Control", "no-store"))
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
        (Method::Get, "/" | "/index.html") => rq.respond(file(INDEX_HTML, "text/html; charset=utf-8")),
        (Method::Get, "/keyboard.js") => rq.respond(file(KEYBOARD_JS, "text/javascript; charset=utf-8")),
        (Method::Get, "/app.js") => rq.respond(file(APP_JS, "text/javascript; charset=utf-8")),
        (Method::Get, "/style.css") => rq.respond(file(STYLE_CSS, "text/css; charset=utf-8")),
        (Method::Get, "/icon.png" | "/favicon.ico") => rq.respond(file(ICON_PNG, "image/png")),
        (Method::Get, "/fonts/Sarabun-Regular.ttf") => rq.respond(font(FONT_REGULAR)),
        (Method::Get, "/fonts/Sarabun-SemiBold.ttf") => rq.respond(font(FONT_SEMIBOLD)),
        (Method::Get, "/fonts/Sarabun-Bold.ttf") => rq.respond(font(FONT_BOLD)),
        (Method::Post, p) if p.starts_with("/api/") => {
            let cmd = p["/api/".len()..].to_string();
            let response = api(handle, &mut rq, &cmd);
            rq.respond(response)
        }
        _ => rq.respond(error(404, "not found")),
    };
    if let Err(e) = result {
        eprintln!("server: {e}");
    }
}

fn api(handle: &AppHandle, rq: &mut Request, cmd: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let Some(terminal) = terminal_id(rq) else {
        return error(400, "missing terminal id; reload the page");
    };
    let mut body = Vec::new();
    if rq.as_reader().take(MAX_BODY + 1).read_to_end(&mut body).is_err() || body.len() as u64 > MAX_BODY {
        return error(413, "request too large");
    }
    let args = if body.is_empty() {
        serde_json::Value::Null
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return error(400, "invalid JSON"),
        }
    };
    let app = handle.state::<App>();
    match dispatch(&app, &terminal, cmd, args) {
        Ok(v) => json(200, v.to_string()),
        Err(e) => error(400, &e.to_string()),
    }
}
