use pos_core::*;

struct Fixture {
    pos: Pos,
    admin: Session,
    staff: Session,
    burger: Product,
    soup: Product,
    table: i64,
}

fn setup() -> Fixture {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    let mains = pos.create_category(&admin, "Mains", Some("#ff0000"), 1).unwrap();
    let starters = pos.create_category(&admin, "Starters", None, 0).unwrap();
    let mut item = |name: &str, price, cat: i64, sku: Option<&str>| {
        pos.create_product(&admin, NewProduct {
            category_id: Some(cat), sku: sku.map(str::to_string), barcode: None, name: name.into(), price_cents: price,
            cost_cents: 100, tax_rate_bp: 1000, stock_qty: 5, track_stock: true, starter: false,
        })
        .unwrap()
    };
    let burger = item("Burger", 1000, mains.id, Some("B1"));
    let soup = item("Soup", 0, starters.id, None);
    pos.set_starter(&admin, soup.id, true).unwrap();
    pos.set_variants(&admin, soup.id, &[
        VariantInput { id: None, name: "Chicken".into(), price_cents: 650 },
        VariantInput { id: None, name: "Beef".into(), price_cents: 750 },
    ])
    .unwrap();
    let size = pos.create_option_group(&admin, "Size", 1, 1).unwrap();
    pos.save_group_options(&admin, size.id, &[
        OptionInput { id: None, name: "Regular".into(), price_cents: 0 },
        OptionInput { id: None, name: "Large".into(), price_cents: 200 },
    ])
    .unwrap();
    pos.set_product_option_groups(&admin, burger.id, &[size.id]).unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&admin, floor.id, &[TableLayout { id: None, name: "T1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    Fixture { pos, admin, staff, burger, soup, table: t[0].id }
}

/// A paid takeaway order with a burger (Large) and a beef soup.
fn sale(f: &mut Fixture, s: &Session) -> Order {
    let o = f.pos.open_order(s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    let opts = f.pos.menu_options(s).unwrap();
    let large = opts.groups[0].options[1].id;
    f.pos.add_item(s, o.id, f.burger.id, 1, None, None, &[large]).unwrap();
    let beef = f.pos.list_variants(s, Some(f.soup.id)).unwrap()[1].id;
    f.pos.add_item(s, o.id, f.soup.id, 1, None, Some(beef), &[]).unwrap();
    f.pos.pay_order(s, o.id, PaymentMethod::Card, 0).unwrap()
}

#[test]
fn menu_backup_round_trip_replaces_the_menu() {
    let mut f = setup();
    let old = { let who = f.staff.clone(); sale(&mut f, &who) };
    let backup = f.pos.export_menu(&f.admin).unwrap();
    assert_eq!((backup.categories.len(), backup.items.len(), backup.option_groups.len()), (2, 2, 1));
    let soup = backup.items.iter().find(|i| i.name == "Soup").unwrap();
    assert!(soup.starter && soup.choices.len() == 2);
    // The JSON file round-trips.
    let json = serde_json::to_string(&backup).unwrap();
    let parsed: MenuBackup = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, backup);

    // Change the menu, then restore.
    f.pos.create_category(&f.admin, "Desserts", None, 9).unwrap();
    let counts = f.pos.import_menu(&f.admin, &parsed).unwrap();
    assert_eq!((counts.categories, counts.items, counts.option_groups), (2, 2, 1));
    let menu = f.pos.menu(&f.admin).unwrap();
    assert_eq!(menu.len(), 2);
    let cats: Vec<String> = f.pos.list_categories(&f.admin, false).unwrap().into_iter().map(|c| c.name).collect();
    assert_eq!(cats, ["Starters", "Mains"]);
    let burger = menu.iter().find(|p| p.name == "Burger").unwrap();
    assert_eq!((burger.sku.as_deref(), burger.stock_qty, burger.price_cents), (Some("B1"), 4, 1000));
    // The restored menu works for a new sale, choices and options included.
    f.burger = burger.clone();
    f.soup = menu.iter().find(|p| p.name == "Soup").unwrap().clone();
    assert!(f.soup.starter);
    let again = { let who = f.admin.clone(); sale(&mut f, &who) };
    assert_eq!(again.total_cents, old.total_cents);
    // The old order still shows what was sold.
    let old = f.pos.get_order(&f.admin, old.id).unwrap();
    assert_eq!(old.items[1].name, "Soup (Beef)");

    // Staff cannot back up or restore; other JSON is refused.
    assert!(matches!(f.pos.export_menu(&f.staff), Err(PosError::Forbidden)));
    let mut bad = backup.clone();
    bad.format = "something".into();
    assert!(f.pos.import_menu(&f.admin, &bad).is_err());
}

#[test]
fn reset_keeps_what_was_not_chosen() {
    let mut f = setup();
    { let who = f.staff.clone(); sale(&mut f, &who) };
    f.pos.create_customer(&f.admin, &CustomerInput { name: "Ann".into(), phone: Some("07700900001".into()), ..Default::default() }).unwrap();
    // An open order on a table blocks deleting the floor plan alone.
    let open = f.pos.open_order(&f.admin, &NewOrder { order_type: OrderType::DineIn, table_id: Some(f.table), channel_id: None, details: Default::default() }).unwrap();
    let err = f.pos.reset_data(&f.admin, &ResetOptions { floor_plan: true, ..Default::default() }).unwrap_err();
    assert!(err.to_string().contains("open orders"), "{err}");
    // Staff cannot reset.
    assert!(matches!(f.pos.reset_data(&f.staff, &ResetOptions::default()), Err(PosError::Forbidden)));

    // Delete sales history and staff; keep menu, customers, floor plan, settings.
    let r = f.pos.reset_data(&f.admin, &ResetOptions { sales: Some(SalesReset::All), staff: true, ..Default::default() }).unwrap();
    assert_eq!((r.orders, r.staff), (2, 1));
    assert!(f.pos.list_open_orders(&f.admin).unwrap().is_empty());
    assert!(matches!(f.pos.get_order(&f.admin, open.id), Err(PosError::NotFound)));
    assert_eq!(f.pos.menu(&f.admin).unwrap().len(), 2);
    assert_eq!(f.pos.search_customers(&f.admin, "Ann", 5).unwrap().len(), 1);
    assert_eq!(f.pos.list_floors(&f.admin).unwrap().len(), 1);
    let users = f.pos.list_users(&f.admin).unwrap();
    assert_eq!(users.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["Owner"]);
    assert!(f.pos.login("2222").is_err());
    // The PIN can be given to someone new.
    f.pos.create_user(&f.admin, "New", Role::Staff, "2222").unwrap();

    // Now everything else.
    let r = f.pos.reset_data(&f.admin, &ResetOptions { customers: true, menu: true, floor_plan: true, settings: true, ..Default::default() }).unwrap();
    assert_eq!((r.customers, r.items, r.tables), (1, 2, 1));
    assert!(f.pos.menu(&f.admin).unwrap().is_empty());
    assert!(f.pos.list_categories(&f.admin, true).unwrap().is_empty());
    assert!(f.pos.list_option_groups(&f.admin, true).unwrap().is_empty());
    assert!(f.pos.list_floors(&f.admin).unwrap().is_empty());
    assert_eq!(f.pos.list_channels(&f.admin).unwrap().len(), 3);
    // Names and SKUs can be used again.
    f.pos.create_category(&f.admin, "Mains", None, 0).unwrap();
    f.pos.create_product(&f.admin, NewProduct {
        category_id: None, sku: Some("B1".into()), barcode: None, name: "Burger".into(), price_cents: 900,
        cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter: false,
    })
    .unwrap();
}

#[test]
fn reset_sales_before_a_date_keeps_newer_and_open_orders() {
    let mut f = setup();
    { let who = f.admin.clone(); sale(&mut f, &who) };
    let open = f.pos.open_order(&f.admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    let today = chrono::Local::now().date_naive();
    // Before today: today's sale stays.
    let r = f.pos.reset_data(&f.admin, &ResetOptions { sales: Some(SalesReset::Before { date: today.format("%Y-%m-%d").to_string() }), ..Default::default() }).unwrap();
    assert_eq!(r.orders, 0);
    // Before tomorrow: today's paid sale goes, the open order stays.
    let tomorrow = (today + chrono::Days::new(1)).format("%Y-%m-%d").to_string();
    let r = f.pos.reset_data(&f.admin, &ResetOptions { sales: Some(SalesReset::Before { date: tomorrow }), ..Default::default() }).unwrap();
    assert_eq!(r.orders, 1);
    assert_eq!(f.pos.list_open_orders(&f.admin).unwrap()[0].id, open.id);
    assert!(f.pos.reset_data(&f.admin, &ResetOptions { sales: Some(SalesReset::Before { date: "soon".into() }), ..Default::default() }).is_err());
}

#[test]
fn pin_check_and_empty_day_summary() {
    let mut f = setup();
    assert!(f.pos.verify_pin(&f.admin, "1111").is_ok());
    assert!(matches!(f.pos.verify_pin(&f.admin, "2222"), Err(PosError::InvalidPin)));
    let day = f.pos.close_day(&f.admin).unwrap();
    assert!(day.summary.is_empty());
    // The next day starts the second after the close.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    { let who = f.admin.clone(); sale(&mut f, &who) };
    let day = f.pos.close_day(&f.admin).unwrap();
    assert!(!day.summary.is_empty());
}
