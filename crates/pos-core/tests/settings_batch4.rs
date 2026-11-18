//! Settings batch 4: start screen, order types, discount buttons and the
//! staff discount limit, receipt options, auto-logout warning, button size.

use pos_core::*;

struct Shop {
    pos: Pos,
    admin: Session,
    staff: Session,
    curry: Product,
}

fn setup() -> Shop {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.set_setting(&admin, "prices_include_tax", "0").unwrap();
    pos.set_setting(&admin, "store_name", "Demo Bistro").unwrap();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    pos.create_user(&admin, "Mia", Role::Manager, "3333").unwrap();
    let staff = pos.login("2222").unwrap();
    let curry = pos
        .create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: "Curry".into(), price_cents: 1000,
            cost_cents: 0, tax_rate_bp: 2000, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap();
    Shop { pos, admin, staff, curry }
}

fn takeaway(pos: &mut Pos, s: &Session) -> Order {
    pos.open_order(s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap()
}

/// An order with 2 x Curry (20.00) and the line's id.
fn two_curries(shop: &mut Shop) -> (Order, i64) {
    let s = shop.staff.clone();
    let o = takeaway(&mut shop.pos, &s);
    let o = shop.pos.add_item(&s, o.id, shop.curry.id, 2, None, None, &[]).unwrap();
    let line = o.items[0].id;
    (o, line)
}

fn disc(qty: i64, cents: i64) -> ItemUpdate {
    ItemUpdate { qty, discount_cents: cents, note: None }
}

#[test]
fn settings_with_a_fixed_shape_are_checked() {
    let shop = setup();
    let (pos, a) = (&shop.pos, &shop.admin);
    for (key, good, bad) in [
        ("start_screen", "takeaway", "kitchen"),
        ("start_screen", "orders", "Tables"),
        ("show_takeaway", "0", "no"),
        ("show_delivery", "1", ""),
        ("button_size", "large", "huge"),
        ("auto_logout_warn_secs", "10", "121"),
        ("auto_logout_warn_secs", "0", "-1"),
        ("discount_staff_max_pct", "", "101"),
        ("discount_staff_max_pct", "15", "12.5"),
        ("discount_presets", "5, 10,15,20", "5,0"),
        ("discount_presets", "", "1,2,3,4,5,6,7,8,9"),
    ] {
        pos.set_setting(a, key, good).unwrap_or_else(|e| panic!("{key}={good:?}: {e}"));
        assert!(matches!(pos.set_setting(a, key, bad), Err(PosError::Validation(_))), "{key}={bad:?} should be refused");
        assert_eq!(pos.setting(key).unwrap().as_deref(), Some(good), "a refused value leaves the old one");
    }
    assert_eq!(discount_presets("5, 10,15%").unwrap(), vec![5, 10, 15]);
    // Other keys stay free text.
    pos.set_setting(a, "store_name", "anything at all").unwrap();
    // These are admin settings, not kitchen ones.
    for key in ["start_screen", "show_takeaway", "button_size", "discount_presets", DISCOUNT_LIMIT_KEY, "auto_logout_warn_secs"] {
        assert!(matches!(pos.set_setting(&shop.staff, key, "1"), Err(PosError::Forbidden)), "{key}");
    }
}

#[test]
fn hidden_order_types_still_work_on_the_server() {
    let mut shop = setup();
    for key in ["show_takeaway", "show_collection", "show_delivery"] {
        shop.pos.set_setting(&shop.admin, key, "0").unwrap();
    }
    // Only the new-order buttons hide: orders of every type can still be opened and paid.
    let s = shop.staff.clone();
    let o = takeaway(&mut shop.pos, &s);
    shop.pos.add_item(&s, o.id, shop.curry.id, 1, None, None, &[]).unwrap();
    assert_eq!(shop.pos.pay_order(&s, o.id, PaymentMethod::Card, 0).unwrap().status, "paid");
}

#[test]
fn without_a_limit_staff_discounts_work_as_before() {
    let mut shop = setup();
    let (_, line) = two_curries(&mut shop);
    let s = shop.staff.clone();
    let o = shop.pos.update_item(&s, line, &disc(2, 1500)).unwrap();
    assert_eq!(o.discount_cents, 1500);
    // An empty limit is no limit too.
    shop.pos.set_setting(&shop.admin, DISCOUNT_LIMIT_KEY, "").unwrap();
    assert_eq!(shop.pos.update_item(&s, line, &disc(2, 2000)).unwrap().discount_cents, 2000);
}

#[test]
fn staff_need_a_manager_pin_over_the_limit() {
    let mut shop = setup();
    shop.pos.set_setting(&shop.admin, DISCOUNT_LIMIT_KEY, "10").unwrap();
    let (_, line) = two_curries(&mut shop);
    let s = shop.staff.clone();

    // Up to 10% of 20.00 is fine.
    assert_eq!(shop.pos.update_item(&s, line, &disc(2, 200)).unwrap().discount_cents, 200);
    // 2.01 is over: refused, and the line keeps its discount.
    let err = shop.pos.update_item(&s, line, &disc(2, 201)).unwrap_err();
    assert!(err.to_string().contains("needs a manager PIN"), "{err}");
    assert_eq!(shop.pos.get_order(&s, shop.pos.order_of_item(line).unwrap()).unwrap().discount_cents, 200);

    // Another staff member's PIN cannot approve it.
    let other = shop.pos.pin_owner_for("2222", Some(s.user_id)).unwrap();
    let err = shop.pos.update_item_approved(&s, Some(&other), line, &disc(2, 500)).unwrap_err();
    assert!(err.to_string().contains("manager's or admin's PIN"), "{err}");

    // A manager's PIN approves it, through the kitchen-aware path the app uses.
    let mia = shop.pos.pin_owner_for("3333", Some(s.user_id)).unwrap();
    let (o, _) = shop.pos.update_item_for_kitchen_approved(&s, Some(&mia), line, &disc(2, 500)).unwrap();
    assert_eq!(o.discount_cents, 500);

    // Lowering it again, or adding to the line, needs nobody.
    assert_eq!(shop.pos.update_item(&s, line, &disc(2, 400)).unwrap().discount_cents, 400);
    assert_eq!(shop.pos.update_item(&s, line, &disc(3, 400)).unwrap().discount_cents, 400);
    // Fewer dishes with the same money off is a bigger share: 4.00 of 10.00 is 40%.
    assert!(shop.pos.update_item(&s, line, &disc(1, 400)).is_err());
    // Taking it off is always fine.
    assert_eq!(shop.pos.update_item(&s, line, &disc(1, 0)).unwrap().discount_cents, 0);
}

#[test]
fn managers_and_admins_are_not_limited() {
    let mut shop = setup();
    shop.pos.set_setting(&shop.admin, DISCOUNT_LIMIT_KEY, "0").unwrap();
    let (_, line) = two_curries(&mut shop);
    let s = shop.staff.clone();
    // 0%: staff cannot give any discount alone.
    assert!(shop.pos.update_item(&s, line, &disc(2, 1)).is_err());
    let mia = shop.pos.login("3333").unwrap();
    assert_eq!(shop.pos.update_item(&mia, line, &disc(2, 1000)).unwrap().discount_cents, 1000);
    let admin = shop.admin.clone();
    assert_eq!(shop.pos.update_item(&admin, line, &disc(2, 2000)).unwrap().discount_cents, 2000);
    // Still never more than the line.
    assert!(shop.pos.update_item(&admin, line, &disc(2, 2001)).is_err());
}

#[test]
fn receipt_can_show_the_tax_breakdown_and_hide_who_served() {
    let mut shop = setup();
    let (o, _) = two_curries(&mut shop);
    let admin = shop.admin.clone();

    // Default: who served, no tax table (the layout as before).
    let cfg = shop.pos.receipt_settings().unwrap();
    assert!(!cfg.tax_breakdown && cfg.show_staff);
    let plain = shop.pos.receipt(&admin, o.id).unwrap().to_text(42);
    assert!(plain.contains("Served by Sam") && !plain.contains("GROSS"), "{plain}");

    let mut cfg = cfg;
    cfg.tax_breakdown = true;
    cfg.show_staff = false;
    shop.pos.set_receipt_settings(&admin, &cfg).unwrap();
    let r = shop.pos.receipt(&admin, o.id).unwrap();
    let t = r.to_text(42);
    assert!(!t.contains("Served by"), "{t}");
    for needle in ["TAX", "NET", "VAT", "GROSS", "A 20%", "£20.00", "£4.00", "£24.00"] {
        assert!(t.contains(needle), "{needle} missing in\n{t}");
    }
    // The tax table comes after the total and before the footer.
    assert!(t.find("TOTAL").unwrap() < t.find("GROSS").unwrap());
    assert!(t.find("GROSS").unwrap() < t.find("Thank you!").unwrap());
    // The staff copy always names who served.
    assert!(r.staff_to_text(42).contains("Served by Sam"));

    // Saved settings from before this version still load, with the defaults.
    let old: ReceiptSettings = serde_json::from_str(r#"{"kind":"network","host":"192.168.1.60","port":9100,"width":32,"footer":"Bye"}"#).unwrap();
    assert!(!old.tax_breakdown && old.show_staff);
}

#[test]
fn tax_inside_prices_says_so_under_the_breakdown() {
    let mut shop = setup();
    shop.pos.set_setting(&shop.admin, "prices_include_tax", "1").unwrap();
    let (o, _) = two_curries(&mut shop);
    let admin = shop.admin.clone();
    let mut cfg = shop.pos.receipt_settings().unwrap();
    cfg.tax_breakdown = true;
    shop.pos.set_receipt_settings(&admin, &cfg).unwrap();
    let t = shop.pos.receipt(&admin, o.id).unwrap().to_text(42);
    // 20.00 at 20% inside the price: 16.67 + 3.33.
    assert!(t.contains("£16.67") && t.contains("£3.33") && t.contains("Prices include tax"), "{t}");
    assert!(!t.lines().any(|l| l.starts_with("Tax ")), "no tax line in the sum when it is inside the prices\n{t}");
}

#[test]
fn a_discount_at_the_limit_rounds_to_the_nearest_penny() {
    let mut shop = setup();
    let (admin, s) = (shop.admin.clone(), shop.staff.clone());
    shop.pos.set_setting(&admin, "discount_staff_max_pct", "15").unwrap();
    let fries = shop.pos
        .create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: "Fries".into(), price_cents: 350,
            cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap();
    let o = takeaway(&mut shop.pos, &s);
    let o = shop.pos.add_item(&s, o.id, fries.id, 1, None, None, &[]).unwrap();
    let line = o.items[0].id;
    // 15% of 3.50 is 52.5p: the till shows 53p, and that is within the limit.
    assert_eq!(shop.pos.update_item(&s, line, &disc(1, 53)).unwrap().discount_cents, 53);
    assert!(shop.pos.update_item(&s, line, &disc(1, 54)).is_err());
}

#[test]
fn closing_the_day_drops_orders_left_empty() {
    let mut shop = setup();
    let (admin, s) = (shop.admin.clone(), shop.staff.clone());
    let empty = takeaway(&mut shop.pos, &s);
    assert_eq!(shop.pos.business_day(&admin).unwrap().open_orders, 0, "an empty order is not counted as open");
    let (real, _) = two_curries(&mut shop);
    assert_eq!(shop.pos.business_day(&admin).unwrap().open_orders, 1);
    assert!(shop.pos.close_day(&admin).is_err(), "a real open order still blocks the day");
    shop.pos.cancel_order(&admin, real.id, "test").unwrap();
    shop.pos.close_day(&admin).unwrap();
    let e = shop.pos.get_order(&admin, empty.id).unwrap();
    assert_eq!((e.status.as_str(), e.close_reason.as_deref()), ("cancelled", Some("empty")));
}

#[test]
fn your_own_pin_does_not_reset_wrong_guesses_at_someone_elses() {
    let mut shop = setup();
    let s = shop.staff.clone();
    let mut locked = false;
    for _ in 0..10 {
        if let Err(PosError::LockedOut(_)) = shop.pos.pin_owner_for("9999", Some(s.user_id)) {
            locked = true;
            break;
        }
        let _ = shop.pos.pin_owner_for("2222", Some(s.user_id));
    }
    assert!(locked, "alternating with your own PIN must still lock out");
}

#[test]
fn big_amounts_in_the_tax_table_still_fit_narrow_paper() {
    let mut shop = setup();
    let (o, line) = two_curries(&mut shop);
    let admin = shop.admin.clone();
    // 1000 + 200 curries: £12,000.00 net, £2,400.00 VAT, £14,400.00 gross.
    shop.pos.update_item(&admin, line, &disc(1000, 0)).unwrap();
    shop.pos.add_item(&admin, o.id, shop.curry.id, 200, None, None, &[]).unwrap();
    let mut cfg = shop.pos.receipt_settings().unwrap();
    cfg.tax_breakdown = true;
    shop.pos.set_receipt_settings(&admin, &cfg).unwrap();
    let r = shop.pos.receipt(&admin, o.id).unwrap();
    for width in [32, 42, 48] {
        let t = r.to_text(width);
        let start = t.find("GROSS").unwrap();
        let table: Vec<&str> = t[t[..start].rfind('\n').map_or(0, |i| i + 1)..].lines().take(5).collect();
        for l in &table {
            assert!(l.chars().count() <= width, "{width}: too wide: {l:?}\n{t}");
        }
        let total = table.iter().find(|l| l.starts_with("Total")).unwrap();
        // Amounts never run together.
        assert_eq!(total.split_whitespace().count(), 4, "{width}: {total:?}");
        assert!(total.contains("14400.00"), "{total:?}");
    }
}
