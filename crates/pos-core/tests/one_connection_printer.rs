//! A printer in the compatible image mode (older printers) modelled as QA
//! found it: it takes one connection at a time, resets any other that comes
//! in while one is open, and drains about 40 KB a second. Every ticket of a
//! cash payment (drawer, receipts, staff copy) and of a kitchen job sent
//! right before a receipt must still arrive.
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pos_core::*;

const KICK: [u8; 5] = [0x1B, b'p', 0, 25, 250];
const CUT: [u8; 4] = [0x1D, b'V', 66, 0];
/// The drawer pulse as sent on its own (with an init before it).
const DRAWER: [u8; 7] = [0x1B, b'@', 0x1B, b'p', 0, 25, 250];

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// What the old printer got: each connection it took (its bytes), and how
/// many it turned away.
#[derive(Default)]
struct Seen {
    taken: Vec<Vec<u8>>,
    reset: usize,
    open: usize,
}

/// The old printer, on 127.0.0.1. Returns its port and what it saw.
fn old_printer() -> (u16, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for c in listener.incoming() {
            let Ok(mut c) = c else { break };
            let mut st = s2.lock().unwrap();
            if st.open > 0 {
                st.reset += 1;
                let _ = socket2::SockRef::from(&c).set_linger(Some(Duration::ZERO));
                drop(c);
                continue;
            }
            st.open += 1;
            drop(st);
            let s3 = s2.clone();
            std::thread::spawn(move || {
                let mut got = Vec::new();
                let mut buf = [0u8; 2048];
                c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
                // About 40 KB a second.
                loop {
                    match c.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                let mut st = s3.lock().unwrap();
                st.taken.push(got);
                st.open -= 1;
            });
        }
    });
    (port, seen)
}

/// Wait until the printer has taken `n` connections and is idle (or 40 s).
fn taken(seen: &Arc<Mutex<Seen>>, n: usize) -> (Vec<Vec<u8>>, usize) {
    let end = Instant::now() + Duration::from_secs(40);
    while Instant::now() < end {
        {
            let st = seen.lock().unwrap();
            if st.taken.len() >= n && st.open == 0 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(wait_for_prints(Duration::from_secs(20)));
    std::thread::sleep(Duration::from_millis(300));
    let st = seen.lock().unwrap();
    (st.taken.clone(), st.reset)
}

fn setup() -> (Pos, Session, Product, Product) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let cat = pos.create_category(&admin, "Food", None, 1).unwrap();
    let mut item = |name: &str, starter| {
        pos.create_product(&admin, NewProduct {
            category_id: Some(cat.id), sku: None, barcode: None, name: name.into(), price_cents: 500, cost_cents: 0,
            tax_rate_bp: Some(0), stock_qty: 0, track_stock: false, starter, name_alt: Some("ไก่ผัด".into()), prep: false, team_id: None,
        })
        .unwrap()
    };
    let soup = item("Soup", true);
    let curry = item("Curry", false);
    (pos, admin, soup, curry)
}

fn printer(port: u16, mode: ImageMode, slow: bool) -> PrinterConfig {
    PrinterConfig {
        target: PrinterTarget::Network { host: "127.0.0.1".into(), port },
        copies: 2, copy_delay_secs: 0, image_mode: mode, slow_send: slow, names: Some(PrintNames::Both), ..Default::default()
    }
}

fn receipts(pos: &Pos, s: &Session, port: u16, slow: bool, auto_print: bool) {
    let settings = ReceiptSettings { printer: printer(port, ImageMode::Column, slow), auto_print, staff_copy: true, drawer_on_cash: true, ..Default::default() };
    pos.set_receipt_settings(s, &settings).unwrap();
}

fn takeaway(pos: &mut Pos, s: &Session, items: &[&Product]) -> Order {
    let o = pos.open_order(s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: OrderDetails::default() }).unwrap();
    for p in items {
        pos.add_item(s, o.id, p.id, 1, None, None, &[]).unwrap();
    }
    pos.get_order(s, o.id).unwrap()
}

#[test]
fn a_cash_payment_prints_everything_on_an_old_printer_sent_slowly_or_not() {
    for slow in [true, false] {
        let (mut pos, s, soup, curry) = setup();
        let (port, seen) = old_printer();
        receipts(&pos, &s, port, slow, true);
        let o = takeaway(&mut pos, &s, &[&soup, &curry]);
        let paid = pos.pay_order(&s, o.id, PaymentMethod::Cash, 5000).unwrap();
        let job = pos.payment_print_job(&s, &paid).unwrap().unwrap();
        // One job, one connection: the drawer first, then 2 receipts and the staff copy.
        assert!(job.is_one_connection() && job.more.is_empty() && job.urgent, "slow {slow}");
        let want = job.bytes();
        assert!(want[2..7] == KICK);
        job.send_in_background().unwrap();
        let (got, reset) = taken(&seen, 1);
        assert_eq!(got.len(), 1, "slow {slow}");
        assert_eq!(got[0], want);
        assert_eq!((count(&got[0], &KICK), count(&got[0], &CUT), reset), (1, 3, 0));
    }
}

#[test]
fn drawer_and_staff_copy_without_automatic_receipts_share_one_connection() {
    for slow in [true, false] {
        let (mut pos, s, soup, _) = setup();
        let (port, seen) = old_printer();
        receipts(&pos, &s, port, slow, false);
        let o = takeaway(&mut pos, &s, &[&soup]);
        let paid = pos.pay_order(&s, o.id, PaymentMethod::Cash, 5000).unwrap();
        let job = pos.payment_print_job(&s, &paid).unwrap().unwrap();
        assert!(job.is_one_connection() && job.parts.len() == 2 && job.parts[0] == DRAWER);
        let want = job.bytes();
        job.send_in_background().unwrap();
        let (got, reset) = taken(&seen, 1);
        assert_eq!((got, reset), (vec![want], 0), "slow {slow}");
    }
}

#[test]
fn a_kitchen_job_then_a_receipt_on_one_old_printer_both_print() {
    for slow in [true, false] {
        let (mut pos, s, soup, curry) = setup();
        let (port, seen) = old_printer();
        pos.set_kitchen_printer(&s, &PrinterConfig { copies: 3, copy_delay_secs: 1, ..printer(port, ImageMode::Column, slow) }).unwrap();
        receipts(&pos, &s, port, slow, true);
        let o = takeaway(&mut pos, &s, &[&soup, &curry]);
        let sent = pos.send_to_kitchen(&s, o.id).unwrap();
        let kitchen = pos.kitchen_job(&s, o.id, &sent, false).unwrap().unwrap();
        let paid = pos.pay_order(&s, o.id, PaymentMethod::Cash, 5000).unwrap();
        let receipt = pos.payment_print_job(&s, &paid).unwrap().unwrap();
        let after = print_failures(None).last;
        // Back to back, the till waiting for neither.
        let t = Instant::now();
        let (k, r) = (kitchen.bytes(), receipt.bytes());
        kitchen.send_in_background().unwrap();
        receipt.send_in_background().unwrap();
        assert!(t.elapsed() < Duration::from_millis(500), "the till waited {:?}", t.elapsed());
        let (got, reset) = taken(&seen, 2);
        assert_eq!(reset, 0, "slow {slow}");
        assert_eq!(got.len(), 2);
        // The kitchen job is being sent: the receipt (urgent) goes next, never into it.
        assert!(got.contains(&k) && got.contains(&r), "slow {slow}");
        assert_eq!(count(&got.concat(), &CUT), 3 + 3);
        let here = format!("127.0.0.1:{port} ");
        assert!(!print_failures(Some(after)).failures.iter().any(|f| f.message.contains(&here)));
    }
}

#[test]
fn a_printer_busy_with_another_till_is_tried_again_and_a_lasting_refusal_is_shown_on_the_tills() {
    let (mut pos, s, soup, _) = setup();
    let (port, seen) = old_printer();
    pos.set_kitchen_printer(&s, &printer(port, ImageMode::Column, false)).unwrap();
    let o = takeaway(&mut pos, &s, &[&soup]);
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    let job = pos.kitchen_job(&s, o.id, &sent, false).unwrap().unwrap();
    let want = job.bytes();
    // Another till holds the printer for 1.5 s: turned away, tried again, printed.
    let other = TcpStream::connect(("127.0.0.1", port)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    job.clone().send_in_background().unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    drop(other);
    let (got, reset) = taken(&seen, 2);
    assert!(reset >= 1);
    assert_eq!(got.iter().filter(|g| **g == want).count(), 1, "printed once");

    // Held for good: the failure reaches the tills ("print it again").
    let after = print_failures(None).last;
    let _other = TcpStream::connect(("127.0.0.1", port)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    job.send_in_background().unwrap();
    assert!(wait_for_prints(Duration::from_secs(20)));
    let here = format!("127.0.0.1:{port} ");
    let failures: Vec<_> = print_failures(Some(after)).failures.into_iter().filter(|f| f.message.contains(&here)).collect();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].message.contains("refused") && failures[0].message.contains("print it again"), "{}", failures[0].message);
}

#[test]
fn standard_and_small_modes_send_receipts_as_before() {
    for mode in [ImageMode::Raster, ImageMode::RasterSmall] {
        for slow in [true, false] {
            let (mut pos, s, soup, _) = setup();
            let settings = ReceiptSettings { printer: printer(9, mode, slow), auto_print: true, staff_copy: true, drawer_on_cash: true, ..Default::default() };
            pos.set_receipt_settings(&s, &settings).unwrap();
            let o = takeaway(&mut pos, &s, &[&soup]);
            let paid = pos.pay_order(&s, o.id, PaymentMethod::Cash, 5000).unwrap();
            let job = pos.payment_print_job(&s, &paid).unwrap().unwrap();
            assert!(!job.one_connection);
            if slow {
                // The pulse on its own, ahead; the receipts follow as before.
                assert!(job.urgent && job.parts == vec![DRAWER.to_vec()] && job.more.len() == 1);
                let r = &job.more[0];
                assert!(!r.urgent && !r.one_connection && r.parts.len() == 3 && r.parts[0] == r.parts[1]);
            } else {
                let mut first = DRAWER.to_vec();
                first.extend_from_slice(&job.parts[1]);
                assert!(!job.urgent && job.more.is_empty() && job.parts.len() == 3 && job.parts[0] == first);
            }
        }
    }
}
