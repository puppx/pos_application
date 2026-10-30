use pos_core::*;


#[allow(dead_code)] // shared setup; not every test uses every item
struct Menu {
    soup: Product,
    wings: Product,
    burger: Product,
    cola: Product,
}

fn setup() -> (Pos, Session, Menu, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.set_setting(&admin, "prices_include_tax", "0").unwrap(); // these tests check tax added on top
    let starters = pos.create_category(&admin, "Starters", None, 1).unwrap();
    let mains = pos.create_category(&admin, "Mains", None, 2).unwrap();
    let drinks = pos.create_category(&admin, "Drinks", None, 3).unwrap();
    let mut item = |name: &str, price, cat: i64, starter| {
        pos.create_product(&admin, NewProduct {
            category_id: Some(cat), sku: None, barcode: None, name: name.into(), price_cents: price,
            cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter, name_alt: None, prep: false, team_id: None,
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

fn printer(copies: u8, starter: Option<u8>, mains: Option<u8>) -> PrinterConfig {
    PrinterConfig {
        target: PrinterTarget::File { path: std::env::temp_dir().join("pos-unused.bin").display().to_string() },
        copies, starter_copies: starter, mains_copies: mains, copy_delay_secs: 0, call_table_only: false, ..Default::default()
    }
}

#[test]
fn main_course_can_be_called_again_with_a_short_ticket() {
    let (mut pos, s, m, table) = setup();
    pos.set_kitchen_printer(&s, &printer(1, Some(2), Some(3))).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 2, None, None, &[]).unwrap();
    // Nothing to call before the mains are in the kitchen.
    assert!(pos.main_course_job(&s, o.id).is_err());

    // Starters first: printed with the starter copies.
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    let job = pos.kitchen_job(&s, o.id, &sent, false).unwrap().unwrap();
    assert_eq!(job.parts.len(), 2);
    // Mains away: the mains copies.
    let sent = pos.call_mains(&s, o.id).unwrap();
    let job = pos.kitchen_job(&s, o.id, &sent, false).unwrap().unwrap();
    assert_eq!(job.parts.len(), 3);

    // Called again: table, MAIN COURSE and the dishes only.
    let job = pos.main_course_job(&s, o.id).unwrap().unwrap();
    assert_eq!(job.parts.len(), 3);
    let text = String::from_utf8_lossy(&job.parts[0]).to_string();
    assert!(text.contains("MAIN COURSE") && text.contains("T1") && text.contains("2 x Burger"), "{text}");
    assert!(!text.contains("Soup") && !text.contains("Owner") && !text.contains('#'), "{text}");
    // It changes nothing on the order and can be repeated.
    let before = pos.get_order(&s, o.id).unwrap();
    pos.main_course_job(&s, o.id).unwrap();
    assert_eq!(pos.get_order(&s, o.id).unwrap().items.len(), before.items.len());
}

#[test]
fn main_course_after_all_courses_together() {
    let (mut pos, s, m, table) = setup();
    pos.set_kitchen_printer(&s, &printer(2, None, None)).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.cola.id, 1, None, None, &[]).unwrap();
    let sent = pos.call_mains(&s, o.id).unwrap();
    assert_eq!(sent.len(), 3);
    let job = pos.kitchen_job(&s, o.id, &sent, false).unwrap().unwrap();
    assert_eq!(job.parts.len(), 2, "copies when nothing else is set");
    let job = pos.main_course_job(&s, o.id).unwrap().unwrap();
    assert_eq!(job.parts.len(), 2);
    let text = String::from_utf8_lossy(&job.parts[0]).to_string();
    assert!(text.contains("1 x Burger") && text.contains("1 x Cola") && !text.contains("Soup"), "{text}");
    assert!(pos.set_kitchen_printer(&s, &PrinterConfig { starter_copies: Some(10), ..printer(1, None, None) }).is_err());
}

#[test]
fn every_line_is_bold_and_hash_prints_as_hash() {
    let (mut pos, s, m, table) = setup();
    pos.set_kitchen_printer(&s, &printer(1, None, None)).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    let b = pos.kitchen_job(&s, o.id, &sent, false).unwrap().unwrap().bytes();
    // USA character set, so "#12" is not printed as "£12".
    assert_eq!(&b[..8], &[0x1B, b'@', 0x1B, b't', 0, 0x1B, b'R', 0]);
    let lines_not_bold = b.windows(3).filter(|w| *w == [0x1B, b'E', 0]).count();
    assert_eq!(lines_not_bold, 1, "only the reset at the end");
    // Bold can be switched off: then only headings are bold.
    pos.set_kitchen_printer(&s, &PrinterConfig { bold: false, ..printer(1, None, None) }).unwrap();
    let b = pos.kitchen_reprint_job(&s, o.id).unwrap().unwrap().bytes();
    assert!(b.windows(3).filter(|w| *w == [0x1B, b'E', 0]).count() > 1);
    // Older saved settings print bold.
    let old: PrinterConfig = serde_json::from_str(r#"{"kind":"none"}"#).unwrap();
    assert!(old.bold && old.starter_copies.is_none());
}

#[test]
fn tax_can_be_changed_on_one_order() {
    let (mut pos, s, m, table) = setup();
    let p = &m.burger;
    pos.update_product(&s, p.id, ProductUpdate {
        category_id: p.category_id, sku: None, barcode: None, name: p.name.clone(), price_cents: 1000, cost_cents: 0,
        tax_rate_bp: 2000, track_stock: false, active: true, starter: false, name_alt: None, prep: false, team_id: None,
    })
    .unwrap();
    pos.create_user(&s, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    let o = pos.open_order(&staff, &dine_in(table)).unwrap();
    let o = pos.add_item(&staff, o.id, m.burger.id, 1, None, None, &[]).unwrap();
    pos.add_item(&staff, o.id, m.cola.id, 1, None, None, &[]).unwrap();
    assert_eq!((o.tax_bp, o.tax_cents), (None, 200));

    // Staff set 0% (tax-free customer): every line, and lines added after.
    let o = pos.set_order_tax(&staff, o.id, Some(0)).unwrap();
    assert_eq!((o.tax_bp, o.tax_cents, o.total_cents), (Some(0), 0, 1250));
    let o = pos.add_item(&staff, o.id, m.burger.id, 1, Some("no salt"), None, &[]).unwrap();
    assert_eq!(o.tax_cents, 0);
    // A different rate.
    let o = pos.set_order_tax(&staff, o.id, Some(500)).unwrap();
    assert!(o.items.iter().all(|i| i.tax_rate_bp == 500));
    assert!(pos.set_order_tax(&staff, o.id, Some(10_001)).is_err());
    // Back to the menu rates.
    let o = pos.set_order_tax(&staff, o.id, None).unwrap();
    assert_eq!(o.tax_bp, None);
    assert_eq!(o.items.iter().map(|i| i.tax_rate_bp).collect::<Vec<_>>(), [2000, 0, 2000]);
    assert_eq!(o.tax_cents, 400);

    // Paid orders keep their tax.
    let o = pos.set_order_tax(&staff, o.id, Some(0)).unwrap();
    let paid = pos.pay_order(&staff, o.id, PaymentMethod::Card, 0).unwrap();
    assert_eq!((paid.tax_cents, paid.total_cents), (0, 2250));
    assert!(pos.set_order_tax(&staff, paid.id, None).is_err());
    let r = pos.receipt(&staff, paid.id).unwrap().to_text(42);
    assert!(r.contains("Tax") && r.contains("£0.00"), "{r}");
}

#[test]
fn prepare_ahead_dishes_get_a_ticket_while_mains_wait() {
    let (mut pos, s, m, table) = setup();
    pos.set_kitchen_printer(&s, &PrinterConfig { prep_copies: Some(2), ..printer(1, None, None) }).unwrap();
    pos.set_prep(&s, m.burger.id, true).unwrap();
    assert!(pos.get_product(&s, m.burger.id).unwrap().prep);
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 2, Some("medium"), None, &[]).unwrap();
    pos.add_item(&s, o.id, m.cola.id, 1, None, None, &[]).unwrap();

    // Starters go; burger and cola wait. The burger gets a PREPARE NOW ticket.
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    assert_eq!(sent.len(), 1);
    let prep = pos.send_prep(&s, o.id).unwrap();
    assert_eq!(prep.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["Burger"]);
    let job = pos.prep_job(&s, o.id, &prep).unwrap().unwrap();
    assert_eq!(job.parts.len(), 2, "prep copies");
    let text = pos.prep_ticket(&s, o.id, &prep).unwrap().to_text(42);
    assert!(text.contains("PREPARE NOW") && text.contains("2 x Burger") && text.contains("medium"), "{text}");
    assert!(text.contains("SERVE WITH MAINS") && !text.contains("Cola") && !text.contains("Soup"), "{text}");
    // Only once: sending again prints no second PREPARE NOW.
    pos.send_to_kitchen(&s, o.id).unwrap();
    assert!(pos.send_prep(&s, o.id).unwrap().is_empty());
    // The burger still waits and goes out with the mains.
    let o2 = pos.get_order(&s, o.id).unwrap();
    let burger = o2.items.iter().find(|i| i.name == "Burger").unwrap();
    assert!(burger.held && burger.sent_at.is_none() && burger.prep_sent_at.is_some());
    let mains = pos.call_mains(&s, o.id).unwrap();
    assert_eq!(mains.len(), 2);

    // Takeaway (no courses): everything goes at once, no PREPARE NOW.
    let t = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: OrderDetails::default() }).unwrap();
    pos.add_item(&s, t.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, t.id, m.burger.id, 1, None, None, &[]).unwrap();
    assert_eq!(pos.send_to_kitchen(&s, t.id).unwrap().len(), 2);
    assert!(pos.send_prep(&s, t.id).unwrap().is_empty());
}

#[test]
fn prepare_ahead_follows_category_and_backups() {
    let (mut pos, s, _, _) = setup();
    let cakes = pos.create_category(&s, "Cakes", None, 9).unwrap();
    let cakes = pos.set_category_defaults(&s, cakes.id, None, false, true, None).unwrap();
    assert!(cakes.prep);
    pos.import_items(&s, &[ImportRow { name: "Lava cake".into(), price_cents: 700, category: Some("cakes".into()), tax_rate_bp: None, name_alt: None }]).unwrap();
    let all = pos.list_products(&s, false, 100, 0).unwrap();
    let lava = all.iter().find(|p| p.name == "Lava cake").unwrap();
    assert!(lava.prep);
    assert!(pos.duplicate_product(&s, lava.id).unwrap().prep);
    let backup = pos.export_menu(&s).unwrap();
    assert!(backup.items.iter().any(|i| i.name == "Lava cake" && i.prep));
    pos.import_menu(&s, &backup).unwrap();
    let again = pos.export_menu(&s).unwrap();
    assert!(again.items.iter().any(|i| i.name == "Lava cake" && i.prep));
    assert!(again.categories.iter().any(|c| c.name == "Cakes" && c.prep));
    // Older backups (no "prep") still load.
    let old: MenuItem = serde_json::from_str(r#"{"name":"Tea","price_cents":200}"#).unwrap();
    assert!(!old.prep);
}

#[test]
fn calling_mains_again_can_print_only_the_table() {
    let (mut pos, s, m, table) = setup();
    // The default: just the table on a big bar, no dishes.
    pos.set_kitchen_printer(&s, &PrinterConfig { call_table_only: true, ..printer(1, None, None) }).unwrap();
    assert!(pos.kitchen_printer().unwrap().call_table_only);
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, m.soup.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, m.burger.id, 2, None, None, &[]).unwrap();
    pos.call_mains(&s, o.id).unwrap();
    let job = pos.main_course_job(&s, o.id).unwrap().unwrap();
    let b = job.bytes();
    assert_eq!(job.parts.len(), 1);
    assert_eq!(b.windows(4).filter(|w| *w == [0x1D, b'v', b'0', 0]).count(), 1, "the table as one big bar");
    assert!(String::from_utf8_lossy(&b).contains("MAIN COURSE") && !String::from_utf8_lossy(&b).contains("Burger"));
    // Switched off: the dishes come with it.
    pos.set_kitchen_printer(&s, &printer(1, None, None)).unwrap();
    let b = pos.main_course_job(&s, o.id).unwrap().unwrap().bytes();
    assert!(String::from_utf8_lossy(&b).contains("2 x Burger"));
}
