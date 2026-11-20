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
            cost_cents: 0, tax_rate_bp: Some(0), stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
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

fn takeaway() -> NewOrder {
    NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }
}

#[test]
fn staff_cannot_multiply_a_cheaper_price_without_a_pin() {
    let (mut pos, _admin, staff, manager, product) = setup();
    let o = pos.open_order(&staff, &takeaway()).unwrap();
    let line = pos.add_item(&staff, o.id, product, 1, None, None, &[]).unwrap().items[0].id;
    pos.set_item_price(&staff, Some(&manager), line, 0).unwrap();
    let more = ItemUpdate { qty: 10, discount_cents: 0, note: None };
    let err = pos.update_item(&staff, line, &more).unwrap_err().to_string();
    assert!(err.contains("needs a manager"), "{err}");
    // The + stepper: one more is refused too.
    let err = pos.update_item(&staff, line, &ItemUpdate { qty: 2, discount_cents: 0, note: None }).unwrap_err().to_string();
    assert!(err.contains("needs a manager"), "{err}");
    // Another staff PIN does not approve it.
    let err = pos.update_item_approved(&staff, Some(&staff.clone()), line, &more).unwrap_err().to_string();
    assert!(err.contains("needs a manager"), "{err}");
    assert_eq!(pos.get_order(&staff, o.id).unwrap().items[0].qty, 1);
    // A manager's PIN does; a manager may directly.
    let o2 = pos.update_item_approved(&staff, Some(&manager), line, &more).unwrap();
    assert_eq!((o2.items[0].qty, o2.total_cents), (10, 0));
    pos.update_item(&manager, line, &ItemUpdate { qty: 11, discount_cents: 0, note: None }).unwrap();
    // Fewer, a note: no PIN.
    let o3 = pos.update_item(&staff, line, &ItemUpdate { qty: 3, discount_cents: 0, note: Some("no chilli".into()) }).unwrap();
    assert_eq!(o3.items[0].qty, 3);
}

#[test]
fn a_dearer_price_or_one_put_back_needs_no_pin_for_more() {
    let (mut pos, _admin, staff, manager, product) = setup();
    let o = pos.open_order(&staff, &takeaway()).unwrap();
    let line = pos.add_item(&staff, o.id, product, 1, None, None, &[]).unwrap().items[0].id;
    pos.set_item_price(&staff, Some(&manager), line, 1500).unwrap();
    pos.update_item(&staff, line, &ItemUpdate { qty: 2, discount_cents: 0, note: None }).unwrap();
    pos.set_item_price(&staff, Some(&manager), line, 400).unwrap();
    assert!(pos.update_item(&staff, line, &ItemUpdate { qty: 3, discount_cents: 0, note: None }).is_err());
    // Back to the menu price (from the first change, not the 15.00).
    pos.set_item_price(&staff, Some(&manager), line, 1000).unwrap();
    let o = pos.update_item(&staff, line, &ItemUpdate { qty: 3, discount_cents: 0, note: None }).unwrap();
    assert_eq!(o.total_cents, 3000);
}

#[test]
fn tapping_the_dish_again_adds_it_at_the_menu_price() {
    let (mut pos, _admin, staff, manager, product) = setup();
    let o = pos.open_order(&staff, &takeaway()).unwrap();
    let line = pos.add_item(&staff, o.id, product, 1, None, None, &[]).unwrap().items[0].id;
    pos.set_item_price(&staff, Some(&manager), line, 0).unwrap();
    // A menu tile, or + on a sent line (it adds the dish again): a new line
    // at 10.00, never more of the 0.00 one.
    let o = pos.add_item(&staff, o.id, product, 1, None, None, &[]).unwrap();
    assert_eq!(o.items.len(), 2);
    assert_eq!(o.total_cents, 1000);
    pos.send_to_kitchen(&staff, o.id).unwrap();
    let o = pos.add_item(&staff, o.id, product, 1, None, None, &[]).unwrap();
    assert_eq!((o.items.len(), o.total_cents), (3, 2000));
    assert_eq!(o.items.iter().find(|i| i.id == line).unwrap().qty, 1);
    // A sent line never grows: "add it again instead".
    let err = pos.update_item(&manager, line, &ItemUpdate { qty: 2, discount_cents: 0, note: None }).unwrap_err().to_string();
    assert!(err.contains("already sent"), "{err}");
}

#[test]
fn a_cheaper_line_moved_to_another_table_keeps_needing_a_pin() {
    let (mut pos, admin, staff, manager, product) = setup();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let layout: Vec<TableLayout> = ["1", "2"]
        .iter()
        .map(|n| TableLayout { is_table: Some(true), id: None, name: (*n).into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 })
        .collect();
    let tables: Vec<i64> = pos.save_floor_layout(&admin, floor.id, &layout).unwrap().into_iter().map(|t| t.id).collect();
    let o = pos.open_order(&staff, &NewOrder { order_type: OrderType::DineIn, table_id: Some(tables[0]), channel_id: None, details: Default::default() }).unwrap();
    let line = pos.add_item(&staff, o.id, product, 2, None, None, &[]).unwrap().items[0].id;
    pos.set_item_price(&staff, Some(&manager), line, 100).unwrap();
    let moved = pos.move_items(&staff, o.id, tables[1], &[(line, 1)]).unwrap();
    let there = moved.items[0].id;
    assert_eq!(moved.items[0].unit_price_cents, 100);
    let err = pos.update_item(&staff, there, &ItemUpdate { qty: 5, discount_cents: 0, note: None }).unwrap_err().to_string();
    assert!(err.contains("needs a manager"), "{err}");
}
