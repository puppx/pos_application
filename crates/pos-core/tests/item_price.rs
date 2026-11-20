//! Changing one order line's price: managers and admins directly, staff
//! with a manager's PIN; not once the bill is part paid.

use pos_core::*;

fn setup() -> (Pos, Session, Session, Session, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Ada", "1234").unwrap();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    pos.create_user(&admin, "Mo", Role::Manager, "3333").unwrap();
    let staff = pos.login("2222").unwrap();
    let manager = pos.login("3333").unwrap();
    let cat = pos.create_category(&admin, "Mains", None, 1).unwrap();
    let p = pos
        .create_product(&admin, NewProduct {
            category_id: Some(cat.id), sku: None, barcode: None, name: "Curry".into(), price_cents: 1000,
            cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap();
    (pos, admin, staff, manager, p.id)
}

#[test]
fn manager_changes_a_price_and_the_total_follows() {
    let (mut pos, _admin, _staff, manager, product) = setup();
    let o = pos.open_order(&manager, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    let o = pos.add_item(&manager, o.id, product, 2, None, None, &[]).unwrap();
    let line = o.items[0].id;
    let o = pos.set_item_price(&manager, None, line, 1250).unwrap();
    assert_eq!(o.items[0].unit_price_cents, 1250);
    assert_eq!(o.items[0].line_total_cents, 2500);
    assert_eq!(o.total_cents, 2500);
}

#[test]
fn staff_need_a_manager_pin() {
    let (mut pos, _admin, staff, manager, product) = setup();
    let o = pos.open_order(&staff, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    let line = pos.add_item(&staff, o.id, product, 1, None, None, &[]).unwrap().items[0].id;
    let err = pos.set_item_price(&staff, None, line, 500).unwrap_err().to_string();
    assert!(err.contains("needs a manager"), "{err}");
    let err = pos.set_item_price(&staff, Some(&staff.clone()), line, 500).unwrap_err().to_string();
    assert!(err.contains("needs a manager"), "{err}");
    let o = pos.set_item_price(&staff, Some(&manager), line, 500).unwrap();
    assert_eq!(o.total_cents, 500);
}

#[test]
fn a_discount_is_cut_to_the_new_price_and_bad_prices_are_refused() {
    let (mut pos, admin, _staff, _manager, product) = setup();
    let o = pos.open_order(&admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    let line = pos.add_item(&admin, o.id, product, 1, None, None, &[]).unwrap().items[0].id;
    pos.update_item(&admin, line, &ItemUpdate { qty: 1, discount_cents: 800, note: None }).unwrap();
    let o = pos.set_item_price(&admin, None, line, 300).unwrap();
    assert_eq!(o.items[0].discount_cents, 300);
    assert_eq!(o.total_cents, 0);
    assert!(pos.set_item_price(&admin, None, line, -1).is_err());
    assert!(pos.set_item_price(&admin, None, line, 10_000_001).is_err());
}
