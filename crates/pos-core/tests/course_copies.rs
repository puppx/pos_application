//! Copies of each kind of kitchen ticket, with the "All courses together"
//! and "Call mains again" copy settings, and how a printer in the
//! compatible image mode is sent them.
use pos_core::*;

struct Menu {
    soup: Product,
    burger: Product,
    curry: Product,
    cola: Product,
}

fn setup() -> (Pos, Session, Menu, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let cat = pos.create_category(&admin, "Food", None, 1).unwrap();
    let drinks = pos.create_category(&admin, "Drinks", None, 2).unwrap();
    let mut item = |name: &str, cat: i64, starter, prep| {
        pos.create_product(&admin, NewProduct {
            category_id: Some(cat), sku: None, barcode: None, name: name.into(), price_cents: 500,
            cost_cents: 0, tax_rate_bp: Some(0), stock_qty: 0, track_stock: false, starter, name_alt: None, prep, team_id: None,
        })
        .unwrap()
    };
    let soup = item("Soup", cat.id, true, false);
    let burger = item("Burger", cat.id, false, false);
    let curry = item("Curry", cat.id, false, true);
    let cola = item("Cola", drinks.id, false, false);
    pos.set_drink(&admin, cola.id, true).unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&admin, floor.id, &[
            TableLayout { is_table: Some(true), id: None, name: "T1".into(), shape: TableShape::Rect, x: 10, y: 10, w: 80, h: 80 },
            TableLayout { is_table: Some(true), id: None, name: "T2".into(), shape: TableShape::Rect, x: 100, y: 10, w: 80, h: 80 },
        ])
        .unwrap();
    (pos, admin, Menu { soup, burger, curry, cola }, t[0].id)
}

fn dine_in(table_id: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table_id), channel_id: None, details: OrderDetails::default() }
}

fn file() -> PrinterTarget {
    PrinterTarget::File { path: std::env::temp_dir().join("pos-unused.bin").display().to_string() }
}

fn printer(copies: u8) -> PrinterConfig {
    PrinterConfig { target: file(), copies, copy_delay_secs: 0, call_table_only: false, app_font: false, ..Default::default() }
}

/// Copies of the job, on the main printer.
fn copies(job: Option<PrintJob>) -> usize {
    job.unwrap().parts.len()
}

#[test]
fn all_courses_together_and_call_mains_again_have_their_own_copies() {
    let (mut pos, s, m, table) = setup();
    // "Same": as before, the printer's copies for both.
    pos.set_kitchen_printer(&s, &printer(3)).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    let sent = pos.call_mains(&s, o.id).unwrap();
    assert_eq!(copies(pos.kitchen_job(&s, o.id, &sent, false).unwrap()), 3);
    assert_eq!(copies(pos.main_course_job(&s, o.id).unwrap()), 3);
    // Mains copies still count for the call again when it is "Same".
    pos.set_kitchen_printer(&s, &PrinterConfig { mains_copies: Some(2), ..printer(3) }).unwrap();
    assert_eq!(copies(pos.main_course_job(&s, o.id).unwrap()), 2);

    // Set: all courses together 3, call mains again 1 (the owner's wish).
    let cfg = PrinterConfig { all_courses_copies: Some(3), call_again_copies: Some(1), mains_copies: Some(2), ..printer(1) };
    pos.set_kitchen_printer(&s, &cfg).unwrap();
    assert_eq!(pos.kitchen_printer().unwrap(), cfg, "saved and read back");
    assert_eq!(copies(pos.main_course_job(&s, o.id).unwrap()), 1);
    // A new order on another table, all courses together: 3.
    let t2 = pos.list_tables(&s, pos.list_floors(&s).unwrap()[0].id).unwrap().into_iter().find(|t| t.id != table).unwrap();
    let o2 = pos.open_order(&s, &dine_in(t2.id)).unwrap();
    pos.add_item(&s, o2.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o2.id, m.burger.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o2.id, m.cola.id, 1, None, None, &[]).unwrap();
    let sent = pos.call_mains(&s, o2.id).unwrap();
    assert_eq!(copies(pos.kitchen_job(&s, o2.id, &sent, false).unwrap()), 3);
    // More of both later, all courses together again (ADDITIONAL): its setting too.
    pos.add_item(&s, o2.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o2.id, m.burger.id, 1, None, None, &[]).unwrap();
    let sent = pos.call_mains(&s, o2.id).unwrap();
    let job = pos.kitchen_job(&s, o2.id, &sent, false).unwrap().unwrap();
    assert!(String::from_utf8_lossy(&job.parts[0]).contains("ADDITIONAL"));
    assert_eq!(job.parts.len(), 3);
    // Only mains added later: the added-later copies (1), not all courses.
    pos.add_item(&s, o2.id, m.burger.id, 1, None, None, &[]).unwrap();
    let sent = pos.send_to_kitchen(&s, o2.id).unwrap();
    assert_eq!(copies(pos.kitchen_job(&s, o2.id, &sent, false).unwrap()), 1);
    // A reprint: one.
    assert_eq!(copies(pos.kitchen_reprint_job(&s, o2.id).unwrap()), 1);
    // Bad counts are refused like the other copy settings.
    assert!(pos.set_kitchen_printer(&s, &PrinterConfig { all_courses_copies: Some(10), ..printer(1) }).is_err());
    assert!(pos.set_kitchen_printer(&s, &PrinterConfig { call_again_copies: Some(0), ..printer(1) }).is_err());
}

#[test]
fn course_by_course_tickets_keep_their_copies() {
    let (mut pos, s, m, table) = setup();
    let cfg = PrinterConfig {
        starter_copies: Some(2), mains_copies: Some(4), prep_copies: Some(5), additional_copies: Some(6),
        all_courses_copies: Some(7), call_again_copies: Some(8), ..printer(3)
    };
    pos.set_kitchen_printer(&s, &cfg).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.curry.id, 1, None, None, &[]).unwrap();
    // Starters first (the curry waits, prepared ahead): starter and prepare copies.
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    assert_eq!(copies(pos.kitchen_job(&s, o.id, &sent, false).unwrap()), 2);
    let prep = pos.send_prep(&s, o.id).unwrap();
    assert_eq!(copies(pos.prep_job(&s, o.id, &prep).unwrap()), 5);
    // Call mains: mains copies; then called again: its own.
    let sent = pos.call_mains(&s, o.id).unwrap();
    assert_eq!(copies(pos.kitchen_job(&s, o.id, &sent, false).unwrap()), 4);
    assert_eq!(copies(pos.main_course_job(&s, o.id).unwrap()), 8);
    // More later: added-later copies.
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    assert_eq!(copies(pos.kitchen_job(&s, o.id, &sent, false).unwrap()), 6);
    // A takeaway with a starter and a main is no "all courses together".
    let t = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: OrderDetails::default() }).unwrap();
    pos.add_item(&s, t.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, t.id, m.burger.id, 1, None, None, &[]).unwrap();
    let sent = pos.send_to_kitchen(&s, t.id).unwrap();
    assert_eq!(copies(pos.kitchen_job(&s, t.id, &sent, false).unwrap()), 3);
}

#[test]
fn extra_printers_print_their_own_copies_of_every_ticket() {
    let (mut pos, s, m, table) = setup();
    pos.set_kitchen_printer(&s, &PrinterConfig { all_courses_copies: Some(1), call_again_copies: Some(1), ..printer(3) }).unwrap();
    let bar = KitchenRoute { label: "Bar".into(), target: file(), width: 42, copies: 2, team_ids: vec![], drinks: true, desserts: false, image_mode: ImageMode::Column, slow_send: true };
    pos.set_kitchen_routes(&s, &[bar]).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.cola.id, 1, None, None, &[]).unwrap();
    let sent = pos.call_mains(&s, o.id).unwrap();
    let job = pos.kitchen_job(&s, o.id, &sent, false).unwrap().unwrap();
    // Main printer: its all-courses setting; the bar: its own copies, sent
    // over one connection (compatible mode); the main one as before.
    assert_eq!(job.parts.len(), 1);
    assert!(!job.one_connection);
    assert_eq!(job.more.len(), 1);
    assert_eq!(job.more[0].parts.len(), 2);
    assert!(job.more[0].one_connection && job.more[0].is_one_connection());
}

#[test]
fn compatible_mode_kitchen_and_receipt_jobs_go_over_one_connection() {
    let (mut pos, s, m, table) = setup();
    pos.set_kitchen_printer(&s, &PrinterConfig { image_mode: ImageMode::Column, ..printer(3) }).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    let sent = pos.call_mains(&s, o.id).unwrap();
    let job = pos.kitchen_job(&s, o.id, &sent, false).unwrap().unwrap();
    assert!(job.is_one_connection() && job.parts.len() == 3);
    let call = pos.main_course_job(&s, o.id).unwrap().unwrap();
    assert!(call.is_one_connection() && call.parts.len() == 3);
    // Each copy whole: from the init to the feed and cut, nothing after it.
    for part in job.parts.iter().chain(&call.parts) {
        assert!(part.starts_with(&[0x1B, b'@']));
        assert!(part.ends_with(&[0x1B, b'd', 4, 0x1D, b'V', 66, 0]));
    }
    // The standard mode: as before, each copy on its own.
    pos.set_kitchen_printer(&s, &printer(3)).unwrap();
    let call = pos.main_course_job(&s, o.id).unwrap().unwrap();
    assert!(!call.one_connection && call.parts.len() == 3);
}
