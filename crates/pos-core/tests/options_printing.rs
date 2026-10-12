use pos_core::*;

fn setup() -> (Pos, Session) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    (pos, admin)
}

fn product(pos: &mut Pos, admin: &Session, name: &str, price: i64) -> Product {
    pos.create_product(admin, NewProduct {
        category_id: None, sku: None, barcode: None, name: name.into(), price_cents: price,
        cost_cents: 0, tax_rate_bp: 1000, stock_qty: 0, track_stock: false,
    })
    .unwrap()
}

fn opt(name: &str, price: i64) -> OptionInput {
    OptionInput { id: None, name: name.into(), price_cents: price }
}

/// Burger (10.00) with "Burger size" (exactly one: Regular +0, Large +2.50)
/// and "Extras" (up to 2: Cheese +1.00, Bacon +1.50, Egg +0.80).
fn burger_with_options(pos: &mut Pos, admin: &Session) -> (Product, OptionGroup, OptionGroup) {
    let burger = product(pos, admin, "Burger", 1000);
    let size = pos.create_option_group(admin, "Burger size", 1, 1).unwrap();
    let size = pos.save_group_options(admin, size.id, &[opt("Regular", 0), opt("Large", 250)]).unwrap();
    let extras = pos.create_option_group(admin, "Extras", 0, 2).unwrap();
    let extras = pos.save_group_options(admin, extras.id, &[opt("Cheese", 100), opt("Bacon", 150), opt("Egg", 80)]).unwrap();
    pos.set_product_option_groups(admin, burger.id, &[size.id, extras.id]).unwrap();
    (burger, size, extras)
}

fn takeaway(pos: &Pos, s: &Session) -> Order {
    pos.open_order(s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, details: Default::default() }).unwrap()
}

fn id(g: &OptionGroup, name: &str) -> i64 {
    g.options.iter().find(|o| o.name == name).unwrap().id
}

#[test]
fn options_price_and_rules() {
    let (mut pos, admin) = setup();
    let (burger, size, extras) = burger_with_options(&mut pos, &admin);
    let o = takeaway(&pos, &admin);

    // Size is required, extras limited to two, options must belong to the item.
    assert!(pos.add_item(&admin, o.id, burger.id, 1, None, &[]).is_err());
    assert!(pos.add_item(&admin, o.id, burger.id, 1, None, &[id(&size, "Regular"), id(&size, "Large")]).is_err());
    assert!(pos
        .add_item(&admin, o.id, burger.id, 1, None, &[id(&size, "Large"), id(&extras, "Cheese"), id(&extras, "Bacon"), id(&extras, "Egg")])
        .is_err());
    let fries = product(&mut pos, &admin, "Fries", 300);
    assert!(pos.add_item(&admin, o.id, fries.id, 1, None, &[id(&extras, "Cheese")]).is_err());
    assert!(pos.add_item(&admin, o.id, burger.id, 1, None, &[id(&size, "Large"), id(&size, "Large")]).is_err());

    // Large + Bacon + Cheese = 10.00 + 2.50 + 1.50 + 1.00 = 15.00, listed in menu order.
    let large_bacon_cheese = [id(&extras, "Bacon"), id(&size, "Large"), id(&extras, "Cheese")];
    let order = pos.add_item(&admin, o.id, burger.id, 1, None, &large_bacon_cheese).unwrap();
    let line = &order.items[0];
    assert_eq!(line.unit_price_cents, 1500);
    assert_eq!(line.options.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["Large", "Cheese", "Bacon"]);
    assert_eq!(line.options[0].group_name, "Burger size");
    assert_eq!(order.total_cents, 1650); // + 10% tax

    // Same choices (any order) merge; different choices make a new line.
    let order = pos.add_item(&admin, o.id, burger.id, 1, None, &[id(&extras, "Cheese"), id(&extras, "Bacon"), id(&size, "Large")]).unwrap();
    assert_eq!((order.items.len(), order.items[0].qty), (1, 2));
    let order = pos.add_item(&admin, o.id, burger.id, 1, None, &[id(&size, "Regular")]).unwrap();
    assert_eq!(order.items.len(), 2);
    assert_eq!(order.items[1].unit_price_cents, 1000);

    // Editing the menu later does not change the order line.
    pos.save_group_options(&admin, size.id, &[OptionInput { id: Some(id(&size, "Large")), name: "Big".into(), price_cents: 500 }]).unwrap();
    let order = pos.get_order(&admin, o.id).unwrap();
    assert_eq!((order.items[0].unit_price_cents, order.items[0].options[0].name.as_str()), (1500, "Large"));
    // ...but "Regular" was retired, so it can no longer be chosen.
    assert!(pos.add_item(&admin, o.id, burger.id, 1, None, &[id(&size, "Regular")]).is_err());

    let paid = pos.pay_order(&admin, o.id, PaymentMethod::Card, 0).unwrap();
    assert_eq!(paid.total_cents, 2 * 1650 + 1100);
    let top = pos.top_products(&admin, 0, i64::MAX, 5).unwrap();
    assert_eq!(top[0].revenue_cents, 4000);
}

#[test]
fn removing_a_line_removes_its_options() {
    let (mut pos, admin) = setup();
    let (burger, size, _) = burger_with_options(&mut pos, &admin);
    let o = takeaway(&pos, &admin);
    let order = pos.add_item(&admin, o.id, burger.id, 1, None, &[id(&size, "Large")]).unwrap();
    let line = &order.items[0];
    let order = pos.update_item(&admin, line.id, &ItemUpdate { qty: 0, discount_cents: 0, note: None }).unwrap();
    assert!(order.items.is_empty());
}

#[test]
fn option_admin_and_menu_feed() {
    let (mut pos, admin) = setup();
    let (burger, size, extras) = burger_with_options(&mut pos, &admin);
    assert!(pos.create_option_group(&admin, "burger SIZE", 0, 1).is_err()); // duplicate name
    assert!(pos.create_option_group(&admin, "Bad", 3, 2).is_err());
    assert!(pos.save_group_options(&admin, size.id, &[opt("", 0)]).is_err());

    let feed = pos.menu_options(&admin).unwrap();
    assert_eq!(feed.groups.len(), 2);
    let linked: Vec<i64> = feed.links.iter().filter(|l| l.product_id == burger.id).map(|l| l.group_id).collect();
    assert_eq!(linked, [size.id, extras.id]);

    // Retiring a group hides it from the menu feed and from validation.
    pos.update_option_group(&admin, size.id, "Burger size", 1, 1, 0, false).unwrap();
    let feed = pos.menu_options(&admin).unwrap();
    assert_eq!(feed.groups.len(), 1);
    let o = takeaway(&pos, &admin);
    assert!(pos.add_item(&admin, o.id, burger.id, 1, None, &[]).is_ok());

    // Staff can read options but not change them.
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    assert!(pos.menu_options(&staff).is_ok());
    assert!(matches!(pos.create_option_group(&staff, "X", 0, 1), Err(PosError::Forbidden)));
    assert!(matches!(pos.set_product_option_groups(&staff, burger.id, &[]), Err(PosError::Forbidden)));
}

#[test]
fn kitchen_tickets_per_round() {
    let (mut pos, admin) = setup();
    let (burger, size, extras) = burger_with_options(&mut pos, &admin);
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&admin, floor.id, &[TableLayout { id: None, name: "T3".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    let o = pos.open_order(&admin, &NewOrder { order_type: OrderType::DineIn, table_id: Some(t[0].id), details: Default::default() }).unwrap();
    pos.add_item(&admin, o.id, burger.id, 2, Some("no onions"), &[id(&size, "Large"), id(&extras, "Cheese")]).unwrap();

    // No printer configured: sending still works, nothing to print.
    let sent = pos.send_to_kitchen(&admin, o.id).unwrap();
    assert_eq!(sent[0].options.len(), 2);
    assert!(pos.kitchen_job(&admin, o.id, &sent, false).unwrap().is_none());

    let text = pos.kitchen_ticket(&admin, o.id, &sent, false).unwrap().to_text(42);
    assert!(text.contains("NEW ORDER"), "{text}");
    assert!(text.contains("DINE-IN T3"));
    assert!(text.contains("Ground"));
    assert!(text.contains("2 x Burger\n   + Large\n   + Cheese\n   ** no onions\n"), "{text}");

    // Second round is marked as additional and only lists the new items.
    pos.add_item(&admin, o.id, burger.id, 1, None, &[id(&size, "Regular")]).unwrap();
    let sent = pos.send_to_kitchen(&admin, o.id).unwrap();
    let text = pos.kitchen_ticket(&admin, o.id, &sent, false).unwrap().to_text(42);
    assert!(text.contains("ADDITIONAL"));
    assert!(text.contains("1 x Burger\n   + Regular\n"));
    assert!(!text.contains("2 x Burger"));

    // Configure a file printer and reprint everything.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kitchen.bin");
    let cfg = PrinterConfig { target: PrinterTarget::File { path: path.display().to_string() }, width: 32 };
    pos.set_kitchen_printer(&admin, &cfg).unwrap();
    assert_eq!(pos.kitchen_printer().unwrap(), cfg);
    let job = pos.kitchen_reprint_job(&admin, o.id).unwrap().unwrap();
    job.send().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let as_text = String::from_utf8_lossy(&bytes);
    assert!(as_text.contains("REPRINT"));
    assert!(as_text.contains("2 x Burger") && as_text.contains("1 x Burger"));
    assert!(bytes.ends_with(&[0x1D, b'V', 66, 0]));
}

#[test]
fn printer_settings_are_admin_only_and_validated() {
    let (mut pos, admin) = setup();
    assert_eq!(pos.kitchen_printer().unwrap().target, PrinterTarget::None);
    let bad = PrinterConfig { target: PrinterTarget::Network { host: "".into(), port: 9100 }, width: 42 };
    assert!(pos.set_kitchen_printer(&admin, &bad).is_err());
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    let good = PrinterConfig { target: PrinterTarget::Network { host: "192.168.1.50".into(), port: 9100 }, width: 42 };
    assert!(matches!(pos.set_kitchen_printer(&staff, &good), Err(PosError::Forbidden)));
    assert!(matches!(pos.test_print_job(&staff, &good), Err(PosError::Forbidden)));
    pos.set_kitchen_printer(&admin, &good).unwrap();
    // Internal setting is not exposed through the generic settings API.
    assert!(pos.setting("_kitchen_printer").unwrap().is_none());
    assert!(pos.set_setting(&admin, "_kitchen_printer", "{}").is_err());
}
