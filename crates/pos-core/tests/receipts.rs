use pos_core::*;

const CUT: [u8; 4] = [0x1D, b'V', 66, 0];
const KICK: [u8; 5] = [0x1B, b'p', 0, 25, 250];

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Menu: Starters (pos 1) > Soup; Mains (pos 2) > Burger with a size option;
/// Drinks (pos 3) > Cola; Bread without a category.
struct Menu {
    soup: Product,
    burger: Product,
    cola: Product,
    bread: Product,
    large: i64,
}

fn setup() -> (Pos, Session, Menu) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.set_setting(&admin, "store_name", "Demo Bistro").unwrap();
    // Created out of order on purpose: grouping follows the category position.
    let drinks = pos.create_category(&admin, "Drinks", None, 3).unwrap();
    let starters = pos.create_category(&admin, "Starters", None, 1).unwrap();
    let mains = pos.create_category(&admin, "Mains", None, 2).unwrap();
    let mut item = |name: &str, price, cat: Option<i64>| {
        pos.create_product(&admin, NewProduct {
            category_id: cat, sku: None, barcode: None, name: name.into(), price_cents: price,
            cost_cents: 0, tax_rate_bp: 1000, stock_qty: 0, track_stock: false, starter: false,
        })
        .unwrap()
    };
    let soup = item("Soup", 500, Some(starters.id));
    let burger = item("Burger", 1000, Some(mains.id));
    let cola = item("Cola", 250, Some(drinks.id));
    let bread = item("Bread", 200, None);
    let size = pos.create_option_group(&admin, "Size", 0, 1).unwrap();
    let size = pos.save_group_options(&admin, size.id, &[OptionInput { id: None, name: "Large".into(), price_cents: 200 }]).unwrap();
    pos.set_product_option_groups(&admin, burger.id, &[size.id]).unwrap();
    let large = size.options[0].id;
    (pos, admin, Menu { soup, burger, cola, bread, large })
}

fn order_everything(pos: &mut Pos, s: &Session, m: &Menu) -> Order {
    let o = pos.open_order(s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    // Added in a jumbled order.
    pos.add_item(s, o.id, m.cola.id, 2, None, None, &[]).unwrap();
    pos.add_item(s, o.id, m.bread.id, 1, None, None, &[]).unwrap();
    pos.add_item(s, o.id, m.burger.id, 1, Some("no onions"), None, &[m.large]).unwrap();
    pos.add_item(s, o.id, m.soup.id, 1, None, None, &[]).unwrap()
}

#[test]
fn kitchen_ticket_grouped_by_category_in_menu_order() {
    let (mut pos, admin, m) = setup();
    let o = order_everything(&mut pos, &admin, &m);
    let sent = pos.send_to_kitchen(&admin, o.id).unwrap();
    let text = pos.kitchen_ticket(&admin, o.id, &sent, false).unwrap().to_text(42);
    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle} missing in\n{text}"));
    assert!(at("-- STARTERS --") < at("1 x Soup"));
    assert!(at("1 x Soup") < at("-- MAINS --"));
    assert!(at("-- MAINS --") < at("1 x Burger"));
    assert!(at("1 x Burger\n   + Large\n   ** no onions") < at("-- DRINKS --"));
    assert!(at("-- DRINKS --") < at("2 x Cola"));
    assert!(at("2 x Cola") < at("-- OTHER --"));
    assert!(at("-- OTHER --") < at("1 x Bread"));
}

#[test]
fn receipt_layout_grouped_with_totals_and_change() {
    let (mut pos, admin, m) = setup();
    let o = order_everything(&mut pos, &admin, &m);
    let settings = ReceiptSettings {
        header: Some("1 High Street, London\nVAT 123 4567 89".into()),
        footer: Some("Thank you for your visit!".into()),
        ..Default::default()
    };
    pos.set_receipt_settings(&admin, &settings).unwrap();

    // Before payment it is a bill.
    let bill = pos.receipt(&admin, o.id).unwrap().to_text(42);
    assert!(bill.contains("BILL #"), "{bill}");
    assert!(!bill.contains("Change"));

    // 5.00 + 12.00 + 2x2.50 + 2.00 = 24.00, +10% tax = 26.40
    let paid = pos.pay_order(&admin, o.id, PaymentMethod::Cash, 3000).unwrap();
    assert_eq!(paid.total_cents, 2640);
    let r = pos.receipt(&admin, o.id).unwrap().to_text(42);
    println!("{r}");
    for needle in [
        "Demo Bistro", "1 High Street, London", "VAT 123 4567 89", "RECEIPT #", "TAKEAWAY",
        "-- STARTERS --", "-- MAINS --", "-- DRINKS --", "-- OTHER --",
        "Thank you for your visit!",
    ] {
        assert!(r.contains(needle), "{needle} missing in\n{r}");
    }
    // Amounts are right-aligned to the paper width.
    let line = |start: &str| r.lines().find(|l| l.trim_start().starts_with(start)).unwrap().to_string();
    assert_eq!(line("1 x Burger"), format!("1 x Burger{}£12.00", " ".repeat(42 - 10 - 6)));
    assert!(line("2 x Cola").ends_with("£5.00"));
    assert!(line("Subtotal").ends_with("£24.00"));
    assert!(line("Tax").ends_with("£2.40"));
    assert!(line("TOTAL").ends_with("£26.40"));
    assert!(line("Cash").ends_with("£30.00"));
    assert!(line("Change").ends_with("£3.60"));
    assert!(r.find("-- STARTERS --").unwrap() < r.find("-- DRINKS --").unwrap());

    // £ goes to the printer as code page 437 (0x9C), not UTF-8.
    let bytes = pos.receipt(&admin, o.id).unwrap().to_escpos(42);
    assert!(bytes.contains(&0x9C));
    assert!(!bytes.windows(2).any(|w| w == "£".as_bytes()));

    // Cancelled orders have no receipt; voided ones print as VOID.
    pos.void_order(&admin, o.id, "test").unwrap();
    assert!(pos.receipt(&admin, o.id).unwrap().to_text(42).contains("VOID #"));
}

#[test]
fn payment_prints_receipt_copies_and_opens_drawer_for_cash() {
    let (mut pos, admin, m) = setup();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipt.bin");
    let printer = PrinterConfig { target: PrinterTarget::File { path: path.display().to_string() }, width: 42, copies: 2, copy_delay_secs: 0 };
    let mut settings = ReceiptSettings { printer: printer.clone(), ..Default::default() };

    // No receipt printer: nothing to print, drawer button explains why.
    let o = order_everything(&mut pos, &admin, &m);
    let paid = pos.pay_order(&admin, o.id, PaymentMethod::Cash, 5000).unwrap();
    assert!(pos.payment_print_job(&admin, &paid).unwrap().is_none());
    assert!(pos.drawer_job(&admin).is_err());

    pos.set_receipt_settings(&admin, &settings).unwrap();
    assert_eq!(pos.receipt_settings().unwrap(), settings);

    // Cash: drawer pulse first, then 2 copies.
    let job = pos.payment_print_job(&admin, &paid).unwrap().unwrap();
    assert!(job.bytes().starts_with(&[0x1B, b'@']) && job.bytes()[2..7] == KICK);
    assert_eq!(count(&job.bytes(), &CUT), 2);
    job.send().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), job.bytes());

    // Card: receipts only.
    let o = order_everything(&mut pos, &admin, &m);
    let paid = pos.pay_order(&admin, o.id, PaymentMethod::Card, 0).unwrap();
    let job = pos.payment_print_job(&admin, &paid).unwrap().unwrap();
    assert_eq!((count(&job.bytes(), &KICK), count(&job.bytes(), &CUT)), (0, 2));

    // Automatic receipts off: cash still opens the drawer, nothing printed.
    settings.auto_print = false;
    pos.set_receipt_settings(&admin, &settings).unwrap();
    let o = order_everything(&mut pos, &admin, &m);
    let paid = pos.pay_order(&admin, o.id, PaymentMethod::Cash, 5000).unwrap();
    let job = pos.payment_print_job(&admin, &paid).unwrap().unwrap();
    assert_eq!((count(&job.bytes(), &KICK), count(&job.bytes(), &CUT)), (1, 0));
    // ...and with the drawer off too, there is nothing to send.
    settings.drawer_on_cash = false;
    pos.set_receipt_settings(&admin, &settings).unwrap();
    assert!(pos.payment_print_job(&admin, &paid).unwrap().is_none());

    // Manual reprint: always one copy, never the drawer.
    let job = pos.receipt_job(&admin, paid.id).unwrap();
    assert_eq!((count(&job.bytes(), &KICK), count(&job.bytes(), &CUT)), (0, 1));

    // Staff can open the drawer; it is a pulse only.
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    let job = pos.drawer_job(&staff).unwrap();
    assert_eq!((count(&job.bytes(), &KICK), count(&job.bytes(), &CUT)), (1, 0));
    // Only admins change receipt settings.
    assert!(matches!(pos.set_receipt_settings(&staff, &settings), Err(PosError::Forbidden)));
    assert!(matches!(pos.test_receipt_job(&staff, &settings, true), Err(PosError::Forbidden)));

    // Settings tests: the drawer test is the pulse only; the receipt test has no pulse.
    let drawer_test = pos.test_receipt_job(&admin, &settings, true).unwrap();
    assert_eq!((count(&drawer_test.bytes(), &KICK), count(&drawer_test.bytes(), &CUT)), (1, 0));
    let receipt_test = pos.test_receipt_job(&admin, &settings, false).unwrap();
    assert_eq!((count(&receipt_test.bytes(), &KICK), count(&receipt_test.bytes(), &CUT)), (0, 2));
}

#[test]
fn kitchen_copies_and_old_settings_still_load() {
    let (mut pos, admin, m) = setup();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("k.bin").display().to_string();
    // A v0.1-v0.3 style setting has no "copies": it defaults to 1.
    let old: PrinterConfig = serde_json::from_str(&format!(r#"{{"kind":"file","path":{file:?},"width":42}}"#)).unwrap();
    assert_eq!(old.copies, 1);
    pos.set_kitchen_printer(&admin, &PrinterConfig { copies: 3, ..old }).unwrap();

    let o = order_everything(&mut pos, &admin, &m);
    let sent = pos.send_to_kitchen(&admin, o.id).unwrap();
    let job = pos.kitchen_job(&admin, o.id, &sent, false).unwrap().unwrap();
    assert_eq!(count(&job.bytes(), &CUT), 3);
    let reprint = pos.kitchen_reprint_job(&admin, o.id).unwrap().unwrap();
    assert_eq!(count(&reprint.bytes(), &CUT), 1);

    assert!(pos.set_kitchen_printer(&admin, &PrinterConfig { target: PrinterTarget::None, width: 42, copies: 0, copy_delay_secs: 0 }).is_err());
    assert!(pos.set_kitchen_printer(&admin, &PrinterConfig { target: PrinterTarget::None, width: 42, copies: 10, copy_delay_secs: 0 }).is_err());

    // Receipt settings JSON shape (as the UI sends it).
    let rs: ReceiptSettings = serde_json::from_str(
        r#"{"kind":"network","host":"192.168.1.60","port":9100,"width":32,"copies":2,"auto_print":false,"footer":"Bye"}"#,
    )
    .unwrap();
    assert_eq!(rs.printer.target, PrinterTarget::Network { host: "192.168.1.60".into(), port: 9100 });
    assert_eq!((rs.printer.width, rs.printer.copies, rs.auto_print, rs.drawer_on_cash), (32, 2, false, true));
    assert_eq!(rs.currency, "£");
}
