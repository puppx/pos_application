//! Kitchen tickets for ESC/POS thermal printers (Epson TM, Star, Xprinter and
//! the many compatibles), plus the shared layout and printer transports used
//! by customer receipts (see `receipt.rs`).
//!
//! Building a ticket needs the database; sending it does not. Callers build a
//! [`PrintJob`] while holding the database, release it, then call
//! [`PrintJob::send`], so a slow or offline printer never blocks the till.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::invalid;
pub use crate::raster::ImageMode;
use crate::{ItemUpdate, Order, OrderItem, OrderType, Permission, Pos, PosError, Result, Session};

const SETTING_KEY: &str = "_kitchen_printer";
const NET_TIMEOUT: Duration = Duration::from_secs(4);

/// Where tickets go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PrinterTarget {
    /// Printing switched off.
    None,
    /// Network printer, raw TCP (almost always port 9100).
    Network { host: String, port: u16 },
    /// A printer installed in Windows (USB or any driver), sent as RAW data.
    Windows { name: String },
    /// A device path or share, e.g. `COM3` or `\\PC\Kitchen`.
    File { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrinterConfig {
    #[serde(flatten)]
    pub target: PrinterTarget,
    /// Characters per line in the normal font: 42 for 80 mm paper, 32 for 58 mm.
    #[serde(default = "default_width")]
    pub width: usize,
    /// How many copies each automatic print makes (manual reprints make one).
    #[serde(default = "one")]
    pub copies: u8,
    /// Seconds to wait between copies, so each one can be torn off.
    #[serde(default = "default_copy_delay")]
    pub copy_delay_secs: u8,
    /// Which names to print; None = this printer's usual (kitchen: both,
    /// receipt: English).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names: Option<PrintNames>,
    /// Print all text in bold, not only headings (on by default).
    #[serde(default = "yes")]
    pub bold: bool,
    /// Kitchen only: dishes, options and notes in double-size text (on by
    /// default); off = double height only, more per line.
    #[serde(default = "yes")]
    pub large_text: bool,
    /// Kitchen only: copies of a ticket that sends starters ahead of the
    /// mains, and of mains tickets (mains away / main course call).
    /// None = `copies`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter_copies: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mains_copies: Option<u8>,
    /// Kitchen only: copies of PREPARE NOW tickets. None = `copies`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prep_copies: Option<u8>,
    /// Kitchen only: copies of ADDITIONAL tickets (more ordered later).
    /// None = 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_copies: Option<u8>,
    /// Kitchen only: copies of the ticket of a dine-in order sent with all
    /// courses together (starters and mains at once). None = as without it:
    /// `copies` for a new order (see [`Pos::kitchen_job`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_courses_copies: Option<u8>,
    /// Kitchen only: copies of the "Call mains again" ticket (MAIN COURSE).
    /// None = `mains_copies`, else `copies`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_again_copies: Option<u8>,
    /// Kitchen only: print ticket text in the app's font (Sarabun, the same
    /// as the team bars) instead of the printer's own (on by default).
    #[serde(default = "yes")]
    pub app_font: bool,
    /// Kitchen only: "Call mains again" prints just the table in huge
    /// letters, without the dishes (on by default).
    #[serde(default = "yes")]
    pub call_table_only: bool,
    /// How this printer is sent images (Thai text, the app's font, team
    /// bars). Saved settings without it print as before (standard raster).
    #[serde(default)]
    pub image_mode: ImageMode,
    /// Send slowly: network and serial/shared printers get each job in
    /// pieces of [`SLOW_CHUNK`] bytes, [`SLOW_PAUSE`] apart (about 20 KB a
    /// second), so a cheap printer whose buffer overflows on a fast burst
    /// does not lose bytes and print junk. On by default, also for settings
    /// saved before it existed; the till does not wait for it (see
    /// [`PrintJob::send_in_background`]). Printers installed in Windows are
    /// never paced: the spooler and USB already pace the data.
    #[serde(default = "yes")]
    pub slow_send: bool,
}

fn yes() -> bool {
    true
}

const ROUTES_KEY: &str = "kitchen_routes";

/// Another kitchen printer (bar, dessert station...) and what it prints:
/// drinks, desserts and/or the dishes of some kitchen teams. Everything
/// else prints on the main kitchen printer, as before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KitchenRoute {
    /// What the staff call it, e.g. "Bar" (not `name`: a Windows printer's
    /// name sits next to it).
    pub label: String,
    #[serde(flatten)]
    pub target: PrinterTarget,
    #[serde(default = "default_width")]
    pub width: usize,
    /// Copies of each ticket on this printer.
    #[serde(default = "one")]
    pub copies: u8,
    #[serde(default)]
    pub team_ids: Vec<i64>,
    #[serde(default)]
    pub drinks: bool,
    #[serde(default)]
    pub desserts: bool,
    /// This printer's own way of printing images (see [`PrinterConfig`]).
    #[serde(default)]
    pub image_mode: ImageMode,
    /// Send slowly (see [`PrinterConfig::slow_send`]); on by default.
    #[serde(default = "yes")]
    pub slow_send: bool,
}

/// Language of item names on a printout. Items without a Thai name always
/// print in English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrintNames {
    English,
    /// The English name with the Thai name under it.
    Both,
    Thai,
}

impl PrintNames {
    /// Name to print on the item's line, and the second name under it.
    pub(crate) fn pick<'a>(self, name: &'a str, alt: Option<&'a str>) -> (&'a str, Option<&'a str>) {
        let alt = alt.filter(|a| !a.trim().is_empty() && *a != name);
        match (self, alt) {
            (PrintNames::Both, Some(a)) => (name, Some(a)),
            (PrintNames::Thai, Some(a)) => (a, None),
            _ => (name, None),
        }
    }

    /// One-line form, for options and category headings: "Large / ใหญ่".
    pub(crate) fn inline(self, name: &str, alt: Option<&str>) -> String {
        match self.pick(name, alt) {
            (n, Some(a)) => format!("{n} / {a}"),
            (n, None) => n.to_string(),
        }
    }
}

fn default_width() -> usize {
    42
}

fn default_copy_delay() -> u8 {
    2
}

fn one() -> u8 {
    1
}

impl Default for PrinterConfig {
    fn default() -> Self {
        PrinterConfig {
            target: PrinterTarget::None,
            width: default_width(),
            copies: 1,
            copy_delay_secs: default_copy_delay(),
            names: None,
            bold: true,
            large_text: true,
            starter_copies: None,
            mains_copies: None,
            prep_copies: None,
            additional_copies: None,
            all_courses_copies: None,
            call_again_copies: None,
            app_font: true,
            call_table_only: true,
            image_mode: ImageMode::Raster,
            slow_send: true,
        }
    }
}

impl PrinterConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if !(24..=64).contains(&self.width) {
            return invalid("paper width must be between 24 and 64 characters");
        }
        if [Some(self.copies), self.starter_copies, self.mains_copies, self.prep_copies, self.additional_copies, self.all_courses_copies, self.call_again_copies].into_iter().flatten().any(|c| !(1..=9).contains(&c)) {
            return invalid("copies must be between 1 and 9");
        }
        if self.copy_delay_secs > 30 {
            return invalid("the delay between copies must be 0 to 30 seconds");
        }
        match &self.target {
            PrinterTarget::Network { host, port } if host.trim().is_empty() || *port == 0 => {
                invalid("enter the printer's IP address and port")
            }
            PrinterTarget::Windows { name } if name.trim().is_empty() => invalid("choose a Windows printer"),
            PrinterTarget::File { path } if path.trim().is_empty() => invalid("enter the port or path"),
            _ => Ok(()),
        }
    }
}

/// Ready-to-send bytes plus their destination. A job with several copies
/// holds one part per copy, sent `gap` apart.
#[derive(Debug, Clone)]
pub struct PrintJob {
    pub target: PrinterTarget,
    pub parts: Vec<Vec<u8>>,
    pub gap: Duration,
    /// The same print for other printers (kitchen tickets routed to a bar or
    /// dessert printer). Each is sent even if another one fails.
    pub more: Vec<PrintJob>,
    /// Sent slowly, in paced pieces ([`PrinterConfig::slow_send`]).
    pub slow: bool,
    /// Goes ahead of the paced jobs still waiting for this printer (the cash
    /// drawer), never into the middle of one being sent.
    pub urgent: bool,
    /// What it is, for a failure message: "Kitchen ticket #94".
    pub what: String,
    /// Every copy over one connection, in one turn of the printer's line
    /// (the wait between copies included), and the connection then ended
    /// cleanly ([`Conn::finish`]): for printers in the "Compatible (older
    /// printers)" image mode ([`PrintJob::for_mode`]). An old printer
    /// reported that copies arriving on new connections, while it was still
    /// busy with the first, only came out when the next job came.
    pub one_connection: bool,
}

impl PrintJob {
    pub fn new(target: PrinterTarget, bytes: Vec<u8>) -> PrintJob {
        PrintJob { target, parts: vec![bytes], gap: Duration::ZERO, more: Vec::new(), slow: false, urgent: false, what: String::new(), one_connection: false }
    }

    /// Sent as a printer in image `mode` needs it: in the compatible mode
    /// (older printers) every copy over one connection, ended cleanly (see
    /// [`PrintJob::one_connection`]); the other modes as before.
    pub fn for_mode(mut self, mode: ImageMode) -> PrintJob {
        self.one_connection = mode == ImageMode::Column;
        self
    }

    /// True when this job goes over one connection (see
    /// [`PrintJob::one_connection`]): network printers and ports / paths.
    pub fn is_one_connection(&self) -> bool {
        self.one_connection && matches!(self.target, PrinterTarget::Network { .. } | PrinterTarget::File { .. })
    }

    /// This job sent slowly or not (the printer's "Send slowly" setting).
    pub fn paced(mut self, slow: bool) -> PrintJob {
        self.slow = slow;
        self
    }

    /// Ahead of the jobs waiting for this printer (see [`PrintJob::urgent`]).
    pub fn urgent(mut self) -> PrintJob {
        self.urgent = true;
        self
    }

    /// Named for failure messages (see [`PrintJob::what`]); the other
    /// printers' parts too.
    pub fn about(mut self, what: impl Into<String>) -> PrintJob {
        self.what = what.into();
        for m in &mut self.more {
            m.what = self.what.clone();
        }
        self
    }

    /// True when this job's data goes out in paced pieces: "Send slowly" on,
    /// to a network printer or a port / shared path.
    pub fn is_paced(&self) -> bool {
        self.slow && matches!(self.target, PrinterTarget::Network { .. } | PrinterTarget::File { .. })
    }

    /// `copies` of `doc`, the first one preceded by `lead` (e.g. the drawer pulse).
    pub(crate) fn copies(target: PrinterTarget, lead: &[u8], doc: Vec<u8>, copies: u8, delay_secs: u8) -> PrintJob {
        let mut first = lead.to_vec();
        first.extend_from_slice(&doc);
        let mut parts = vec![first];
        parts.extend(std::iter::repeat_n(doc, usize::from(copies.max(1)) - 1));
        PrintJob { target, parts, gap: Duration::from_secs(delay_secs.into()), more: Vec::new(), slow: false, urgent: false, what: String::new(), one_connection: false }
    }

    /// Everything the printers receive, in order (this printer, then the others).
    pub fn bytes(&self) -> Vec<u8> {
        let mut all = self.parts.concat();
        for m in &self.more {
            all.extend(m.bytes());
        }
        all
    }

    /// Join several jobs (one per printer) into one.
    pub(crate) fn join(jobs: Vec<PrintJob>) -> Option<PrintJob> {
        let mut it = jobs.into_iter();
        let mut first = it.next()?;
        first.more.extend(it);
        Some(first)
    }

    /// Send every part, waiting `gap` between them, then the other printers.
    /// Waits for all of it, paced pieces included.
    pub fn send(&self) -> Result<()> {
        if self.is_one_connection() {
            let _turn = Ticket::new(&self.target, self.urgent).wait();
            send_whole(open_or_busy(&self.target), &self.target, &self.parts, self.gap, self.is_paced())?;
            for m in &self.more {
                m.send()?;
            }
            return Ok(());
        }
        for (i, part) in self.parts.iter().enumerate() {
            if i > 0 {
                std::thread::sleep(self.gap);
            }
            if self.is_paced() {
                send_paced(&self.target, part, self.urgent && i == 0)?;
            } else {
                send_bytes(&self.target, part)?;
            }
        }
        for m in &self.more {
            m.send()?;
        }
        Ok(())
    }

    /// Send the first copy now (its result is returned) and any further
    /// copies on a background thread, so the till does not wait out the delay.
    /// Every printer is tried; their errors are reported together.
    ///
    /// A paced job ([`PrintJob::is_paced`]) only connects now, so an
    /// unreachable printer is still reported at once; its bytes then go out
    /// on a background thread, piece by piece. Jobs for one printer take
    /// turns (one connection at a time, in the order sent). When the printer
    /// is still busy with an earlier paced job, this one waits its turn on
    /// the background thread and the till does not wait at all. Callers send
    /// after releasing the database (see the module notes), so nothing here
    /// holds up other terminals.
    pub fn send_in_background(mut self) -> Result<()> {
        let more = std::mem::take(&mut self.more);
        let mut errors: Vec<String> = Vec::new();
        if let Err(e) = self.send_one() {
            errors.push(e.to_string());
        }
        for m in more {
            if let Err(e) = m.send_in_background() {
                errors.push(e.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(PosError::Printer(errors.join("; ")))
        }
    }

    fn send_one(self) -> Result<()> {
        // One connection (the compatible mode), sent slowly or not: in the
        // printer's line too, so no other job ever connects while it is open.
        if self.is_paced() || self.is_one_connection() {
            return self.send_one_queued();
        }
        let mut parts = self.parts.into_iter();
        if let Some(first) = parts.next() {
            send_bytes(&self.target, &first)?;
        }
        let rest: Vec<Vec<u8>> = parts.collect();
        if !rest.is_empty() {
            let (target, gap, what) = (self.target, self.gap, self.what);
            let sending = SENDING.start();
            std::thread::spawn(move || {
                let _sending = sending;
                for part in rest {
                    std::thread::sleep(gap);
                    if let Err(e) = send_bytes(&target, &part) {
                        record_failure(&what, &target, &e.to_string());
                        break;
                    }
                }
            });
        }
        Ok(())
    }

    /// [`PrintJob::send_one`] for a paced or a one-connection job: in the
    /// printer's line, on a background thread.
    fn send_one_queued(self) -> Result<()> {
        let (together, paced) = (self.is_one_connection(), self.is_paced());
        let PrintJob { target, parts, gap, urgent, what, .. } = self;
        // Free now: connect here, so the error (if any) reaches the till.
        // Busy: take a place in the printer's line now (so jobs keep the
        // order they were sent in) and wait for it on the thread.
        let place = match Turn::try_take(&target) {
            // One connection: a printer that resets it now is busy, not out
            // of reach; that is tried again on the thread (see send_whole).
            Some(turn) => Place::Now(turn, if together { open_or_busy(&target)? } else { Some(open(&target)?) }),
            None => Place::Queued(Ticket::new(&target, urgent)),
        };
        // Counted until the last byte is out: closing the app waits for it.
        let sending = SENDING.start();
        std::thread::spawn(move || {
            let _sending = sending;
            if parts.is_empty() {
                return;
            }
            // One connection: every copy now, in this turn. Otherwise the
            // first copy now and each other one on its own connection.
            let (now, later) = parts.split_at(if together { parts.len() } else { 1 });
            let (turn, conn) = match place {
                Place::Now(turn, conn) => (turn, Ok(conn)),
                Place::Queued(ticket) => {
                    let turn = ticket.wait();
                    let conn = if together { open_or_busy(&target) } else { open(&target).map(Some) };
                    (turn, conn)
                }
            };
            let mut result = if together {
                send_whole(conn, &target, now, gap, paced)
            } else {
                // Not one connection: always opened (Some), as before.
                conn.and_then(|c| c.ok_or_else(|| PosError::Printer("not connected".into())))
                    .and_then(|c| write_parts(c, now, gap, true, false).map_err(printer_error))
            };
            drop(turn);
            for part in later {
                if result.is_err() {
                    break;
                }
                std::thread::sleep(gap);
                result = send_paced(&target, part, false);
            }
            if let Err(e) = result {
                record_failure(&what, &target, &e.to_string());
            }
        });
        Ok(())
    }
}

/// Bytes a slow printer ([`PrinterConfig::slow_send`]) is sent at a time.
pub const SLOW_CHUNK: usize = 1024;
/// Pause after each piece: 1 KB per 50 ms is about 20 KB a second, some 40 mm
/// of picture a second on 80 mm paper (504 dots = 63 bytes a row, 8 rows a
/// mm), slower than even a cheap printer prints, so its buffer never fills.
pub const SLOW_PAUSE: Duration = Duration::from_millis(50);

/// Write `bytes` in order, at most `chunk` at a time, waiting `pause` between
/// pieces (none after the last). The printer gets exactly `bytes`.
pub fn write_paced(w: &mut impl Write, bytes: &[u8], chunk: usize, pause: Duration) -> std::io::Result<()> {
    for (i, piece) in bytes.chunks(chunk.max(1)).enumerate() {
        if i > 0 {
            std::thread::sleep(pause);
        }
        w.write_all(piece)?;
        w.flush()?;
    }
    Ok(())
}

/// How long [`Conn::finish`] waits at most for the printer to take the last
/// of a job and close its side.
const FINISH_WAIT: Duration = Duration::from_secs(5);

/// Write `parts` over `conn`, `gap` apart, each in paced pieces or at once;
/// then end the connection cleanly when `finish` ([`Conn::finish`]), or
/// just close it.
fn write_parts(mut conn: Conn, parts: &[Vec<u8>], gap: Duration, paced: bool, finish: bool) -> std::io::Result<()> {
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(gap);
        }
        if paced {
            write_paced(&mut conn, part, SLOW_CHUNK, SLOW_PAUSE)?;
        } else {
            conn.write_all(part)?;
            conn.flush()?;
        }
    }
    if finish {
        conn.finish()?;
    }
    Ok(())
}

fn printer_error(e: std::io::Error) -> PosError {
    PosError::Printer(e.to_string())
}

/// How long a new connection is watched, before anything is sent, for a
/// printer that turns it away (resets or closes it at once: busy with
/// another connection, e.g. from another till).
const CONNECT_CHECK: Duration = Duration::from_millis(150);
/// Turned away: tries again this many times, this far apart. Nothing has
/// been sent yet, so trying again never prints anything twice.
const REFUSED_RETRIES: usize = 3;
const REFUSED_PAUSE: Duration = Duration::from_secs(1);

/// A one-connection job ([`PrintJob::one_connection`]): over `conn` (or a
/// new connection if the printer turned it away), every part `gap` apart,
/// then ended cleanly ([`Conn::finish`]).
///
/// `conn` is from [`open_or_busy`]: None (turned away while connecting) is
/// tried again like a connection turned away just after.
fn send_whole(conn: Result<Option<Conn>>, target: &PrinterTarget, parts: &[Vec<u8>], gap: Duration, paced: bool) -> Result<()> {
    let mut conn = conn?;
    let mut tries = 0;
    loop {
        if let Some(c) = conn.take() {
            if !c.turned_away() {
                return write_parts(c, parts, gap, paced, true).map_err(printer_error);
            }
        }
        if tries == REFUSED_RETRIES {
            return Err(PosError::Printer("the printer refused the connection (busy with another one?)".into()));
        }
        tries += 1;
        std::thread::sleep(REFUSED_PAUSE);
        conn = open_or_busy(target)?;
    }
}

/// How long the whole of `len` bytes takes to send slowly (the pauses).
pub fn paced_duration(len: usize) -> Duration {
    SLOW_PAUSE * u32::try_from(len.div_ceil(SLOW_CHUNK).saturating_sub(1)).unwrap_or(u32::MAX)
}

/// An open connection to a network printer, or an open port / shared path.
enum Conn {
    Tcp(TcpStream),
    File(std::fs::File),
}

impl Conn {
    /// End a job cleanly: say it is the end (TCP half-close), then wait, at
    /// most [`FINISH_WAIT`], until the printer has read it all and closes its
    /// side, reading (and dropping) any status bytes it sends meanwhile.
    /// The next job then never connects while the printer is still taking
    /// this one in, and unread status bytes never turn the close into a
    /// reset that can lose the end of the job.
    ///
    /// A reset meanwhile is an error: the printer may not have printed it all.
    fn finish(self) -> std::io::Result<()> {
        let Conn::Tcp(mut s) = self else { return Ok(()) };
        s.shutdown(Shutdown::Write)?;
        let end = Instant::now() + FINISH_WAIT;
        let mut buf = [0u8; 256];
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            s.set_read_timeout(Some(left))?;
            match s.read(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(e) if is_reset(&e) => return Err(e),
                Err(_) => return Ok(()),
            }
        }
    }

    /// True when the printer turns this new connection away: resets or
    /// closes it within [`CONNECT_CHECK`], before anything was sent. Status
    /// bytes it sends are left to be read (see [`Conn::finish`]).
    fn turned_away(&self) -> bool {
        let Conn::Tcp(s) = self else { return false };
        if s.set_read_timeout(Some(CONNECT_CHECK)).is_err() {
            return false;
        }
        let away = match s.peek(&mut [0u8; 1]) {
            Ok(n) => n == 0,
            Err(e) => is_reset(&e),
        };
        let _ = s.set_read_timeout(None);
        away
    }
}

fn is_reset(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{BrokenPipe, ConnectionAborted, ConnectionReset};
    matches!(e.kind(), ConnectionReset | ConnectionAborted | BrokenPipe)
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.write(buf),
            Conn::File(f) => f.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Tcp(s) => s.flush(),
            Conn::File(f) => f.flush(),
        }
    }
}

fn connect(host: &str, port: u16) -> Result<TcpStream> {
    connect_io(host, port).map_err(|(_, message)| PosError::Printer(message))
}

/// [`connect`], keeping the error itself next to its message, so a
/// connection the printer reset (busy) can be told from one that failed.
fn connect_io(host: &str, port: u16) -> std::result::Result<TcpStream, (std::io::Error, String)> {
    let plain = |e: std::io::Error| {
        let m = e.to_string();
        (e, m)
    };
    let addr = (host.trim(), port).to_socket_addrs().map_err(plain)?.next().ok_or_else(|| {
        let m = format!("cannot resolve {host}");
        (std::io::Error::other(m.clone()), m)
    })?;
    let s = TcpStream::connect_timeout(&addr, NET_TIMEOUT).map_err(|e| {
        let m = format!("cannot reach {host}:{port} ({e})");
        (e, m)
    })?;
    s.set_write_timeout(Some(NET_TIMEOUT)).map_err(plain)?;
    Ok(s)
}

fn open_file(path: &str) -> Result<std::fs::File> {
    std::fs::OpenOptions::new().create(true).append(true).open(path.trim()).map_err(|e| PosError::Printer(format!("{path}: {e}")))
}

/// Open a paced printer (network, or port / path).
/// Open a one-connection job's printer ([`PrintJob::one_connection`]):
/// None when the printer turned the connection away while it was being
/// made (reset it: busy with another connection). That is tried again
/// ([`send_whole`]), never reported as the printer being out of reach.
fn open_or_busy(target: &PrinterTarget) -> Result<Option<Conn>> {
    match target {
        PrinterTarget::Network { host, port } => match connect_io(host, *port) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                Ok(Some(Conn::Tcp(s)))
            }
            Err((e, _)) if is_reset(&e) => Ok(None),
            Err((_, message)) => Err(PosError::Printer(message)),
        },
        _ => open(target).map(Some),
    }
}

fn open(target: &PrinterTarget) -> Result<Conn> {
    match target {
        PrinterTarget::Network { host, port } => {
            let s = connect(host, *port)?;
            // Each piece leaves at once, not held back to join the next.
            let _ = s.set_nodelay(true);
            Ok(Conn::Tcp(s))
        }
        PrinterTarget::File { path } => Ok(Conn::File(open_file(path)?)),
        _ => Err(PosError::Printer("this printer is not sent slowly".into())),
    }
}

/// Send one part slowly: wait for the printer's turn (ahead of the others
/// waiting when `urgent`), connect, write it in paced pieces, close.
fn send_paced(target: &PrinterTarget, bytes: &[u8], urgent: bool) -> Result<()> {
    let _turn = Ticket::new(target, urgent).wait();
    let mut conn = open(target)?;
    write_paced(&mut conn, bytes, SLOW_CHUNK, SLOW_PAUSE).map_err(|e| PosError::Printer(e.to_string()))
}

/// Where a paced job stands when the till hands it over: sending now (the
/// printer was free and is connected), or waiting in the printer's line.
enum Place {
    /// The connection: None when a one-connection job's printer turned it
    /// away (see [`open_or_busy`]).
    Now(Turn, Option<Conn>),
    Queued(Ticket),
}

/// Background sends still running. [`wait_for_prints`] lets the app finish
/// them before it closes or restarts for an update: a picture cut off half
/// way makes the printer print the next job as junk.
pub struct InFlight {
    count: Mutex<usize>,
    idle: Condvar,
}

/// One send counted by [`InFlight::start`]; dropping it ends it.
pub struct Sending<'a>(&'a InFlight);

impl InFlight {
    pub const fn new() -> InFlight {
        InFlight { count: Mutex::new(0), idle: Condvar::new() }
    }

    pub fn start(&self) -> Sending<'_> {
        *self.count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        Sending(self)
    }

    /// Sends still running.
    pub fn running(&self) -> usize {
        *self.count.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Wait until none is running, at most `limit`. True when all finished.
    pub fn wait(&self, limit: Duration) -> bool {
        let end = Instant::now() + limit;
        let mut n = self.count.lock().unwrap_or_else(|e| e.into_inner());
        while *n > 0 {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            n = self.idle.wait_timeout(n, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        true
    }
}

impl Default for InFlight {
    fn default() -> Self {
        InFlight::new()
    }
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        let mut n = self.0.count.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        if *n == 0 {
            self.0.idle.notify_all();
        }
    }
}

static SENDING: InFlight = InFlight::new();

/// Before the app closes or restarts: let prints still going out finish,
/// waiting at most `limit`. True when none is left.
pub fn wait_for_prints(limit: Duration) -> bool {
    SENDING.wait(limit)
}

/// A print that failed after the till was told it was sent (a paced job, or
/// a later copy): shown on the tills and written to the print error log.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrintFailure {
    /// Increasing number: ask for those after the last one seen.
    pub id: u64,
    /// Unix seconds.
    pub at: i64,
    pub message: String,
}

const MAX_FAILURES: usize = 20;
/// The print error log is started afresh (the old one kept as `.old`) past this size.
const MAX_LOG_BYTES: u64 = 256 * 1024;

static FAILURE_ID: AtomicU64 = AtomicU64::new(0);

fn failures() -> &'static Mutex<VecDeque<PrintFailure>> {
    static LIST: OnceLock<Mutex<VecDeque<PrintFailure>>> = OnceLock::new();
    LIST.get_or_init(Default::default)
}

fn error_log() -> &'static Mutex<Option<PathBuf>> {
    static LOG: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    LOG.get_or_init(Default::default)
}

/// Where print failures are written (the app sets `print-errors.log` in its
/// data folder; none = not written).
pub fn set_print_error_log(path: Option<PathBuf>) {
    *error_log().lock().unwrap_or_else(|e| e.into_inner()) = path;
}

fn printer_name(t: &PrinterTarget) -> String {
    match t {
        PrinterTarget::None => "no printer".into(),
        PrinterTarget::Network { host, port } => format!("the printer at {}:{port}", host.trim()),
        PrinterTarget::Windows { name } => format!("printer {name}"),
        PrinterTarget::File { path } => format!("printer {}", path.trim()),
    }
}

/// Remember a failure for the tills and append it to the print error log.
fn record_failure(what: &str, target: &PrinterTarget, error: &str) {
    let what = if what.is_empty() { "A print" } else { what };
    let message = format!("{what} did not finish printing on {} ({error}): print it again", printer_name(target));
    let now = chrono::Local::now();
    {
        let mut list = failures().lock().unwrap_or_else(|e| e.into_inner());
        list.push_back(PrintFailure { id: FAILURE_ID.fetch_add(1, Ordering::SeqCst) + 1, at: now.timestamp(), message: message.clone() });
        while list.len() > MAX_FAILURES {
            list.pop_front();
        }
    }
    eprintln!("printer: {message}");
    let path = error_log().lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(path) = path {
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
            let _ = std::fs::rename(&path, path.with_extension("old.log"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{}  {message}", now.format("%Y-%m-%d %H:%M:%S"));
        }
    }
}

/// What a till's poll for print failures gets.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrintFailures {
    pub failures: Vec<PrintFailure>,
    /// The newest failure's number (0 = none yet): ask for those after it next.
    pub last: u64,
}

/// Print failures after number `after`; with none, those of the last two
/// minutes (a till that just started or logged in).
pub fn print_failures(after: Option<u64>) -> PrintFailures {
    let since = chrono::Local::now().timestamp() - 120;
    let list = failures().lock().unwrap_or_else(|e| e.into_inner());
    let failures = list.iter().filter(|f| after.map_or(f.at >= since, |a| f.id > a)).cloned().collect();
    PrintFailures { failures, last: list.back().map_or(0, |f| f.id) }
}

/// One printer's line: whether a job is sending now, and the jobs waiting,
/// in the order they go (an urgent one is put at the front).
#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    turn: Condvar,
}

#[derive(Default)]
struct GateState {
    busy: bool,
    waiting: VecDeque<u64>,
    next: u64,
}

fn gate(target: &PrinterTarget) -> Arc<Gate> {
    static GATES: OnceLock<Mutex<HashMap<String, Arc<Gate>>>> = OnceLock::new();
    let key = match target {
        PrinterTarget::Network { host, port } => format!("net {}:{port}", host.trim().to_lowercase()),
        PrinterTarget::File { path } => format!("file {}", path.trim().to_lowercase()),
        PrinterTarget::Windows { name } => format!("win {name}"),
        PrinterTarget::None => String::new(),
    };
    let mut gates = GATES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    gates.entry(key).or_default().clone()
}

/// Holding a [`Turn`] = sending to that printer now; dropping it lets the
/// next job go. Paced jobs to one printer go one at a time, in order, so a
/// printer that takes one connection at a time never sees two.
struct Turn(Arc<Gate>);

impl Turn {
    /// The turn if nobody is sending to (or waiting for) this printer.
    fn try_take(target: &PrinterTarget) -> Option<Turn> {
        let gate = gate(target);
        let mut st = gate.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.busy || !st.waiting.is_empty() {
            return None;
        }
        st.busy = true;
        drop(st);
        Some(Turn(gate))
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut st = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        st.busy = false;
        self.0.turn.notify_all();
    }
}

/// A place in a printer's line, taken when the job is handed over (so jobs
/// keep their order whichever thread runs first); [`Ticket::wait`] turns it
/// into the [`Turn`]. Urgent: at the front of the line, after the job
/// sending now.
struct Ticket {
    gate: Arc<Gate>,
    mine: u64,
    done: bool,
}

impl Ticket {
    fn new(target: &PrinterTarget, urgent: bool) -> Ticket {
        let gate = gate(target);
        let mut st = gate.state.lock().unwrap_or_else(|e| e.into_inner());
        let mine = st.next;
        st.next += 1;
        if urgent {
            st.waiting.push_front(mine);
        } else {
            st.waiting.push_back(mine);
        }
        drop(st);
        Ticket { gate, mine, done: false }
    }

    fn wait(mut self) -> Turn {
        let mut st = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        while st.busy || st.waiting.front() != Some(&self.mine) {
            st = self.gate.turn.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.waiting.pop_front();
        st.busy = true;
        drop(st);
        self.done = true;
        Turn(self.gate.clone())
    }
}

impl Drop for Ticket {
    /// A place never used: leave the line so the jobs behind still go.
    fn drop(&mut self) {
        if !self.done {
            let mut st = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
            st.waiting.retain(|&t| t != self.mine);
            self.gate.turn.notify_all();
        }
    }
}

fn send_bytes(target: &PrinterTarget, bytes: &[u8]) -> Result<()> {
    let fail = PosError::Printer;
    match target {
        PrinterTarget::None => Ok(()),
        PrinterTarget::Network { host, port } => {
            let mut s = connect(host, *port)?;
            s.write_all(bytes).and_then(|_| s.flush()).map_err(|e| fail(e.to_string()))
        }
        PrinterTarget::File { path } => open_file(path)?.write_all(bytes).map_err(|e| fail(e.to_string())),
        PrinterTarget::Windows { name } => windows_spool::print_raw(name, bytes).map_err(fail),
    }
}

/// Printers installed in Windows (empty elsewhere).
pub fn system_printers() -> Vec<String> {
    windows_spool::list()
}

// ------------------------------------------------------------------ ticket

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketItem {
    /// The set menu this dish belongs to ("SET A"); its dishes print together
    /// under the set's name, each section under its own rule. Laid out by
    /// course (see `KitchenTicket::by_course`): every set on the ticket, one
    /// per line ("2 x LUNCH SET\nDINNER SET"), and the course as `category`.
    pub set: Option<String>,
    /// The team header to print above this dish when every team shares one
    /// ticket (separate team tickets switched off).
    pub team: Option<String>,
    /// Menu category, used to group the ticket ("STARTERS", "MAINS"...).
    pub category: Option<String>,
    pub qty: i64,
    pub name: String,
    /// Printed under the name (Thai), when the printer prints both.
    pub name_alt: Option<String>,
    pub options: Vec<String>,
    pub note: Option<String>,
    /// The note on the set line itself ("no peanuts in anything"), printed
    /// under the set's name. By course, every set's note, one per line
    /// ("DINNER SET: no peanuts" when there are several sets).
    pub set_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KitchenTicket {
    /// "NEW ORDER", "ADDITIONAL", "MAINS AWAY" or "REPRINT".
    pub title: String,
    /// "DINE-IN T3", "DELIVERY", ...
    pub heading: String,
    /// Floor, customer name/phone, order notes.
    pub details: Vec<String>,
    pub order_id: i64,
    pub time: String,
    pub staff: String,
    pub items: Vec<TicketItem>,
    /// Printed after the items, e.g. "MAINS TO FOLLOW: 3".
    pub footer: Option<String>,
    /// Just the title, table and dishes (the "main course" call).
    pub brief: bool,
    /// Dishes in double-size text (see `PrinterConfig::large_text`).
    pub large: bool,
    /// The team's name as a big white-on-black header, at the very top
    /// (name, then "\n" and the Thai name when shown).
    pub banner: Option<String>,
    /// Set dishes grouped by course rather than set by set: two or more sets
    /// sent with their starters and mains at once (see `Pos::sets_by_course`).
    /// The sets' names head the dishes, then all the starters together, then
    /// all the mains, identical dishes on one line.
    pub by_course: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Size {
    Normal,
    /// Double height: same characters per line, easy to read from a distance.
    Tall,
    /// Double width and height: half the characters per line.
    Big,
}

pub(crate) struct Line {
    pub size: Size,
    pub bold: bool,
    pub center: bool,
    pub text: String,
    /// A big header: white text on a solid black bar, printed as an image.
    /// `text` is the name, then "\n" and a second name when there is one.
    pub inverse: bool,
    /// Words that must stay on one line (the "MAINS TO FOLLOW" footer):
    /// in the app's font the first of these that fits prints, as big as it
    /// fits (double size first). Empty for an ordinary line.
    pub fit: Vec<String>,
    /// A line of two columns (a dish and its price): the left words and the
    /// amount that `text` pads to the right edge. When the line prints as
    /// an image (Thai), the amount is drawn at the paper's edge, as the
    /// padding in characters does not match the Thai font's widths.
    pub columns: Option<(String, String)>,
}

impl Line {
    pub fn new(size: Size, bold: bool, center: bool, text: impl Into<String>) -> Line {
        Line { size, bold, center, text: text.into(), inverse: false, fit: Vec::new(), columns: None }
    }

    /// This line as `left` and `right` columns (see [`Line::columns`]).
    pub fn with_columns(mut self, left: &str, right: &str) -> Line {
        self.columns = Some((left.to_string(), right.to_string()));
        self
    }

    /// This line on one line in the app's font: the first of `words` that
    /// fits (see [`Line::fit`]).
    pub fn fitting(mut self, words: Vec<String>) -> Line {
        self.fit = words;
        self
    }

    pub fn banner(text: impl Into<String>) -> Line {
        Line { size: Size::Big, bold: true, center: true, text: text.into(), inverse: true, fit: Vec::new(), columns: None }
    }
}

/// A line of `ch` across the paper: "=====..." between blocks, "-----..."
/// between sections.
pub(crate) fn rule(ch: char, width: usize) -> String {
    ch.to_string().repeat(width)
}

/// `text` in the middle of a rule: "--------- STARTERS ---------".
pub(crate) fn titled_rule(text: &str, ch: char, width: usize) -> String {
    let inner = format!(" {text} ");
    let n = cols(&inner);
    // Thai prints as an image whose letters are wider than the printer's own,
    // so it gets a short rule each side rather than one filled to the edge.
    if n + 4 > width || crate::raster::needs_raster(text) {
        return format!("{ch}{ch}{inner}{ch}{ch}");
    }
    let left = (width - n) / 2;
    format!("{}{inner}{}", rule(ch, left), rule(ch, width - n - left))
}

/// Plain-text rendering of laid-out lines (previews and tests).
pub(crate) fn render_text(lines: &[Line], width: usize) -> String {
    let mut s = String::new();
    for l in lines {
        if l.inverse {
            // Plain-text stand-in for the black bar.
            for part in l.text.split('\n') {
                let text = format!("[ {} ]", part.trim_start_matches('\u{1}'));
                s.push_str(&" ".repeat(width.saturating_sub(cols(&text)) / 2));
                s.push_str(&text);
                s.push('\n');
            }
            continue;
        }
        let w = if l.size == Size::Big { width / 2 } else { width };
        let pad = if l.center { w.saturating_sub(cols(&l.text)) / 2 } else { 0 };
        s.push_str(&" ".repeat(pad));
        s.push_str(l.text.trim_end());
        s.push('\n');
    }
    s
}

/// ESC/POS bytes for laid-out lines, code page 437, ending with a feed and
/// partial cut. Lines with characters outside code page 437 (Thai) are
/// printed as images, `width` characters wide. `bold` prints every line in
/// bold (like the Thai font), not just the headings.
pub(crate) fn render_escpos(lines: &[Line], width: usize, bold: bool) -> Vec<u8> {
    render_escpos_font(lines, width, bold, false)
}

/// As [`render_escpos`]; with `app_font` every line of words prints in the
/// app's own font (Sarabun, as on the team bars) instead of the printer's.
/// Rules ("=====", "--- SET ---") and blank lines stay the printer's, so
/// they still run edge to edge.
pub(crate) fn render_escpos_font(lines: &[Line], width: usize, bold: bool, app_font: bool) -> Vec<u8> {
    render_escpos_in(lines, width, bold, app_font, ImageMode::Raster)
}

/// As [`render_escpos_font`], with images sent in `mode` (the printer's
/// "Image mode" setting).
pub(crate) fn render_escpos_in(lines: &[Line], width: usize, bold: bool, app_font: bool, mode: ImageMode) -> Vec<u8> {
    let dots = width * crate::raster::DOTS_PER_CHAR;
    let is_rule = |t: &str| {
        let t = t.trim();
        t.is_empty() || ((t.starts_with('=') || t.starts_with('-')) && !crate::raster::needs_raster(t))
    };
    // Init, code page 437, and the USA character set: printers set to the UK
    // set print "#" as "£", which turned "#12" into "£12".
    let mut b = vec![0x1B, b'@', 0x1B, b't', 0, 0x1B, b'R', 0];
    b.extend_from_slice(mode.init_extra());
    for l in lines {
        if l.inverse {
            b.extend(crate::raster::escpos_banner_in(&l.text, dots, mode));
            continue;
        }
        // A heading inside a rule ("------ MAIN ------") in the app's font: the
        // words drawn with a short rule each side, as the font is wider.
        let t = l.text.trim();
        let rule_ch = t.chars().next().filter(|c| *c == '=' || *c == '-');
        if let (true, Some(c)) = (app_font, rule_ch) {
            if t.chars().any(char::is_alphanumeric) {
                let inner = t.trim_matches(c).trim();
                let short = c.to_string().repeat(3);
                let ruled = format!("{short} {inner} {short}");
                // Too long for the paper with its rules ("=== SET: …" with "===" alone
                // on the next line): the words alone, smaller if they still do not fit.
                let fits = |s: &str, size| crate::raster::fits_line(s, size, dots);
                let (text, size) = if fits(&ruled, l.size) {
                    (ruled, l.size)
                } else if fits(inner, l.size) {
                    (inner.to_string(), l.size)
                } else {
                    (inner.to_string(), Size::Normal)
                };
                b.extend(crate::raster::escpos_line_in(&text, size, true, dots, mode));
                continue;
            }
        }
        // Words that must stay on one line (the footer): the first that
        // fits, at the biggest size it fits; else the shortest, normal size.
        if app_font && !l.fit.is_empty() {
            let fits = |s: &str, size| crate::raster::fits_line(s, size, dots);
            let (text, size) = [Size::Big, Size::Tall, Size::Normal]
                .into_iter()
                .find_map(|size| l.fit.iter().find(|t| fits(t, size)).map(|t| (t.as_str(), size)))
                .unwrap_or((l.fit[l.fit.len() - 1].as_str(), Size::Normal));
            b.extend(crate::raster::escpos_line_in(text, size, l.center, dots, mode));
            continue;
        }
        if let Some((left, right)) = l.columns.as_ref().filter(|_| crate::raster::needs_raster(&l.text)) {
            b.extend(crate::raster::escpos_columns_in(left, right, l.size, dots, mode));
            continue;
        }
        if crate::raster::needs_raster(&l.text) || (app_font && !is_rule(&l.text)) {
            b.extend(crate::raster::escpos_line_in(l.text.trim_end(), l.size, l.center, dots, mode));
            continue;
        }
        b.extend_from_slice(&[0x1B, b'a', u8::from(l.center)]);
        b.extend_from_slice(&[0x1D, b'!', match l.size {
            Size::Normal => 0x00,
            Size::Tall => 0x01,
            Size::Big => 0x11,
        }]);
        b.extend_from_slice(&[0x1B, b'E', u8::from(bold || l.bold)]);
        b.extend(l.text.trim_end().chars().map(cp437));
        b.push(b'\n');
    }
    b.extend_from_slice(&[0x1D, b'!', 0, 0x1B, b'E', 0, 0x1B, b'a', 0]);
    b.extend_from_slice(&[0x1B, b'd', 4]); // feed past the cutter
    b.extend_from_slice(&[0x1D, b'V', 66, 0]); // partial cut
    b
}

/// Word-wrap `text` to `width` columns. The first line starts with `first`,
/// continuation lines with `indent`; over-long words are split.
pub(crate) fn wrap(text: &str, width: usize, first: &str, indent: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = first.to_string();
    let mut start = cols(first);
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = cols(&cur);
            let space = usize::from(used > start);
            let len = word.iter().filter(|c| !is_mark(**c)).count();
            if used + space + len <= width {
                if space == 1 {
                    cur.push(' ');
                }
                cur.extend(word.iter());
                break;
            }
            if used > start {
                out.push(std::mem::replace(&mut cur, indent.to_string()));
                start = cols(indent);
                continue;
            }
            // Split an over-long word where Thai allows (see `may_break`).
            let room = width.saturating_sub(used).max(1);
            let mut take = 0;
            let mut seen = 0;
            while take < word.len() && (seen < room || is_mark(word[take])) {
                seen += usize::from(!is_mark(word[take]));
                take += 1;
            }
            let take = split_at(&word, take, room as f32, |i| word[..i].iter().filter(|c| !is_mark(**c)).count() as f32);
            cur.extend(word.drain(..take));
            out.push(std::mem::replace(&mut cur, indent.to_string()));
            start = cols(indent);
            if word.is_empty() {
                break;
            }
        }
    }
    if cols(&cur) > start || out.is_empty() {
        out.push(cur);
    }
    out
}

/// Thai vowel and tone marks sit above or below the letter before them and
/// take no width of their own.
fn is_mark(c: char) -> bool {
    matches!(c, '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}')
}

/// Thai vowels written before the consonant they follow in speech
/// (เ แ โ ใ ไ): they always start a syllable.
fn is_leading_vowel(c: char) -> bool {
    matches!(c, '\u{0E40}'..='\u{0E44}')
}

/// True when a word may be split before `chars[i]` (Thai has no spaces
/// between words). Never inside a letter's cluster (its vowel and tone
/// marks, sara am), never after a leading vowel (it goes with the next
/// letter), never before a vowel that cannot start a syllable (ะ า ำ ๅ),
/// ๆ or ฯ, and never before a letter silenced by ์ or the letter before
/// that (they end the syllable: "พานต์", "ศุกร์"). Any other place is
/// allowed, as in English.
pub(crate) fn may_break(chars: &[char], i: usize) -> bool {
    if i == 0 || i >= chars.len() {
        return false;
    }
    let (prev, next) = (chars[i - 1], chars[i]);
    if is_mark(next) || matches!(next, '\u{0E30}' | '\u{0E32}' | '\u{0E33}' | '\u{0E45}' | '\u{0E46}' | '\u{0E2F}') || is_leading_vowel(prev) {
        return false;
    }
    let marks = |k: usize| chars.get(k + 1..).unwrap_or_default().iter().take_while(|c| is_mark(**c));
    let silenced = |k: usize| marks(k).any(|c| *c == '\u{0E4C}');
    let after = i + 1 + marks(i).count();
    !(silenced(i) || (after < chars.len() && silenced(after)))
}

/// A place [`may_break`] allows that most likely starts a Thai syllable:
/// before a leading vowel, or after ะ, ำ or ๆ. Preferred when it does not
/// leave the line too short.
pub(crate) fn good_break(chars: &[char], i: usize) -> bool {
    may_break(chars, i) && (is_leading_vowel(chars[i]) || matches!(chars[i - 1], '\u{0E30}' | '\u{0E33}' | '\u{0E46}'))
}

/// True when `chars[i]` is most likely the last letter of a syllable: a
/// bare Thai consonant at the end of the word or before a leading vowel
/// (the น of "หวานไก่"). Splitting before it is allowed, but a poor choice.
fn ends_syllable(chars: &[char], i: usize) -> bool {
    let bare = matches!(chars[i], '\u{0E01}'..='\u{0E2E}') && !chars.get(i + 1).is_some_and(|c| is_mark(*c));
    bare && chars.get(i + 1).is_none_or(|c| is_leading_vowel(*c) || !matches!(c, '\u{0E00}'..='\u{0E7F}'))
}

/// Where to split a word that does not fit: the last place at or before
/// `max` (characters) that [`may_break`] allows, preferring a likely
/// syllable start (see [`good_break`]) that leaves the line at least half
/// full (`fill(i)` is the width up to `i`, `room` the line's), then one that
/// does not leave a syllable's last letter behind; `max` if none is allowed.
pub(crate) fn split_at(chars: &[char], max: usize, room: f32, fill: impl Fn(usize) -> f32) -> usize {
    let max = max.min(chars.len());
    if max == chars.len() {
        return max;
    }
    let last = |pick: &dyn Fn(usize) -> bool| (1..=max).rev().find(|&i| pick(i));
    last(&|i| good_break(chars, i))
        .filter(|&i| fill(i) * 2.0 >= room)
        .or_else(|| last(&|i| may_break(chars, i) && !ends_syllable(chars, i)))
        .or_else(|| last(&|i| may_break(chars, i)))
        .unwrap_or(max)
}

/// Real dish names, and one with English and Thai.
#[cfg(test)]
pub(crate) const THAI_DISHES: [&str; 6] =
    ["แกงเขียวหวานไก่", "ผัดไทยกุ้งสด", "ต้มยำกุ้งน้ำข้น", "ไก่ผัดเม็ดมะม่วงหิมพานต์", "ข้าวเหนียวมะม่วง", "Pad Thai ผัดไทย"];

/// Checks pieces of wrapped text, written apart from `may_break`: no
/// piece starts with a mark, sara am or a vowel that cannot start a
/// syllable, and none ends with a leading vowel.
#[cfg(test)]
pub(crate) fn assert_thai_breaks(pieces: &[String]) {
    for pair in pieces.windows(2) {
        let (a, b) = (pair[0].trim(), pair[1].trim());
        let (Some(last), Some(first)) = (a.chars().last(), b.chars().next()) else { continue };
        assert!(!('\u{0E40}'..='\u{0E44}').contains(&last), "leading vowel left at the end: {pieces:?}");
        assert!(
            !matches!(first, '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}' | 'ะ' | 'า' | 'ำ' | 'ๅ' | 'ๆ' | 'ฯ'),
            "piece starts with {first:?}: {pieces:?}"
        );
    }
}

/// Printed width in characters.
pub(crate) fn cols(text: &str) -> usize {
    text.chars().filter(|c| !is_mark(*c)).count()
}

/// `text` cut to at most `w` printed columns, keeping the marks on the
/// last letter.
pub(crate) fn take_cols(text: &str, w: usize) -> String {
    let mut n = 0;
    text.chars()
        .take_while(|c| {
            n += usize::from(!is_mark(*c));
            n <= w
        })
        .collect()
}

/// `text` padded with spaces to `w` printed columns.
pub(crate) fn pad_cols(text: &str, w: usize) -> String {
    format!("{text}{}", " ".repeat(w.saturating_sub(cols(text))))
}

/// The footer of a ticket sent with mains still waiting: "MAINS TO FOLLOW: 3".
const MAINS_TO_FOLLOW: &str = "MAINS TO FOLLOW: ";
/// Its shorter form, for narrow paper.
const MAINS_LATER: &str = "MAINS LATER: ";

/// A ticket footer's words, longest first: "MAINS TO FOLLOW: 11", then
/// "MAINS LATER: 11" (which fits a double-size line on 58 mm paper).
fn footer_words(footer: &str) -> Vec<String> {
    let mut v = vec![footer.to_string()];
    if let Some(n) = footer.strip_prefix(MAINS_TO_FOLLOW) {
        v.push(format!("{MAINS_LATER}{n}"));
    }
    v
}

/// A set section that holds drinks or desserts by its name ("Drinks",
/// "Soft drinks", "Dessert", "เครื่องดื่ม", "ของหวาน"): not called with the mains.
fn not_a_main_section(name: Option<&str>, alt: Option<&str>) -> bool {
    let words = ["drink", "beverage", "dessert", "sweet", "เครื่องดื่ม", "ของหวาน"];
    [name, alt].into_iter().flatten().any(|n| {
        let n = n.to_lowercase();
        words.iter().any(|w| n.contains(w))
    })
}

/// The lines the kitchen cooks: everything but a set's own (billing) line.
pub(crate) fn kitchen_only(items: &[OrderItem]) -> Vec<OrderItem> {
    items.iter().filter(|i| !i.is_set).cloned().collect()
}

/// The courses of set dishes on a ticket laid out by course, in print order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Course {
    Drinks,
    Starters,
    Mains,
    Desserts,
}

impl Course {
    /// The rule over the course's dishes, as on the test ticket ("Starters / อาหารเรียกน้ำย่อย").
    fn label(self, names: PrintNames) -> String {
        let (en, th) = match self {
            Course::Drinks => ("Drinks", "เครื่องดื่ม"),
            Course::Starters => ("Starters", "อาหารเรียกน้ำย่อย"),
            Course::Mains => ("Mains", "อาหารจานหลัก"),
            Course::Desserts => ("Desserts", "ของหวาน"),
        };
        names.inline(en, Some(th))
    }
}

/// Copies of a kitchen ticket titled `title` on `cfg`'s printer: one for a
/// reprint; else the setting for the kind of ticket (`all_courses`: a
/// dine-in send with all courses together; `starters`: only starters),
/// "Same" (None) meaning the printer's copies, or one for ADDITIONAL.
pub(crate) fn kitchen_copies(cfg: &PrinterConfig, title: &str, reprint: bool, all_courses: bool, starters: bool) -> u8 {
    let usual = if title == "MAINS AWAY" {
        cfg.mains_copies.unwrap_or(cfg.copies)
    } else if title == "ADDITIONAL" {
        // More ordered later: one copy unless set otherwise.
        cfg.additional_copies.unwrap_or(1)
    } else if starters {
        // Starters: ahead of the mains, or all the table ordered for now.
        cfg.starter_copies.unwrap_or(cfg.copies)
    } else {
        cfg.copies
    };
    if reprint {
        1
    } else if all_courses {
        cfg.all_courses_copies.unwrap_or(usual)
    } else {
        usual
    }
}

/// Copies of the "Call mains again" ticket (MAIN COURSE) on `cfg`'s printer.
pub(crate) fn call_again_copies(cfg: &PrinterConfig) -> u8 {
    cfg.call_again_copies.or(cfg.mains_copies).unwrap_or(cfg.copies)
}

/// The ESC/POS bytes of several tickets one after another (each ends with a cut).
fn escpos_all(tickets: &[KitchenTicket], cfg: &PrinterConfig) -> Vec<u8> {
    tickets.iter().flat_map(|t| t.escpos_for(cfg)).collect()
}

impl KitchenTicket {
    fn lines(&self, width: usize) -> Vec<Line> {
        let mut v = Vec::new();
        let gap = || Line::new(Size::Normal, false, false, String::new());
        if let Some(b) = &self.banner {
            v.push(Line::banner(b.clone()));
            v.push(gap());
        }
        let mut push = |l: Line| v.push(l);
        let big = width / 2;
        // Dishes, options and notes: double size (large) or double height.
        let (size, w) = if self.large { (Size::Big, big) } else { (Size::Tall, width) };
        let (cont, opt, opt_cont, note, note_cont) =
            if self.large { ("  ", "  + ", "    ", "  ** ", "     ") } else { ("    ", "   + ", "     ", "   ** ", "      ") };
        let small = if self.large { Size::Tall } else { Size::Normal };
        // The "call mains" ticket without dishes: the table on a big bar, and
        // the title under it.
        // Its team's bar (if any) is already at the top.
        if self.brief && self.items.is_empty() {
            push(Line::new(Size::Normal, false, false, rule('=', width)));
            for t in wrap(&self.title, big, "", "") {
                push(Line::new(Size::Big, true, true, t));
            }
            push(Line::new(Size::Normal, false, false, rule('=', width)));
            push(Line::banner(format!("\u{1}{}", self.heading)));
            // One shared ticket: which teams it is for.
            for d in &self.details {
                for t in wrap(d, width, "", "") {
                    push(Line::new(Size::Tall, true, true, t));
                }
            }
            push(Line::new(Size::Normal, false, false, rule('-', width)));
            push(Line::new(Size::Tall, true, true, format!("Called {}", self.time)));
            push(Line::new(Size::Normal, false, false, rule('=', width)));
            return v;
        }
        // Title between double rules, then where it goes and who sent it.
        push(Line::new(Size::Normal, false, false, rule('=', width)));
        for t in wrap(&self.title, big, "", "") {
            push(Line::new(Size::Big, true, true, t));
        }
        push(Line::new(Size::Normal, false, false, rule('=', width)));
        for t in wrap(&self.heading, big, "", "") {
            push(Line::new(Size::Big, true, self.brief, t));
        }
        if !self.brief {
            for d in &self.details {
                for t in wrap(d, width, "", "  ") {
                    push(Line::new(small, false, false, t));
                }
            }
            push(Line::new(Size::Normal, false, false, rule('-', width)));
            push(Line::new(small, false, false, format!("#{}  {}  {}", self.order_id, self.time, self.staff)));
        }
        push(Line::new(Size::Normal, false, false, rule('=', width)));
        push(gap());
        // No menu categories on tickets: only a set's sections get a heading.
        let mut current: Option<Option<&str>> = None;
        let mut team: Option<&str> = None;
        let mut first = true;
        let mut set: Option<&str> = None;
        for it in &self.items {
            if it.team.as_deref() != team {
                // Plenty of room around each team's bar, so its dishes read as one block.
                push(gap());
                push(gap());
                match &it.team {
                    Some(b) => push(Line::banner(b.clone())),
                    // Dishes without a team, after the teams' ones.
                    None => push(Line::new(Size::Normal, false, false, rule('=', width))),
                }
                push(gap());
                push(gap());
                team = it.team.as_deref();
                // Each team's set dishes under their set's name again.
                set = None;
                current = None;
                first = true;
            }
            // A set's dishes: the set's name between double rules.
            if it.set.as_deref() != set {
                if let Some(name) = &it.set {
                    if !first {
                        push(gap());
                        push(gap());
                    }
                    // By course: one rule per set on the ticket.
                    for one in name.lines() {
                        push(Line::new(small, true, true, titled_rule(&format!("SET: {one}"), '=', width)));
                    }
                    // The set line's own note, for every dish of the set.
                    if let Some(n) = &it.set_note {
                        for one in n.lines() {
                            for t in wrap(one, w, note, note_cont) {
                                push(Line::new(size, true, false, t));
                            }
                        }
                    }
                    push(gap());
                }
                set = it.set.as_deref();
                current = None;
                first = true;
            }
            let cat = it.category.as_deref();
            if it.set.is_some() && current != Some(cat) {
                if current.is_some() {
                    push(gap());
                }
                push(Line::new(small, true, true, titled_rule(&cat.unwrap_or("Other").to_uppercase(), '-', width)));
                current = Some(cat);
                first = true;
            }
            // A blank line between dishes, so each one stands apart.
            if !first {
                push(gap());
            }
            first = false;
            for (i, t) in wrap(&format!("{} x {}", it.qty, it.name), w, "", cont).into_iter().enumerate() {
                push(Line::new(size, i == 0, false, t));
            }
            if let Some(alt) = &it.name_alt {
                for t in wrap(alt, w, cont, cont) {
                    push(Line::new(size, false, false, t));
                }
            }
            for o in &it.options {
                for t in wrap(o, w, opt, opt_cont) {
                    push(Line::new(size, false, false, t));
                }
            }
            if let Some(n) = &it.note {
                for t in wrap(n, w, note, note_cont) {
                    push(Line::new(size, true, false, t));
                }
            }
        }
        push(gap());
        push(Line::new(Size::Normal, false, false, rule('=', width)));
        if let Some(f) = &self.footer {
            // On one line, double size when it fits: on 58 mm paper (16
            // double-size characters) "MAINS TO FOLLOW: 11" becomes
            // "MAINS LATER: 11"; the app's font picks again when printing.
            let words = footer_words(f);
            let pick = [(Size::Big, big), (Size::Tall, width)]
                .into_iter()
                .find_map(|(size, room)| words.iter().find(|t| cols(t) <= room).map(|t| (size, t.clone())));
            match pick {
                Some((size, t)) => push(Line::new(size, true, true, t).fitting(words)),
                None => {
                    for t in wrap(f, big, "", "") {
                        push(Line::new(Size::Big, true, true, t));
                    }
                }
            }
            push(Line::new(Size::Normal, false, false, rule('-', width)));
        }
        // The table again at the bottom, where the ticket is read on the pass.
        for t in wrap(&self.heading, big, "", "") {
            push(Line::new(Size::Big, true, true, t));
        }
        v
    }

    /// Plain-text rendering (previews and tests).
    pub fn to_text(&self, width: usize) -> String {
        render_text(&self.lines(width), width)
    }

    /// ESC/POS bytes, code page 437, ending with a feed and partial cut.
    pub fn to_escpos(&self, width: usize, bold: bool) -> Vec<u8> {
        render_escpos(&self.lines(width), width, bold)
    }

    /// ESC/POS bytes, optionally all in the app's font (see `PrinterConfig::app_font`).
    pub fn to_escpos_font(&self, width: usize, bold: bool, app_font: bool) -> Vec<u8> {
        render_escpos_font(&self.lines(width), width, bold, app_font)
    }

    /// The ticket as `cfg`'s printer prints it: its paper width, bold, font
    /// and image mode.
    pub fn escpos_for(&self, cfg: &PrinterConfig) -> Vec<u8> {
        render_escpos_in(&self.lines(cfg.width), cfg.width, cfg.bold, cfg.app_font, cfg.image_mode)
    }
}

/// Map a character to code page 437, the default on ESC/POS printers.
pub(crate) fn cp437(c: char) -> u8 {
    if c.is_ascii() && !c.is_ascii_control() {
        return c as u8;
    }
    match c {
        'Ç' => 0x80, 'ü' => 0x81, 'é' => 0x82, 'â' => 0x83, 'ä' => 0x84, 'à' => 0x85, 'å' => 0x86,
        'ç' => 0x87, 'ê' => 0x88, 'ë' => 0x89, 'è' => 0x8A, 'ï' => 0x8B, 'î' => 0x8C, 'ì' => 0x8D,
        'Ä' => 0x8E, 'Å' => 0x8F, 'É' => 0x90, 'æ' => 0x91, 'Æ' => 0x92, 'ô' => 0x93, 'ö' => 0x94,
        'ò' => 0x95, 'û' => 0x96, 'ù' => 0x97, 'ÿ' => 0x98, 'Ö' => 0x99, 'Ü' => 0x9A, '£' => 0x9C,
        'á' => 0xA0, 'í' => 0xA1, 'ó' => 0xA2, 'ú' => 0xA3, 'ñ' => 0xA4, 'Ñ' => 0xA5, '°' => 0xF8,
        '€' => b'E',
        _ => b'?',
    }
}

/// Setting: cancelling a whole order prints a CANCELLED kitchen ticket
/// ("1", the default) or not ("0"). See [`Pos::prints_cancelled_order`].
pub const PRINT_CANCELLED_ORDER_KEY: &str = "print_cancelled_order";

/// What the tickets call each kind of order, editable in Settings
/// (settings `label_dine_in`, `label_takeaway`, `label_collection`,
/// `label_delivery`, and `label_table` for the "main course" call).
pub const LABEL_KEYS: [(&str, &str); 5] = [
    ("label_dine_in", "DINE-IN"),
    ("label_takeaway", "TAKEAWAY"),
    ("label_collection", "COLLECTION"),
    ("label_delivery", "DELIVERY"),
    ("label_table", "TABLE"),
];

impl Pos {
    /// A saved label, else the default. A dine-in label may be blank (the
    /// table alone is printed); the others fall back to their default.
    pub(crate) fn label(&self, key: &str) -> Result<String> {
        let default = LABEL_KEYS.iter().find(|(k, _)| *k == key).map_or("", |(_, d)| *d);
        Ok(match self.setting(key)? {
            Some(v) if key == "label_dine_in" || key == "label_table" => v.trim().to_string(),
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => default.to_string(),
        })
    }

    /// "DINE-IN T3", "TAKEAWAY"... with the labels from Settings.
    pub(crate) fn heading(&self, o: &Order) -> Result<String> {
        if let Some(ch) = &o.channel_name {
            let r = o.external_ref.as_deref().map(|r| format!(" {r}")).unwrap_or_default();
            return Ok(format!("{}{r}", ch.to_uppercase()));
        }
        Ok(match o.order_type {
            OrderType::DineIn => format!("{} {}", self.label("label_dine_in")?, o.table_name.as_deref().unwrap_or("?")).trim().to_string(),
            OrderType::Takeaway => self.label("label_takeaway")?,
            OrderType::Collection => self.label("label_collection")?,
            OrderType::Delivery => self.label("label_delivery")?,
        })
    }

    /// Items in menu-category order (category position, then name; items
    /// without a category last), each with its category name. Within a
    /// category the order they were added is kept.
    /// `names` decides how the category is labelled.
    pub(crate) fn by_category<'a>(&self, items: &'a [OrderItem], names: PrintNames) -> Result<Vec<(Option<String>, &'a OrderItem)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.name, c.sort_order, c.name_alt FROM products p LEFT JOIN categories c ON c.id = p.category_id
             WHERE p.id = ?1",
        )?;
        let mut keyed = Vec::with_capacity(items.len());
        for (i, it) in items.iter().enumerate() {
            let (name, sort, alt): (Option<String>, Option<i64>, Option<String>) = stmt
                .query_row([it.product_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?
                .unwrap_or((None, None, None));
            let label = name.as_deref().map(|n| names.inline(n, alt.as_deref()));
            keyed.push(((name.is_none(), sort.unwrap_or(0), name.unwrap_or_default(), i), label, it));
        }
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(keyed.into_iter().map(|(_, n, it)| (n, it)).collect())
    }

    pub fn kitchen_printer(&self) -> Result<PrinterConfig> {
        let raw: Option<String> = self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([SETTING_KEY], |r| r.get(0))
            .optional()?;
        Ok(raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default())
    }

    /// The other kitchen printers (none = everything on the main one).
    pub fn kitchen_routes(&self) -> Result<Vec<KitchenRoute>> {
        Ok(self.setting(ROUTES_KEY)?.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default())
    }

    pub fn set_kitchen_routes(&self, s: &Session, routes: &[KitchenRoute]) -> Result<()> {
        s.require(Permission::ManageSettings)?;
        if routes.len() > 8 {
            return invalid("up to 8 extra kitchen printers");
        }
        for r in routes {
            if r.label.trim().is_empty() {
                return invalid("give every printer a name, e.g. Bar");
            }
            if r.target == PrinterTarget::None {
                return invalid(format!("{}: choose the printer", r.label));
            }
            self.route_config(r)?.validate()?;
        }
        let json = serde_json::to_string(routes).map_err(|e| PosError::Internal(e.to_string()))?;
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [ROUTES_KEY, &json],
        )?;
        self.audit(Some(s.user_id), "printer.routes", &json)
    }

    /// An extra printer's settings: the main kitchen printer's look (names,
    /// font, size), its own place, paper width and copies.
    pub fn route_config(&self, r: &KitchenRoute) -> Result<PrinterConfig> {
        Ok(PrinterConfig {
            target: r.target.clone(),
            width: r.width,
            copies: r.copies,
            starter_copies: None,
            mains_copies: None,
            prep_copies: None,
            additional_copies: Some(r.copies),
            all_courses_copies: None,
            call_again_copies: None,
            image_mode: r.image_mode,
            slow_send: r.slow_send,
            ..self.kitchen_printer()?
        })
    }

    /// `items` split by the printer that prints them, the main one first.
    /// Drinks and desserts go to a printer that takes them, other dishes to
    /// the printer of their team, the rest to the main kitchen printer.
    pub(crate) fn route_items(&self, items: &[OrderItem]) -> Result<Vec<(PrinterConfig, Vec<OrderItem>)>> {
        let main = self.kitchen_printer()?;
        let routes = self.kitchen_routes()?;
        if items.is_empty() {
            return Ok(Vec::new());
        }
        if routes.is_empty() {
            return Ok(vec![(main, items.to_vec())]);
        }
        let mut groups: Vec<Vec<OrderItem>> = vec![Vec::new(); routes.len() + 1];
        let mut stmt = self.conn.prepare_cached("SELECT drink, dessert, team_id FROM products WHERE id = ?1")?;
        for it in items {
            let (drink, dessert, menu_team): (bool, bool, Option<i64>) =
                stmt.query_row([it.product_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?.unwrap_or((false, false, None));
            // A dish moved to another team on the order goes to that team's printer.
            let team = it.team_id.or(menu_team);
            let slot = routes
                .iter()
                .position(|r| (drink && r.drinks) || (dessert && r.desserts))
                .or_else(|| routes.iter().position(|r| !drink && !dessert && team.is_some_and(|t| r.team_ids.contains(&t))))
                .map_or(0, |i| i + 1);
            groups[slot].push(it.clone());
        }
        drop(stmt);
        let mut out = Vec::new();
        for (i, group) in groups.into_iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            let cfg = if i == 0 { main.clone() } else { self.route_config(&routes[i - 1])? };
            out.push((cfg, group));
        }
        Ok(out)
    }

    pub fn set_kitchen_printer(&self, s: &Session, cfg: &PrinterConfig) -> Result<()> {
        s.require(Permission::ManageSettings)?;
        cfg.validate()?;
        let json = serde_json::to_string(cfg).map_err(|e| PosError::Internal(e.to_string()))?;
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [SETTING_KEY, &json],
        )?;
        self.audit(Some(s.user_id), "printer.kitchen", &json)
    }

    /// Ticket for the given lines of an order (normally what was just sent).
    pub fn kitchen_ticket(&self, s: &Session, order_id: i64, items: &[OrderItem], reprint: bool) -> Result<KitchenTicket> {
        let items = &kitchen_only(items);
        self.ticket_for(s, order_id, items, reprint, self.sets_by_course(items))
    }

    /// Two or more sets whose starters and mains go to the kitchen at once
    /// ("All courses together", a takeaway, a reprint...): their dishes are
    /// then printed course by course, not set by set (`KitchenTicket::by_course`).
    /// Worked out from the whole send, so every printer's and team's ticket
    /// is laid out the same. One set, or one course at a time: set by set.
    /// Drinks are no course here (they go with the starters anyway).
    pub(crate) fn sets_by_course(&self, items: &[OrderItem]) -> bool {
        let food: Vec<&OrderItem> = items.iter().filter(|i| i.parent_id.is_some() && !i.is_set && !self.is_drink(i.product_id)).collect();
        let mut sets: Vec<i64> = food.iter().filter_map(|i| i.parent_id).collect();
        sets.sort_unstable();
        sets.dedup();
        sets.len() > 1 && food.iter().any(|i| i.starter) && food.iter().any(|i| !i.starter)
    }

    /// [`Pos::kitchen_ticket`], its set dishes by course or set by set.
    fn ticket_for(&self, s: &Session, order_id: i64, items: &[OrderItem], reprint: bool, by_course: bool) -> Result<KitchenTicket> {
        let o = self.get_order(s, order_id)?;
        let first_round = !reprint && o.items.iter().filter(|i| !i.is_set).all(|i| i.sent_at.is_none() || items.iter().any(|x| x.id == i.id));
        let mut details = Vec::new();
        if let Some(f) = &o.floor_name {
            details.push(f.clone());
        }
        let who: Vec<&str> = [o.customer_name.as_deref(), o.customer_phone.as_deref()].into_iter().flatten().collect();
        if !who.is_empty() {
            details.push(who.join("  "));
        }
        if o.channel_name.is_none() && o.order_type == OrderType::Delivery {
            if let Some(a) = &o.delivery_address {
                details.push(a.clone());
            }
        }
        if let Some(n) = &o.notes {
            details.push(format!("NOTE: {n}"));
        }
        let waiting = o.items.iter().filter(|i| i.held && i.sent_at.is_none()).map(|i| i.qty).sum::<i64>();
        // Dine-in: the first mains after the starters went alone are the main
        // course, even when they were ordered later (not ADDITIONAL).
        let mains = |i: &&OrderItem| !i.starter && !i.is_set;
        let first_mains = o.order_type == OrderType::DineIn
            && items.iter().filter(mains).any(|i| !self.is_drink(i.product_id))
            && !o.items.iter().filter(mains).any(|i| i.sent_at.is_some() && !items.iter().any(|x| x.id == i.id) && !self.is_drink(i.product_id));
        let title = if reprint {
            "REPRINT"
        } else if first_round {
            "NEW ORDER"
        } else if items.iter().any(|i| i.held) || first_mains {
            "MAINS AWAY"
        } else {
            "ADDITIONAL"
        };
        let names = self.kitchen_printer()?.names.unwrap_or(PrintNames::Both);
        Ok(KitchenTicket {
            title: title.into(),
            heading: self.heading(&o)?,
            details,
            order_id,
            time: chrono::Local::now().format("%H:%M").to_string(),
            staff: s.name.clone(),
            items: self.ticket_items(items, names, by_course)?,
            footer: (!reprint && waiting > 0).then(|| format!("{MAINS_TO_FOLLOW}{waiting}")),
            brief: false,
            large: self.kitchen_printer()?.large_text,
            banner: None,
            by_course,
        })
    }

    /// The dishes of a ticket, grouped by menu category.
    /// Dishes in menu order, then each set's dishes together under the
    /// set's name, section by section; or, `by_course`, all the sets' dishes
    /// course by course (see [`KitchenTicket::by_course`]).
    fn ticket_items(&self, items: &[OrderItem], names: PrintNames, by_course: bool) -> Result<Vec<TicketItem>> {
        let (dishes, in_sets): (Vec<OrderItem>, Vec<OrderItem>) = kitchen_only(items).into_iter().partition(|i| i.parent_id.is_none());
        let item = |i: &OrderItem, category: Option<String>, set: Option<String>| {
            let (name, name_alt) = names.pick(&i.name, i.name_alt.as_deref());
            let mut t = TicketItem {
                team: None,
                set,
                category,
                qty: i.qty,
                name: name.to_string(),
                name_alt: name_alt.map(str::to_string),
                options: i.options.iter().map(|o| names.inline(&o.name, o.name_alt.as_deref())).collect(),
                note: i.note.clone(),
                set_note: None,
            };
            // A set dish: what it comes with, under the dish like an option.
            if let Some(w) = &i.with_name {
                t.options.insert(0, format!("with {}", names.inline(w, i.with_alt.as_deref())));
            }
            t
        };
        let mut out: Vec<TicketItem> = self.by_category(&dishes, names)?.into_iter().map(|(c, i)| item(i, c, None)).collect();
        let mut parents: Vec<i64> = in_sets.iter().filter_map(|i| i.parent_id).collect();
        parents.dedup();
        let mut stmt = self.conn.prepare_cached("SELECT name, name_alt, qty, note FROM order_items WHERE id = ?1")?;
        // By course: (set's name, how many) for the heading, and each dish
        // with its course and its section's place among that course's sections.
        let mut sets: Vec<(String, i64)> = Vec::new();
        let mut courses: Vec<(Course, usize, Option<String>, TicketItem)> = Vec::new();
        // Section headings under MAINS, in the order first met (one block each).
        let mut headings: Vec<String> = Vec::new();
        // By course: each set's own note (the set line's), for under the heading.
        let mut set_notes: Vec<(String, String)> = Vec::new();
        for parent in parents {
            let (name, alt, qty, set_note): (String, Option<String>, i64, Option<String>) =
                stmt.query_row([parent], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            let set_note = set_note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            let label = names.inline(&name.to_uppercase(), alt.as_deref());
            let children = in_sets.iter().filter(|i| i.parent_id == Some(parent));
            if by_course {
                if let Some(n) = &set_note {
                    if !set_notes.iter().any(|(l, x)| *l == label && x == n) {
                        set_notes.push((label.clone(), n.clone()));
                    }
                }
                match sets.iter_mut().find(|s| s.0 == label) {
                    Some(s) => s.1 += qty,
                    None => sets.push((label, qty)),
                }
                // The set's own main section: the first of its sections (over the
                // whole set, not just this ticket's dishes) whose dishes are mains.
                let main_section = self.main_section_of(parent)?;
                for i in children {
                    let course = self.course_of(i)?;
                    // Under MAINS, a set's other sections (sides, or a "Drinks"
                    // or "Dessert" section whose dishes are not ticked as such)
                    // keep their own heading, so a dessert is never read as a main.
                    let own = (course == Course::Mains && i.section_name != main_section)
                        .then(|| i.section_name.as_deref().map(|n| names.inline(&n.to_uppercase(), i.section_alt.as_deref())))
                        .flatten();
                    if let Some(h) = &own {
                        if !headings.contains(h) {
                            headings.push(h.clone());
                        }
                    }
                    let rank = own.as_ref().and_then(|h| headings.iter().position(|x| x == h)).map_or(0, |p| p + 1);
                    courses.push((course, rank, own, item(i, None, None)));
                }
                continue;
            }
            let set = if qty > 1 { format!("{qty} x {label}") } else { label };
            for i in children {
                let section = i.section_name.as_deref().map(|n| names.inline(&n.to_uppercase(), i.section_alt.as_deref()));
                let mut t = item(i, section, Some(set.clone()));
                t.set_note = set_note.clone();
                out.push(t);
            }
        }
        if courses.is_empty() {
            return Ok(out);
        }
        let heading = sets.iter().map(|(l, q)| if *q > 1 { format!("{q} x {l}") } else { l.clone() }).collect::<Vec<_>>().join("\n");
        // Course by course; under MAINS the mains first, then each other
        // section's block. A stable sort keeps the dishes in set order.
        courses.sort_by_key(|c| (c.0, c.1));
        let mut merged: Vec<(Course, Option<String>, TicketItem)> = Vec::new();
        for (course, _, own, t) in courses {
            // The same dish, options and note from another set: one line.
            let same = |m: &&mut (Course, Option<String>, TicketItem)| {
                m.0 == course && m.1 == own && m.2.name == t.name && m.2.name_alt == t.name_alt && m.2.options == t.options && m.2.note == t.note
            };
            match merged.iter_mut().find(same) {
                Some(m) => m.2.qty += t.qty,
                None => merged.push((course, own, t)),
            }
        }
        // One set: its note as it is; several: which set each note is for.
        let several = sets.len() > 1;
        let set_note = (!set_notes.is_empty())
            .then(|| set_notes.iter().map(|(l, n)| if several { format!("{l}: {n}") } else { n.clone() }).collect::<Vec<_>>().join("\n"));
        for (course, own, mut t) in merged {
            t.set = Some(heading.clone());
            t.set_note = set_note.clone();
            t.category = Some(own.unwrap_or_else(|| course.label(names)));
            out.push(t);
        }
        Ok(out)
    }

    /// The section of set line `parent` that holds its mains: the first
    /// section, in the set's order, with a dish that is neither a starter
    /// nor ticked Drink or Dessert. Looked at over all the set's dishes, so a
    /// team's ticket that has only the sides still knows they are sides.
    fn main_section_of(&self, parent: i64) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT i.section_name FROM order_items i LEFT JOIN products p ON p.id = i.product_id
             WHERE i.parent_id = ?1 AND i.starter = 0 AND COALESCE(p.drink, 0) = 0 AND COALESCE(p.dessert, 0) = 0
             ORDER BY i.id LIMIT 1",
        )?;
        Ok(stmt.query_row([parent], |r| r.get::<_, Option<String>>(0)).optional()?.flatten())
    }

    /// A set dish's course on a ticket laid out by course: drinks and
    /// desserts (as ticked on the menu) apart, else starter or main.
    fn course_of(&self, i: &OrderItem) -> Result<Course> {
        let (drink, dessert): (bool, bool) = self
            .conn
            .prepare_cached("SELECT drink, dessert FROM products WHERE id = ?1")?
            .query_row([i.product_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .unwrap_or_default();
        Ok(if drink {
            Course::Drinks
        } else if i.starter {
            Course::Starters
        } else if dessert {
            Course::Desserts
        } else {
            Course::Mains
        })
    }

    /// `items` split by the team that cooks them, in team order; items with
    /// no team come last. Each group is (the team's header, its items).
    /// The team is the one the dish was moved to on the order, else its menu
    /// item's, looked up now, so a menu change applies to orders already open.
    fn team_groups(&self, items: &[OrderItem]) -> Result<Vec<(Option<String>, Vec<OrderItem>)>> {
        // Settings → Kitchen teams can switch teams off altogether.
        if self.setting("teams_on")?.as_deref() == Some("0") {
            return Ok(vec![(None, items.to_vec())]);
        }
        let teams = crate::teams::load_teams(&self.conn)?;
        let label = self.team_label()?;
        let mut stmt = self.conn.prepare_cached("SELECT team_id FROM products WHERE id = ?1")?;
        let mut groups: Vec<(Option<String>, Vec<OrderItem>)> = teams
            .iter()
            .map(|t| {
                (Some(label(&t.name, t.name_alt.as_deref())), Vec::new())
            })
            .collect();
        groups.push((None, Vec::new()));
        let general = groups.len() - 1;
        for it in items {
            let team: Option<i64> = match it.team_id {
                Some(t) => Some(t),
                None => stmt.query_row([it.product_id], |r| r.get(0)).optional()?.flatten(),
            };
            let slot = team.and_then(|id| teams.iter().position(|t| t.id == id)).unwrap_or(general);
            groups[slot].1.push(it.clone());
        }
        groups.retain(|g| !g.1.is_empty());
        Ok(groups)
    }

    /// One kitchen ticket per team that has something in `items`, each headed
    /// by the team's name; items with no team get a plain ticket. The title
    /// (NEW ORDER, ADDITIONAL...) is worked out from the whole send, so every
    /// team's ticket says the same. With no teams in use this is the single
    /// plain ticket of `kitchen_ticket`.
    /// Drinks come first, on their own DRINKS ticket.
    pub fn kitchen_tickets(&self, s: &Session, order_id: i64, items: &[OrderItem], reprint: bool) -> Result<Vec<KitchenTicket>> {
        let items = &kitchen_only(items);
        let base = self.kitchen_ticket(s, order_id, items, reprint)?;
        self.split_for_kitchen(base, items)
    }

    /// A short ticket telling the kitchen about a change to dishes already
    /// sent: "CANCELLED" (removed or fewer) or "CHANGED" (a new note), split
    /// like any other ticket so the right team gets it. One copy.
    pub fn kitchen_change_job(&self, s: &Session, order_id: i64, title: &str, items: &[OrderItem]) -> Result<Option<PrintJob>> {
        let items = kitchen_only(items);
        let by_course = self.sets_by_course(&items);
        let mut jobs = Vec::new();
        for (cfg, group) in self.route_items(&items)? {
            if cfg.target == PrinterTarget::None {
                continue;
            }
            let mut base = self.ticket_for(s, order_id, &group, false, by_course)?;
            base.title = title.to_string();
            base.footer = None;
            let tickets = self.split_for_kitchen(base, &group)?;
            jobs.push(PrintJob::copies(cfg.target.clone(), &[], escpos_all(&tickets, &cfg), 1, cfg.copy_delay_secs).paced(cfg.slow_send).for_mode(cfg.image_mode));
        }
        Ok(PrintJob::join(jobs).map(|j| j.about(format!("{title} ticket #{order_id}"))))
    }

    /// [`Pos::update_item`], with what the kitchen must be told about a dish
    /// already sent: fewer or removed prints CANCELLED (a set with the dishes
    /// of it that were sent), a new note CHANGED. None when nothing to tell
    /// or no kitchen printer is set up.
    pub fn update_item_for_kitchen(&mut self, s: &Session, item_id: i64, upd: &ItemUpdate) -> Result<(Order, Option<PrintJob>)> {
        self.update_item_for_kitchen_approved(s, None, item_id, upd)
    }

    /// [`Pos::update_item_for_kitchen`] with a manager's approval for a
    /// discount over the staff limit (see [`Pos::update_item_approved`]).
    pub fn update_item_for_kitchen_approved(
        &mut self,
        s: &Session,
        approver: Option<&Session>,
        item_id: i64,
        upd: &ItemUpdate,
    ) -> Result<(Order, Option<PrintJob>)> {
        let order_id = self.order_of_item(item_id)?;
        // Closed first: a closed order may be hidden from this user.
        crate::orders::require_open(&self.conn, order_id)?;
        let before = self.get_order(s, order_id)?;
        let Some(cur) = before.items.iter().find(|i| i.id == item_id && i.sent_at.is_some()).cloned() else {
            return Ok((self.update_item_approved(s, approver, item_id, upd)?, None));
        };
        if upd.qty < cur.qty {
            let gone = cur.qty - upd.qty;
            let lines: Vec<OrderItem> = if cur.is_set {
                before
                    .items
                    .iter()
                    .filter(|i| i.parent_id == Some(cur.id) && i.sent_at.is_some())
                    .map(|i| OrderItem { qty: i.qty / cur.qty * gone, ..i.clone() })
                    .collect()
            } else {
                vec![OrderItem { qty: gone, ..cur.clone() }]
            };
            // Removed altogether: the ticket is made first, while the set's
            // line (its name heads its dishes) is still on the order.
            if upd.qty == 0 {
                let job = self.kitchen_change_job(s, order_id, "CANCELLED", &lines)?;
                return Ok((self.update_item_approved(s, approver, item_id, upd)?, job));
            }
            let order = self.update_item_approved(s, approver, item_id, upd)?;
            let job = self.kitchen_change_job(s, order_id, "CANCELLED", &lines)?;
            return Ok((order, job));
        }
        let order = self.update_item_approved(s, approver, item_id, upd)?;
        let job = if upd.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) != cur.note.as_deref() {
            // A set's note: its dishes sent so far, under the set's name and new note.
            let now: Vec<OrderItem> = order
                .items
                .iter()
                .filter(|i| if cur.is_set { i.parent_id == Some(item_id) && i.sent_at.is_some() } else { i.id == item_id })
                .cloned()
                .collect();
            self.kitchen_change_job(s, order_id, "CHANGED", &now)?
        } else {
            None
        };
        Ok((order, job))
    }

    /// [`Pos::cancel_order`], with a CANCELLED ticket for the dishes already
    /// sent and not yet marked done on the kitchen screen, routed like any
    /// other ticket. None when there are none, no kitchen printer is set up,
    /// or the ticket is switched off (setting `print_cancelled_order` = "0",
    /// see [`Pos::prints_cancelled_order`]).
    pub fn cancel_order_for_kitchen(&mut self, s: &Session, order_id: i64, reason: &str) -> Result<Option<PrintJob>> {
        self.cancel_for_kitchen_logged(s, s, "", order_id, reason)
    }

    /// The till's Cancel button: `s` (logged in here) cancels the order,
    /// with a CANCELLED ticket as in [`Pos::cancel_order_for_kitchen`].
    /// - No `pin` (or an empty one): only for a role with
    ///   [`Permission::CancelWithoutPin`] (admins and managers), sent to the
    ///   kitchen or not. Anyone else is refused ([`PosError::Forbidden`]).
    /// - A `pin`: its owner approves and cancels it with their own rights
    ///   (see [`Pos::pin_owner_for`]): any PIN for an order the kitchen has
    ///   not got, a manager's or admin's once it has.
    ///
    /// The audit log records `s` as who cancelled it, and the PIN's owner.
    pub fn cancel_order_confirmed(&mut self, s: &Session, order_id: i64, reason: &str, pin: Option<&str>) -> Result<Option<PrintJob>> {
        let pin = pin.map(str::trim).filter(|p| !p.is_empty()).map(crate::HashedPin::later);
        self.cancel_order_confirmed_hashed(s, order_id, reason, pin.as_ref())
    }

    /// [`Pos::cancel_order_confirmed`] with the PIN (trimmed, not empty)
    /// possibly hashed already, before the database lock was taken.
    pub fn cancel_order_confirmed_hashed(&mut self, s: &Session, order_id: i64, reason: &str, pin: Option<&crate::HashedPin>) -> Result<Option<PrintJob>> {
        match pin.filter(|p| !p.is_empty()) {
            None => {
                // A mistake order (nothing sent, nothing paid) is anyone's
                // to cancel with a confirm; once the kitchen has a dish or a
                // payment is taken, staff need a PIN as before.
                if !s.role.can(Permission::CancelWithoutPin) {
                    let (sent, paid): (bool, bool) = self.conn.query_row(
                        "SELECT EXISTS (SELECT 1 FROM order_items WHERE order_id = ?1 AND (sent_at IS NOT NULL OR prep_sent_at IS NOT NULL)),
                                EXISTS (SELECT 1 FROM payments WHERE order_id = ?1)",
                        [order_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    if sent || paid {
                        return Err(crate::PosError::Forbidden);
                    }
                }
                let note = format!("(no PIN, {})", s.role.as_str());
                self.cancel_for_kitchen_logged(s, s, &note, order_id, reason)
            }
            Some(p) => {
                let approver = self.pin_owner_hashed(p, Some(s.user_id))?;
                let note = format!("(PIN of {} #{}, {})", approver.name, approver.user_id, approver.role.as_str());
                self.cancel_for_kitchen_logged(&approver, s, &note, order_id, reason)
            }
        }
    }

    /// [`Pos::cancel_order_for_kitchen`] with `s`'s rights, logged as done by
    /// `by` (see `cancel_order_logged`).
    fn cancel_for_kitchen_logged(&mut self, s: &Session, by: &Session, note: &str, order_id: i64, reason: &str) -> Result<Option<PrintJob>> {
        s.require(Permission::Sell)?;
        crate::orders::require_open(&self.conn, order_id)?;
        // Made before cancelling: a cancelled order is not every user's to read.
        let cooking: Vec<i64> = self
            .conn
            // Dishes sent, and prepare-ahead dishes whose PREPARE NOW ticket printed
            // (the kitchen has started them), not yet marked done.
            .prepare_cached("SELECT id FROM order_items WHERE order_id = ?1 AND (sent_at IS NOT NULL OR prep_sent_at IS NOT NULL) AND kitchen_done_at IS NULL")?
            .query_map([order_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let job = if cooking.is_empty() || !self.prints_cancelled_order()? {
            None
        } else {
            let o = self.get_order(s, order_id)?;
            let lines: Vec<OrderItem> = o.items.into_iter().filter(|i| cooking.contains(&i.id)).collect();
            self.kitchen_change_job(s, order_id, "CANCELLED", &lines)?
        };
        self.cancel_order_logged(s, by, note, order_id, reason)?;
        Ok(job)
    }

    /// Whether cancelling a whole order prints a CANCELLED ticket for the
    /// dishes still cooking (setting `print_cancelled_order`: "1" or "0";
    /// not set = "1", as before the setting). Removing or reducing one dish
    /// already sent always prints one.
    pub fn prints_cancelled_order(&self) -> Result<bool> {
        Ok(self.setting(PRINT_CANCELLED_ORDER_KEY)?.as_deref().map(str::trim) != Some("0"))
    }

    /// Drinks first and desserts last on tickets of their own (each when
    /// switched on in Settings: `drinks_split`, `desserts_split`), the rest
    /// one ticket per team.
    pub(crate) fn split_for_kitchen(&self, mut base: KitchenTicket, items: &[OrderItem]) -> Result<Vec<KitchenTicket>> {
        let names = self.kitchen_printer()?.names.unwrap_or(PrintNames::Both);
        let drinks_on = self.setting("drinks_split")?.as_deref() != Some("0");
        let desserts_on = self.setting("desserts_split")?.as_deref() != Some("0");
        let mut stmt = self.conn.prepare_cached("SELECT drink, dessert FROM products WHERE id = ?1")?;
        let (mut drinks, mut desserts, mut food) = (Vec::new(), Vec::new(), Vec::new());
        for it in items {
            let (drink, dessert): (bool, bool) = stmt.query_row([it.product_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?.unwrap_or_default();
            match (drink && drinks_on, dessert && desserts_on) {
                (true, _) => drinks.push(it.clone()),
                (_, true) => desserts.push(it.clone()),
                _ => food.push(it.clone()),
            }
        }
        drop(stmt);
        if drinks.is_empty() && desserts.is_empty() {
            return self.split_by_team(base, items, names);
        }
        let label = self.team_label()?;
        let own = |group: &[OrderItem], en: &str, th: &str, base: &KitchenTicket| -> Result<KitchenTicket> {
            Ok(KitchenTicket { items: self.ticket_items(group, names, base.by_course)?, banner: Some(label(en, Some(th))), footer: None, ..base.clone() })
        };
        let mut out = Vec::new();
        if !drinks.is_empty() {
            out.push(own(&drinks, "Drinks", "เครื่องดื่ม", &base)?);
        }
        let last = (!desserts.is_empty()).then(|| own(&desserts, "Desserts", "ของหวาน", &base)).transpose()?;
        if !food.is_empty() {
            base.items = self.ticket_items(&food, names, base.by_course)?;
            out.extend(self.split_by_team(base, &food, names)?);
        }
        out.extend(last);
        Ok(out)
    }

    /// How a team's name shows on its bar, one line: "MAIN", "MAIN / จานหลัก"
    /// or "จานหลัก" (setting `team_names`: english, both or thai; both by
    /// default). A team without a Thai name always shows its English one.
    fn team_label(&self) -> Result<impl Fn(&str, Option<&str>) -> String> {
        let names = match self.setting("team_names")?.as_deref() {
            Some("english") => PrintNames::English,
            Some("thai") => PrintNames::Thai,
            _ => PrintNames::Both,
        };
        Ok(move |name: &str, alt: Option<&str>| {
            let upper = name.to_uppercase();
            match names.pick(&upper, alt) {
                (n, Some(a)) => format!("{n} / {a}"),
                (n, None) => n.to_string(),
            }
        })
    }

    fn is_drink(&self, product_id: i64) -> bool {
        self.conn
            .query_row("SELECT drink FROM products WHERE id = ?1", [product_id], |r| r.get(0))
            .unwrap_or(false)
    }

    /// `items` split into (drinks, everything else), looked up on the menu now.
    pub(crate) fn drinks_apart(&self, items: &[OrderItem]) -> Result<(Vec<OrderItem>, Vec<OrderItem>)> {
        let mut stmt = self.conn.prepare_cached("SELECT drink FROM products WHERE id = ?1")?;
        let (mut drinks, mut food) = (Vec::new(), Vec::new());
        for it in items {
            let drink: bool = stmt.query_row([it.product_id], |r| r.get(0)).optional()?.unwrap_or(false);
            if drink { drinks.push(it.clone()) } else { food.push(it.clone()) }
        }
        Ok((drinks, food))
    }

    fn split_by_team(&self, base: KitchenTicket, items: &[OrderItem], names: PrintNames) -> Result<Vec<KitchenTicket>> {
        // Settings → Kitchen teams can switch the separate tickets off.
        let groups = self.team_groups(items)?;
        if groups.len() <= 1 && groups.iter().all(|g| g.0.is_none()) {
            return Ok(vec![base]);
        }
        // Switched off: one ticket, with each team's header over its dishes.
        if self.setting("teams_split")?.as_deref() == Some("0") {
            let mut t = base.clone();
            t.items = Vec::new();
            for (banner, group) in groups {
                let mut part = if base.brief { self.brief_items(&group, names)? } else { self.ticket_items(&group, names, base.by_course)? };
                for it in &mut part {
                    it.team = banner.clone();
                }
                t.items.extend(part);
            }
            return Ok(vec![t]);
        }
        groups
            .into_iter()
            .map(|(banner, group)| {
                let mut t = base.clone();
                t.items = if base.brief { self.brief_items(&group, names)? } else { self.ticket_items(&group, names, base.by_course)? };
                t.banner = banner;
                Ok(t)
            })
            .collect()
    }

    /// Dishes for the short "main course" ticket: laid out as on the full
    /// tickets (a set's dishes under its name and its note, each dish with
    /// its note), without the options.
    fn brief_items(&self, items: &[OrderItem], names: PrintNames) -> Result<Vec<TicketItem>> {
        let mut out = self.ticket_items(items, names, false)?;
        for it in &mut out {
            // A set's main keeps what it comes with ("with rice", put first by
            // ticket_items), as on the full ticket; other options are left off.
            let with = it.set.is_some() && it.options.first().is_some_and(|o| o.starts_with("with "));
            it.options.truncate(usize::from(with));
        }
        Ok(out)
    }

    /// Print job for `items`, or None when no kitchen printer is set up.
    pub fn kitchen_job(&self, s: &Session, order_id: i64, items: &[OrderItem], reprint: bool) -> Result<Option<PrintJob>> {
        let items = &kitchen_only(items);
        if items.is_empty() {
            return Ok(None);
        }
        // NEW ORDER / ADDITIONAL... is worked out from the whole send, so
        // every printer's tickets say the same.
        let whole = self.kitchen_ticket(s, order_id, items, reprint)?;
        let title = whole.title.as_str();
        let food = self.drinks_apart(items)?.1;
        // "All courses together": a dine-in send with starters and mains at
        // once (sent course by course, starters always go alone).
        let all_courses = !reprint
            && self.get_order(s, order_id)?.order_type == OrderType::DineIn
            && food.iter().any(|i| i.starter)
            && food.iter().any(|i| !i.starter && !i.is_set);
        let mut jobs = Vec::new();
        for (cfg, group) in self.route_items(items)? {
            if cfg.target == PrinterTarget::None {
                continue;
            }
            // One ticket per team, printed one after another as a single job.
            let mut base = self.ticket_for(s, order_id, &group, reprint, whole.by_course)?;
            base.title = whole.title.clone();
            let tickets = self.split_for_kitchen(base, &group)?;
            let copies = kitchen_copies(&cfg, title, reprint, all_courses, !food.is_empty() && food.iter().all(|i| i.starter));
            jobs.push(PrintJob::copies(cfg.target.clone(), &[], escpos_all(&tickets, &cfg), copies, cfg.copy_delay_secs).paced(cfg.slow_send).for_mode(cfg.image_mode));
        }
        Ok(PrintJob::join(jobs).map(|j| j.about(format!("Kitchen ticket #{order_id}"))))
    }

    /// PREPARE NOW ticket for prepare-ahead dishes that wait for the mains
    /// (from [`Pos::send_prep`]). None when no kitchen printer is set up.
    pub fn prep_job(&self, s: &Session, order_id: i64, items: &[OrderItem]) -> Result<Option<PrintJob>> {
        let mut jobs = Vec::new();
        for (cfg, group) in self.route_items(items)? {
            if cfg.target == PrinterTarget::None {
                continue;
            }
            let base = self.prep_ticket(s, order_id, &group)?;
            let names = cfg.names.unwrap_or(PrintNames::Both);
            let tickets = self.split_by_team(base, &group, names)?;
            let copies = cfg.prep_copies.unwrap_or(cfg.copies);
            jobs.push(PrintJob::copies(cfg.target.clone(), &[], escpos_all(&tickets, &cfg), copies, cfg.copy_delay_secs).paced(cfg.slow_send).for_mode(cfg.image_mode));
        }
        Ok(PrintJob::join(jobs).map(|j| j.about(format!("PREPARE NOW ticket #{order_id}"))))
    }

    /// The PREPARE NOW ticket: like a kitchen ticket, saying the dishes are
    /// to be got ready now and served with the mains.
    pub fn prep_ticket(&self, s: &Session, order_id: i64, items: &[OrderItem]) -> Result<KitchenTicket> {
        let thai = self.kitchen_printer()?.names.unwrap_or(PrintNames::Both) != PrintNames::English;
        let mut t = self.kitchen_ticket(s, order_id, items, false)?;
        t.title = if thai { "PREPARE NOW / เตรียมก่อน" } else { "PREPARE NOW" }.into();
        t.footer = Some(if thai { "SERVE WITH MAINS / เสิร์ฟพร้อมจานหลัก" } else { "SERVE WITH MAINS" }.into());
        Ok(t)
    }

    /// "Call mains" again once the mains are already in the kitchen (sent
    /// with the starters, or called before): a short ticket with the table,
    /// MAIN COURSE and the main dishes, so the kitchen fires them now.
    /// Changes nothing on the order. None when no kitchen printer is set up.
    pub fn main_course_job(&self, s: &Session, order_id: i64) -> Result<Option<PrintJob>> {
        let o = self.get_order(s, order_id)?;
        let sent: Vec<OrderItem> = o.items.iter().filter(|i| i.sent_at.is_some() && !i.starter && !i.is_set).cloned().collect();
        // Of a set, its main and its sides: not a drink or dessert, whether
        // ticked as such on the menu or only in a section named so ("Drinks").
        let mut mains = Vec::new();
        for i in self.drinks_apart(&sent)?.1 {
            if i.parent_id.is_some() && (self.course_of(&i)? != Course::Mains || not_a_main_section(i.section_name.as_deref(), i.section_alt.as_deref())) {
                continue;
            }
            mains.push(i);
        }
        if mains.is_empty() {
            return invalid("no main courses have been sent to the kitchen yet");
        }
        self.audit(Some(s.user_id), "kitchen.main_course", &order_id.to_string())?;
        let mut jobs = Vec::new();
        for (cfg, mains) in self.route_items(&mains)? {
            if cfg.target == PrinterTarget::None {
                continue;
            }
            jobs.push(self.main_course_part(&o, &cfg, &mains)?);
        }
        Ok(PrintJob::join(jobs).map(|j| j.about(format!("Main course call #{order_id}"))))
    }

    /// One printer's part of the main course call.
    fn main_course_part(&self, o: &Order, cfg: &PrinterConfig, mains: &[OrderItem]) -> Result<PrintJob> {
        let names = cfg.names.unwrap_or(PrintNames::Both);
        let base = self.main_course_ticket(o, mains)?;
        let tickets = if cfg.call_table_only {
            // Just the table, in huge letters: one ticket per team that has a
            // main waiting (headed with the team), or one listing the teams.
            let groups = self.team_groups(mains)?;
            let teams: Vec<String> = groups.iter().filter_map(|g| g.0.as_ref().map(|b| b.split(" / ").next().unwrap_or("").to_string())).collect();
            let split = self.setting("teams_split")?.as_deref() != Some("0");
            let bare = KitchenTicket { items: vec![], ..base };
            if teams.is_empty() {
                vec![bare]
            } else if split {
                groups.into_iter().map(|(banner, _)| KitchenTicket { banner, ..bare.clone() }).collect()
            } else {
                vec![KitchenTicket { details: vec![format!("FOR: {}", teams.join(", "))], ..bare }]
            }
        } else {
            self.split_by_team(base, mains, names)?
        };
        let copies = call_again_copies(cfg);
        Ok(PrintJob::copies(cfg.target.clone(), &[], escpos_all(&tickets, cfg), copies, cfg.copy_delay_secs).paced(cfg.slow_send).for_mode(cfg.image_mode))
    }

    fn main_course_ticket(&self, o: &Order, mains: &[OrderItem]) -> Result<KitchenTicket> {
        let names = self.kitchen_printer()?.names.unwrap_or(PrintNames::Both);
        let table = self.label("label_table")?;
        Ok(KitchenTicket {
            title: "MAIN COURSE".into(),
            // "TABLE 4" for numbered tables; named ones ("Patio 2") as they are.
            heading: match &o.table_name {
                Some(t) if t.chars().all(|c| c.is_ascii_digit()) => format!("{table} {t}").trim().to_string(),
                Some(t) => t.clone(),
                None => self.heading(o)?,
            },
            details: vec![],
            order_id: o.id,
            time: chrono::Local::now().format("%H:%M").to_string(),
            staff: String::new(),
            items: self.brief_items(mains, names)?,
            footer: None,
            brief: true,
            large: self.kitchen_printer()?.large_text,
            banner: None,
            by_course: false,
        })
    }

    /// Everything already sent on an order, for "Reprint".
    pub fn kitchen_reprint_job(&self, s: &Session, order_id: i64) -> Result<Option<PrintJob>> {
        let o = self.get_order(s, order_id)?;
        let sent: Vec<OrderItem> = o.items.into_iter().filter(|i| i.sent_at.is_some()).collect();
        if sent.is_empty() {
            return invalid("nothing has been sent to the kitchen yet");
        }
        self.kitchen_job(s, order_id, &sent, true)
    }

    /// A sample ticket for every team, to see the headers on the kitchen
    /// printer. None when no kitchen printer is set up.
    pub fn test_teams_job(&self, s: &Session) -> Result<Option<PrintJob>> {
        s.require(Permission::KitchenSettings)?;
        let teams = crate::teams::load_teams(&self.conn)?;
        if teams.is_empty() {
            return invalid("add a team first");
        }
        let cfg = self.kitchen_printer()?;
        if cfg.target == PrinterTarget::None {
            return Ok(None);
        }
        let names = cfg.names.unwrap_or(PrintNames::Both);
        let time = chrono::Local::now().format("%H:%M").to_string();
        let label = self.team_label()?;
        let tickets: Vec<KitchenTicket> = teams
            .iter()
            .map(|t| {
                let (dish, dish_alt) = names.pick("Sample dish", Some("เมนูตัวอย่าง"));
                KitchenTicket {
                    title: "TEST PRINT".into(),
                    heading: "DINE-IN T1".into(),
                    details: vec![],
                    order_id: 0,
                    time: time.clone(),
                    staff: s.name.clone(),
                    items: vec![TicketItem {
                        team: None, set: None,
                        category: None,
                        qty: 1,
                        name: dish.to_string(),
                        name_alt: dish_alt.map(str::to_string),
                        options: vec![],
                        note: None,
                        set_note: None,
                    }],
                    footer: None,
                    brief: false,
                    large: cfg.large_text,
                    banner: Some(label(&t.name, t.name_alt.as_deref())),
                    by_course: false,
                }
            })
            .collect();
        Ok(Some(PrintJob::copies(cfg.target.clone(), &[], escpos_all(&tickets, &cfg), 1, cfg.copy_delay_secs).paced(cfg.slow_send).for_mode(cfg.image_mode).about("Sample team tickets")))
    }

    /// A sample ticket to check a printer configuration before saving it.
    pub fn test_print_job(&self, s: &Session, cfg: &PrinterConfig) -> Result<PrintJob> {
        s.require(Permission::ManageSettings)?;
        cfg.validate()?;
        let names = cfg.names.unwrap_or(PrintNames::Both);
        let item = |name: &str, alt: &str| {
            let (n, a) = names.pick(name, Some(alt));
            (n.to_string(), a.map(str::to_string))
        };
        let ((soup, soup_alt), (burger, burger_alt)) = (item("Soup of the day", "ซุปประจำวัน"), item("Example burger", "เบอร์เกอร์"));
        let ticket = KitchenTicket {
            title: "TEST PRINT".into(),
            heading: "DINE-IN T1".into(),
            details: vec!["Printer is working".into()],
            order_id: 0,
            time: chrono::Local::now().format("%H:%M").to_string(),
            staff: s.name.clone(),
            items: vec![
                TicketItem {
                    team: None, set: None,
                    category: Some(names.inline("Starters", Some("อาหารเรียกน้ำย่อย"))),
                    qty: 1,
                    name: soup,
                    name_alt: soup_alt,
                    options: vec![],
                    note: None,
                    set_note: None,
                },
                TicketItem {
                    team: None, set: None,
                    category: Some(names.inline("Mains", Some("อาหารจานหลัก"))),
                    qty: 2,
                    name: burger,
                    name_alt: burger_alt,
                    options: vec![names.inline("Large", Some("ใหญ่")), names.inline("Extra cheese", Some("เพิ่มชีส"))],
                    note: Some("no onions".into()),
                    set_note: None,
                },
            ],
            footer: None,
            brief: false,
            large: cfg.large_text,
            banner: None,
            by_course: false,
        };
        Ok(PrintJob::copies(cfg.target.clone(), &[], ticket.escpos_for(cfg), cfg.copies, cfg.copy_delay_secs).paced(cfg.slow_send).for_mode(cfg.image_mode).about("Test ticket"))
    }

    /// "Test Thai printing": one short page with the Thai sample in every
    /// image mode (see [`thai_test_page`]), to see which this printer takes.
    pub fn thai_test_job(&self, s: &Session, cfg: &PrinterConfig) -> Result<PrintJob> {
        s.require(Permission::ManageSettings)?;
        cfg.validate()?;
        Ok(PrintJob::new(cfg.target.clone(), thai_test_page(cfg.width)).paced(cfg.slow_send).for_mode(cfg.image_mode).about("Thai test page"))
    }
}

/// Thai sample on the test page.
pub(crate) const THAI_SAMPLE: &str = "ทดสอบภาษาไทย ผัดไทย 120.00";

/// The Thai test page, `width` characters wide: an ASCII header, then for
/// each mode in [`ImageMode::TEST_ORDER`] an ASCII label ("1. Compatible
/// (older printers)", the name in the settings) and the Thai sample drawn in that mode,
/// then a cut. Each mode starts from a fresh init (ESC @ and the code page,
/// plus FS . for the modes that send it, as real jobs do), so one the printer cannot take upsets
/// the next as little as possible; the standard raster mode, the one that
/// prints junk when it fails, comes last.
pub(crate) fn thai_test_page(width: usize) -> Vec<u8> {
    // The same start as a real job; each sample adds what its mode adds
    // (FS . for the new modes), so "3. Standard" prints exactly as Standard does.
    const INIT: [u8; 8] = [0x1B, b'@', 0x1B, b't', 0, 0x1B, b'R', 0];
    let dots = width * crate::raster::DOTS_PER_CHAR;
    let text = |b: &mut Vec<u8>, bold: bool, t: &str| {
        b.extend_from_slice(&[0x1B, b'a', 0, 0x1D, b'!', 0, 0x1B, b'E', u8::from(bold)]);
        for l in wrap(t, width, "", "") {
            b.extend(l.chars().map(cp437));
            b.push(b'\n');
        }
    };
    let mut b = INIT.to_vec();
    text(&mut b, true, "Thai test - pick the mode that prints clearly");
    text(&mut b, false, "Then choose it under Image mode (for Thai text) in this printer's settings.");
    for (i, mode) in ImageMode::TEST_ORDER.into_iter().enumerate() {
        b.push(b'\n');
        b.extend_from_slice(&INIT);
        b.extend_from_slice(mode.init_extra());
        text(&mut b, true, &format!("{}. {}", i + 1, mode.label()));
        b.extend(crate::raster::escpos_line_in(THAI_SAMPLE, Size::Normal, false, dots, mode));
    }
    b.push(b'\n');
    b.extend_from_slice(&INIT);
    text(&mut b, false, "End of test");
    b.extend_from_slice(&[0x1B, b'd', 4]); // feed past the cutter
    b.extend_from_slice(&[0x1D, b'V', 66, 0]); // partial cut
    b
}

// ------------------------------------------------------- Windows spooler

#[cfg(windows)]
mod windows_spool {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Graphics::Printing::{
        ClosePrinter, EndDocPrinter, EndPagePrinter, EnumPrintersW, OpenPrinterW, StartDocPrinterW,
        StartPagePrinter, WritePrinter, DOC_INFO_1W, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
        PRINTER_INFO_4W,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn last_error() -> String {
        std::io::Error::last_os_error().to_string()
    }

    /// Send bytes to the spooler as RAW, so the driver passes ESC/POS through.
    pub fn print_raw(name: &str, data: &[u8]) -> Result<(), String> {
        let wname = wide(name.trim());
        let mut doc_name = wide("POS kitchen ticket");
        let mut datatype = wide("RAW");
        unsafe {
            let mut h: HANDLE = null_mut();
            if OpenPrinterW(wname.as_ptr(), &mut h, null()) == 0 {
                return Err(format!("cannot open printer \"{name}\": {}", last_error()));
            }
            let doc = DOC_INFO_1W {
                pDocName: doc_name.as_mut_ptr(),
                pOutputFile: null_mut(),
                pDatatype: datatype.as_mut_ptr(),
            };
            let result = if StartDocPrinterW(h, 1, &doc) == 0 {
                Err(last_error())
            } else {
                let mut written = 0u32;
                let ok = StartPagePrinter(h) != 0
                    && WritePrinter(h, data.as_ptr() as *const c_void, data.len() as u32, &mut written) != 0
                    && written as usize == data.len();
                let err = (!ok).then(last_error);
                EndPagePrinter(h);
                EndDocPrinter(h);
                err.map_or(Ok(()), Err)
            };
            ClosePrinter(h);
            result
        }
    }

    pub fn list() -> Vec<String> {
        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        unsafe {
            let (mut needed, mut count) = (0u32, 0u32);
            EnumPrintersW(flags, null(), 4, null_mut(), 0, &mut needed, &mut count);
            if needed == 0 {
                return Vec::new();
            }
            // u64 buffer keeps the struct array suitably aligned.
            let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
            if EnumPrintersW(flags, null(), 4, buf.as_mut_ptr() as *mut u8, needed, &mut needed, &mut count) == 0 {
                return Vec::new();
            }
            let infos = std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_4W, count as usize);
            infos
                .iter()
                .filter(|i| !i.pPrinterName.is_null())
                .map(|i| {
                    let mut len = 0;
                    while *i.pPrinterName.add(len) != 0 {
                        len += 1;
                    }
                    String::from_utf16_lossy(std::slice::from_raw_parts(i.pPrinterName, len))
                })
                .collect()
        }
    }
}

#[cfg(not(windows))]
mod windows_spool {
    pub fn print_raw(_name: &str, _data: &[u8]) -> Result<(), String> {
        Err("Windows printers are only available on Windows".into())
    }

    pub fn list() -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket() -> KitchenTicket {
        KitchenTicket {
            title: "NEW ORDER".into(),
            heading: "DINE-IN T3".into(),
            details: vec!["Ground".into()],
            order_id: 12,
            time: "19:05".into(),
            staff: "Sam".into(),
            items: vec![
                TicketItem { team: None, set: None, category: None, qty: 2, name: "Burger".into(), name_alt: None, options: vec!["Large".into(), "Extra cheese".into()], note: Some("no onions".into()), set_note: None },
                TicketItem { team: None, set: None, category: None, qty: 1, name: "Crème brûlée".into(), name_alt: None, options: vec![], note: None, set_note: None },
            ],
            footer: None,
            brief: false,
            large: false,
            banner: None,
            by_course: false,
        }
    }

    #[test]
    fn text_layout() {
        let t = ticket().to_text(32);
        let eq = "=".repeat(32);
        let expected = format!(
            "{eq}\n   NEW ORDER\n{eq}\nDINE-IN T3\nGround\n{}\n#12  19:05  Sam\n{eq}\n\n\
             2 x Burger\n   + Large\n   + Extra cheese\n   ** no onions\n\n1 x Crème brûlée\n\n{eq}\n   DINE-IN T3\n",
            "-".repeat(32)
        );
        assert_eq!(t, expected);
    }

    #[test]
    fn large_text_and_table_at_the_bottom() {
        let t = KitchenTicket { large: true, ..ticket() };
        let lines = t.lines(42);
        // Dishes, options and notes are double size (21 per line on 80 mm).
        for text in ["2 x Burger", "  + Large", "  ** no onions"] {
            assert!(lines.iter().any(|l| l.text == text && l.size == Size::Big), "{text}");
        }
        // The table is printed again at the end, big and centred.
        let last = lines.last().unwrap();
        assert!(last.text == "DINE-IN T3" && last.size == Size::Big && last.center);
        let long = KitchenTicket {
            items: vec![TicketItem { team: None, set: None, category: None, qty: 1, name: "Chicken satay with peanut sauce".into(), name_alt: None, options: vec![], note: None, set_note: None }],
            ..t
        };
        assert!(long.lines(42).iter().filter(|l| l.size == Size::Big).all(|l| l.text.chars().count() <= 21));
    }

    #[test]
    fn thai_name_under_the_english_one() {
        let mut t = ticket();
        t.items[0].name_alt = Some("เบอร์เกอร์".into());
        t.items[0].options[0] = PrintNames::Both.inline("Large", Some("ใหญ่"));
        let text = t.to_text(32);
        assert!(text.contains("2 x Burger\n    เบอร์เกอร์\n   + Large / ใหญ่\n"), "{text}");
        // Thai lines go out as raster images; the rest stays text.
        let b = t.to_escpos(32, true);
        // Two Thai lines; each image goes out in bands small enough for any printer.
        let heads: Vec<usize> = (0..b.len().saturating_sub(8)).filter(|&i| b[i..i + 4] == [0x1D, b'v', b'0', 0]).collect();
        assert!(heads.len() >= 2);
        for &i in &heads {
            let (rb, h) = (usize::from(b[i + 4]) | usize::from(b[i + 5]) << 8, usize::from(b[i + 6]) | usize::from(b[i + 7]) << 8);
            assert!(rb * h <= 2048 && h > 0, "band {rb}x{h}");
        }
        assert!(b.windows(10).any(|w| w == b"2 x Burger"));
        assert!(!b.windows(3).any(|w| w == "เ".as_bytes()));
    }

    #[test]
    fn choosing_names() {
        assert_eq!(PrintNames::Both.pick("Pad Thai", Some("ผัดไทย")), ("Pad Thai", Some("ผัดไทย")));
        assert_eq!(PrintNames::Thai.pick("Pad Thai", Some("ผัดไทย")), ("ผัดไทย", None));
        assert_eq!(PrintNames::English.pick("Pad Thai", Some("ผัดไทย")), ("Pad Thai", None));
        // No Thai name: English everywhere.
        assert_eq!(PrintNames::Thai.pick("Cola", None), ("Cola", None));
        assert_eq!(PrintNames::Both.pick("Cola", Some(" ")), ("Cola", None));
        assert_eq!(PrintNames::Both.inline("Large", Some("ใหญ่")), "Large / ใหญ่");
        assert_eq!(PrintNames::Thai.inline("Large", None), "Large");
    }

    #[test]
    fn wraps_long_lines() {
        assert_eq!(wrap("1 x Very long dish name that goes on", 16, "", "    "), ["1 x Very long", "    dish name", "    that goes on"]);
        assert_eq!(wrap("ABCDEFGHIJKLMNOPQRST", 8, "", ""), ["ABCDEFGH", "IJKLMNOP", "QRST"]);
        assert_eq!(wrap("no onions please", 14, "   ** ", "      "), ["   ** no", "      onions", "      please"]);
        assert_eq!(wrap("", 8, "", ""), [""]);
        // Thai marks take no width: 6 letters + 3 marks fit in 6 columns.
        assert_eq!(wrap("+ ไข่ดาว", 8, "", ""), ["+ ไข่ดาว"]);
        // Split only where Thai allows: never before า (it was "วไข่ด" / "าว"),
        // and not after a syllable's last letter when there is a choice.
        assert_eq!(wrap("ไข่ดาวไข่ดาว", 4, "", ""), ["ไข่", "ดาว", "ไข่", "ดาว"]);
    }

    #[test]
    fn thai_names_on_tickets_and_receipts_break_between_syllables() {
        let mut t = ticket();
        t.items = THAI_DISHES
            .iter()
            .enumerate()
            .map(|(i, th)| {
                let name = if th.is_ascii() || th.contains(' ') { th.to_string() } else { format!("Dish {i}") };
                let alt = (!th.contains(' ')).then(|| th.to_string());
                TicketItem { team: None, set: None, category: None, qty: 1, name, name_alt: alt, options: vec![th.to_string()], note: Some(th.to_string()), set_note: None }
            })
            .collect();
        for width in [32, 42] {
            for large in [false, true] {
                t.large = large;
                let lines = t.lines(width);
                let texts: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
                assert_thai_breaks(&texts);
                for l in &lines {
                    let w = if l.size == Size::Big { width / 2 } else { width };
                    assert!(cols(&l.text) <= w, "{} at {width}", l.text);
                    // And as the printer draws them, in the app's font or not.
                    if crate::raster::needs_raster(&l.text) {
                        assert_thai_breaks(&crate::raster::line_pieces(&l.text, l.size, l.center, width * crate::raster::DOTS_PER_CHAR).2);
                    }
                }
                for app_font in [false, true] {
                    for image_mode in [ImageMode::Raster, ImageMode::Column, ImageMode::RasterSmall] {
                        let b = t.escpos_for(&PrinterConfig { width, app_font, image_mode, ..PrinterConfig::default() });
                        assert!(b.ends_with(&[0x1D, b'V', 66, 0]));
                    }
                }
            }
        }
    }

    #[test]
    fn thai_dish_names_wrap_between_syllables() {
        // The kitchen's double-size lines on 58 mm paper (16 columns): the
        // curry wrapped as "…หวานไ" / "ก่", splitting ไ from its letter.
        assert_eq!(wrap("แกงเขียวหวานไก่", 12, "", ""), ["แกงเขียวหวาน", "ไก่"]);
        assert_eq!(wrap("1 x แกงเขียวหวานไก่", 16, "", "  "), ["1 x", "  แกงเขียวหวานไก่"]);
        assert_eq!(wrap("ข้าวเหนียวมะม่วง", 12, "  ", "  "), ["  ข้าวเหนียวมะ", "  ม่วง"]);
        assert_eq!(wrap("ไก่ผัดเม็ดมะม่วงหิมพานต์", 16, "  ", "  "), ["  ไก่ผัดเม็ดมะ", "  ม่วงหิมพานต์"]);
        assert_eq!(wrap("ต้มยำกุ้งน้ำข้น", 5, "", ""), ["ต้มยำ", "กุ้งน้ำ", "ข้น"]);
        // Never before a letter silenced by ์, or the letter before it
        // ("หิมพา" / "นต์").
        assert_eq!(wrap("หิมพานต์", 4, "", ""), ["หิม", "พานต์"]);
        assert_eq!(wrap("ศุกร์ศุกร์", 3, "", ""), ["ศุกร์", "ศุกร์"]);
        for width in [32, 42] {
            for w in [width, width / 2, width / 3] {
                for (first, indent) in [("1 x ", "    "), ("", ""), ("  ", "  "), ("   + ", "     ")] {
                    for dish in THAI_DISHES {
                        let lines = wrap(dish, w, first, indent);
                        assert_thai_breaks(&lines);
                        assert!(lines.iter().all(|l| cols(l) <= w), "{lines:?} at {w}");
                        let joined: String = lines.concat().chars().filter(|c| !c.is_whitespace()).collect();
                        assert_eq!(joined, format!("{first}{dish}").replace(' ', ""), "nothing lost at {w}");
                    }
                }
            }
        }
    }

    #[test]
    fn escpos_bytes() {
        let b = ticket().to_escpos(42, false);
        assert_eq!(&b[..5], &[0x1B, b'@', 0x1B, b't', 0]);
        assert!(b.ends_with(&[0x1D, b'V', 66, 0]));
        // é and û are mapped to code page 437, not sent as UTF-8.
        let needle = [b'C', b'r', 0x8A, b'm', b'e'];
        assert!(b.windows(needle.len()).any(|w| w == needle));
        assert!(!b.windows(2).any(|w| w == "é".as_bytes()));
    }

    #[test]
    fn network_printer_receives_bytes() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let reader = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut s, &mut buf).unwrap();
            buf
        });
        let job = PrintJob::new(PrinterTarget::Network { host: "127.0.0.1".into(), port }, ticket().to_escpos(42, true));
        job.send().unwrap();
        assert_eq!(reader.join().unwrap(), job.bytes());
    }

    #[test]
    fn unreachable_printer_is_an_error_not_a_hang() {
        let (_held, port) = no_printer();
        let job = PrintJob::new(PrinterTarget::Network { host: "127.0.0.1".into(), port }, vec![1]);
        assert!(job.send().is_err());
    }

    /// Records every write separately.
    #[derive(Default)]
    struct Pieces(Vec<Vec<u8>>);

    impl Write for Pieces {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.push(buf.to_vec());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn slow_sending_writes_the_same_bytes_in_order_in_small_pieces() {
        let job = ticket().to_escpos_font(42, true, true);
        assert!(job.len() > 4 * SLOW_CHUNK, "a ticket in the app's font is tens of KB: {}", job.len());
        for chunk in [1, 7, 512, SLOW_CHUNK, job.len(), job.len() + 1] {
            let mut w = Pieces::default();
            write_paced(&mut w, &job, chunk, Duration::ZERO).unwrap();
            assert_eq!(w.0.concat(), job, "chunk {chunk}");
            assert_eq!(w.0.len(), job.len().div_ceil(chunk));
            assert!(w.0.iter().all(|p| !p.is_empty() && p.len() <= chunk));
        }
        // A pause between pieces, none after the last.
        let started = std::time::Instant::now();
        write_paced(&mut Pieces::default(), &[0; 3000], 1000, Duration::from_millis(40)).unwrap();
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(80) && took < Duration::from_millis(500), "{took:?}");
        assert_eq!(paced_duration(0), Duration::ZERO);
        assert_eq!(paced_duration(SLOW_CHUNK), Duration::ZERO);
        assert_eq!(paced_duration(30 * SLOW_CHUNK + 1), SLOW_PAUSE * 30);
    }

    /// One connection's bytes, and whether another came in while it was open.
    type Received = (Vec<u8>, bool);

    /// A port on 127.0.0.1 where no printer answers: held (bound, never
    /// listening) for as long as the socket is kept, so no other test's
    /// printer can be given the same port and take the connection.
    fn no_printer() -> (socket2::Socket, u16) {
        let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        s.bind(&std::net::SocketAddr::from(([127, 0, 0, 1], 0)).into()).unwrap();
        let port = s.local_addr().unwrap().as_socket().unwrap().port();
        (s, port)
    }

    /// How much later than the true moment the fake printer may notice a
    /// connection's close (its reading thread woken under load). A job still
    /// sending when the next one connects stays open far longer: the jobs in
    /// these tests are sent in paced pieces for over twice this.
    const CLOSE_SEEN_LATE: Duration = Duration::from_millis(100);

    /// A network printer on 127.0.0.1. Returns its port and a thread giving,
    /// per connection (in the order they came in), the bytes and whether a
    /// later connection came in while this one was open.
    ///
    /// Each connection gets its own reading thread, which notes when the
    /// till closed it; the next connection's arrival is noted as it is
    /// accepted. The till closes a connection before the next job opens
    /// one, so the next may only arrive before this one's close was seen
    /// by at most [`CLOSE_SEEN_LATE`].
    fn slow_printer(connections: usize) -> (u16, std::thread::JoinHandle<Vec<Received>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let reader = std::thread::spawn(move || {
            let readers: Vec<_> = (0..connections)
                .map(|_| {
                    let (mut s, _) = listener.accept().unwrap();
                    let came = std::time::Instant::now();
                    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
                    let read = std::thread::spawn(move || {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut s, &mut buf).unwrap();
                        (buf, std::time::Instant::now())
                    });
                    (came, read)
                })
                .collect();
            let got: Vec<_> = readers.into_iter().map(|(came, read)| (came, read.join().unwrap())).collect();
            got.iter()
                .enumerate()
                .map(|(i, (_, (buf, closed)))| {
                    let overlap = got[i + 1..].iter().any(|(came, _)| *came + CLOSE_SEEN_LATE < *closed);
                    (buf.clone(), overlap)
                })
                .collect()
        });
        (port, reader)
    }

    #[test]
    fn slow_network_printer_gets_the_job_in_paced_pieces_without_the_till_waiting() {
        let (port, reader) = slow_printer(2);
        let target = PrinterTarget::Network { host: "127.0.0.1".into(), port };
        let a = PrintJob::new(target.clone(), ticket().to_escpos_font(42, true, true)).paced(true);
        let b = PrintJob::new(target, b"second job".to_vec()).paced(true);
        assert!(a.is_paced() && a.bytes().len() > 8 * SLOW_CHUNK);
        let started = std::time::Instant::now();
        a.clone().send_in_background().unwrap();
        // The printer is busy with `a`: `b` queues behind it, no waiting here either.
        b.clone().send_in_background().unwrap();
        let returned = started.elapsed();
        assert!(returned < Duration::from_millis(200), "the till waited {returned:?}");
        let got = reader.join().unwrap();
        let took = started.elapsed();
        assert!(took >= paced_duration(a.bytes().len()), "paced: {took:?}");
        // The exact bytes, in order, one connection at a time.
        assert_eq!(got, vec![(a.bytes(), false), (b.bytes(), false)]);
    }

    #[test]
    fn jobs_keep_their_place_in_line_and_the_drawer_goes_first() {
        let (port, reader) = slow_printer(4);
        let target = PrinterTarget::Network { host: "127.0.0.1".into(), port };
        // Each job is six paced pieces (five pauses, 250 ms), so one that
        // started before the last had finished would show (see slow_printer).
        let bytes = |b: &[u8]| {
            let mut v = b.to_vec();
            v.resize(5 * SLOW_CHUNK + 1, b'.');
            v
        };
        assert!(paced_duration(bytes(b"x").len()) > 2 * CLOSE_SEEN_LATE);
        let job = |b: &[u8]| PrintJob::new(target.clone(), bytes(b)).paced(true);
        // Busy: a job is sending (its turn held here).
        let busy = Turn::try_take(&target).unwrap();
        for b in [&b"kitchen A"[..], b"kitchen B"] {
            job(b).send_in_background().unwrap();
        }
        // The drawer: ahead of A and B, which have not started.
        job(b"drawer").urgent().send_in_background().unwrap();
        // The threads have had time to run in any order: the line is fixed.
        std::thread::sleep(Duration::from_millis(100));
        drop(busy);
        job(b"receipt").send_in_background().unwrap();
        let got: Vec<Vec<u8>> = reader.join().unwrap().into_iter().map(|(b, overlap)| {
            assert!(!overlap);
            b
        }).collect();
        assert_eq!(got, [bytes(b"drawer"), bytes(b"kitchen A"), bytes(b"kitchen B"), bytes(b"receipt")]);
        assert!(wait_for_prints(Duration::from_secs(5)));
    }

    #[test]
    fn waiting_for_prints_in_flight() {
        let f = InFlight::new();
        assert!(f.wait(Duration::ZERO), "nothing running");
        let started = std::time::Instant::now();
        std::thread::scope(|sc| {
            let a = f.start();
            let b = f.start();
            assert_eq!(f.running(), 2);
            sc.spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                drop(a);
                std::thread::sleep(Duration::from_millis(100));
                drop(b);
            });
            assert!(f.wait(Duration::from_secs(5)), "both finished");
        });
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert_eq!(f.running(), 0);
        // One that never ends: give up after the limit.
        let stuck = f.start();
        let started = std::time::Instant::now();
        assert!(!f.wait(Duration::from_millis(100)));
        assert!(started.elapsed() >= Duration::from_millis(100) && started.elapsed() < Duration::from_secs(2));
        drop(stuck);
        assert!(f.wait(Duration::ZERO));
    }

    #[test]
    fn a_failure_after_connecting_is_logged_and_shown_on_the_tills() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("print-errors.log");
        set_print_error_log(Some(log.clone()));
        let before = print_failures(None).last;
        // The printer takes the connection, reads a little, then goes away.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let printer = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut piece = [0u8; 100];
            let _ = std::io::Read::read(&mut s, &mut piece);
        });
        let job = PrintJob::new(PrinterTarget::Network { host: "127.0.0.1".into(), port }, vec![0x41; 40 * SLOW_CHUNK]).paced(true).about("Kitchen ticket #94");
        job.send_in_background().unwrap();
        printer.join().unwrap();
        assert!(wait_for_prints(Duration::from_secs(10)));
        let seen = print_failures(Some(before));
        let mine: Vec<&PrintFailure> = seen.failures.iter().filter(|f| f.message.contains("#94")).collect();
        assert_eq!(mine.len(), 1, "{seen:?}");
        assert!(mine[0].message.starts_with(&format!("Kitchen ticket #94 did not finish printing on the printer at 127.0.0.1:{port}")), "{}", mine[0].message);
        assert!(seen.last >= mine[0].id);
        // Asked again after the last one: nothing new.
        assert!(print_failures(Some(seen.last)).failures.iter().all(|f| !f.message.contains("#94")));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.lines().any(|l| l.contains("Kitchen ticket #94 did not finish printing")), "{text}");
        // Past the size limit the log starts afresh, the old one kept.
        std::fs::write(&log, vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();
        record_failure("Receipt #7", &PrinterTarget::File { path: "COM3".into() }, "gone");
        assert!(std::fs::read_to_string(&log).unwrap().contains("Receipt #7 did not finish printing on printer COM3 (gone)"));
        assert!(std::fs::metadata(dir.path().join("print-errors.old.log")).unwrap().len() > MAX_LOG_BYTES);
        set_print_error_log(None);
    }

    #[test]
    fn slow_printer_that_cannot_be_reached_is_still_reported_at_once() {
        let (_held, port) = no_printer();
        let job = PrintJob::new(PrinterTarget::Network { host: "127.0.0.1".into(), port }, vec![1; 5000]).paced(true);
        assert!(job.clone().send_in_background().is_err());
        assert!(job.send().is_err());
    }

    /// The tickets in a stream of ESC/POS bytes as the POS sends them, read
    /// command by command (so image data is never taken for a command):
    /// each from its init (ESC @) to its cut, with what follows the last cut.
    /// Panics on a command it does not know, an image stripe not ended by
    /// LF, or a ticket that starts before the one before it was cut.
    fn tickets_in(b: &[u8]) -> (Vec<&[u8]>, &[u8]) {
        let (mut tickets, mut start, mut i) = (Vec::new(), None, 0);
        while i < b.len() {
            let len = match (b[i], b.get(i + 1).copied().unwrap_or(0)) {
                (0x1B, b'@') => {
                    assert!(start.is_none(), "a ticket starts at {i} before the last one was cut");
                    start = Some(i);
                    2
                }
                (0x1B, b'2') | (0x1C, b'.') => 2,
                (0x1B, b't' | b'R' | b'a' | b'E' | b'd' | b'3') | (0x1D, b'!') => 3,
                (0x1B, b'*') => {
                    assert_eq!(b[i + 2], 33);
                    let n = usize::from(b[i + 3]) | usize::from(b[i + 4]) << 8;
                    assert_eq!(b.get(i + 5 + 3 * n), Some(&b'\n'), "stripe at {i} not ended by LF");
                    5 + 3 * n + 1
                }
                (0x1D, b'v') => {
                    let rb = usize::from(b[i + 4]) | usize::from(b[i + 5]) << 8;
                    let h = usize::from(b[i + 6]) | usize::from(b[i + 7]) << 8;
                    8 + rb * h
                }
                (0x1D, b'V') => {
                    assert_eq!(b[i + 2], 66);
                    let from = start.take().expect("a cut without a ticket");
                    tickets.push(&b[from..i + 4]);
                    4
                }
                (0x1B | 0x1C | 0x1D | 0x10, c) => panic!("unknown command {:#x} {c:#x} at {i}", b[i]),
                _ => 1,
            };
            i += len;
        }
        let tail = match tickets.last() {
            Some(last) => &b[last.as_ptr() as usize - b.as_ptr() as usize + last.len()..],
            None => b,
        };
        (tickets, tail)
    }

    /// Ends with the feed past the cutter and the partial cut.
    fn fed_and_cut(t: &[u8]) -> bool {
        t.ends_with(&[0x1B, b'd', 4, 0x1D, b'V', 66, 0])
    }

    #[test]
    fn compatible_mode_sends_every_copy_complete_over_one_connection() {
        // A ticket with Thai, the app's font and a team bar: column images.
        let mut t = ticket();
        t.banner = Some("CURRY / แกง".into());
        let cfg = PrinterConfig { image_mode: ImageMode::Column, copies: 3, ..PrinterConfig::default() };
        let doc = t.escpos_for(&cfg);
        // Sent slowly below, the printer's own font (a short job, so other
        // tests waiting for the prints in flight do not wait for this one).
        let short = t.escpos_for(&PrinterConfig { app_font: false, ..cfg.clone() });
        for d in [&doc, &short] {
            assert!(d.windows(3).any(|w| w == [0x1B, b'*', 33]));
            let (one, tail) = tickets_in(d);
            assert!(one.len() == 1 && tail.is_empty() && fed_and_cut(one[0]));
        }
        assert!(short.len() < 6 * SLOW_CHUNK, "{}", short.len());
        let doc_paced = short;

        // Compatible: all three copies on one connection, the wait between
        // them included, each a whole ticket fed and cut; nothing after the
        // last cut, and the connection ended (the printer read to its end).
        let (port, reader) = slow_printer(1);
        let target = PrinterTarget::Network { host: "127.0.0.1".into(), port };
        let job = PrintJob::copies(target.clone(), &[], doc_paced.clone(), 3, 0).paced(true).for_mode(cfg.image_mode);
        assert!(job.is_one_connection() && job.is_paced());
        job.clone().send_in_background().unwrap();
        let got = reader.join().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, doc_paced.repeat(3));
        let (copies, tail) = tickets_in(&got[0].0);
        assert_eq!(copies.len(), 3);
        assert!(copies.iter().all(|c| fed_and_cut(c) && *c == doc_paced.as_slice()), "every copy whole, fed and cut");
        assert!(tail.is_empty(), "nothing left after the last cut: {tail:?}");
        assert!(wait_for_prints(Duration::from_secs(10)));

        // Not paced (send slowly off): the same, one connection.
        let (port, reader) = slow_printer(1);
        let target = PrinterTarget::Network { host: "127.0.0.1".into(), port };
        let job = PrintJob::copies(target, &[], doc.clone(), 3, 0).paced(false).for_mode(ImageMode::Column);
        assert!(job.is_one_connection() && !job.is_paced());
        job.send_in_background().unwrap();
        assert_eq!(reader.join().unwrap(), vec![(doc.repeat(3), false)]);
        assert!(wait_for_prints(Duration::from_secs(10)));

        // Waiting for every copy (PrintJob::send): one connection too.
        let (port, reader) = slow_printer(1);
        let target = PrinterTarget::Network { host: "127.0.0.1".into(), port };
        PrintJob::copies(target, &[], doc_paced.clone(), 2, 0).paced(true).for_mode(ImageMode::Column).send().unwrap();
        assert_eq!(reader.join().unwrap(), vec![(doc_paced.repeat(2), false)]);
    }

    #[test]
    fn standard_and_small_modes_still_send_each_copy_on_its_own_connection() {
        for mode in [ImageMode::Raster, ImageMode::RasterSmall] {
            let cfg = PrinterConfig { image_mode: mode, copies: 3, ..PrinterConfig::default() };
            let doc = ticket().escpos_for(&cfg);
            let (port, reader) = slow_printer(3);
            let target = PrinterTarget::Network { host: "127.0.0.1".into(), port };
            let job = PrintJob::copies(target, &[], doc.clone(), 3, 0).paced(false).for_mode(mode);
            assert!(!job.one_connection && !job.is_one_connection());
            job.send_in_background().unwrap();
            let got = reader.join().unwrap();
            assert_eq!(got, vec![(doc.clone(), false); 3], "{mode:?}");
            let (copies, tail) = tickets_in(&got[0].0);
            assert!(copies.len() == 1 && fed_and_cut(copies[0]) && tail.is_empty());
        }
        // Windows printers: never one connection (each copy its own spooler job).
        let job = PrintJob::new(PrinterTarget::Windows { name: "Kitchen".into() }, vec![1]).for_mode(ImageMode::Column);
        assert!(job.one_connection && !job.is_one_connection());
        assert!(PrintJob::new(PrinterTarget::File { path: "COM3".into() }, vec![1]).for_mode(ImageMode::Column).is_one_connection());
    }

    #[test]
    fn copies_of_each_kind_of_kitchen_ticket() {
        // (title, reprint, all courses together, only starters)
        let new_order = ("NEW ORDER", false, false, false);
        let starters = ("NEW ORDER", false, false, true);
        let mains_away = ("MAINS AWAY", false, false, false);
        let added = ("ADDITIONAL", false, false, false);
        let together = ("NEW ORDER", false, true, false);
        let together_after_drinks = ("MAINS AWAY", false, true, false);
        let together_added = ("ADDITIONAL", false, true, false);
        let reprint = ("REPRINT", true, false, false);
        let n = |cfg: &PrinterConfig, (title, re, all, st): (&str, bool, bool, bool)| kitchen_copies(cfg, title, re, all, st);

        // Nothing set ("Same" everywhere): as before.
        let plain = PrinterConfig { copies: 3, ..PrinterConfig::default() };
        let want = [(new_order, 3), (starters, 3), (mains_away, 3), (added, 1), (together, 3), (together_after_drinks, 3), (together_added, 1), (reprint, 1)];
        for (kind, copies) in want {
            assert_eq!(n(&plain, kind), copies, "{kind:?}");
        }
        assert_eq!(call_again_copies(&plain), 3);

        // The older settings still apply as before.
        let old = PrinterConfig { starter_copies: Some(2), mains_copies: Some(4), additional_copies: Some(5), ..plain.clone() };
        let want = [(new_order, 3), (starters, 2), (mains_away, 4), (added, 5), (together, 3), (together_after_drinks, 4), (together_added, 5), (reprint, 1)];
        for (kind, copies) in want {
            assert_eq!(n(&old, kind), copies, "{kind:?}");
        }
        assert_eq!(call_again_copies(&old), 4, "Same = Mains copies");

        // The new settings: only their own tickets change.
        let new = PrinterConfig { all_courses_copies: Some(1), call_again_copies: Some(2), ..old.clone() };
        let want = [(new_order, 3), (starters, 2), (mains_away, 4), (added, 5), (together, 1), (together_after_drinks, 1), (together_added, 1), (reprint, 1)];
        for (kind, copies) in want {
            assert_eq!(n(&new, kind), copies, "{kind:?}");
        }
        assert_eq!(call_again_copies(&new), 2);
        let five = PrinterConfig { all_courses_copies: Some(5), ..plain.clone() };
        assert_eq!(n(&five, together), 5);
        assert_eq!(n(&five, together_added), 5);
        assert_eq!(n(&five, reprint), 1, "reprints stay at one");

        // Saved without them: "Same"; saved as set; checked like the others.
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("all_courses_copies") && !json.contains("call_again_copies"), "{json}");
        let back: PrinterConfig = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
        assert_eq!((back.all_courses_copies, back.call_again_copies), (Some(1), Some(2)));
        for bad in [0, 10] {
            assert!(PrinterConfig { all_courses_copies: Some(bad), ..plain.clone() }.validate().is_err());
            assert!(PrinterConfig { call_again_copies: Some(bad), ..plain.clone() }.validate().is_err());
        }
        assert!(new.validate().is_ok());
    }

    #[test]
    fn send_slowly_is_on_by_default_and_only_for_network_and_port_printers() {
        // Saved before the setting: on (the cheap printers that need it are common).
        let cfg: PrinterConfig = serde_json::from_str(r#"{"kind":"network","host":"192.168.1.50","port":9100,"width":42}"#).unwrap();
        assert!(cfg.slow_send && PrinterConfig::default().slow_send);
        let route: KitchenRoute = serde_json::from_str(r#"{"label":"Bar","kind":"network","host":"10.0.0.9","port":9100,"drinks":true}"#).unwrap();
        assert!(route.slow_send);
        let receipt: crate::ReceiptSettings = serde_json::from_str(r#"{"kind":"file","path":"COM3","auto_print":true}"#).unwrap();
        assert!(receipt.printer.slow_send);
        // Switched off: saved and read back.
        let off = PrinterConfig { slow_send: false, ..cfg };
        let json = serde_json::to_string(&off).unwrap();
        assert!(json.contains(r#""slow_send":false"#), "{json}");
        assert!(!serde_json::from_str::<PrinterConfig>(&json).unwrap().slow_send);
        // Paced: network and ports or shared paths; never Windows printers
        // (the spooler and USB pace the data) or no printer.
        let job = |t: PrinterTarget| PrintJob::new(t, vec![1]).paced(true);
        assert!(job(PrinterTarget::Network { host: "h".into(), port: 9100 }).is_paced());
        assert!(job(PrinterTarget::File { path: "COM3".into() }).is_paced());
        assert!(!job(PrinterTarget::Windows { name: "EPSON".into() }).is_paced());
        assert!(!job(PrinterTarget::None).is_paced());
        assert!(!PrintJob::new(PrinterTarget::File { path: "COM3".into() }, vec![1]).is_paced());
    }

    #[test]
    fn config_json_shape() {
        let cfg: PrinterConfig = serde_json::from_str(r#"{"kind":"network","host":"192.168.1.50","port":9100}"#).unwrap();
        assert_eq!(cfg.width, 42);
        assert_eq!(cfg.target, PrinterTarget::Network { host: "192.168.1.50".into(), port: 9100 });
        assert_eq!(cfg.copy_delay_secs, 2);
        let ok = PrinterConfig::default();
        assert!(PrinterConfig { target: PrinterTarget::Windows { name: " ".into() }, ..ok.clone() }.validate().is_err());
        assert!(PrinterConfig { width: 10, ..ok.clone() }.validate().is_err());
        assert!(PrinterConfig { copy_delay_secs: 31, ..ok.clone() }.validate().is_err());
        assert!(PrinterConfig { copy_delay_secs: 0, ..ok }.validate().is_ok());
    }

    #[test]
    fn old_settings_load_with_the_standard_image_mode() {
        let cfg: PrinterConfig = serde_json::from_str(r#"{"kind":"network","host":"192.168.1.50","port":9100,"width":42,"bold":true}"#).unwrap();
        assert_eq!(cfg.image_mode, ImageMode::Raster);
        let route: KitchenRoute = serde_json::from_str(r#"{"label":"Bar","kind":"windows","name":"Bar","width":32,"copies":1,"drinks":true}"#).unwrap();
        assert_eq!(route.image_mode, ImageMode::Raster);
        let receipt: crate::ReceiptSettings = serde_json::from_str(r#"{"kind":"file","path":"COM3","auto_print":true}"#).unwrap();
        assert_eq!(receipt.printer.image_mode, ImageMode::Raster);
        // The names saved and sent by the settings screen.
        for (mode, name) in [(ImageMode::Raster, "raster"), (ImageMode::Column, "column"), (ImageMode::RasterSmall, "raster_small")] {
            let json = serde_json::to_string(&PrinterConfig { image_mode: mode, ..PrinterConfig::default() }).unwrap();
            assert!(json.contains(&format!(r#""image_mode":"{name}""#)), "{json}");
            assert_eq!(serde_json::from_str::<PrinterConfig>(&json).unwrap().image_mode, mode);
        }
        assert!(serde_json::from_str::<PrinterConfig>(r#"{"kind":"none","image_mode":"bogus"}"#).is_err());
    }

    #[test]
    fn every_image_on_a_ticket_follows_the_image_mode() {
        let mut t = ticket();
        t.items[0].name_alt = Some("เบอร์เกอร์".into());
        t.banner = Some("CURRY / แกง".into());
        let gs_v0 = |b: &[u8]| b.windows(4).filter(|w| *w == [0x1D, b'v', b'0', 0]).count();
        let esc_star = |b: &[u8]| b.windows(3).filter(|w| *w == [0x1B, b'*', 33]).count();
        for (width, bold, app_font) in [(42, true, true), (32, false, false), (48, true, false)] {
            let cfg = PrinterConfig { width, bold, app_font, ..PrinterConfig::default() };
            // The default is exactly what the ticket always printed.
            assert_eq!(t.escpos_for(&cfg), t.to_escpos_font(width, bold, app_font));
            let std = t.escpos_for(&cfg);
            assert!(gs_v0(&std) > 0 && esc_star(&std) == 0);
            assert!(!std.windows(2).any(|w| w == [0x1C, b'.']));
            // Compatible: no raster command left, the bar and Thai as column images.
            let col = t.escpos_for(&PrinterConfig { image_mode: ImageMode::Column, ..cfg.clone() });
            assert_eq!(&col[..10], &[0x1B, b'@', 0x1B, b't', 0, 0x1B, b'R', 0, 0x1C, b'.']);
            assert_eq!(gs_v0(&col), 0);
            assert!(esc_star(&col) >= 2, "bar and Thai name");
            assert!(col.ends_with(&[0x1D, b'V', 66, 0]));
            // Small pieces: more, smaller raster commands.
            let small = t.escpos_for(&PrinterConfig { image_mode: ImageMode::RasterSmall, ..cfg });
            assert_eq!(&small[8..10], &[0x1C, b'.']);
            assert!(gs_v0(&small) > gs_v0(&std));
            assert_eq!(esc_star(&small), 0);
        }
    }

    #[test]
    fn thai_test_page_has_every_mode_safest_first() {
        const INIT: [u8; 8] = [0x1B, b'@', 0x1B, b't', 0, 0x1B, b'R', 0];
        for width in [32, 42, 48] {
            let b = thai_test_page(width);
            let find = |needle: &[u8]| b.windows(needle.len()).position(|w| w == needle).unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(needle)));
            assert!(b.starts_with(&INIT));
            let head = find(b"Thai test - pick the mode");
            let labels = ["1. Compatible (older printers)", "2. Small pieces", "3. Standard"].map(|l| find(l.as_bytes()));
            let end = find(b"End of test");
            assert!(head < labels[0] && labels[0] < labels[1] && labels[1] < labels[2] && labels[2] < end);
            assert!(b.ends_with(&[0x1B, b'd', 4, 0x1D, b'V', 66, 0]));
            // Each mode starts afresh, so one that fails upsets the next one less.
            // Standard gets no FS ., exactly like a real Standard job.
            for (&l, mode) in labels.iter().zip(ImageMode::TEST_ORDER) {
                assert!(b[..l].ends_with(&[INIT.as_slice(), mode.init_extra(), &[0x1B, b'a', 0, 0x1D, b'!', 0, 0x1B, b'E', 1]].concat()), "{mode:?}");
            }
            // The header and the end start like a real Standard job too (no FS .).
            assert!(b[INIT.len()..].starts_with(&[0x1B, b'a', 0]));
            assert!(b[..end].ends_with(&[INIT.as_slice(), &[0x1B, b'a', 0, 0x1D, b'!', 0, 0x1B, b'E', 0]].concat()));
            // The sample under each label is drawn in that label's mode.
            let dots = width * crate::raster::DOTS_PER_CHAR;
            let sections = [&b[labels[0]..labels[1]], &b[labels[1]..labels[2]], &b[labels[2]..end]];
            for (mode, sec) in ImageMode::TEST_ORDER.into_iter().zip(sections) {
                let img = crate::raster::escpos_line_in(THAI_SAMPLE, Size::Normal, false, dots, mode);
                assert!(sec.windows(img.len()).any(|w| w == img.as_slice()), "{mode:?} at {width}");
            }
            assert_eq!(ImageMode::TEST_ORDER, [ImageMode::Column, ImageMode::RasterSmall, ImageMode::Raster]);
            // Nothing but the images is outside plain ASCII.
            let mut text = b.clone();
            for mode in ImageMode::TEST_ORDER {
                let img = crate::raster::escpos_line_in(THAI_SAMPLE, Size::Normal, false, dots, mode);
                let at = text.windows(img.len()).position(|w| w == img.as_slice()).unwrap();
                text.drain(at..at + img.len());
            }
            assert!(text.is_ascii());
        }
    }

    #[test]
    fn the_audit_log_says_who_cancelled_and_whose_pin_approved_it() {
        use crate::{NewOrder, NewProduct, OrderDetails, Role};
        let mut pos = Pos::open_in_memory().unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        let sam = pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        let quinn = pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
        let curry = pos
            .create_product(&admin, NewProduct {
                category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 900, cost_cents: 0, tax_rate_bp: Some(0),
                stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
            })
            .unwrap();
        let new = NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: OrderDetails::default() };
        let last = |pos: &Pos| -> (i64, String, i64) {
            pos.conn
                .query_row(
                    "SELECT a.user_id, a.detail, o.closed_by FROM audit_log a JOIN orders o ON o.id = CAST(a.detail AS INTEGER)
                     WHERE a.action = 'order.cancel' ORDER BY a.id DESC LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap()
        };
        // Quinn (manager) with no PIN.
        let q = pos.login("4646").unwrap();
        let o = pos.open_order(&q, &new).unwrap();
        pos.cancel_order_confirmed(&q, o.id, "mistake", None).unwrap();
        assert_eq!(last(&pos), (quinn.id, format!("{} mistake (no PIN, manager)", o.id), quinn.id));
        // Sam (staff) sends a dish; Quinn's PIN approves the cancel on Sam's till.
        let s = pos.login("2222").unwrap();
        let o = pos.open_order(&s, &new).unwrap();
        pos.add_item(&s, o.id, curry.id, 1, None, None, &[]).unwrap();
        pos.send_to_kitchen(&s, o.id).unwrap();
        pos.cancel_order_confirmed(&s, o.id, "", Some("4646")).unwrap();
        assert_eq!(last(&pos), (sam.id, format!("{} (PIN of Quinn #{}, manager)", o.id, quinn.id), quinn.id));
        // Sam's own PIN on an order the kitchen has not got.
        let o = pos.open_order(&s, &new).unwrap();
        pos.cancel_order_confirmed(&s, o.id, "", Some("2222")).unwrap();
        assert_eq!(last(&pos), (sam.id, format!("{} (PIN of Sam #{}, staff)", o.id, sam.id), sam.id));
    }

    #[test]
    fn a_long_set_heading_in_the_app_font_drops_its_rules_rather_than_wrap() {
        // "=== SET: QA LUNCH SET" with "===" alone on the next line, at 80 mm.
        for (width, mode) in [42, 32].into_iter().flat_map(|w| ImageMode::TEST_ORDER.map(|m| (w, m))) {
            let dots = width * crate::raster::DOTS_PER_CHAR;
            let print = |name: &str| {
                let line = Line::new(Size::Tall, true, true, titled_rule(&format!("SET: {name}"), '=', width));
                render_escpos_in(&[line], width, true, true, mode)
            };
            let has = |b: &[u8], text: &str, size| {
                let img = crate::raster::escpos_line_in(text, size, true, dots, mode);
                b.windows(img.len()).any(|w| w == img.as_slice())
            };
            // Short: its rules stay, on one line.
            assert!(has(&print("Pad"), "=== SET: Pad ===", Size::Tall), "{width} {mode:?}");
            // Long: the words alone, on one line.
            let b = print("QA LUNCH SET");
            assert!(!has(&b, "=== SET: QA LUNCH SET ===", Size::Tall), "{width} {mode:?}");
            assert!(has(&b, "SET: QA LUNCH SET", Size::Tall), "{width} {mode:?}");
            assert_eq!(crate::raster::line_images("SET: QA LUNCH SET", Size::Tall, true, dots).len(), 1, "{width}");
            // Too long even alone: smaller (and wrapped only if it must be).
            let name = "THE BIG SUNDAY SHARING PLATTER";
            assert!(!crate::raster::fits_line(&format!("SET: {name}"), Size::Tall, dots));
            assert!(has(&print(name), &format!("SET: {name}"), Size::Normal), "{width} {mode:?}");
        }
    }

    #[test]
    fn mains_to_follow_stays_on_one_line_on_58_mm_paper() {
        // "MAINS TO FOLLOW: 11" took 2-3 lines on 58 mm paper, in both fonts.
        let footer = |n: i64| KitchenTicket { footer: Some(format!("{MAINS_TO_FOLLOW}{n}")), ..ticket() };
        let one = |lines: &[Line], text: &str, size| lines.iter().filter(|l| l.text.contains("MAINS")).map(|l| (l.text.as_str(), l.size)).eq([(text, size)]);
        // The printer's font: double size, the short words when the long ones do not fit.
        assert!(one(&footer(11).lines(42), "MAINS TO FOLLOW: 11", Size::Big));
        assert!(one(&footer(11).lines(32), "MAINS LATER: 11", Size::Big));
        assert!(one(&footer(123).lines(32), "MAINS LATER: 123", Size::Big));
        assert!(one(&footer(12345).lines(32), "MAINS TO FOLLOW: 12345", Size::Tall));
        assert!(footer(11).to_text(32).contains("\nMAINS LATER: 11\n"), "{}", footer(11).to_text(32));
        // The app's font, in each image mode: one image line, as big as it fits.
        for (width, mode) in [42, 32].into_iter().flat_map(|w| ImageMode::TEST_ORDER.map(|m| (w, m))) {
            let dots = width * crate::raster::DOTS_PER_CHAR;
            let has = |b: &[u8], text: &str, size| {
                let img = crate::raster::escpos_line_in(text, size, true, dots, mode);
                b.windows(img.len()).any(|w| w == img.as_slice())
            };
            // What prints (80 mm is 504 dots, 58 mm 384): the long words at
            // double size while they fit, then the short ones, then smaller.
            let expect = [
                (42, 3, "MAINS TO FOLLOW: 3", Size::Big),
                (42, 11, "MAINS TO FOLLOW: 11", Size::Big),
                (42, 123, "MAINS LATER: 123", Size::Big),
                (32, 3, "MAINS LATER: 3", Size::Big),
                (32, 11, "MAINS LATER: 11", Size::Big),
                (32, 123, "MAINS LATER: 123", Size::Tall),
            ];
            for &(_, n, text, size) in expect.iter().filter(|e| e.0 == width) {
                let b = render_escpos_in(&footer(n).lines(width), width, true, true, mode);
                assert!(has(&b, text, size), "{width} {mode:?} {n}");
                assert_eq!(crate::raster::line_images(text, size, true, dots).len(), 1, "{width} {n}");
            }
        }
    }
}
