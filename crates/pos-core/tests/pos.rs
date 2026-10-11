use pos_core::*;

fn setup() -> (Pos, Session) {
    let mut pos = Pos::open_in_memory().unwrap();
    assert!(pos.needs_setup().unwrap());
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    (pos, admin)
}

fn product(pos: &mut Pos, admin: &Session, name: &str, price: i64, tax_bp: i64, stock: i64) -> Product {
    pos.create_product(
        admin,
        NewProduct {
            sku: None,
            barcode: Some(format!("BC-{name}")),
            name: name.into(),
            price_cents: price,
            cost_cents: 0,
            tax_rate_bp: tax_bp,
            stock_qty: stock,
            track_stock: true,
        },
    )
    .unwrap()
}

#[test]
fn setup_only_once_and_pin_login() {
    let (mut pos, admin) = setup();
    assert_eq!(admin.role, Role::Admin);
    assert!(!pos.needs_setup().unwrap());
    assert!(matches!(pos.setup_first_admin("Evil", "2222"), Err(PosError::Forbidden)));

    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let s = pos.login("2222").unwrap();
    assert_eq!((s.name.as_str(), s.role), ("Sam", Role::Staff));
    assert!(matches!(pos.login("9999"), Err(PosError::InvalidPin)));
    assert!(matches!(pos.login("abc"), Err(PosError::InvalidPin)));
}

#[test]
fn pins_are_unique() {
    let (pos, admin) = setup();
    let err = pos.create_user(&admin, "Dup", Role::Staff, "1111").unwrap_err();
    assert!(matches!(err, PosError::Conflict(_)));
}

#[test]
fn lockout_after_repeated_failures() {
    let (mut pos, _) = setup();
    for _ in 0..5 {
        assert!(matches!(pos.login("0000"), Err(PosError::InvalidPin)));
    }
    // Locked even for the correct PIN.
    assert!(matches!(pos.login("1111"), Err(PosError::LockedOut(_))));
}

#[test]
fn staff_permissions() {
    let (mut pos, admin) = setup();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    assert!(matches!(pos.list_users(&staff), Err(PosError::Forbidden)));
    assert!(matches!(pos.sales_summary(&staff, 0, i64::MAX), Err(PosError::Forbidden)));
    let p = NewProduct {
        sku: None, barcode: None, name: "x".into(), price_cents: 1,
        cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: true,
    };
    assert!(matches!(pos.create_product(&staff, p), Err(PosError::Forbidden)));
    assert!(pos.search_products(&staff, "", 10).is_ok());
}

#[test]
fn cannot_remove_last_admin_and_deactivated_users_lose_access() {
    let (mut pos, admin) = setup();
    let err = pos.update_user(&admin, admin.user_id, "Owner", Role::Staff, true).unwrap_err();
    assert!(matches!(err, PosError::Validation(_)));

    let sam = pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    pos.update_user(&admin, sam.id, "Sam", Role::Staff, false).unwrap();
    assert!(matches!(pos.refresh_session(&staff), Err(PosError::Forbidden)));
    assert!(matches!(pos.login("2222"), Err(PosError::InvalidPin)));

    pos.reset_pin(&admin, sam.id, "3333").unwrap();
    pos.update_user(&admin, sam.id, "Sam", Role::Staff, true).unwrap();
    assert!(pos.login("3333").is_ok());
}

#[test]
fn sale_totals_stock_and_void() {
    let (mut pos, admin) = setup();
    let cola = product(&mut pos, &admin, "Cola", 150, 1500, 10); // 1.50, 15% tax
    let bread = product(&mut pos, &admin, "Bread", 250, 0, 5);

    let cart = [
        CartLine { product_id: cola.id, qty: 3, discount_cents: 50 },
        CartLine { product_id: bread.id, qty: 1, discount_cents: 0 },
    ];
    let q = pos.quote_sale(&admin, &cart).unwrap();
    assert_eq!(q.subtotal_cents, 700);
    assert_eq!(q.discount_cents, 50);
    assert_eq!(q.tax_cents, 60); // 15% of 400
    assert_eq!(q.total_cents, 710);

    assert!(pos.complete_sale(&admin, &cart, PaymentMethod::Cash, 700).is_err());
    let sale = pos.complete_sale(&admin, &cart, PaymentMethod::Cash, 1000).unwrap();
    assert_eq!(sale.total_cents, 710);
    assert_eq!(sale.change_cents, 290);
    assert_eq!(sale.items.len(), 2);
    assert_eq!(pos.get_product(&admin, cola.id).unwrap().stock_qty, 7);
    assert_eq!(pos.get_product(&admin, bread.id).unwrap().stock_qty, 4);

    let summary = pos.sales_summary(&admin, 0, i64::MAX).unwrap();
    assert_eq!((summary.sale_count, summary.total_cents), (1, 710));

    // Price changes do not alter recorded sales.
    let upd = ProductUpdate {
        sku: None, barcode: cola.barcode.clone(), name: "Cola".into(), price_cents: 999,
        cost_cents: 0, tax_rate_bp: 1500, track_stock: true, active: true,
    };
    pos.update_product(&admin, cola.id, upd).unwrap();
    assert_eq!(pos.get_sale(&admin, sale.id).unwrap().items[0].unit_price_cents, 150);

    let voided = pos.void_sale(&admin, sale.id, "customer changed mind").unwrap();
    assert_eq!(voided.status, "voided");
    assert_eq!(pos.get_product(&admin, cola.id).unwrap().stock_qty, 10);
    assert_eq!(pos.get_product(&admin, bread.id).unwrap().stock_qty, 5);
    assert!(pos.void_sale(&admin, sale.id, "again").is_err());
    let summary = pos.sales_summary(&admin, 0, i64::MAX).unwrap();
    assert_eq!((summary.sale_count, summary.voided_count), (0, 1));
}

#[test]
fn staff_sees_only_own_sales() {
    let (mut pos, admin) = setup();
    let cola = product(&mut pos, &admin, "Cola", 150, 0, 10);
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    let cart = [CartLine { product_id: cola.id, qty: 1, discount_cents: 0 }];
    let admin_sale = pos.complete_sale(&admin, &cart, PaymentMethod::Card, 0).unwrap();
    let staff_sale = pos.complete_sale(&staff, &cart, PaymentMethod::Card, 0).unwrap();

    let mine = pos.list_sales(&staff, 0, i64::MAX, 50, 0).unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].id, staff_sale.id);
    assert!(matches!(pos.get_sale(&staff, admin_sale.id), Err(PosError::Forbidden)));
    assert!(matches!(pos.void_sale(&staff, staff_sale.id, "x"), Err(PosError::Forbidden)));
    assert_eq!(pos.list_sales(&admin, 0, i64::MAX, 50, 0).unwrap().len(), 2);
}

#[test]
fn search_and_barcode_lookup() {
    let (mut pos, admin) = setup();
    product(&mut pos, &admin, "Coca Cola 330ml", 150, 0, 0);
    product(&mut pos, &admin, "Chocolate Bar", 120, 0, 0);
    product(&mut pos, &admin, "100% Juice", 300, 0, 0);

    let names = |v: Vec<Product>| v.into_iter().map(|p| p.name).collect::<Vec<_>>();
    assert_eq!(names(pos.search_products(&admin, "co", 10).unwrap()), ["Coca Cola 330ml"]);
    let mut hits = names(pos.search_products(&admin, "cola", 10).unwrap());
    hits.sort();
    assert_eq!(hits, ["Chocolate Bar", "Coca Cola 330ml"]);
    assert_eq!(names(pos.search_products(&admin, "330", 10).unwrap()), ["Coca Cola 330ml"]);
    assert_eq!(names(pos.search_products(&admin, "1%", 10).unwrap()), Vec::<String>::new());
    assert_eq!(names(pos.search_products(&admin, "10", 10).unwrap()), ["100% Juice"]);
    assert_eq!(pos.search_products(&admin, "", 10).unwrap().len(), 3);
    assert_eq!(pos.search_products(&admin, "\"oops", 10).unwrap().len(), 0);

    let hit = pos.find_by_code(&admin, "BC-Chocolate Bar").unwrap().unwrap();
    assert_eq!(hit.name, "Chocolate Bar");
    assert!(pos.find_by_code(&admin, "nope").unwrap().is_none());

    let dup = NewProduct {
        sku: None, barcode: Some("BC-Chocolate Bar".into()), name: "Dup".into(), price_cents: 1,
        cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: true,
    };
    assert!(matches!(pos.create_product(&admin, dup), Err(PosError::Conflict(_))));
}

#[test]
fn file_database_persists_and_backs_up() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("pos.db");
    {
        let mut pos = Pos::open(&db).unwrap();
        let admin = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.backup_to(&admin, dir.path().join("backup.db")).unwrap();
    }
    let mut pos = Pos::open(&db).unwrap();
    assert!(!pos.needs_setup().unwrap());
    assert!(pos.login("1111").is_ok());
    let mut restored = Pos::open(dir.path().join("backup.db")).unwrap();
    assert!(restored.login("1111").is_ok());
}
