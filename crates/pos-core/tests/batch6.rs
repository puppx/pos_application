//! Batch 6: History by business day, payment method names, kitchen screen
//! beep setting, sub-category "off" defaults, backups kept.

use chrono::{Local, NaiveDate, TimeZone};
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

fn pay(method: PaymentMethod, tendered: i64) -> PaymentInput {
    PaymentInput { method, amount_cents: None, tendered_cents: tendered, tip_cents: 0, items: None }
}

fn midnight(d: NaiveDate) -> i64 {
    Local.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest().unwrap().timestamp()
}

fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

// ------------------------------------------------------------ business day

#[test]
fn business_day_gives_history_its_previous_day_and_week() {
    let (mut pos, s, steak, table) = setup();
    // Before any close: calendar days around the business date.
    let day = pos.business_day(&s).unwrap();
    let d = date(&day.date);
    assert_eq!(day.since, midnight(d));
    assert_eq!(day.prev_since, midnight(d - chrono::Days::new(1)));
    assert_eq!(day.prev_date, None);
    assert_eq!(day.week_since, midnight(d - chrono::Days::new(6)));

    // An order in the open business day is listed from `since` on (History "Today").
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, steak.id, 1, None, None, &[]).unwrap();
    pos.add_payment(&s, o.id, &pay(PaymentMethod::Card, 0)).unwrap();
    let far = 4_102_444_800;
    assert_eq!(pos.list_orders(&s, day.since, far, 50, 0).unwrap().len(), 1);

    // Close twice: the last close is "Yesterday", both are in the last 7 days.
    let first = pos.close_day(&s).unwrap();
    let second = pos.close_day(&s).unwrap();
    let day = pos.business_day(&s).unwrap();
    assert_eq!(day.since, second.to);
    assert_eq!(day.prev_since, second.from);
    assert_eq!(day.prev_since, first.to);
    assert_eq!(day.prev_date.as_deref(), Some(second.business_date.as_str()));
    // No close older than a week: the days before the first close are calendar days.
    assert_eq!(day.week_since, first.from.min(midnight(date(&day.date) - chrono::Days::new(6))));
    assert!(day.week_since <= day.prev_since && day.prev_since <= day.since);
    // The order is no longer "Today" but is in "Last 7 days".
    assert!(pos.list_orders(&s, day.since, far, 50, 0).unwrap().is_empty());
    assert_eq!(pos.list_orders(&s, day.week_since, far, 50, 0).unwrap().len(), 1);
}

// ------------------------------------------------------------ payment names

#[test]
fn payment_names_change_only_what_is_shown() {
    let (mut pos, s, steak, table) = setup();
    assert_eq!(pos.payment_names().unwrap(), PaymentNames::default());
    pos.open_shift(&s, 0).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, steak.id, 1, None, None, &[]).unwrap();
    // 22.00: 10.00 by card, the rest in cash from 20.00.
    pos.add_payment(&s, o.id, &PaymentInput { amount_cents: Some(1000), ..pay(PaymentMethod::Card, 0) }).unwrap();
    let o = pos.add_payment(&s, o.id, &pay(PaymentMethod::Cash, 2000)).unwrap();
    assert_eq!(o.status, "paid");

    let r = pos.receipt(&s, o.id).unwrap();
    let (text, bytes, staff) = (r.to_text(42), r.to_escpos(42, false), r.staff_to_text(42));
    assert_eq!(r.payment[0].0, "Card");
    assert!(staff.contains("Paid: Cash"), "{staff}");

    // Empty names are the defaults: byte for byte the same receipt.
    pos.set_setting(&s, PAY_NAME_CASH_KEY, "").unwrap();
    pos.set_setting(&s, PAY_NAME_CARD_KEY, "  ").unwrap();
    let r = pos.receipt(&s, o.id).unwrap();
    assert_eq!(r.to_text(42), text);
    assert_eq!(r.to_escpos(42, false), bytes);
    assert_eq!(r.staff_to_text(42), staff);

    pos.set_setting(&s, PAY_NAME_CASH_KEY, "เงินสด").unwrap();
    pos.set_setting(&s, PAY_NAME_CARD_KEY, "QR / Card").unwrap();
    let r = pos.receipt(&s, o.id).unwrap();
    let labels: Vec<&str> = r.payment.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["QR / Card", "เงินสด", "  Change"]);
    let staff = r.staff_to_text(42);
    assert!(staff.contains("Paid: QR / Card") && staff.contains("Paid: เงินสด"), "{staff}");
    // Thai prints as a picture, the rest as before.
    assert_ne!(r.to_escpos(42, false), bytes);

    // Stored values and report keys stay "cash" / "card".
    let o = pos.get_order(&s, o.id).unwrap();
    let methods: Vec<&str> = o.payments.iter().map(|p| p.method.as_str()).collect();
    assert_eq!(methods, ["card", "cash"]);
    let day = pos.business_day(&s).unwrap();
    let sum = pos.sales_summary(&s, day.since, 4_102_444_800).unwrap();
    let mut keys: Vec<&str> = sum.by_payment.iter().map(|g| g.key.as_str()).collect();
    keys.sort();
    assert_eq!(keys, ["card", "cash"]);

    // Shift report and end of day use the names.
    let sh = pos.current_shift(&s).unwrap().unwrap();
    let t = pos.shift_text(&sh).unwrap();
    assert!(t.contains("เงินสด sales") && t.contains("QR / Card"), "{t}");
    assert!(!t.contains("Cash sales"), "{t}");
    let close = pos.close_day(&s).unwrap();
    let t = pos.day_close_text(&close).unwrap();
    assert!(t.contains("เงินสด (1)") && t.contains("QR / Card (1)"), "{t}");

    // Too long or on two lines: refused.
    assert!(pos.set_setting(&s, PAY_NAME_CASH_KEY, &"x".repeat(MAX_PAY_NAME + 1)).is_err());
    assert!(pos.set_setting(&s, PAY_NAME_CARD_KEY, "QR\nCard").is_err());
    assert!(pos.set_setting(&s, PAY_NAME_CARD_KEY, &"ก".repeat(MAX_PAY_NAME)).is_ok());
}

#[test]
fn default_shift_and_day_reports_are_unchanged() {
    let (mut pos, s, steak, table) = setup();
    pos.open_shift(&s, 0).unwrap();
    let o = pos.open_order(&s, &dine_in(table)).unwrap();
    pos.add_item(&s, o.id, steak.id, 1, None, None, &[]).unwrap();
    pos.add_payment(&s, o.id, &pay(PaymentMethod::Cash, 2200)).unwrap();
    let sh = pos.current_shift(&s).unwrap().unwrap();
    let t = pos.shift_text(&sh).unwrap();
    assert!(t.contains("Cash sales") && t.lines().any(|l| l.starts_with("Card ")), "{t}");
    let close = pos.close_day(&s).unwrap();
    let t = pos.day_close_text(&close).unwrap();
    assert!(t.lines().any(|l| l.starts_with("Cash (1) ")), "{t}");
}

// ------------------------------------------------------------ kitchen screen beep

#[test]
fn managers_switch_the_kitchen_beep() {
    let (mut pos, s, _, _) = setup();
    pos.create_user(&s, "Mia", Role::Manager, "4321").unwrap();
    let m = pos.login("4321").unwrap();
    pos.set_setting(&m, "kds_beep", "1").unwrap();
    assert_eq!(pos.setting("kds_beep").unwrap().as_deref(), Some("1"));
    assert!(pos.set_setting(&m, "kds_beep", "loud").is_err());
}

// ------------------------------------------------------------ sub-category defaults

#[test]
fn a_sub_category_can_switch_off_its_main_categorys_defaults() {
    let (mut pos, s, _, _) = setup();
    let drinks = pos.create_category(&s, "Drinks", None, 1).unwrap();
    pos.set_category_drink(&s, drinks.id, true, false).unwrap();
    let food = pos.create_category(&s, "Food", None, 2).unwrap();
    let hot = pos.create_category(&s, "Hot drinks", None, 3).unwrap();
    assert_eq!(hot.flags_off, 0);

    // Only a sub-category can switch something off.
    assert!(pos.set_category_flags_off(&s, hot.id, FLAG_OFF_DRINK).is_err());
    pos.set_category_parent(&s, hot.id, Some(drinks.id)).unwrap();
    let hot = pos.set_category_flags_off(&s, hot.id, FLAG_OFF_DRINK | FLAG_OFF_STARTER).unwrap();
    assert_eq!(hot.flags_off, FLAG_OFF_DRINK | FLAG_OFF_STARTER);
    assert!(pos.set_category_flags_off(&s, hot.id, 16).is_err());
    assert!(pos.set_category_flags_off(&s, hot.id, -1).is_err());
    assert!(pos.set_category_flags_off(&s, food.id, 0).is_ok());
    let listed = pos.list_categories(&s, true).unwrap().into_iter().find(|c| c.id == hot.id).unwrap();
    assert_eq!(listed.flags_off, FLAG_OFF_DRINK | FLAG_OFF_STARTER);

    // Kept in a menu backup and restored.
    let backup = pos.export_menu(&s).unwrap();
    let json = serde_json::to_string(&backup).unwrap();
    pos.import_menu(&s, &serde_json::from_str(&json).unwrap()).unwrap();
    let hot = pos.list_categories(&s, true).unwrap().into_iter().find(|c| c.name == "Hot drinks").unwrap();
    assert_eq!(hot.flags_off, FLAG_OFF_DRINK | FLAG_OFF_STARTER);
    assert!(hot.parent_id.is_some());

    // A main category again: nothing to switch off.
    let hot = pos.set_category_parent(&s, hot.id, None).unwrap();
    assert_eq!(hot.flags_off, 0);
    // Deleting the main category makes its sub-categories main ones, with nothing off.
    let drinks = pos.list_categories(&s, true).unwrap().into_iter().find(|c| c.name == "Drinks").unwrap();
    pos.set_category_parent(&s, hot.id, Some(drinks.id)).unwrap();
    pos.set_category_flags_off(&s, hot.id, FLAG_OFF_DESSERT).unwrap();
    pos.delete_category(&s, drinks.id).unwrap();
    let hot = pos.list_categories(&s, true).unwrap().into_iter().find(|c| c.id == hot.id).unwrap();
    assert_eq!((hot.parent_id, hot.flags_off), (None, 0));
}

#[test]
fn old_menu_backups_without_flags_off_still_restore() {
    let (mut pos, s, _, _) = setup();
    let json = r#"{"format":"pos-menu","version":1,"exported_at":0,
        "categories":[{"key":1,"name":"Drinks","drink":true},{"key":2,"name":"Hot","parent":1}],
        "option_groups":[],"items":[]}"#;
    pos.import_menu(&s, &serde_json::from_str(json).unwrap()).unwrap();
    let cats = pos.list_categories(&s, true).unwrap();
    assert!(cats.iter().all(|c| c.flags_off == 0));
    // A backup with nothing switched off has no flags_off field at all.
    let out = serde_json::to_string(&pos.export_menu(&s).unwrap()).unwrap();
    assert!(!out.contains("flags_off"), "{out}");
}

// ------------------------------------------------------------ backups kept

#[test]
fn backups_kept_defaults_to_fourteen_and_is_checked() {
    let (pos, s, _, _) = setup();
    assert_eq!(pos.backups_kept().unwrap(), BACKUPS_KEPT_DEFAULT);
    assert_eq!(BACKUPS_KEPT_DEFAULT, 14);
    pos.set_setting(&s, BACKUPS_KEPT_KEY, "30").unwrap();
    assert_eq!(pos.backups_kept().unwrap(), 30);
    for bad in ["2", "91", "ten", "1.5", "-3"] {
        assert!(pos.set_setting(&s, BACKUPS_KEPT_KEY, bad).is_err(), "{bad}");
    }
    pos.set_setting(&s, BACKUPS_KEPT_KEY, "").unwrap();
    assert_eq!(pos.backups_kept().unwrap(), 14);
    pos.set_setting(&s, BACKUPS_KEPT_KEY, "3").unwrap();
    assert_eq!(pos.backups_kept().unwrap(), 3);
}
