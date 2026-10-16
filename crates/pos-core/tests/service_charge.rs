use pos_core::*;

fn setup() -> (Pos, Session, Product, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.set_setting(&admin, "store_name", "Bistro").unwrap();
    let mains = pos.create_category(&admin, "Mains", None, 1).unwrap();
    let steak = pos
        .create_product(&admin, NewProduct {
            category_id: Some(mains.id), sku: None, barcode: None, name: "Steak".into(), price_cents: 2000,
            cost_cents: 0, tax_rate_bp: 1000, stock_qty: 0, track_stock: false, starter: false,
        })
        .unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&admin, floor.id, &[TableLayout { id: None, name: "T1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    (pos, admin, steak, t[0].id)
}

fn order(t: OrderType, table: Option<i64>) -> NewOrder {
    NewOrder { order_type: t, table_id: table, channel_id: None, details: Default::default() }
}

#[test]
fn off_by_default_and_unchanged_totals() {
    let (mut pos, s, steak, table) = setup();
    assert_eq!(pos.service_settings().unwrap(), ServiceSettings::default());
    let o = pos.open_order(&s, &order(OrderType::DineIn, Some(table))).unwrap();
    let o = pos.add_item(&s, o.id, steak.id, 1, None, None, &[]).unwrap();
    assert_eq!((o.service_bp, o.service_cents, o.total_cents), (0, 0, 2200));
}

#[test]
fn dine_in_gets_service_and_it_flows_everywhere() {
    let (mut pos, s, steak, table) = setup();
    pos.set_service_settings(&s, &ServiceSettings { rate_bp: 1250, label: " Service ".into(), order_types: vec![OrderType::DineIn] }).unwrap();
    assert_eq!(pos.service_settings().unwrap().label, "Service");
    assert!(pos.set_service_settings(&s, &ServiceSettings { rate_bp: 3100, ..Default::default() }).is_err());

    // Dine-in: 2 steaks = 40.00, discount 4.00 -> service 12.5% of 36.00 = 4.50, tax 3.60.
    let o = pos.open_order(&s, &order(OrderType::DineIn, Some(table))).unwrap();
    assert_eq!(o.service_bp, 1250);
    let o = pos.add_item(&s, o.id, steak.id, 2, None, None, &[]).unwrap();
    let o = pos.update_item(&s, o.items[0].id, &ItemUpdate { qty: 2, discount_cents: 400, note: None }).unwrap();
    assert_eq!((o.subtotal_cents, o.discount_cents, o.tax_cents, o.service_cents, o.total_cents), (4000, 400, 360, 450, 4410));
    // The live views include it too.
    assert_eq!(pos.list_open_orders(&s).unwrap()[0].total_cents, 4410);
    let floor = pos.list_floors(&s).unwrap()[0].id;
    assert_eq!(pos.floor_status(&s, floor).unwrap()[0].total_cents, 4410);
    // The bill shows it.
    let bill = pos.receipt(&s, o.id).unwrap().to_text(42);
    assert!(bill.contains("Service 12.5%") && bill.contains("£4.50"), "{bill}");

    // Cash must cover the service too.
    assert!(pos.pay_order(&s, o.id, PaymentMethod::Cash, 4400).is_err());
    let paid = pos.pay_order(&s, o.id, PaymentMethod::Cash, 5000).unwrap();
    assert_eq!((paid.service_cents, paid.total_cents, paid.change_cents), (450, 4410, Some(590)));
    let receipt = pos.receipt(&s, paid.id).unwrap().to_text(42);
    assert!(receipt.contains("Service 12.5%"), "{receipt}");

    // Takeaway is not in the list: no service.
    let t = pos.open_order(&s, &order(OrderType::Takeaway, None)).unwrap();
    assert_eq!(t.service_bp, 0);
    let t = pos.add_item(&s, t.id, steak.id, 1, None, None, &[]).unwrap();
    pos.pay_order(&s, t.id, PaymentMethod::Card, 0).unwrap();

    // Reports and the end-of-day summary.
    let day = pos.close_day(&s).unwrap();
    assert_eq!((day.summary.service_cents, day.summary.total_cents), (450, 4410 + 2200));
    let text = pos.day_close_text(&day).unwrap();
    assert!(text.contains("Service charge") && text.contains("£4.50"), "{text}");
}

#[test]
fn staff_can_change_or_remove_it_on_one_order() {
    let (mut pos, admin, steak, table) = setup();
    pos.set_service_settings(&admin, &ServiceSettings { rate_bp: 1000, ..Default::default() }).unwrap();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let s = pos.login("2222").unwrap();
    let o = pos.open_order(&s, &order(OrderType::DineIn, Some(table))).unwrap();
    pos.add_item(&s, o.id, steak.id, 1, None, None, &[]).unwrap();
    let o = pos.set_service_charge(&s, o.id, 0).unwrap();
    assert_eq!((o.service_cents, o.total_cents), (0, 2200));
    let o = pos.set_service_charge(&s, o.id, 1500).unwrap();
    assert_eq!(o.service_cents, 300);
    assert!(pos.set_service_charge(&s, o.id, -1).is_err());
    assert!(pos.set_service_charge(&s, o.id, 3001).is_err());
    let paid = pos.pay_order(&s, o.id, PaymentMethod::Card, 0).unwrap();
    // Closed orders cannot change.
    assert!(pos.set_service_charge(&s, paid.id, 0).is_err());
    // A new order still gets the default rate.
    let o2 = pos.open_order(&s, &order(OrderType::DineIn, Some(table))).unwrap();
    assert_eq!(o2.service_bp, 1000);
}
