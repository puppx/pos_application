//! Changes to the money on an order that staff may not make alone: taking a
//! payment back, removing or lowering the service charge, a tax rate for the
//! whole order. A manager's or admin's PIN approves them; the audit log says
//! who did it and who approved it. And the tax a new menu item gets when the
//! till leaves it out.

use pos_core::*;

struct Kit {
    pos: Pos,
    admin: Session,
    manager: Session,
    staff: Session,
    table: i64,
    curry: Product,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let manager = pos.login("4646").unwrap();
    let staff = pos.login("2222").unwrap();
    pos.set_service_settings(&admin, &ServiceSettings { rate_bp: 1000, ..Default::default() }).unwrap();
    let curry = pos
        .create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: "Curry".into(), price_cents: 1000, cost_cents: 0, tax_rate_bp: Some(2000),
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&admin, floor.id, &[TableLayout { is_table: Some(true), id: None, name: "1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    Kit { pos, admin, manager, staff, table: t[0].id, curry }
}

/// A dine-in order opened by staff: 2 curries (20.00) + 10% service = 22.00.
fn open(k: &mut Kit) -> Order {
    let o = k.pos.open_order(&k.staff, &NewOrder { order_type: OrderType::DineIn, table_id: Some(k.table), channel_id: None, details: Default::default() }).unwrap();
    k.pos.add_item(&k.staff, o.id, k.curry.id, 2, None, None, &[]).unwrap()
}

fn cash(amount: i64) -> PaymentInput {
    PaymentInput { method: PaymentMethod::Cash, amount_cents: Some(amount), tendered_cents: amount, tip_cents: 0, items: None }
}

/// The audit log's last entry with `action`: (user id, detail).
fn last_audit(pos: &Pos, action: &str) -> Option<(Option<i64>, String)> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("copy.db");
    pos.backup_unchecked(&path).unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    c.query_row("SELECT user_id, detail FROM audit_log WHERE action = ?1 ORDER BY id DESC LIMIT 1", [action], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok()
}

fn audits(pos: &Pos, action: &str) -> i64 {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("copy.db");
    pos.backup_unchecked(&path).unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    c.query_row("SELECT COUNT(*) FROM audit_log WHERE action = ?1", [action], |r| r.get(0)).unwrap()
}

#[test]
fn staff_take_a_payment_back_only_with_a_managers_pin() {
    let mut k = setup();
    let o = open(&mut k);
    assert_eq!(o.total_cents, 2200);
    let o = k.pos.add_payment(&k.staff, o.id, &cash(1000)).unwrap();
    let pay = o.payments[0].id;

    // Staff alone: refused, the payment stays, nothing logged.
    let err = k.pos.remove_payment(&k.staff, o.id, pay).unwrap_err();
    assert_eq!(err.to_string(), "taking a payment back needs a manager PIN");
    // Their own PIN (or any staff PIN) does not approve it.
    let own = k.pos.pin_owner_for("2222", Some(k.staff.user_id)).unwrap();
    let err = k.pos.remove_payment_approved(&k.staff, Some(&own), o.id, pay).unwrap_err();
    assert_eq!(err.to_string(), "taking a payment back needs a manager's or admin's PIN");
    // A wrong PIN approves nothing.
    assert!(matches!(k.pos.pin_owner_for("9999", Some(k.staff.user_id)), Err(PosError::InvalidPin)));
    assert_eq!(k.pos.get_order(&k.staff, o.id).unwrap().paid_cents, 1000);
    assert_eq!(audits(&k.pos, "order.payment_removed"), 0);

    // With the manager's PIN: taken back, logged as Sam's, approved by Quinn.
    let quinn = k.pos.pin_owner_for("4646", Some(k.staff.user_id)).unwrap();
    let o = k.pos.remove_payment_approved(&k.staff, Some(&quinn), o.id, pay).unwrap();
    assert_eq!((o.paid_cents, o.payments.len()), (0, 0));
    let (user, detail) = last_audit(&k.pos, "order.payment_removed").unwrap();
    assert_eq!(user, Some(k.staff.user_id));
    assert_eq!(detail, format!("order {} payment {pay} cash 1000 tip 0 taken by Sam (PIN of Quinn #{}, manager)", o.id, k.manager.user_id));

    // A manager (and an admin) need no PIN.
    for who in [k.manager.clone(), k.admin.clone()] {
        let o = k.pos.add_payment(&k.staff, o.id, &cash(500)).unwrap();
        let o = k.pos.remove_payment(&who, o.id, o.payments[0].id).unwrap();
        assert_eq!(o.paid_cents, 0);
        let (user, detail) = last_audit(&k.pos, "order.payment_removed").unwrap();
        assert_eq!(user, Some(who.user_id));
        assert!(detail.ends_with(&format!("(no PIN, {})", who.role.as_str())), "{detail}");
    }
    assert_eq!(audits(&k.pos, "order.payment_removed"), 3);

    // A payment that is not there: not found, even for staff without a PIN.
    assert!(matches!(k.pos.remove_payment(&k.staff, o.id, 999_999), Err(PosError::NotFound)));
    // A paid order's payments cannot be taken back at all (void it instead).
    let paid = k.pos.pay_order(&k.staff, o.id, PaymentMethod::Card, 0).unwrap();
    assert_eq!(paid.status, "paid");
    assert!(k.pos.remove_payment(&k.admin, paid.id, paid.payments[0].id).is_err());
}

#[test]
fn staff_remove_or_lower_the_service_charge_only_with_a_managers_pin() {
    let mut k = setup();
    let o = open(&mut k);
    assert_eq!((o.service_bp, o.service_cents), (1000, 200));

    for rate in [0, 500] {
        let err = k.pos.set_service_charge(&k.staff, o.id, rate).unwrap_err();
        assert_eq!(err.to_string(), "removing or lowering the service charge needs a manager PIN");
        let own = k.pos.pin_owner_for("2222", Some(k.staff.user_id)).unwrap();
        let err = k.pos.set_service_charge_approved(&k.staff, Some(&own), o.id, rate).unwrap_err();
        assert_eq!(err.to_string(), "removing or lowering the service charge needs a manager's or admin's PIN");
    }
    assert_eq!(k.pos.get_order(&k.staff, o.id).unwrap().service_bp, 1000);
    // Raising it, or setting the same rate, is anyone's.
    let o = k.pos.set_service_charge(&k.staff, o.id, 1250).unwrap();
    assert_eq!(o.service_bp, 1250);
    let (user, detail) = last_audit(&k.pos, "order.service").unwrap();
    assert_eq!((user, detail), (Some(k.staff.user_id), format!("{} 1250 (was 1000) (no PIN, staff)", o.id)));
    k.pos.set_service_charge(&k.staff, o.id, 1250).unwrap();
    assert_eq!(audits(&k.pos, "order.service"), 1);

    // With the manager's PIN: removed, and the log says who approved it.
    let quinn = k.pos.pin_owner_for("4646", Some(k.staff.user_id)).unwrap();
    let o = k.pos.set_service_charge_approved(&k.staff, Some(&quinn), o.id, 0).unwrap();
    assert_eq!((o.service_cents, o.total_cents), (0, 2000));
    let (user, detail) = last_audit(&k.pos, "order.service").unwrap();
    assert_eq!(user, Some(k.staff.user_id));
    assert_eq!(detail, format!("{} 0 (was 1250) (PIN of Quinn #{}, manager)", o.id, k.manager.user_id));

    // A manager needs no PIN.
    k.pos.set_service_charge(&k.staff, o.id, 1000).unwrap();
    let o = k.pos.set_service_charge(&k.manager, o.id, 0).unwrap();
    assert_eq!(o.service_bp, 0);
    assert!(last_audit(&k.pos, "order.service").unwrap().1.ends_with("(no PIN, manager)"));
}

#[test]
fn staff_set_a_tax_rate_for_the_order_only_with_a_managers_pin() {
    let mut k = setup();
    let o = open(&mut k);
    assert_eq!((o.tax_bp, o.tax_cents), (None, 333));

    for rate in [0, 500, 2500] {
        let err = k.pos.set_order_tax(&k.staff, o.id, Some(rate)).unwrap_err();
        assert_eq!(err.to_string(), "changing the tax on an order needs a manager PIN");
        let own = k.pos.pin_owner_for("2222", Some(k.staff.user_id)).unwrap();
        let err = k.pos.set_order_tax_approved(&k.staff, Some(&own), o.id, Some(rate)).unwrap_err();
        assert_eq!(err.to_string(), "changing the tax on an order needs a manager's or admin's PIN");
    }
    assert_eq!(audits(&k.pos, "order.tax"), 0);
    // Back to the menu rates is anyone's (already there: nothing changes or is logged).
    let o = k.pos.set_order_tax(&k.staff, o.id, None).unwrap();
    assert_eq!(o.tax_cents, 333);
    assert_eq!(audits(&k.pos, "order.tax"), 0);

    // With the manager's PIN: no tax on this order, logged.
    let quinn = k.pos.pin_owner_for("4646", Some(k.staff.user_id)).unwrap();
    let o = k.pos.set_order_tax_approved(&k.staff, Some(&quinn), o.id, Some(0)).unwrap();
    assert_eq!((o.tax_bp, o.tax_cents), (Some(0), 0));
    let (user, detail) = last_audit(&k.pos, "order.tax").unwrap();
    assert_eq!(user, Some(k.staff.user_id));
    assert_eq!(detail, format!("{} Some(0) (was None) (PIN of Quinn #{}, manager)", o.id, k.manager.user_id));
    // The same rate again needs nobody and logs nothing; going back to the menu is staff's own.
    k.pos.set_order_tax(&k.staff, o.id, Some(0)).unwrap();
    assert_eq!(audits(&k.pos, "order.tax"), 1);
    let o = k.pos.set_order_tax(&k.staff, o.id, None).unwrap();
    assert_eq!((o.tax_bp, o.tax_cents), (None, 333));
    assert!(last_audit(&k.pos, "order.tax").unwrap().1.ends_with("(was Some(0)) (no PIN, staff)"));

    // A manager needs no PIN.
    let o = k.pos.set_order_tax(&k.manager, o.id, Some(500)).unwrap();
    assert!(o.items.iter().all(|i| i.tax_rate_bp == 500));
    assert!(last_audit(&k.pos, "order.tax").unwrap().1.ends_with("(no PIN, manager)"));
}

#[test]
fn a_new_item_without_a_tax_rate_gets_the_default_and_an_update_without_one_keeps_its_own() {
    let mut k = setup();
    k.pos.set_setting(&k.admin, "default_tax_bp", "2000").unwrap();
    // What a till sends without the field (or with null).
    let item = |json: serde_json::Value| -> NewProduct { serde_json::from_value(json).unwrap() };
    let plain = k.pos.create_product(&k.admin, item(serde_json::json!({ "name": "Soup", "price_cents": 500 }))).unwrap();
    assert_eq!(plain.tax_rate_bp, 2000);
    let null = k.pos.create_product(&k.admin, item(serde_json::json!({ "name": "Rice", "price_cents": 300, "tax_rate_bp": null }))).unwrap();
    assert_eq!(null.tax_rate_bp, 2000);
    // A category with its own rate gives that one; one without, the main default.
    let drinks = k.pos.create_category(&k.admin, "Drinks", None, 1).unwrap();
    k.pos.set_category_defaults(&k.admin, drinks.id, Some(500), false, false, None).unwrap();
    let mains = k.pos.create_category(&k.admin, "Mains", None, 2).unwrap();
    let tea = k.pos.create_product(&k.admin, item(serde_json::json!({ "name": "Tea", "price_cents": 200, "category_id": drinks.id }))).unwrap();
    assert_eq!(tea.tax_rate_bp, 500);
    let stew = k.pos.create_product(&k.admin, item(serde_json::json!({ "name": "Stew", "price_cents": 900, "category_id": mains.id }))).unwrap();
    assert_eq!(stew.tax_rate_bp, 2000);
    // A rate the till does send, 0% included, is kept.
    let free = k.pos.create_product(&k.admin, item(serde_json::json!({ "name": "Water", "price_cents": 0, "tax_rate_bp": 0 }))).unwrap();
    assert_eq!(free.tax_rate_bp, 0);
    assert!(k.pos.create_product(&k.admin, item(serde_json::json!({ "name": "Bad", "price_cents": 0, "tax_rate_bp": 10_001 }))).is_err());

    // Updating without the field keeps the item's rate (it used to become 0%).
    let upd = |json: serde_json::Value| -> ProductUpdate { serde_json::from_value(json).unwrap() };
    let tea = k.pos.update_product(&k.admin, tea.id, upd(serde_json::json!({ "name": "Green tea", "price_cents": 250, "category_id": drinks.id }))).unwrap();
    assert_eq!((tea.name.as_str(), tea.price_cents, tea.tax_rate_bp), ("Green tea", 250, 500));
    let tea = k.pos.update_product(&k.admin, tea.id, upd(serde_json::json!({ "name": "Green tea", "price_cents": 250, "category_id": drinks.id, "tax_rate_bp": 0 }))).unwrap();
    assert_eq!(tea.tax_rate_bp, 0);
    assert!(matches!(k.pos.update_product(&k.admin, 999_999, upd(serde_json::json!({ "name": "X", "price_cents": 1 }))), Err(PosError::NotFound)));
    // Staff cannot make menu items at all.
    assert!(matches!(k.pos.create_product(&k.staff, item(serde_json::json!({ "name": "Soup", "price_cents": 500 }))), Err(PosError::Forbidden)));
}

/// A takeaway order opened by staff: 2 curries (another bill while the table's is open).
fn takeaway(k: &mut Kit) -> Order {
    let o = k.pos.open_order(&k.staff, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    k.pos.add_item(&k.staff, o.id, k.curry.id, 2, None, None, &[]).unwrap()
}

fn upd(qty: i64) -> ItemUpdate {
    ItemUpdate { qty, discount_cents: 0, note: None }
}

#[test]
fn staff_reduce_or_remove_a_sent_dish_only_with_a_managers_pin() {
    let mut k = setup();
    let o = open(&mut k);
    let line = o.items[0].id;
    k.pos.send_to_kitchen(&k.staff, o.id).unwrap();

    // Staff alone: refused, both curries stay, nothing logged.
    let err = k.pos.update_item(&k.staff, line, &upd(1)).unwrap_err();
    assert_eq!(err.to_string(), "removing a dish already sent to the kitchen needs a manager PIN");
    let err = k.pos.update_item(&k.staff, line, &upd(0)).unwrap_err();
    assert_eq!(err.to_string(), "removing a dish already sent to the kitchen needs a manager PIN");
    // The kitchen-ticket path is refused the same way (no CANCELLED ticket).
    assert!(k.pos.update_item_for_kitchen(&k.staff, line, &upd(0)).is_err());
    // Their own PIN does not approve it; a wrong PIN is an invalid PIN.
    let own = k.pos.pin_owner_for("2222", Some(k.staff.user_id)).unwrap();
    let err = k.pos.update_item_approved(&k.staff, Some(&own), line, &upd(1)).unwrap_err();
    assert_eq!(err.to_string(), "removing a dish already sent to the kitchen needs a manager's or admin's PIN");
    assert!(matches!(k.pos.pin_owner_for("9999", Some(k.staff.user_id)), Err(PosError::InvalidPin)));
    assert_eq!(k.pos.get_order(&k.staff, o.id).unwrap().items[0].qty, 2);
    assert_eq!(audits(&k.pos, "order.item_void"), 0);
    // So the order, with sent dishes, still cannot be cancelled without a PIN.
    assert!(k.pos.cancel_order(&k.staff, o.id, "mistake").is_err());

    // With Quinn's PIN: one fewer, logged with who approved it.
    let quinn = k.pos.pin_owner_for("4646", Some(k.staff.user_id)).unwrap();
    let o2 = k.pos.update_item_approved(&k.staff, Some(&quinn), line, &upd(1)).unwrap();
    assert_eq!(o2.items[0].qty, 1);
    let (who, detail) = last_audit(&k.pos, "order.item_void").unwrap();
    assert_eq!(who, Some(k.staff.user_id));
    assert!(detail.ends_with(&format!("Curry x1 (PIN of Quinn #{}, manager)", k.manager.user_id)), "{detail}");

    // Managers and admins are never asked.
    let o3 = k.pos.update_item(&k.manager, line, &upd(0)).unwrap();
    assert!(o3.items.is_empty());
    assert!(last_audit(&k.pos, "order.item_void").unwrap().1.ends_with("(no PIN, manager)"));
    let o = takeaway(&mut k);
    k.pos.send_to_kitchen(&k.staff, o.id).unwrap();
    k.pos.update_item(&k.admin, o.items[0].id, &upd(1)).unwrap();
    assert!(last_audit(&k.pos, "order.item_void").unwrap().1.ends_with("(no PIN, admin)"));

    // Dishes not yet sent stay free for staff.
    let o = takeaway(&mut k);
    let fresh = o.items.iter().find(|i| i.sent_at.is_none()).unwrap().id;
    assert_eq!(k.pos.update_item(&k.staff, fresh, &upd(1)).unwrap().items.iter().find(|i| i.id == fresh).unwrap().qty, 1);
    k.pos.update_item(&k.staff, fresh, &upd(0)).unwrap();
}

#[test]
fn with_staff_cancel_sent_off_staff_cannot_fix_sent_dishes_even_with_a_pin() {
    let mut k = setup();
    k.pos.set_setting(&k.admin, "staff_cancel_sent", "0").unwrap();
    let o = takeaway(&mut k);
    let line = o.items[0].id;
    k.pos.send_to_kitchen(&k.staff, o.id).unwrap();
    assert!(matches!(k.pos.update_item(&k.staff, line, &upd(1)), Err(PosError::Forbidden)));
    let quinn = k.pos.pin_owner_for("4646", Some(k.staff.user_id)).unwrap();
    assert!(matches!(k.pos.update_item_approved(&k.staff, Some(&quinn), line, &upd(1)), Err(PosError::Forbidden)));
    assert_eq!(k.pos.update_item(&k.manager, line, &upd(1)).unwrap().items[0].qty, 1);
}

#[test]
fn the_staff_discount_limit_is_10_percent_until_set_and_no_limit_stays_no_limit() {
    let mut k = setup();
    let o = takeaway(&mut k);
    let line = o.items[0].id; // 2 x 10.00
    let off = |c| ItemUpdate { qty: 2, discount_cents: c, note: None };
    // Never set: 10% (2.00 of 20.00) is fine, more needs a manager.
    k.pos.update_item(&k.staff, line, &off(200)).unwrap();
    let err = k.pos.update_item(&k.staff, line, &off(2000)).unwrap_err();
    assert_eq!(err.to_string(), "a discount over 10% needs a manager PIN");
    let own = k.pos.pin_owner_for("2222", Some(k.staff.user_id)).unwrap();
    let err = k.pos.update_item_approved(&k.staff, Some(&own), line, &off(2000)).unwrap_err();
    assert_eq!(err.to_string(), "a discount over 10% needs a manager's or admin's PIN");
    let quinn = k.pos.pin_owner_for("4646", Some(k.staff.user_id)).unwrap();
    assert_eq!(k.pos.update_item_approved(&k.staff, Some(&quinn), line, &off(1000)).unwrap().items[0].discount_cents, 1000);
    assert_eq!(last_audit(&k.pos, "order.discount_approved").unwrap().0, Some(k.manager.user_id));
    // Managers are never asked.
    k.pos.update_item(&k.manager, line, &off(2000)).unwrap();

    // "No limit" (stored empty) stays no limit.
    k.pos.set_setting(&k.admin, DISCOUNT_LIMIT_KEY, "").unwrap();
    let o = takeaway(&mut k);
    k.pos.update_item(&k.staff, o.items[0].id, &off(2000)).unwrap();
    // An explicit limit is kept.
    k.pos.set_setting(&k.admin, DISCOUNT_LIMIT_KEY, "50").unwrap();
    let o = takeaway(&mut k);
    k.pos.update_item(&k.staff, o.items[0].id, &off(1000)).unwrap();
    assert_eq!(k.pos.update_item(&k.staff, o.items[0].id, &off(1200)).unwrap_err().to_string(), "a discount over 50% needs a manager PIN");
}

#[test]
fn managers_set_the_staff_discount_limit_and_staff_cannot() {
    let mut k = setup();
    assert!(matches!(k.pos.set_setting(&k.staff, DISCOUNT_LIMIT_KEY, "100"), Err(PosError::Forbidden)));
    assert!(matches!(k.pos.set_setting(&k.staff, DISCOUNT_LIMIT_KEY, ""), Err(PosError::Forbidden)));
    // Still validated for managers; other admin settings stay the admin's.
    assert!(k.pos.set_setting(&k.manager, DISCOUNT_LIMIT_KEY, "101").is_err());
    assert!(matches!(k.pos.set_setting(&k.manager, "discount_presets", "5,10"), Err(PosError::Forbidden)));
    k.pos.set_setting(&k.manager, DISCOUNT_LIMIT_KEY, "").unwrap();
    assert_eq!(k.pos.setting(DISCOUNT_LIMIT_KEY).unwrap().as_deref(), Some(""));
    k.pos.set_setting(&k.manager, DISCOUNT_LIMIT_KEY, "15").unwrap();
    assert_eq!(k.pos.setting(DISCOUNT_LIMIT_KEY).unwrap().as_deref(), Some("15"));

    // Staff over Quinn's limit: asked for a PIN, approved with Quinn's.
    let o = open(&mut k);
    let line = o.items[0].id;
    let off = |c| ItemUpdate { qty: 2, discount_cents: c, note: None };
    k.pos.update_item(&k.staff, line, &off(300)).unwrap();
    assert_eq!(k.pos.update_item(&k.staff, line, &off(400)).unwrap_err().to_string(), "a discount over 15% needs a manager PIN");
    let quinn = k.pos.pin_owner_for("4646", Some(k.staff.user_id)).unwrap();
    assert_eq!(k.pos.update_item_approved(&k.staff, Some(&quinn), line, &off(400)).unwrap().items[0].discount_cents, 400);
}

#[test]
fn staff_add_a_dish_not_on_the_menu_only_with_a_managers_pin() {
    let mut k = setup();
    let o = open(&mut k);
    let err = k.pos.add_open_item(&k.staff, o.id, "Lobster", 1, 1, None, false).unwrap_err();
    assert_eq!(err.to_string(), "a dish not on the menu needs a manager PIN");
    let own = k.pos.pin_owner_for("2222", Some(k.staff.user_id)).unwrap();
    let err = k.pos.add_open_item_approved(&k.staff, Some(&own), o.id, "Lobster", 1, 1, None, false).unwrap_err();
    assert_eq!(err.to_string(), "a dish not on the menu needs a manager's or admin's PIN");
    assert_eq!(k.pos.get_order(&k.staff, o.id).unwrap().items.len(), 1);
    assert_eq!(audits(&k.pos, "order.open_item"), 0);

    let quinn = k.pos.pin_owner_for("4646", Some(k.staff.user_id)).unwrap();
    let o2 = k.pos.add_open_item_approved(&k.staff, Some(&quinn), o.id, "Extra rice", 150, 1, None, false).unwrap();
    assert_eq!(o2.items.len(), 2);
    let (who, detail) = last_audit(&k.pos, "order.open_item").unwrap();
    assert_eq!(who, Some(k.staff.user_id));
    assert!(detail.ends_with(&format!("(PIN of Quinn #{}, manager)", k.manager.user_id)), "{detail}");
    // Managers and admins are never asked.
    k.pos.add_open_item(&k.manager, o.id, "Bread", 200, 1, None, false).unwrap();
    k.pos.add_open_item(&k.admin, o.id, "Water", 100, 1, None, false).unwrap();
    assert!(last_audit(&k.pos, "order.open_item").unwrap().1.ends_with("(no PIN, admin)"));
}

#[test]
fn staff_pay_by_other_only_on_delivery_app_orders_or_with_a_managers_pin() {
    let mut k = setup();
    let other = PaymentInput { method: PaymentMethod::Other, amount_cents: None, tendered_cents: 0, tip_cents: 0, items: None };
    let o = open(&mut k);
    let err = k.pos.add_payment(&k.staff, o.id, &other).unwrap_err();
    assert_eq!(err.to_string(), "a payment by Other (not a delivery app) needs a manager PIN");
    assert!(k.pos.pay_order(&k.staff, o.id, PaymentMethod::Other, 0).is_err());
    let own = k.pos.pin_owner_for("2222", Some(k.staff.user_id)).unwrap();
    let err = k.pos.add_payment_approved(&k.staff, Some(&own), o.id, &other).unwrap_err();
    assert_eq!(err.to_string(), "a payment by Other (not a delivery app) needs a manager's or admin's PIN");
    assert_eq!(k.pos.get_order(&k.staff, o.id).unwrap().status, "open");

    let quinn = k.pos.pin_owner_for("4646", Some(k.staff.user_id)).unwrap();
    let paid = k.pos.add_payment_approved(&k.staff, Some(&quinn), o.id, &other).unwrap();
    assert_eq!(paid.status, "paid");
    let (who, detail) = last_audit(&k.pos, "order.payment_other").unwrap();
    assert_eq!(who, Some(k.staff.user_id));
    assert!(detail.ends_with(&format!("(PIN of Quinn #{}, manager)", k.manager.user_id)), "{detail}");

    // Managers are never asked; cash and card stay free for staff.
    let o = open(&mut k);
    assert_eq!(k.pos.add_payment(&k.manager, o.id, &other).unwrap().status, "paid");
    let o = open(&mut k);
    assert_eq!(k.pos.pay_order(&k.staff, o.id, PaymentMethod::Card, 0).unwrap().status, "paid");

    // A delivery app's order: Other is the app, free for staff.
    let uber = k.pos.list_channels(&k.staff).unwrap()[0].clone();
    let d = k
        .pos
        .open_order(&k.staff, &NewOrder { order_type: OrderType::Delivery, table_id: None, channel_id: Some(uber.id), details: Default::default() })
        .unwrap();
    k.pos.add_item(&k.staff, d.id, k.curry.id, 1, None, None, &[]).unwrap();
    assert_eq!(k.pos.pay_order(&k.staff, d.id, PaymentMethod::Other, 0).unwrap().status, "paid");
}
