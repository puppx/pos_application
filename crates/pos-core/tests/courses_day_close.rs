use std::time::{Duration, Instant};

use pos_core::*;

const CUT: [u8; 4] = [0x1D, b'V', 66, 0];

struct Menu {
    soup: Product,
    wings: Product,
    burger: Product,
    cola: Product,
}

fn setup() -> (Pos, Session, Menu, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let starters = pos.create_category(&admin, "Starters", None, 1).unwrap();
    let mains = pos.create_category(&admin, "Mains", None, 2).unwrap();
    let drinks = pos.create_category(&admin, "Drinks", None, 3).unwrap();
    let mut item = |name: &str, price, cat: i64, starter| {
        pos.create_product(&admin, NewProduct {
            category_id: Some(cat), sku: None, barcode: None, name: name.into(), price_cents: price,
            cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter, name_alt: None,
        })
        .unwrap()
    };
    let soup = item("Soup", 500, starters.id, true);
    let wings = item("Wings", 700, starters.id, false);
    let burger = item("Burger", 1000, mains.id, false);
    let cola = item("Cola", 250, drinks.id, false);
    // "Wings" was added before it was marked as a starter.
    pos.set_starter(&admin, wings.id, true).unwrap();
    let wings = pos.get_product(&admin, wings.id).unwrap();
    assert!(wings.starter);
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&admin, floor.id, &[TableLayout { id: None, name: "T1".into(), shape: TableShape::Rect, x: 10, y: 10, w: 80, h: 80 }])
        .unwrap();
    (pos, admin, Menu { soup, wings, burger, cola }, t[0].id)
}

fn dine_in(table_id: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table_id), channel_id: None, details: OrderDetails::default() }
}

fn names(items: &[OrderItem]) -> Vec<&str> {
    items.iter().map(|i| i.name.as_str()).collect()
}

#[test]
fn starters_go_first_and_mains_wait_for_call() {
    let (mut pos, s, m, table) = setup();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 2, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.cola.id, 1, None, None, &[]).unwrap();

    // Round 1: only the starter; burger and cola wait.
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    assert_eq!(names(&sent), ["Soup"]);
    let ticket = pos.kitchen_ticket(&s, o.id, &sent, false).unwrap();
    assert_eq!(ticket.title, "NEW ORDER");
    assert_eq!(ticket.footer.as_deref(), Some("MAINS TO FOLLOW: 3"));
    let board = pos.list_open_orders(&s).unwrap();
    assert_eq!((board[0].unsent_count, board[0].held_count), (0, 2));
    let tables = pos.floor_status(&s, pos.list_floors(&s).unwrap()[0].id).unwrap();
    assert_eq!((tables[0].unsent_count, tables[0].held_count), (0, 2));

    // A new main added meanwhile also waits; a new starter goes straight out.
    pos.add_item(&s, o.id, m.burger.id, 1, Some("well done"), None, &[]).unwrap();
    pos.add_item(&s, o.id, m.wings.id, 1, None, None, &[]).unwrap();
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    assert_eq!(names(&sent), ["Wings"]);
    assert_eq!(pos.kitchen_ticket(&s, o.id, &sent, false).unwrap().title, "ADDITIONAL");
    // Nothing new: Send does nothing while mains wait.
    assert!(pos.send_to_kitchen(&s, o.id).unwrap().is_empty());

    // Call mains: everything waiting goes, titled MAINS AWAY.
    let sent = pos.call_mains(&s, o.id).unwrap();
    assert_eq!(names(&sent), ["Burger", "Cola", "Burger"]);
    let ticket = pos.kitchen_ticket(&s, o.id, &sent, false).unwrap();
    assert_eq!(ticket.title, "MAINS AWAY");
    assert_eq!(ticket.footer, None);
    assert!(ticket.to_text(42).contains("-- MAINS --"));
    assert_eq!(pos.list_open_orders(&s).unwrap()[0].held_count, 0);

    // Later rounds (dessert, drinks) are ordinary additions sent at once.
    pos.add_item(&s, o.id, m.cola.id, 1, None, None, &[]).unwrap();
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    assert_eq!(names(&sent), ["Cola"]);
    assert_eq!(pos.kitchen_ticket(&s, o.id, &sent, false).unwrap().title, "ADDITIONAL");
}

#[test]
fn courses_only_apply_to_dine_in_with_starters() {
    let (mut pos, s, m, table) = setup();
    // No starters: everything at once.
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.cola.id, 1, None, None, &[]).unwrap();
    assert_eq!(pos.send_to_kitchen(&s, o.id).unwrap().len(), 2);
    pos.cancel_order(&s, o.id, "test").unwrap();

    // "All courses together": starters and mains in one ticket.
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    let sent = pos.call_mains(&s, o.id).unwrap();
    assert_eq!(sent.len(), 2);
    let ticket = pos.kitchen_ticket(&s, o.id, &sent, false).unwrap();
    assert_eq!((ticket.title.as_str(), ticket.footer.clone()), ("NEW ORDER", None));
    pos.cancel_order(&s, o.id, "test").unwrap();

    // Takeaway never holds anything back.
    let o = pos
        .open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() })
        .unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    assert_eq!(pos.send_to_kitchen(&s, o.id).unwrap().len(), 2);
}

#[test]
fn deleted_option_group_disappears_everywhere() {
    let (mut pos, s, m, table) = setup();
    let g = pos.create_option_group(&s, "Extras", 0, 3).unwrap();
    let g = pos.save_group_options(&s, g.id, &[OptionInput { id: None, name: "Cheese".into(), price_cents: 100, name_alt: None }]).unwrap();
    pos.set_product_option_groups(&s, m.burger.id, &[g.id]).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    let o = pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[g.options[0].id]).unwrap();

    pos.create_user(&s, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    assert!(matches!(pos.delete_option_group(&staff, g.id), Err(PosError::Forbidden)));

    pos.delete_option_group(&s, g.id).unwrap();
    assert!(pos.list_option_groups(&s, true).unwrap().is_empty());
    assert!(pos.menu_options(&s).unwrap().links.is_empty());
    // The name can be reused, and the old group cannot be edited or deleted again.
    pos.create_option_group(&s, "Extras", 0, 1).unwrap();
    assert!(matches!(pos.delete_option_group(&s, g.id), Err(PosError::NotFound)));
    assert!(matches!(pos.update_option_group(&s, g.id, "X", 0, 1, 0, true), Err(PosError::NotFound)));
    // The open order keeps its snapshot and can still be paid.
    let o = pos.get_order(&s, o.id).unwrap();
    assert_eq!(o.items[0].options[0].name, "Cheese");
    assert_eq!(pos.pay_order(&s, o.id, PaymentMethod::Card, 0).unwrap().total_cents, 1100);
}

#[test]
fn end_of_day_closes_summarises_and_moves_on() {
    let (mut pos, s, m, table) = setup();
    let day = pos.business_day(&s).unwrap();
    let today = chrono::Local::now().date_naive();
    assert_eq!(day.date, today.format("%Y-%m-%d").to_string());

    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 2, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    // An open order blocks the close.
    let err = pos.close_day(&s).unwrap_err();
    assert!(err.to_string().contains("1 order is still open"), "{err}");
    pos.pay_order(&s, o.id, PaymentMethod::Cash, 5000).unwrap();
    let o = pos
        .open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() })
        .unwrap();
    pos.add_item(&s, o.id, m.cola.id, 2, None, None, &[]).unwrap();
    pos.pay_order(&s, o.id, PaymentMethod::Card, 0).unwrap();

    // Staff can see the reports and close the day.
    pos.create_user(&s, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    assert_eq!(pos.business_day(&staff).unwrap().open_orders, 0);
    assert_eq!(pos.sales_summary(&staff, day.since, day.since + 86_400 * 2).unwrap().order_count, 2);

    let close = pos.close_day(&staff).unwrap();
    let sum = &close.summary;
    assert_eq!((close.business_date.as_str(), sum.order_count, sum.total_cents), (day.date.as_str(), 2, 2500));
    let cats: Vec<(&str, i64, i64)> = sum.by_category.iter().map(|g| (g.key.as_str(), g.count, g.total_cents)).collect();
    assert_eq!(cats, [("Starters", 2, 1000), ("Mains", 1, 1000), ("Drinks", 2, 500)]);
    let text = pos.day_close_text(&close).unwrap();
    for needle in ["END OF DAY", "-- PAYMENTS --", "Cash (1)", "Card (1)", "-- CATEGORIES (before tax) --", "Starters (2)", "TOTAL"] {
        assert!(text.contains(needle), "missing {needle:?} in\n{text}");
    }

    // The next business day starts where this one ended and is empty.
    let next = pos.business_day(&s).unwrap();
    let expected = (today + chrono::Days::new(1)).format("%Y-%m-%d").to_string();
    assert_eq!((next.date.as_str(), next.since), (expected.as_str(), close.to));
    assert_eq!(pos.sales_summary(&s, next.since, next.since + 86_400).unwrap().order_count, 0);
    // Closing the (empty) next day moves on again; history lists both.
    let second = pos.close_day(&s).unwrap();
    assert_eq!(second.summary.order_count, 0);
    let history = pos.day_closes(&s, 10).unwrap();
    assert_eq!(history.iter().map(|d| d.id).collect::<Vec<_>>(), [second.id, close.id]);
    assert_eq!(pos.day_close(&s, close.id).unwrap().summary.total_cents, 2500);

    // Printing goes to the receipt printer when one is set up.
    assert!(pos.day_close_job(&s, &close).unwrap().is_none());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("z.bin");
    let printer = PrinterConfig { target: PrinterTarget::File { path: path.display().to_string() }, width: 42, copies: 3, copy_delay_secs: 0, names: None, bold: true, large_text: true, starter_copies: None, mains_copies: None };
    pos.set_receipt_settings(&s, &ReceiptSettings { printer, ..Default::default() }).unwrap();
    let job = pos.day_close_job(&s, &close).unwrap().unwrap();
    assert_eq!(job.parts.len(), 1, "the report prints once whatever the receipt copies");
}

#[test]
fn copies_are_separate_parts_sent_with_a_delay() {
    let (mut pos, s, m, table) = setup();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kitchen.bin");
    let cfg = PrinterConfig { target: PrinterTarget::File { path: path.display().to_string() }, width: 42, copies: 3, copy_delay_secs: 1, names: None, bold: true, large_text: true, starter_copies: None, mains_copies: None };
    pos.set_kitchen_printer(&s, &cfg).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    let job = pos.kitchen_job(&s, o.id, &sent, false).unwrap().unwrap();
    assert_eq!((job.parts.len(), job.gap), (3, Duration::from_secs(1)));
    assert!(job.parts.iter().all(|p| p.ends_with(&CUT)));

    // The first copy prints straight away; the others follow in the background.
    let started = Instant::now();
    job.clone().send_in_background().unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(std::fs::read(&path).unwrap(), job.parts[0]);
    std::thread::sleep(Duration::from_millis(2600));
    assert_eq!(std::fs::read(&path).unwrap(), job.bytes());

    // Older saved settings without a delay get the 2 second default.
    let old: PrinterConfig = serde_json::from_str(r#"{"kind":"none","copies":2}"#).unwrap();
    assert_eq!(old.copy_delay_secs, 2);
}

#[test]
fn server_settings_round_trip() {
    let (pos, s, _, _) = setup();
    assert_eq!(pos.server_settings().unwrap(), ServerSettings { auto_start: false, port: 8080 });
    let cfg = ServerSettings { auto_start: true, port: 9000 };
    pos.set_server_settings(&s, &cfg).unwrap();
    assert_eq!(pos.server_settings().unwrap(), cfg);
    assert!(pos.set_server_settings(&s, &ServerSettings { auto_start: true, port: 80 }).is_err());
    // Reserved: not readable or writable as a plain setting.
    assert_eq!(pos.setting("_lan_server").unwrap(), None);
}
