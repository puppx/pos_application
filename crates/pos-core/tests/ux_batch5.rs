use pos_core::*;

fn setup() -> (Pos, Session, Product, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.set_setting(&admin, "prices_include_tax", "0").unwrap();
    pos.set_setting(&admin, "store_name", "Bistro").unwrap();
    let steak = pos
        .create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: "Steak".into(), price_cents: 2000,
            cost_cents: 0, tax_rate_bp: 1000, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&admin, floor.id, &[TableLayout { is_table: true, id: None, name: "T1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    (pos, admin, steak, t[0].id)
}

fn dine_in(table: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table), channel_id: None, details: Default::default() }
}

#[test]
fn customer_tax_table_explains_the_service_charge() {
    let (mut pos, s, steak, table) = setup();
    pos.set_service_settings(&s, &ServiceSettings { rate_bp: 1250, label: "Service".into(), order_types: vec![OrderType::DineIn] }).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    let o = pos.add_item(&s, o.id, steak.id, 1, None, None, &[]).unwrap();
    // 20.00 + tax 2.00 + service 12.5% of 20.00 = 2.50 -> 24.50.
    assert_eq!((o.tax_cents, o.service_cents, o.total_cents), (200, 250, 2450));

    // Default (no tax table): no note, the layout as before.
    let plain = pos.receipt(&s, o.id).unwrap().to_text(42);
    assert!(!plain.contains("GROSS") && !plain.contains("(no tax)"), "{plain}");
    assert_eq!(plain.matches("Service 12.5%").count(), 1, "{plain}");

    let mut cfg = pos.receipt_settings().unwrap();
    cfg.tax_breakdown = true;
    pos.set_receipt_settings(&s, &cfg).unwrap();
    let t = pos.receipt(&s, o.id).unwrap().to_text(42);
    // GROSS 22.00 in the table, then the service outside it: 22.00 + 2.50 = TOTAL 24.50.
    let note = t.lines().find(|l| l.starts_with("Service 12.5% (no tax)")).unwrap_or_else(|| panic!("no note in\n{t}"));
    assert!(note.ends_with("£2.50") && note.chars().count() == 42, "{note:?}");
    assert!(t.find("GROSS").unwrap() < t.find("(no tax)").unwrap(), "{t}");
    // Same wording as the staff copy.
    let staff = pos.receipt(&s, o.id).unwrap().staff_to_text(42);
    assert!(staff.contains(note), "{staff}");

    // No service charge: no note even with the table.
    pos.set_service_charge(&s, o.id, 0).unwrap();
    let t = pos.receipt(&s, o.id).unwrap().to_text(42);
    assert!(t.contains("GROSS") && !t.contains("(no tax)"), "{t}");
}

#[test]
fn an_order_cancelled_as_empty_answers_already_closed() {
    // The UI leaves the order screen on this exact message ("closed on another till").
    let (mut pos, s, steak, table) = setup();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    assert!(pos.cancel_if_empty(&s, o.id).unwrap());
    let err = pos.add_item(&s, o.id, steak.id, 1, None, None, &[]).unwrap_err();
    assert_eq!(err.to_string(), "this order is already closed");
}

#[test]
fn a_dish_joined_onto_another_bill_is_not_changed_from_the_old_order() {
    let (mut pos, s, steak, t1) = setup();
    let floor = pos.list_floors(&s).unwrap()[0].id;
    let tables = pos
        .save_floor_layout(&s, floor, &[
            TableLayout { is_table: true, id: Some(t1), name: "T1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 },
            TableLayout { is_table: true, id: None, name: "T2".into(), shape: TableShape::Rect, x: 100, y: 0, w: 80, h: 80 },
        ])
        .unwrap();
    let t2 = tables.iter().find(|t| t.name == "T2").unwrap().id;
    let a = pos.open_order(&s, &dine_in(t1)).unwrap();
    let a = pos.add_item(&s, a.id, steak.id, 1, None, None, &[]).unwrap();
    let dish = a.items[0].id;
    let b = pos.open_order(&s, &dine_in(t2)).unwrap();
    pos.add_item(&s, b.id, steak.id, 1, None, None, &[]).unwrap();
    let a = pos.add_item(&s, a.id, steak.id, 1, Some("rare"), None, &[]).unwrap();
    let other = a.items.iter().find(|i| i.id != dish).unwrap().id;
    pos.check_item_on_order(a.id, dish).unwrap();
    // One dish moved to T2's bill: A is still open, the dish is just not on it.
    pos.move_items(&s, a.id, t2, &[(dish, 1)]).unwrap();
    assert!(matches!(pos.check_item_on_order(a.id, dish), Err(PosError::NotFound)));
    pos.check_item_on_order(a.id, other).unwrap();
    pos.check_item_on_order(b.id, dish).unwrap();
    // Then A is joined onto T2's bill: A is gone, nothing of B is changed through it.
    let joined = pos.merge_into_table(&s, a.id, t2).unwrap();
    assert_eq!(joined.id, b.id);
    assert!(matches!(pos.check_item_on_order(a.id, other), Err(PosError::NotFound)));
    pos.check_item_on_order(b.id, other).unwrap();
}
