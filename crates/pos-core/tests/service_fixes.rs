//! Cancelling a whole order in the kitchen, kitchen screen headings, staff
//! names and shared PINs.

use pos_core::*;

struct Kit {
    pos: Pos,
    admin: Session,
    soup: Product,
    curry: Product,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let item = |pos: &mut Pos, name: &str| {
        pos.create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: name.into(), price_cents: 800, cost_cents: 0, tax_rate_bp: 0,
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap()
    };
    let soup = item(&mut pos, "Tom yum soup");
    let curry = item(&mut pos, "Green curry");
    pos.set_kitchen_printer(&admin, &PrinterConfig {
        target: PrinterTarget::File { path: std::env::temp_dir().join("pos-service-fixes-unused.bin").display().to_string() },
        copy_delay_secs: 0, app_font: false, ..Default::default()
    })
    .unwrap();
    Kit { pos, admin, soup, curry }
}

fn order(kind: OrderType, name: Option<&str>) -> NewOrder {
    let details = OrderDetails { customer_name: name.map(str::to_string), customer_phone: Some("0123".into()), delivery_address: Some("1 High St".into()), ..Default::default() };
    NewOrder { order_type: kind, table_id: None, channel_id: None, details }
}

#[test]
fn cancelling_an_order_in_the_kitchen_prints_cancelled_for_what_is_still_cooking() {
    let mut k = setup();
    let (s, soup, curry) = (k.admin.clone(), k.soup.id, k.curry.id);
    let o = k.pos.open_order(&s, &order(OrderType::Takeaway, None)).unwrap();
    k.pos.add_item(&s, o.id, soup, 1, None, None, &[]).unwrap();
    k.pos.add_item(&s, o.id, curry, 2, None, None, &[]).unwrap();
    let sent = k.pos.send_to_kitchen(&s, o.id).unwrap();
    // The soup is already done on the kitchen screen; the curry is cooking.
    let soup_line = sent.iter().find(|i| i.product_id == soup).unwrap().id;
    k.pos.kitchen_done(&s, &[soup_line], true).unwrap();
    // A staff member may not cancel what the kitchen has: nothing prints, nothing changes.
    k.pos.create_user(&s, "Sam", Role::Staff, "2222").unwrap();
    let sam = k.pos.login("2222").unwrap();
    assert!(matches!(k.pos.cancel_order_for_kitchen(&sam, o.id, "left"), Err(PosError::Forbidden)));
    assert_eq!(k.pos.get_order(&s, o.id).unwrap().status, "open");

    let job = k.pos.cancel_order_for_kitchen(&s, o.id, "left").unwrap().expect("a CANCELLED ticket");
    let text = String::from_utf8_lossy(&job.bytes()).to_string();
    assert!(text.contains("CANCELLED") && text.contains("2 x Green curry"), "{text}");
    assert!(!text.contains("Tom yum"), "done dishes are not cancelled: {text}");
    assert_eq!(k.pos.get_order(&s, o.id).unwrap().status, "cancelled");
    assert!(k.pos.kitchen_board(&s, "all").unwrap().is_empty());
    // Closed already: refused, no ticket.
    assert!(k.pos.cancel_order_for_kitchen(&s, o.id, "").is_err());

    // Nothing sent yet: nothing to tell the kitchen.
    let o = k.pos.open_order(&s, &order(OrderType::Takeaway, None)).unwrap();
    k.pos.add_item(&s, o.id, soup, 1, None, None, &[]).unwrap();
    assert!(k.pos.cancel_order_for_kitchen(&sam, o.id, "").unwrap().is_none());
    assert_eq!(k.pos.get_order(&s, o.id).unwrap().status, "cancelled");
}

#[test]
fn kitchen_screen_cards_without_a_table_carry_the_order_number() {
    let mut k = setup();
    let s = k.admin.clone();
    let mut ids = Vec::new();
    for (kind, name) in [(OrderType::Takeaway, None), (OrderType::Takeaway, Some("Ann")), (OrderType::Collection, Some("Bob")), (OrderType::Delivery, Some("Cy"))] {
        let o = k.pos.open_order(&s, &order(kind, name)).unwrap();
        k.pos.add_item(&s, o.id, k.soup.id, 1, None, None, &[]).unwrap();
        k.pos.send_to_kitchen(&s, o.id).unwrap();
        ids.push(o.id);
    }
    // A delivery platform's order: its own reference, or our number without one.
    let grab = k.pos.list_channels(&s).unwrap()[0].clone();
    for r in [Some("A1B2"), None] {
        let mut new = order(OrderType::Delivery, None);
        new.channel_id = Some(grab.id);
        new.details.external_ref = r.map(str::to_string);
        let o = k.pos.open_order(&s, &new).unwrap();
        k.pos.add_item(&s, o.id, k.soup.id, 1, None, None, &[]).unwrap();
        k.pos.send_to_kitchen(&s, o.id).unwrap();
        ids.push(o.id);
    }
    let board = k.pos.kitchen_board(&s, "all").unwrap();
    let heading = |id: i64| board.iter().find(|b| b.order_id == id).unwrap().heading.clone();
    let ch = grab.name.to_uppercase();
    assert_eq!(heading(ids[0]), format!("TAKEAWAY #{}", ids[0]));
    assert_eq!(heading(ids[1]), format!("TAKEAWAY #{}", ids[1]));
    assert_eq!(heading(ids[2]), format!("COLLECTION #{}", ids[2]));
    assert_eq!(heading(ids[3]), format!("DELIVERY #{}", ids[3]));
    assert_eq!(heading(ids[4]), format!("{ch} A1B2"));
    assert_eq!(heading(ids[5]), format!("{ch} #{}", ids[5]));
    // The customer's name stays beside the heading, not in it.
    assert_eq!(board.iter().find(|b| b.order_id == ids[1]).unwrap().customer_name.as_deref(), Some("Ann"));
}

#[test]
fn two_active_staff_cannot_have_the_same_name() {
    let mut k = setup();
    let s = k.admin.clone();
    let sam = k.pos.create_user(&s, "Sam", Role::Staff, "2222").unwrap();
    for name in ["Sam", " sam ", "SAM", "owner"] {
        let err = k.pos.create_user(&s, name, Role::Staff, "3333").unwrap_err();
        assert!(matches!(&err, PosError::Validation(m) if m.contains("already called")), "{name}: {err}");
    }
    let mia = k.pos.create_user(&s, "Mia", Role::Manager, "4444").unwrap();
    // Renaming onto someone else's name: refused; keeping your own (any case): fine.
    assert!(matches!(k.pos.update_user(&s, mia.id, "sam", Role::Manager, true), Err(PosError::Validation(_))));
    assert_eq!(k.pos.update_user(&s, sam.id, "SAM", Role::Staff, true).unwrap().name, "SAM");
    // A name that only a deactivated user had is free again, but that user
    // cannot come back under it while it is taken.
    k.pos.update_user(&s, sam.id, "SAM", Role::Staff, false).unwrap();
    let new_sam = k.pos.create_user(&s, "Sam", Role::Staff, "5555").unwrap();
    assert!(k.pos.update_user(&s, sam.id, "SAM", Role::Staff, true).is_err());
    assert_eq!(k.pos.update_user(&s, sam.id, "Sam B", Role::Staff, true).unwrap().name, "Sam B");
    let names: Vec<String> = k.pos.login_users().unwrap().into_iter().map(|u| u.name).collect();
    assert_eq!(names, ["Mia", "Owner", "Sam", "Sam B"]);
    assert_eq!(new_sam.name, "Sam");
}

#[test]
fn a_shared_pin_counts_as_the_logged_in_user_when_it_is_theirs() {
    let mut k = setup();
    let s = k.admin.clone();
    let sam = k.pos.create_user(&s, "Sam", Role::Staff, "2222").unwrap();
    let mia = k.pos.create_user(&s, "Mia", Role::Manager, "2222").unwrap();
    // The editor can warn before a PIN is shared (sharing stays allowed).
    assert_eq!(k.pos.pin_shared(&s, "2222", None).unwrap(), ["Mia", "Sam"]);
    assert_eq!(k.pos.pin_shared(&s, "2222", Some(sam.id)).unwrap(), ["Mia"]);
    assert!(k.pos.pin_shared(&s, "7777", None).unwrap().is_empty());
    assert!(matches!(k.pos.pin_shared(&s, "12", None), Err(PosError::Validation(_))));
    let mut t = LoginThrottle::default();
    let mia_s = k.pos.login_as(Some(mia.id), "2222", &mut t).unwrap();
    assert!(matches!(k.pos.pin_shared(&mia_s, "2222", None), Err(PosError::Forbidden)));
    // Deactivated users do not count.
    k.pos.update_user(&s, sam.id, "Sam", Role::Staff, false).unwrap();
    assert_eq!(k.pos.pin_shared(&s, "2222", Some(mia.id)).unwrap(), Vec::<String>::new());
    k.pos.update_user(&s, sam.id, "Sam", Role::Staff, true).unwrap();

    // Mia approves her own cancel with her PIN on her own till: a manager.
    assert_eq!(k.pos.pin_owner_for("2222", Some(mia.id)).unwrap().user_id, mia.id);
    let o = k.pos.open_order(&mia_s, &order(OrderType::Takeaway, None)).unwrap();
    k.pos.add_item(&mia_s, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    k.pos.send_to_kitchen(&mia_s, o.id).unwrap();
    let approver = k.pos.pin_owner_for("2222", Some(mia.id)).unwrap();
    assert_eq!(approver.role, Role::Manager);
    assert!(k.pos.cancel_order_for_kitchen(&approver, o.id, "mistake").unwrap().is_some());
    // Anyone else's till (or none): still the one allowed the least.
    assert_eq!(k.pos.pin_owner_for("2222", Some(s.user_id)).unwrap().role, Role::Staff);
    assert_eq!(k.pos.pin_owner("2222").unwrap().role, Role::Staff);
    // The logged-in user's id never helps a PIN that is not theirs.
    assert!(matches!(k.pos.pin_owner_for("9999", Some(mia.id)), Err(PosError::InvalidPin)));
}
