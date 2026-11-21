//! The till's Cancel button (`Pos::cancel_order_confirmed`): admins and
//! managers confirm with no PIN, staff type a PIN as before.

use pos_core::*;

struct Kit {
    pos: Pos,
    admin: Session,
    manager: Session,
    staff: Session,
    soup: Product,
    curry: Product,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let item = |pos: &mut Pos, name: &str| {
        pos.create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: name.into(), price_cents: 800, cost_cents: 0, tax_rate_bp: Some(0),
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap()
    };
    let soup = item(&mut pos, "Tom yum soup");
    let curry = item(&mut pos, "Green curry");
    pos.set_kitchen_printer(&admin, &PrinterConfig {
        target: PrinterTarget::File { path: std::env::temp_dir().join("pos-cancel-without-pin-unused.bin").display().to_string() },
        copy_delay_secs: 0, app_font: false, ..Default::default()
    })
    .unwrap();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
    let staff = pos.login("2222").unwrap();
    let manager = pos.login("4646").unwrap();
    Kit { pos, admin, manager, staff, soup, curry }
}

fn takeaway() -> NewOrder {
    NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: OrderDetails::default() }
}

/// An order with a curry on it, sent to the kitchen when `sent`.
fn order_with_curry(k: &mut Kit, by: &Session, sent: bool) -> i64 {
    let o = k.pos.open_order(by, &takeaway()).unwrap();
    k.pos.add_item(by, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    if sent {
        k.pos.send_to_kitchen(by, o.id).unwrap();
    }
    o.id
}

fn status(k: &Kit, id: i64) -> String {
    k.pos.get_order(&k.admin, id).unwrap().status
}

#[test]
fn only_admins_and_managers_may_cancel_without_a_pin() {
    assert!(Role::Admin.can(Permission::CancelWithoutPin));
    assert!(Role::Manager.can(Permission::CancelWithoutPin));
    assert!(!Role::Staff.can(Permission::CancelWithoutPin));
}

#[test]
fn a_manager_and_an_admin_cancel_sent_and_unsent_orders_with_no_pin() {
    let mut k = setup();
    for who in [k.manager.clone(), k.admin.clone()] {
        for sent in [false, true] {
            // No PIN at all, and an empty one (what a form sends), both work.
            for pin in [None, Some(""), Some("  ")] {
                let id = order_with_curry(&mut k, &who, sent);
                let job = k.pos.cancel_order_confirmed(&who, id, "", pin).unwrap();
                assert_eq!(job.is_some(), sent, "{:?} sent={sent} pin={pin:?}", who.role);
                assert_eq!(status(&k, id), "cancelled");
            }
        }
    }
}

#[test]
fn staff_with_no_pin_are_refused_once_sent_and_nothing_changes() {
    let mut k = setup();
    let sam = k.staff.clone();
    let id = order_with_curry(&mut k, &sam, true);
    for pin in [None, Some("")] {
        assert!(matches!(k.pos.cancel_order_confirmed(&sam, id, "", pin), Err(PosError::Forbidden)), "pin={pin:?}");
    }
    assert_eq!(status(&k, id), "open");
}

#[test]
fn staff_cancel_a_mistake_order_nothing_sent_or_paid_with_just_a_confirm() {
    let mut k = setup();
    let sam = k.staff.clone();
    for pin in [None, Some("")] {
        let id = order_with_curry(&mut k, &sam, false);
        assert!(k.pos.cancel_order_confirmed(&sam, id, "", pin).unwrap().is_none());
        assert_eq!(status(&k, id), "cancelled");
    }
}

#[test]
fn staff_cancel_an_unsent_order_with_their_own_pin() {
    let mut k = setup();
    let sam = k.staff.clone();
    let id = order_with_curry(&mut k, &sam, false);
    assert!(matches!(k.pos.cancel_order_confirmed(&sam, id, "", Some("9999")), Err(PosError::InvalidPin)));
    assert_eq!(status(&k, id), "open");
    assert!(k.pos.cancel_order_confirmed(&sam, id, "", Some("2222")).unwrap().is_none());
    assert_eq!(status(&k, id), "cancelled");
}

#[test]
fn staff_need_a_manager_or_admin_pin_once_the_kitchen_has_the_order() {
    let mut k = setup();
    let sam = k.staff.clone();
    let id = order_with_curry(&mut k, &sam, true);
    // Their own PIN is not enough.
    assert!(matches!(k.pos.cancel_order_confirmed(&sam, id, "", Some("2222")), Err(PosError::Forbidden)));
    assert_eq!(status(&k, id), "open");
    // A manager's PIN approves it, and the kitchen is told.
    let job = k.pos.cancel_order_confirmed(&sam, id, "", Some("4646")).unwrap().expect("a CANCELLED ticket");
    assert!(String::from_utf8_lossy(&job.bytes()).contains("CANCELLED"));
    assert_eq!(status(&k, id), "cancelled");
    // So does an admin's.
    let id = order_with_curry(&mut k, &sam, true);
    assert!(k.pos.cancel_order_confirmed(&sam, id, "", Some("1111")).unwrap().is_some());
    assert_eq!(status(&k, id), "cancelled");
}

#[test]
fn a_manager_cancelling_with_no_pin_still_prints_cancelled_for_what_is_cooking() {
    let mut k = setup();
    let quinn = k.manager.clone();
    let o = k.pos.open_order(&quinn, &takeaway()).unwrap();
    k.pos.add_item(&quinn, o.id, k.soup.id, 1, None, None, &[]).unwrap();
    k.pos.add_item(&quinn, o.id, k.curry.id, 2, None, None, &[]).unwrap();
    let sent = k.pos.send_to_kitchen(&quinn, o.id).unwrap();
    // The soup is done on the kitchen screen; the curry is still cooking.
    let soup_line = sent.iter().find(|i| i.product_id == k.soup.id).unwrap().id;
    k.pos.kitchen_done(&quinn, &[soup_line], true).unwrap();

    let job = k.pos.cancel_order_confirmed(&quinn, o.id, "", None).unwrap().expect("a CANCELLED ticket");
    let text = String::from_utf8_lossy(&job.bytes()).to_string();
    assert!(text.contains("CANCELLED") && text.contains("2 x Green curry"), "{text}");
    assert!(!text.contains("Tom yum"), "done dishes are not cancelled: {text}");
    assert_eq!(status(&k, o.id), "cancelled");
    assert!(k.pos.kitchen_board(&k.admin, "all").unwrap().is_empty());
    // Closed already: refused, no ticket.
    assert!(k.pos.cancel_order_confirmed(&quinn, o.id, "", None).is_err());
}

#[test]
fn a_part_paid_order_is_still_refused_without_a_pin() {
    let mut k = setup();
    let quinn = k.manager.clone();
    let id = order_with_curry(&mut k, &quinn, false);
    let part = PaymentInput { method: PaymentMethod::Cash, amount_cents: Some(300), tendered_cents: 300, tip_cents: 0, items: None };
    k.pos.add_payment(&quinn, id, &part).unwrap();
    assert!(matches!(k.pos.cancel_order_confirmed(&quinn, id, "", None), Err(PosError::Validation(_))));
    assert_eq!(status(&k, id), "open");
}

#[test]
fn the_cancelled_ticket_for_a_whole_order_can_be_switched_off() {
    let mut k = setup();
    let (quinn, sam) = (k.manager.clone(), k.staff.clone());
    // Not set: printed, as before the setting.
    assert!(k.pos.prints_cancelled_order().unwrap());
    let id = order_with_curry(&mut k, &quinn, true);
    assert!(k.pos.cancel_order_confirmed(&quinn, id, "", None).unwrap().is_some());

    // A kitchen setting: managers may change it, staff may not; 1 or 0 only.
    assert!(matches!(k.pos.set_setting(&sam, PRINT_CANCELLED_ORDER_KEY, "0"), Err(PosError::Forbidden)));
    assert!(matches!(k.pos.set_setting(&quinn, PRINT_CANCELLED_ORDER_KEY, "off"), Err(PosError::Validation(_))));
    k.pos.set_setting(&quinn, PRINT_CANCELLED_ORDER_KEY, "0").unwrap();
    assert!(!k.pos.prints_cancelled_order().unwrap());

    // Off: cancelling works exactly the same, with nothing to print.
    let id = order_with_curry(&mut k, &quinn, true);
    assert!(k.pos.cancel_order_confirmed(&quinn, id, "", None).unwrap().is_none());
    assert_eq!(status(&k, id), "cancelled");
    let id = order_with_curry(&mut k, &sam, true);
    // Staff still need a manager's or admin's PIN once the kitchen has it.
    assert!(matches!(k.pos.cancel_order_confirmed(&sam, id, "", Some("2222")), Err(PosError::Forbidden)));
    assert!(k.pos.cancel_order_confirmed(&sam, id, "", Some("4646")).unwrap().is_none());
    assert_eq!(status(&k, id), "cancelled");
    let id = order_with_curry(&mut k, &quinn, true);
    assert!(k.pos.cancel_order_for_kitchen(&quinn, id, "").unwrap().is_none());
    assert_eq!(status(&k, id), "cancelled");
    assert!(k.pos.kitchen_board(&k.admin, "all").unwrap().is_empty());

    // Taking one sent dish off still tells the kitchen.
    let id = order_with_curry(&mut k, &quinn, true);
    let line = k.pos.get_order(&quinn, id).unwrap().items[0].id;
    let (_, job) = k.pos.update_item_for_kitchen(&quinn, line, &ItemUpdate { qty: 0, discount_cents: 0, note: None }).unwrap();
    assert!(String::from_utf8_lossy(&job.expect("a CANCELLED ticket").bytes()).contains("CANCELLED"));

    // Back on.
    k.pos.set_setting(&quinn, PRINT_CANCELLED_ORDER_KEY, "1").unwrap();
    let id = order_with_curry(&mut k, &quinn, true);
    let job = k.pos.cancel_order_confirmed(&quinn, id, "", None).unwrap().expect("a CANCELLED ticket");
    assert!(String::from_utf8_lossy(&job.bytes()).contains("CANCELLED"));
}
