//! PINs hashed before the database lock is taken (see `Pos::pin_hasher`).

use pos_core::*;

#[test]
fn a_pin_hashed_ahead_logs_in_and_approves_like_a_typed_one() {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let sam = pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    pos.create_user(&admin, "Mia", Role::Manager, "3333").unwrap();
    let hasher = pos.pin_hasher();
    let mut throttle = LoginThrottle::default();

    let s = pos.login_hashed(None, &hasher.hash("2222"), &mut throttle).unwrap();
    assert_eq!(s.user_id, sam.id);
    assert!(pos.login_users().unwrap().iter().any(|u| u.id == sam.id && u.pin_len == Some(4)));
    // Wrong or malformed PINs fail the same way and count towards the lock-out.
    for bad in ["9999", "12", "abcd", ""] {
        assert!(matches!(pos.login_hashed(None, &hasher.hash(bad), &mut throttle), Err(PosError::InvalidPin)), "{bad}");
    }
    assert!(matches!(pos.login_hashed(None, &hasher.hash("8888"), &mut throttle), Err(PosError::InvalidPin)));
    assert!(matches!(pos.login_hashed(None, &hasher.hash("2222"), &mut throttle), Err(PosError::LockedOut(_))));

    // A hash made with another database's salt (one restored meanwhile) is
    // not trusted: the PIN is hashed again here.
    let elsewhere = Pos::open_in_memory().unwrap().pin_hasher();
    let s = pos.login_hashed(Some(sam.id), &elsewhere.hash("2222"), &mut LoginThrottle::default()).unwrap();
    assert_eq!(s.user_id, sam.id);
    assert!(pos.login_hashed(None, &elsewhere.hash("1234"), &mut LoginThrottle::default()).is_err());

    // Approvals: a manager's PIN, hashed ahead or not.
    assert_eq!(pos.pin_owner_hashed(&hasher.hash("3333"), Some(s.user_id)).unwrap().role, Role::Manager);
    assert_eq!(pos.pin_owner_hashed(&HashedPin::later("3333"), Some(s.user_id)).unwrap().role, Role::Manager);
    assert!(matches!(pos.pin_owner_hashed(&hasher.hash("4444"), Some(s.user_id)), Err(PosError::InvalidPin)));
    // An order the kitchen has: staff need a PIN to cancel it (one with nothing
    // sent or paid is anyone's to cancel with a confirm, so it would prove nothing).
    let soup = pos
        .create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: "Soup".into(), price_cents: 500, cost_cents: 0, tax_rate_bp: Some(0),
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap();
    let o = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    pos.add_item(&s, o.id, soup.id, 1, None, None, &[]).unwrap();
    pos.send_to_kitchen(&s, o.id).unwrap();
    assert!(matches!(pos.cancel_order_confirmed_hashed(&s, o.id, "", None), Err(PosError::Forbidden)));
    // Sam's own PIN is not enough once it is in the kitchen; the manager's, hashed ahead, is.
    assert!(matches!(pos.cancel_order_confirmed_hashed(&s, o.id, "", Some(&hasher.hash("2222"))), Err(PosError::Forbidden)));
    pos.cancel_order_confirmed_hashed(&s, o.id, "", Some(&hasher.hash("3333"))).unwrap();
    assert_eq!(pos.get_order(&admin, o.id).unwrap().status, "cancelled");
}
