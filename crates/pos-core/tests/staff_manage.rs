//! Staff only sell. Everything else in Manage (reports, End of day, the
//! shift, the menu, the floor plan, staff, settings, backups, updates) is
//! refused on the server, while selling still works end to end.

use pos_core::*;

struct Kit {
    pos: Pos,
    admin: Session,
    manager: Session,
    staff: Session,
    curry: Product,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let curry = pos
        .create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: "Green curry".into(), price_cents: 900, cost_cents: 0, tax_rate_bp: 0,
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
    let staff = pos.login("2222").unwrap();
    let manager = pos.login("4646").unwrap();
    Kit { pos, admin, manager, staff, curry }
}

fn takeaway() -> NewOrder {
    NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: OrderDetails::default() }
}

fn forbidden<T: std::fmt::Debug>(r: Result<T>, what: &str) {
    assert!(matches!(r, Err(PosError::Forbidden)), "{what}: {r:?}");
}

#[test]
fn staff_hold_only_the_selling_permissions() {
    use Permission::*;
    let all = [
        Sell, AnyDiscount, ViewOwnOrders, ViewAllOrders, VoidOrder, CancelWithoutPin, ManageMenu, AdjustStock, ManageFloorPlan,
        ManageUsers, ViewReports, ManageShift, ManageSettings, KitchenSettings, Backup, Updates, Delete,
    ];
    let staff: Vec<_> = all.into_iter().filter(|&p| Role::Staff.can(p)).collect();
    assert_eq!(staff, [Sell, ViewOwnOrders]);
    // Managers keep reports and the shift, but never install updates.
    assert!(Role::Manager.can(ViewReports) && Role::Manager.can(ManageShift));
    assert!(!Role::Manager.can(Updates));
    assert!(all.into_iter().all(|p| Role::Admin.can(p)));
}

#[test]
fn staff_are_refused_reports_end_of_day_and_the_shift() {
    let k = setup();
    let sam = k.staff.clone();
    forbidden(k.pos.sales_summary(&sam, 0, i64::MAX), "sales_summary");
    forbidden(k.pos.top_products(&sam, 0, i64::MAX, 10), "top_products");
    forbidden(k.pos.close_day(&sam), "close_day");
    forbidden(k.pos.day_closes(&sam, 10), "day_closes");
    forbidden(k.pos.current_shift(&sam), "current_shift");
    forbidden(k.pos.open_shift(&sam, 10_000), "open_shift");
    forbidden(k.pos.list_shifts(&sam, 10), "list_shifts");
    // With a shift opened by a manager, staff still cannot move cash or close it.
    let sh = k.pos.open_shift(&k.manager, 10_000).unwrap();
    forbidden(k.pos.cash_move(&sam, "out", 500, "milk"), "cash_move");
    forbidden(k.pos.close_shift(&sam, 10_000, ""), "close_shift");
    forbidden(k.pos.shift_job(&sam, &sh), "shift_job");
    // Nothing changed: the manager closes it as before, and runs reports.
    assert!(k.pos.cash_move(&k.manager, "in", 500, "change").is_ok());
    assert!(k.pos.close_shift(&k.manager, 10_500, "").unwrap().closed_at.is_some());
    assert!(k.pos.sales_summary(&k.manager, 0, i64::MAX).is_ok());
    assert!(k.pos.close_day(&k.manager).is_ok());
}

#[test]
fn staff_are_refused_menu_floor_staff_and_settings_changes() {
    let mut k = setup();
    let sam = k.staff.clone();
    forbidden(k.pos.list_products(&sam, true, 10, 0), "list_products");
    forbidden(k.pos.set_starter(&sam, k.curry.id, true), "set_starter");
    forbidden(k.pos.adjust_stock(&sam, k.curry.id, 5, "delivery"), "adjust_stock");
    forbidden(k.pos.create_category(&sam, "Drinks", None, 0), "create_category");
    forbidden(k.pos.create_floor(&sam, "Garden"), "create_floor");
    forbidden(k.pos.list_users(&sam), "list_users");
    forbidden(k.pos.create_user(&sam, "Ann", Role::Staff, "5555"), "create_user");
    forbidden(k.pos.save_teams(&sam, &[]), "save_teams");
    forbidden(k.pos.set_setting(&sam, "store_name", "Mine"), "set_setting");
    forbidden(k.pos.set_setting(&sam, "kds_beep", "1"), "set_setting (kitchen)");
    forbidden(k.pos.set_receipt_settings(&sam, &k.pos.receipt_settings().unwrap()), "set_receipt_settings");
    forbidden(k.pos.backup_to(&sam, std::env::temp_dir().join("pos-staff-backup-never.db")), "backup_to");
}

#[test]
fn selling_and_the_kitchen_screen_still_work_for_staff() {
    let mut k = setup();
    let sam = k.staff.clone();
    let o = k.pos.open_order(&sam, &takeaway()).unwrap();
    k.pos.add_item(&sam, o.id, k.curry.id, 2, Some("no chilli"), None, &[]).unwrap();
    let sent = k.pos.send_to_kitchen(&sam, o.id).unwrap();
    assert_eq!(sent.len(), 1);
    let board = k.pos.kitchen_board(&sam, "all").unwrap();
    assert!(board.iter().any(|b| b.order_id == o.id));
    k.pos.kitchen_done(&sam, &[sent[0].id], true).unwrap();
    // Sold out from the order screen stays a selling action.
    k.pos.set_sold_out(&sam, k.curry.id, true).unwrap();
    k.pos.set_sold_out(&sam, k.curry.id, false).unwrap();
    let paid = k.pos.pay_order(&sam, o.id, PaymentMethod::Cash, 2_000).unwrap();
    assert_eq!(paid.status, "paid");
    // Their own orders are still theirs to see (receipts, reprints).
    assert!(k.pos.get_order(&sam, o.id).is_ok());
    assert!(k.pos.business_day(&sam).is_ok());
    // A payment with no shift open is fine.
    assert!(k.pos.current_shift(&k.admin).unwrap().is_none());
}
