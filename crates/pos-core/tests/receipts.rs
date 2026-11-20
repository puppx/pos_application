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
    pos.set_setting(&admin, "prices_include_tax", "0").unwrap(); // these tests check tax added on top
    pos.set_setting(&admin, "store_name", "Demo Bistro").unwrap();
    // Created out of order on purpose: grouping follows the category position.
    let drinks = pos.create_category(&admin, "Drinks", None, 3).unwrap();
    let starters = pos.create_category(&admin, "Starters", None, 1).unwrap();
    let mains = pos.create_category(&admin, "Mains", None, 2).unwrap();
    let mut item = |name: &str, price, cat: Option<i64>| {
        pos.create_product(&admin, NewProduct {
            category_id: cat, sku: None, barcode: None, name: name.into(), price_cents: price,
            cost_cents: 0, tax_rate_bp: 1000, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap()
    };
    let soup = item("Soup", 500, Some(starters.id));
    let burger = item("Burger", 1000, Some(mains.id));
    let cola = item("Cola", 250, Some(drinks.id));
    let bread = item("Bread", 200, None);
    let size = pos.create_option_group(&admin, "Size", 0, 1).unwrap();
    let size = pos.save_group_options(&admin, size.id, &[OptionInput { id: None, name: "Large".into(), price_cents: 200, name_alt: None }]).unwrap();
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
fn kitchen_ticket_in_menu_order_without_category_headings() {
    let (mut pos, admin, m) = setup();
    let o = order_everything(&mut pos, &admin, &m);
    let sent = pos.send_to_kitchen(&admin, o.id).unwrap();
    let text = pos.kitchen_ticket(&admin, o.id, &sent, false).unwrap().to_text(42);
    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle} missing in\n{text}"));
    assert!(at("1 x Soup") < at("1 x Burger"));
    assert!(at("1 x Burger\n  + Large\n  ** no onions") < at("2 x Cola"));
    assert!(at("2 x Cola") < at("1 x Bread"));
    assert!(!text.contains("STARTERS") && !text.contains("DRINKS") && !text.contains("OTHER"), "{text}");
    // A blank line between dishes, no dashed line.
    assert!(text.contains("no onions\n\n2 x Cola") && !text.contains("- - -"), "{text}");
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
    // No category headings on bills; the dishes still follow the menu order.
    assert!(!r.contains("-- ") && !r.contains("STARTERS"), "{r}");
    assert!(r.find("1 x Soup").unwrap() < r.find("1 x Burger").unwrap() && r.find("1 x Burger").unwrap() < r.find("2 x Cola").unwrap());

    // £ goes to the printer as code page 437 (0x9C), not UTF-8.
    let bytes = pos.receipt(&admin, o.id).unwrap().to_escpos(42, true);
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
    let printer = PrinterConfig { target: PrinterTarget::File { path: path.display().to_string() }, width: 42, copies: 2, copy_delay_secs: 0, names: None, bold: true, large_text: true, starter_copies: None, mains_copies: None, prep_copies: None, call_table_only: true, additional_copies: None, app_font: false, image_mode: Default::default(), slow_send: false };
    // The staff copy has its own test below.
    let mut settings = ReceiptSettings { printer: printer.clone(), staff_copy: false, ..Default::default() };

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

    assert!(pos.set_kitchen_printer(&admin, &PrinterConfig { target: PrinterTarget::None, width: 42, copies: 0, copy_delay_secs: 0, names: None, bold: true, large_text: true, starter_copies: None, mains_copies: None, prep_copies: None, call_table_only: true, additional_copies: None, app_font: false, image_mode: Default::default(), slow_send: false }).is_err());
    assert!(pos.set_kitchen_printer(&admin, &PrinterConfig { target: PrinterTarget::None, width: 42, copies: 10, copy_delay_secs: 0, names: None, bold: true, large_text: true, starter_copies: None, mains_copies: None, prep_copies: None, call_table_only: true, additional_copies: None, app_font: false, image_mode: Default::default(), slow_send: false }).is_err());

    // Receipt settings JSON shape (as the UI sends it).
    let rs: ReceiptSettings = serde_json::from_str(
        r#"{"kind":"network","host":"192.168.1.60","port":9100,"width":32,"copies":2,"auto_print":false,"footer":"Bye"}"#,
    )
    .unwrap();
    assert_eq!(rs.printer.target, PrinterTarget::Network { host: "192.168.1.60".into(), port: 9100 });
    assert_eq!((rs.printer.width, rs.printer.copies, rs.auto_print, rs.drawer_on_cash), (32, 2, false, true));
    assert_eq!(rs.currency, "£");
}

#[test]
fn staff_copy_breaks_the_tax_down_by_class() {
    let (mut pos, s, m) = setup();
    // Bread at 0%, the rest at 10%, prices on top of tax.
    pos.update_product(&s, m.bread.id, ProductUpdate {
        category_id: None, sku: None, barcode: None, name: "Bread".into(), price_cents: 200, cost_cents: 0, tax_rate_bp: 0,
        track_stock: false, active: true, starter: false, name_alt: None, prep: false, team_id: None,
    })
    .unwrap();
    let o = order_everything(&mut pos, &s, &m);
    let r = pos.receipt(&s, o.id).unwrap();
    let t = r.staff_to_text(42);
    assert!(t.contains("STAFF COPY") && t.contains("TAX") && t.contains("NET") && t.contains("VAT") && t.contains("GROSS"), "{t}");
    assert_eq!(r.tax_classes.len(), 2);
    assert_eq!(r.tax_classes[0].rate_bp, 0);
    assert_eq!(r.tax_classes[0].vat_cents, 0);
    let (net, vat, gross): (i64, i64, i64) = r.tax_classes.iter().fold((0, 0, 0), |a, c| (a.0 + c.net_cents, a.1 + c.vat_cents, a.2 + c.gross_cents));
    assert_eq!((net, vat), (gross - r.tax_cents, r.tax_cents));
    assert_eq!(gross, r.total_cents);

    // A shop name of its own and one staff copy follow the customer's on payment.
    let mut cfg = pos.receipt_settings().unwrap();
    cfg.printer.target = PrinterTarget::File { path: "unused".into() };
    cfg.shop_name = Some("Siam Kitchen".into());
    pos.set_receipt_settings(&s, &cfg).unwrap();
    assert!(pos.receipt(&s, o.id).unwrap().to_text(42).contains("Siam Kitchen"));
    let paid = pos.pay_order(&s, o.id, PaymentMethod::Card, 0).unwrap();
    let job = pos.payment_print_job(&s, &paid).unwrap().unwrap();
    assert_eq!(job.parts.len(), 2, "customer copy + staff copy");
    cfg.staff_copy = false;
    pos.set_receipt_settings(&s, &cfg).unwrap();
    assert_eq!(pos.payment_print_job(&s, &paid).unwrap().unwrap().parts.len(), 1);
}

#[test]
fn order_words_on_tickets_can_be_changed() {
    let (mut pos, s, m) = setup();
    let o = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    assert!(pos.receipt(&s, o.id).unwrap().to_text(42).contains("TAKEAWAY"));
    pos.set_setting(&s, "label_takeaway", "TO GO").unwrap();
    assert!(pos.receipt(&s, o.id).unwrap().to_text(42).contains("TO GO"));
    // Empty falls back to the standard word.
    pos.set_setting(&s, "label_takeaway", "  ").unwrap();
    assert!(pos.receipt(&s, o.id).unwrap().to_text(42).contains("TAKEAWAY"));
}

#[test]
fn a_bill_printed_before_payment_is_not_printed_again_and_shows_no_included_tax() {
    let (mut pos, admin, m) = setup();
    pos.set_setting(&admin, "prices_include_tax", "1").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let printer = PrinterConfig { target: PrinterTarget::File { path: dir.path().join("r.bin").display().to_string() }, copy_delay_secs: 0, app_font: false, ..Default::default() };
    pos.set_receipt_settings(&admin, &ReceiptSettings { printer, staff_copy: true, ..Default::default() }).unwrap();
    // "Bill" before paying: the customer's copy has no tax line.
    let o = order_everything(&mut pos, &admin, &m);
    let bill = String::from_utf8_lossy(&pos.receipt_job(&admin, o.id).unwrap().bytes()).to_string();
    assert!(bill.contains("TOTAL") && !bill.contains("Tax"));
    // Paying: no second customer copy, only the staff copy (and the drawer).
    let paid = pos.pay_order(&admin, o.id, PaymentMethod::Cash, 5000).unwrap();
    let job = pos.payment_print_job(&admin, &paid).unwrap().unwrap();
    let text = String::from_utf8_lossy(&job.bytes()).to_string();
    assert_eq!((count(&job.bytes(), &KICK), count(&job.bytes(), &CUT)), (1, 1));
    assert!(text.contains("STAFF COPY"));
    // No bill first: the receipt prints at payment as before.
    let o = order_everything(&mut pos, &admin, &m);
    let paid = pos.pay_order(&admin, o.id, PaymentMethod::Card, 0).unwrap();
    assert_eq!(count(&pos.payment_print_job(&admin, &paid).unwrap().unwrap().bytes(), &CUT), 2);
}

/// Pay by card; the tickets cut at payment: 1 = only the staff copy (the
/// customer has the bill), 2 = the customer's receipt and the staff copy.
fn cuts_at_payment(pos: &mut Pos, s: &Session, order_id: i64) -> usize {
    let paid = pos.pay_order(s, order_id, PaymentMethod::Card, 0).unwrap();
    count(&pos.payment_print_job(s, &paid).unwrap().unwrap().bytes(), &CUT)
}

#[test]
fn a_bill_changed_after_printing_gets_a_final_receipt_at_payment() {
    let (mut pos, admin, m) = setup();
    let dir = tempfile::tempdir().unwrap();
    let printer = PrinterConfig { target: PrinterTarget::File { path: dir.path().join("r.bin").display().to_string() }, copy_delay_secs: 0, app_font: false, ..Default::default() };
    pos.set_receipt_settings(&admin, &ReceiptSettings { printer, staff_copy: true, ..Default::default() }).unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let table = |name: &str| TableLayout { is_table: Some(true), id: None, name: name.into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 };
    let tables = pos.save_floor_layout(&admin, floor.id, &[table("1"), table("2")]).unwrap();
    let (t1, t2) = (tables[0].id, tables[1].id);
    let dine_in = |t: i64| NewOrder { order_type: OrderType::DineIn, table_id: Some(t), channel_id: None, details: Default::default() };
    // A table with two soups and the bill printed.
    let billed = |pos: &mut Pos| -> Order {
        let o = pos.open_order(&admin, &dine_in(t1)).unwrap();
        let o = pos.add_item(&admin, o.id, m.soup.id, 2, None, None, &[]).unwrap();
        pos.receipt_job(&admin, o.id).unwrap();
        o
    };
    let upd = |qty, discount_cents, note: Option<&str>| ItemUpdate { qty, discount_cents, note: note.map(str::to_string) };

    // Nothing changed: the bill was the receipt, printed once.
    let o = billed(&mut pos);
    assert_eq!(cuts_at_payment(&mut pos, &admin, o.id), 1);
    // Nor when only something off the bill changed: sent to the kitchen, a note.
    let o = billed(&mut pos);
    pos.send_to_kitchen(&admin, o.id).unwrap();
    assert_eq!(cuts_at_payment(&mut pos, &admin, o.id), 1);
    let o = billed(&mut pos).id;
    let item = pos.get_order(&admin, o).unwrap().items[0].id;
    pos.update_item(&admin, item, &upd(2, 0, Some("extra hot"))).unwrap();
    assert_eq!(cuts_at_payment(&mut pos, &admin, o), 1);
    // A repeated tap on Bill after a change still counts.
    let o = billed(&mut pos);
    pos.add_item(&admin, o.id, m.cola.id, 1, None, None, &[]).unwrap();
    pos.receipt_job(&admin, o.id).unwrap();
    assert_eq!(cuts_at_payment(&mut pos, &admin, o.id), 1);

    // Every change to what it costs means a new receipt at payment.
    type Change<'a> = Box<dyn Fn(&mut Pos, i64, i64) + 'a>;
    let changes: Vec<(&str, Change)> = vec![
        ("dish added", Box::new(|pos, o, _| { pos.add_item(&admin, o, m.cola.id, 1, None, None, &[]).unwrap(); })),
        ("more of a dish", Box::new(|pos, o, _| { pos.add_item(&admin, o, m.soup.id, 1, None, None, &[]).unwrap(); })),
        ("open item", Box::new(|pos, o, _| { pos.add_open_item(&admin, o, "Cake", 300, 1, None, false).unwrap(); })),
        ("fewer", Box::new(|pos, _, item| { pos.update_item(&admin, item, &upd(1, 0, None)).unwrap(); })),
        ("discount", Box::new(|pos, _, item| { pos.update_item(&admin, item, &upd(2, 100, None)).unwrap(); })),
        ("service charge", Box::new(|pos, o, _| { pos.set_service_charge(&admin, o, 1000).unwrap(); })),
        ("tax", Box::new(|pos, o, _| { pos.set_order_tax(&admin, o, Some(2000)).unwrap(); })),
        ("dish moved out", Box::new(|pos, o, item| { pos.move_items(&admin, o, t2, &[(item, 1)]).unwrap(); })),
        ("dish moved in", Box::new(|pos, o, _| {
            let other = pos.open_order(&admin, &dine_in(t2)).unwrap();
            let other = pos.add_item(&admin, other.id, m.bread.id, 1, None, None, &[]).unwrap();
            pos.move_items(&admin, other.id, t1, &[(other.items[0].id, 1)]).unwrap();
            assert_eq!(pos.get_order(&admin, o).unwrap().items.len(), 2);
        })),
    ];
    for (what, change) in changes {
        let o = billed(&mut pos);
        change(&mut pos, o.id, o.items[0].id);
        assert_eq!(cuts_at_payment(&mut pos, &admin, o.id), 2, "{what}");
        // Clear table 2 for the next round.
        if let Some(left) = pos.table_order(&admin, t2).unwrap() {
            pos.cancel_order(&admin, left.id, "").unwrap();
        }
    }
    // A whole other table joining this bill.
    let o = billed(&mut pos);
    let other = pos.open_order(&admin, &dine_in(t2)).unwrap();
    pos.add_item(&admin, other.id, m.bread.id, 1, None, None, &[]).unwrap();
    pos.merge_into_table(&admin, other.id, t1).unwrap();
    assert_eq!(cuts_at_payment(&mut pos, &admin, o.id), 2, "tables joined");
}

#[test]
fn the_staff_copy_shows_what_was_paid_then_cash_tendered_and_change() {
    let (mut pos, admin, m) = setup();
    let o = pos.open_order(&admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    pos.add_item(&admin, o.id, m.soup.id, 1, None, None, &[]).unwrap(); // 5.00 + 10% = 5.50
    pos.pay_order(&admin, o.id, PaymentMethod::Cash, 1000).unwrap();
    let r = pos.receipt(&admin, o.id).unwrap();
    let staff = r.staff_to_text(42);
    let line = |text: &str, start: &str| text.lines().find(|l| l.trim_start().starts_with(start)).map(str::to_string).unwrap_or_else(|| panic!("no {start} in\n{text}"));
    assert!(line(&staff, "Paid: Cash").ends_with("£5.50"), "{staff}");
    assert!(line(&staff, "Tendered").ends_with("£10.00"), "{staff}");
    assert!(line(&staff, "Change").ends_with("£4.50"), "{staff}");
    // The customer's copy as before: what was handed over, then the change.
    let customer = r.to_text(42);
    assert!(line(&customer, "Cash").ends_with("£10.00") && line(&customer, "Change").ends_with("£4.50"), "{customer}");

    // Card, and cash given exactly: just what was paid.
    let o = pos.open_order(&admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    pos.add_item(&admin, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    let half = PaymentInput { method: PaymentMethod::Card, amount_cents: Some(300), tendered_cents: 0, tip_cents: 0, items: None };
    pos.add_payment(&admin, o.id, &half).unwrap();
    let rest = PaymentInput { method: PaymentMethod::Cash, amount_cents: None, tendered_cents: 250, tip_cents: 0, items: None };
    pos.add_payment(&admin, o.id, &rest).unwrap();
    let staff = pos.receipt(&admin, o.id).unwrap().staff_to_text(42);
    assert!(line(&staff, "Paid: Card").ends_with("£3.00") && line(&staff, "Paid: Cash").ends_with("£2.50"), "{staff}");
    assert!(!staff.contains("Tendered") && !staff.contains("Change"), "{staff}");
}
