//! Kitchen tickets for ESC/POS thermal printers (Epson TM, Star, Xprinter and
//! the many compatibles), plus the shared layout and printer transports used
//! by customer receipts (see `receipt.rs`).
//!
//! Building a ticket needs the database; sending it does not. Callers build a
//! [`PrintJob`] while holding the database, release it, then call
//! [`PrintJob::send`], so a slow or offline printer never blocks the till.

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::invalid;
use crate::{Order, OrderItem, OrderType, Permission, Pos, PosError, Result, Session};

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
}

fn yes() -> bool {
    true
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
        }
    }
}

impl PrinterConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if !(24..=64).contains(&self.width) {
            return invalid("paper width must be between 24 and 64 characters");
        }
        if [Some(self.copies), self.starter_copies, self.mains_copies, self.prep_copies].into_iter().flatten().any(|c| !(1..=9).contains(&c)) {
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
}

impl PrintJob {
    pub fn new(target: PrinterTarget, bytes: Vec<u8>) -> PrintJob {
        PrintJob { target, parts: vec![bytes], gap: Duration::ZERO }
    }

    /// `copies` of `doc`, the first one preceded by `lead` (e.g. the drawer pulse).
    pub(crate) fn copies(target: PrinterTarget, lead: &[u8], doc: Vec<u8>, copies: u8, delay_secs: u8) -> PrintJob {
        let mut first = lead.to_vec();
        first.extend_from_slice(&doc);
        let mut parts = vec![first];
        parts.extend(std::iter::repeat_n(doc, usize::from(copies.max(1)) - 1));
        PrintJob { target, parts, gap: Duration::from_secs(delay_secs.into()) }
    }

    /// Everything the printer receives, in order.
    pub fn bytes(&self) -> Vec<u8> {
        self.parts.concat()
    }

    /// Send every part, waiting `gap` between them.
    pub fn send(&self) -> Result<()> {
        for (i, part) in self.parts.iter().enumerate() {
            if i > 0 {
                std::thread::sleep(self.gap);
            }
            send_bytes(&self.target, part)?;
        }
        Ok(())
    }

    /// Send the first copy now (its result is returned) and any further
    /// copies on a background thread, so the till does not wait out the delay.
    pub fn send_in_background(self) -> Result<()> {
        let mut parts = self.parts.into_iter();
        if let Some(first) = parts.next() {
            send_bytes(&self.target, &first)?;
        }
        let rest: Vec<Vec<u8>> = parts.collect();
        if !rest.is_empty() {
            let (target, gap) = (self.target, self.gap);
            std::thread::spawn(move || {
                for part in rest {
                    std::thread::sleep(gap);
                    if send_bytes(&target, &part).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(())
    }
}

fn send_bytes(target: &PrinterTarget, bytes: &[u8]) -> Result<()> {
    let fail = PosError::Printer;
    match target {
        PrinterTarget::None => Ok(()),
        PrinterTarget::Network { host, port } => {
            let addr = (host.trim(), *port)
                .to_socket_addrs()
                .map_err(|e| fail(e.to_string()))?
                .next()
                .ok_or_else(|| fail(format!("cannot resolve {host}")))?;
            let mut s = TcpStream::connect_timeout(&addr, NET_TIMEOUT)
                .map_err(|e| fail(format!("cannot reach {host}:{port} ({e})")))?;
            s.set_write_timeout(Some(NET_TIMEOUT)).map_err(|e| fail(e.to_string()))?;
            s.write_all(bytes).and_then(|_| s.flush()).map_err(|e| fail(e.to_string()))
        }
        PrinterTarget::File { path } => {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path.trim())
                .map_err(|e| fail(format!("{path}: {e}")))?;
            f.write_all(bytes).map_err(|e| fail(e.to_string()))
        }
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
    /// Menu category, used to group the ticket ("STARTERS", "MAINS"...).
    pub category: Option<String>,
    pub qty: i64,
    pub name: String,
    /// Printed under the name (Thai), when the printer prints both.
    pub name_alt: Option<String>,
    pub options: Vec<String>,
    pub note: Option<String>,
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
}

impl Line {
    pub fn new(size: Size, bold: bool, center: bool, text: impl Into<String>) -> Line {
        Line { size, bold, center, text: text.into() }
    }
}

/// Category heading such as "-- STARTERS --".
pub(crate) fn category_heading(name: Option<&str>) -> String {
    format!("-- {} --", name.unwrap_or("Other").to_uppercase())
}

/// Plain-text rendering of laid-out lines (previews and tests).
pub(crate) fn render_text(lines: &[Line], width: usize) -> String {
    let mut s = String::new();
    for l in lines {
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
    // Init, code page 437, and the USA character set: printers set to the UK
    // set print "#" as "£", which turned "#12" into "£12".
    let mut b = vec![0x1B, b'@', 0x1B, b't', 0, 0x1B, b'R', 0];
    for l in lines {
        if crate::raster::needs_raster(&l.text) {
            b.extend(crate::raster::escpos_line(l.text.trim_end(), l.size, l.center, width * crate::raster::DOTS_PER_CHAR));
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
            // Split an over-long word, keeping marks with their letter.
            let room = width.saturating_sub(used).max(1);
            let mut take = 0;
            let mut seen = 0;
            while take < word.len() && (seen < room || is_mark(word[take])) {
                seen += usize::from(!is_mark(word[take]));
                take += 1;
            }
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

/// Printed width in characters.
pub(crate) fn cols(text: &str) -> usize {
    text.chars().filter(|c| !is_mark(*c)).count()
}

impl KitchenTicket {
    fn lines(&self, width: usize) -> Vec<Line> {
        let mut v = Vec::new();
        let mut push = |size, bold, center, text: String| v.push(Line { size, bold, center, text });
        let big = width / 2;
        // Dishes, options and notes: double size (large) or double height.
        let (size, w) = if self.large { (Size::Big, big) } else { (Size::Tall, width) };
        let (cont, opt, opt_cont, note, note_cont) =
            if self.large { ("  ", "  + ", "    ", "  ** ", "     ") } else { ("    ", "   + ", "     ", "   ** ", "      ") };
        let small = if self.large { Size::Tall } else { Size::Normal };
        for t in wrap(&self.title, big, "", "") {
            push(Size::Big, true, true, t);
        }
        for t in wrap(&self.heading, big, "", "") {
            push(Size::Big, true, self.brief, t);
        }
        if !self.brief {
            for d in &self.details {
                for t in wrap(d, width, "", "  ") {
                    push(small, false, false, t);
                }
            }
            push(small, false, false, format!("#{}  {}  {}", self.order_id, self.time, self.staff));
        }
        push(Size::Normal, false, false, "-".repeat(width));
        let grouped = !self.brief && self.items.iter().any(|i| i.category.is_some());
        let mut current: Option<Option<&str>> = None;
        for it in &self.items {
            let cat = it.category.as_deref();
            if grouped && current != Some(cat) {
                if current.is_some() {
                    push(Size::Normal, false, false, String::new());
                }
                push(small, true, true, category_heading(cat));
                current = Some(cat);
            }
            for (i, t) in wrap(&format!("{} x {}", it.qty, it.name), w, "", cont).into_iter().enumerate() {
                push(size, i == 0, false, t);
            }
            if let Some(alt) = &it.name_alt {
                for t in wrap(alt, w, cont, cont) {
                    push(size, false, false, t);
                }
            }
            for o in &it.options {
                for t in wrap(o, w, opt, opt_cont) {
                    push(size, false, false, t);
                }
            }
            if let Some(n) = &it.note {
                for t in wrap(n, w, note, note_cont) {
                    push(size, true, false, t);
                }
            }
        }
        push(Size::Normal, false, false, "-".repeat(width));
        if let Some(f) = &self.footer {
            for t in wrap(f, big, "", "") {
                push(Size::Big, true, true, t);
            }
        }
        // The table again at the bottom, where the ticket is read on the pass.
        for t in wrap(&self.heading, big, "", "") {
            push(Size::Big, true, true, t);
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

pub(crate) fn heading(o: &Order) -> String {
    if let Some(ch) = &o.channel_name {
        let r = o.external_ref.as_deref().map(|r| format!(" {r}")).unwrap_or_default();
        return format!("{}{r}", ch.to_uppercase());
    }
    match o.order_type {
        OrderType::DineIn => format!("DINE-IN {}", o.table_name.as_deref().unwrap_or("?")),
        OrderType::Takeaway => "TAKEAWAY".into(),
        OrderType::Collection => "COLLECTION".into(),
        OrderType::Delivery => "DELIVERY".into(),
    }
}

impl Pos {
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
        let o = self.get_order(s, order_id)?;
        let first_round = !reprint && o.items.iter().all(|i| i.sent_at.is_none() || items.iter().any(|x| x.id == i.id));
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
        let title = if reprint {
            "REPRINT"
        } else if first_round {
            "NEW ORDER"
        } else if items.iter().any(|i| i.held) {
            "MAINS AWAY"
        } else {
            "ADDITIONAL"
        };
        let names = self.kitchen_printer()?.names.unwrap_or(PrintNames::Both);
        Ok(KitchenTicket {
            title: title.into(),
            heading: heading(&o),
            details,
            order_id,
            time: chrono::Local::now().format("%H:%M").to_string(),
            staff: s.name.clone(),
            items: self
                .by_category(items, names)?
                .into_iter()
                .map(|(category, i)| {
                    let (name, name_alt) = names.pick(&i.name, i.name_alt.as_deref());
                    TicketItem {
                        category,
                        qty: i.qty,
                        name: name.to_string(),
                        name_alt: name_alt.map(str::to_string),
                        options: i.options.iter().map(|o| names.inline(&o.name, o.name_alt.as_deref())).collect(),
                        note: i.note.clone(),
                    }
                })
                .collect(),
            footer: (!reprint && waiting > 0).then(|| format!("MAINS TO FOLLOW: {waiting}")),
            brief: false,
            large: self.kitchen_printer()?.large_text,
        })
    }

    /// Print job for `items`, or None when no kitchen printer is set up.
    pub fn kitchen_job(&self, s: &Session, order_id: i64, items: &[OrderItem], reprint: bool) -> Result<Option<PrintJob>> {
        let cfg = self.kitchen_printer()?;
        if cfg.target == PrinterTarget::None || items.is_empty() {
            return Ok(None);
        }
        let ticket = self.kitchen_ticket(s, order_id, items, reprint)?;
        let copies = if reprint {
            1
        } else if ticket.title == "MAINS AWAY" {
            cfg.mains_copies.unwrap_or(cfg.copies)
        } else if ticket.footer.is_some() && items.iter().all(|i| i.starter) {
            // Starters going ahead of the mains.
            cfg.starter_copies.unwrap_or(cfg.copies)
        } else {
            cfg.copies
        };
        Ok(Some(PrintJob::copies(cfg.target, &[], ticket.to_escpos(cfg.width, cfg.bold), copies, cfg.copy_delay_secs)))
    }

    /// PREPARE NOW ticket for prepare-ahead dishes that wait for the mains
    /// (from [`Pos::send_prep`]). None when no kitchen printer is set up.
    pub fn prep_job(&self, s: &Session, order_id: i64, items: &[OrderItem]) -> Result<Option<PrintJob>> {
        let cfg = self.kitchen_printer()?;
        if cfg.target == PrinterTarget::None || items.is_empty() {
            return Ok(None);
        }
        let ticket = self.prep_ticket(s, order_id, items)?;
        let copies = cfg.prep_copies.unwrap_or(cfg.copies);
        Ok(Some(PrintJob::copies(cfg.target, &[], ticket.to_escpos(cfg.width, cfg.bold), copies, cfg.copy_delay_secs)))
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
        let mains: Vec<OrderItem> = o.items.iter().filter(|i| i.sent_at.is_some() && !i.starter).cloned().collect();
        if mains.is_empty() {
            return invalid("no main courses have been sent to the kitchen yet");
        }
        self.audit(Some(s.user_id), "kitchen.main_course", &order_id.to_string())?;
        let cfg = self.kitchen_printer()?;
        if cfg.target == PrinterTarget::None {
            return Ok(None);
        }
        let ticket = self.main_course_ticket(&o, &mains)?;
        let copies = cfg.mains_copies.unwrap_or(cfg.copies);
        Ok(Some(PrintJob::copies(cfg.target, &[], ticket.to_escpos(cfg.width, cfg.bold), copies, cfg.copy_delay_secs)))
    }

    fn main_course_ticket(&self, o: &Order, mains: &[OrderItem]) -> Result<KitchenTicket> {
        let names = self.kitchen_printer()?.names.unwrap_or(PrintNames::Both);
        Ok(KitchenTicket {
            title: "MAIN COURSE".into(),
            // "TABLE 4" for numbered tables; named ones ("Patio 2") as they are.
            heading: match &o.table_name {
                Some(t) if t.chars().all(|c| c.is_ascii_digit()) => format!("TABLE {t}"),
                Some(t) => t.clone(),
                None => heading(o),
            },
            details: vec![],
            order_id: o.id,
            time: chrono::Local::now().format("%H:%M").to_string(),
            staff: String::new(),
            items: mains
                .iter()
                .map(|i| {
                    let (name, name_alt) = names.pick(&i.name, i.name_alt.as_deref());
                    TicketItem {
                        category: None,
                        qty: i.qty,
                        name: name.to_string(),
                        name_alt: name_alt.map(str::to_string),
                        options: vec![],
                        note: None,
                    }
                })
                .collect(),
            footer: None,
            brief: true,
            large: self.kitchen_printer()?.large_text,
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
                    category: Some(names.inline("Starters", Some("อาหารเรียกน้ำย่อย"))),
                    qty: 1,
                    name: soup,
                    name_alt: soup_alt,
                    options: vec![],
                    note: None,
                },
                TicketItem {
                    category: Some(names.inline("Mains", Some("อาหารจานหลัก"))),
                    qty: 2,
                    name: burger,
                    name_alt: burger_alt,
                    options: vec![names.inline("Large", Some("ใหญ่")), names.inline("Extra cheese", Some("เพิ่มชีส"))],
                    note: Some("no onions".into()),
                },
            ],
            footer: None,
            brief: false,
            large: cfg.large_text,
        };
        Ok(PrintJob::copies(cfg.target.clone(), &[], ticket.to_escpos(cfg.width, cfg.bold), cfg.copies, cfg.copy_delay_secs))
    }
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
                TicketItem { category: None, qty: 2, name: "Burger".into(), name_alt: None, options: vec!["Large".into(), "Extra cheese".into()], note: Some("no onions".into()) },
                TicketItem { category: None, qty: 1, name: "Crème brûlée".into(), name_alt: None, options: vec![], note: None },
            ],
            footer: None,
            brief: false,
            large: false,
        }
    }

    #[test]
    fn text_layout() {
        let t = ticket().to_text(32);
        let expected = "   NEW ORDER\nDINE-IN T3\nGround\n#12  19:05  Sam\n--------------------------------\n\
                        2 x Burger\n   + Large\n   + Extra cheese\n   ** no onions\n1 x Crème brûlée\n--------------------------------\n   DINE-IN T3\n";
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
            items: vec![TicketItem { category: None, qty: 1, name: "Chicken satay with peanut sauce".into(), name_alt: None, options: vec![], note: None }],
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
        assert_eq!(b.windows(4).filter(|w| *w == [0x1D, b'v', b'0', 0]).count(), 2);
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
        assert_eq!(wrap("ไข่ดาวไข่ดาว", 4, "", ""), ["ไข่ดา", "วไข่ด", "าว"]);
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
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let job = PrintJob::new(PrinterTarget::Network { host: "127.0.0.1".into(), port }, vec![1]);
        assert!(job.send().is_err());
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
}
