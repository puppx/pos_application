use pos_core::*;

fn setup() -> (Pos, Session, Product, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let mains = pos.create_category(&admin, "Mains", None, 1).unwrap();
    let dish = pos
        .create_product(&admin, NewProduct {
            category_id: Some(mains.id), sku: None, barcode: None, name: "Steak".into(), price_cents: 1000,
            cost_cents: 0, tax_rate_bp: 2000, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false,
        })
        .unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&admin, floor.id, &[TableLayout { id: None, name: "T1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    (pos, admin, dish, t[0].id)
}

fn dine_in(table: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table), channel_id: None, details: Default::default() }
}

#[test]
fn the_price_already_holds_the_tax() {
    let (mut pos, s, dish, table) = setup();
    // Nothing set: prices include tax. 10.00 at 20% = 8.33 food + 1.67 tax, and the customer pays 10.00.
    assert!(pos.prices_include_tax().unwrap());
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    assert!(o.tax_incl);
    let o = pos.add_item(&s, o.id, dish.id, 1, None, None, &[]).unwrap();
    assert_eq!((o.subtotal_cents, o.tax_cents, o.total_cents), (1000, 167, 1000));
    assert_eq!(o.items[0].tax_cents, 167);
    // The same, from the 7% Thai VAT point of view: 1.07 holds 0.07.
    let p = pos.update_product(&s, dish.id, ProductUpdate {
        category_id: dish.category_id, sku: None, barcode: None, name: "Steak".into(), price_cents: 107, cost_cents: 0,
        tax_rate_bp: 700, track_stock: false, active: true, starter: false, name_alt: None, prep: false,
    })
    .unwrap();
    let t2 = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    let t2 = pos.add_item(&s, t2.id, p.id, 1, None, None, &[]).unwrap();
    assert_eq!((t2.tax_cents, t2.total_cents), (7, 107));
}

#[test]
fn discount_service_and_every_view_agree() {
    let (mut pos, s, dish, table) = setup();
    pos.set_service_settings(&s, &ServiceSettings { rate_bp: 1000, label: "Service".into(), order_types: vec![OrderType::DineIn] }).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    let o = pos.add_item(&s, o.id, dish.id, 2, None, None, &[]).unwrap();
    // 2 x 10.00 - 2.00 discount = 18.00 (tax inside: 3.00); service 10% of 18.00 = 1.80.
    let o = pos.update_item(&s, o.items[0].id, &ItemUpdate { qty: 2, discount_cents: 200, note: None }).unwrap();
    assert_eq!((o.subtotal_cents, o.discount_cents, o.tax_cents, o.service_cents, o.total_cents), (2000, 200, 300, 180, 1980));
    assert_eq!(pos.list_open_orders(&s).unwrap()[0].total_cents, 1980);
    let floor = pos.list_floors(&s).unwrap()[0].id;
    assert_eq!(pos.floor_status(&s, floor).unwrap()[0].total_cents, 1980);

    let bill = pos.receipt(&s, o.id).unwrap().to_text(42);
    assert!(bill.contains("Tax (included)") && bill.contains("£3.00") && bill.contains("£19.80"), "{bill}");
    assert!(pos.pay_order(&s, o.id, PaymentMethod::Cash, 1979).is_err());
    let paid = pos.pay_order(&s, o.id, PaymentMethod::Cash, 2000).unwrap();
    assert_eq!((paid.tax_cents, paid.total_cents, paid.change_cents), (300, 1980, Some(20)));

    // Reports: the tax is shown as included, the category sales are before tax.
    let day = pos.close_day(&s).unwrap();
    let sum = &day.summary;
    assert_eq!((sum.tax_cents, sum.tax_included_cents, sum.service_cents, sum.total_cents), (300, 300, 180, 1980));
    assert_eq!(sum.by_category[0].total_cents, 1500, "18.00 less the 3.00 of tax inside it");
    let text = pos.day_close_text(&day).unwrap();
    assert!(text.contains("Sales (menu prices)") && text.contains("tax included in prices"), "{text}");
    assert!(!text.contains("\nTax "), "no tax was added on top: {text}");
}

#[test]
fn each_order_keeps_the_way_it_was_rung_up() {
    let (mut pos, s, dish, table) = setup();
    let first = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, first.id, dish.id, 1, None, None, &[]).unwrap();
    // Switch to "tax added on top": the open order is unchanged, new ones add tax.
    pos.set_setting(&s, "prices_include_tax", "0").unwrap();
    assert!(!pos.prices_include_tax().unwrap());
    let first = pos.get_order(&s, first.id).unwrap();
    assert_eq!((first.tax_incl, first.tax_cents, first.total_cents), (true, 167, 1000));
    let t = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    let t = pos.add_item(&s, t.id, dish.id, 1, None, None, &[]).unwrap();
    assert_eq!((t.tax_incl, t.tax_cents, t.total_cents), (false, 200, 1200));
    // Only "0" turns it off.
    pos.set_setting(&s, "prices_include_tax", "1").unwrap();
    assert!(pos.prices_include_tax().unwrap());

    // A different rate on one included-tax order only changes the split.
    let o = pos.set_order_tax(&s, first.id, Some(500)).unwrap();
    assert_eq!((o.tax_cents, o.total_cents), (48, 1000)); // 10.00 x 5 / 105
    let o = pos.set_order_tax(&s, first.id, Some(0)).unwrap();
    assert_eq!((o.tax_cents, o.total_cents), (0, 1000));
}
