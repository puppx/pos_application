use pos_core::*;

#[test]
fn login_without_ok_once_every_pin_length_is_known() {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    assert_eq!(pos.auto_login_length().unwrap(), Some(4));
    let sam = pos.create_user(&admin, "Sam", Role::Staff, "123456").unwrap();
    assert_eq!(pos.auto_login_length().unwrap(), Some(6), "the longest PIN");
    // Switched off in Settings.
    pos.set_setting(&admin, "pin_auto_login", "0").unwrap();
    assert_eq!(pos.auto_login_length().unwrap(), None);
    pos.set_setting(&admin, "pin_auto_login", "1").unwrap();
    pos.reset_pin(&admin, sam.id, "2222").unwrap();
    assert_eq!(pos.auto_login_length().unwrap(), Some(4));
}
