use pos_core::*;

fn setup() -> (Pos, Session) {
    let mut pos = Pos::open_in_memory().unwrap();
    assert!(pos.needs_setup().unwrap());
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.set_setting(&admin, "prices_include_tax", "0").unwrap(); // these tests check tax added on top
    (pos, admin)
}

fn staff(pos: &mut Pos, admin: &Session) -> Session {
    pos.create_user(admin, "Sam", Role::Staff, "2222").unwrap();
    pos.login("2222").unwrap()
}

fn item(pos: &mut Pos, admin: &Session, name: &str, price: i64, tax_bp: i64, stock: Option<i64>) -> Product {
    pos.create_product(
        admin,
        NewProduct {
            category_id: None,
            sku: None,
            barcode: None,
            name: name.into(),
            price_cents: price,
            cost_cents: 0,
            tax_rate_bp: tax_bp,
            stock_qty: stock.unwrap_or(0),
            track_stock: stock.is_some(),
            starter: false, name_alt: None, prep: false, team_id: None,
        },
    )
    .unwrap()
}

fn layout(id: Option<i64>, name: &str, x: i64) -> TableLayout {
    TableLayout { id, name: name.into(), shape: TableShape::Rect, x, y: 10, w: 80, h: 80 }
}

/// Two floors: "Ground" with T1, T2 and "First" with T10.
fn floors(pos: &mut Pos, admin: &Session) -> (Floor, Vec<DiningTable>, Floor, Vec<DiningTable>) {
    let ground = pos.create_floor(admin, "Ground").unwrap();
    let first = pos.create_floor(admin, "First").unwrap();
    let g = pos.save_floor_layout(admin, ground.id, &[layout(None, "T1", 10), layout(None, "T2", 120)]).unwrap();
    let f = pos.save_floor_layout(admin, first.id, &[layout(None, "T10", 10)]).unwrap();
    (ground, g, first, f)
}

fn dine_in(table_id: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table_id), channel_id: None, details: OrderDetails::default() }
}

// ------------------------------------------------------------------ auth

#[test]
fn setup_only_once_and_pin_login() {
    let (mut pos, admin) = setup();
    assert_eq!(admin.role, Role::Admin);
    assert!(!pos.needs_setup().unwrap());
    assert!(matches!(pos.setup_first_admin("Evil", "2222"), Err(PosError::Forbidden)));
    let s = staff(&mut pos, &admin);
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
    assert!(matches!(pos.login("1111"), Err(PosError::LockedOut(_))));
}

#[test]
fn cannot_remove_last_admin_and_deactivated_users_lose_access() {
    let (mut pos, admin) = setup();
    let err = pos.update_user(&admin, admin.user_id, "Owner", Role::Staff, true).unwrap_err();
    assert!(matches!(err, PosError::Validation(_)));

    let s = staff(&mut pos, &admin);
    pos.update_user(&admin, s.user_id, "Sam", Role::Staff, false).unwrap();
    assert!(matches!(pos.refresh_session(&s), Err(PosError::Forbidden)));
    assert!(matches!(pos.login("2222"), Err(PosError::InvalidPin)));
    pos.reset_pin(&admin, s.user_id, "3333").unwrap();
    pos.update_user(&admin, s.user_id, "Sam", Role::Staff, true).unwrap();
    assert!(pos.login("3333").is_ok());
}

#[test]
fn staff_cannot_manage() {
    let (mut pos, admin) = setup();
    let s = staff(&mut pos, &admin);
    let floor = pos.create_floor(&admin, "Main").unwrap();
    assert!(matches!(pos.list_users(&s), Err(PosError::Forbidden)));
    // Staff can see sales reports (and close the day), but not manage.
    assert!(pos.sales_summary(&s, 0, i64::MAX).is_ok());
    assert!(matches!(pos.create_floor(&s, "Roof"), Err(PosError::Forbidden)));
    assert!(matches!(pos.save_floor_layout(&s, floor.id, &[]), Err(PosError::Forbidden)));
    assert!(matches!(pos.create_category(&s, "Mains", None, 0), Err(PosError::Forbidden)));
    // ...but can see the floor and menu to take orders.
    assert!(pos.list_floors(&s).is_ok());
    assert!(pos.floor_status(&s, floor.id).is_ok());
    assert!(pos.menu(&s).is_ok());
}

// ------------------------------------------------------------ floor plan

#[test]
fn multi_floor_layout_editing() {
    let (mut pos, admin) = setup();
    let (ground, g, first, f) = floors(&mut pos, &admin);
    assert_eq!(pos.list_floors(&admin).unwrap().iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Ground", "First"]);
    assert_eq!(g.len(), 2);
    assert_eq!(f.len(), 1);

    // Move T1, swap the two names, add T3: one atomic save.
    let saved = pos
        .save_floor_layout(
            &admin,
            ground.id,
            &[
                TableLayout { id: Some(g[0].id), name: "T2".into(), shape: TableShape::Round, x: 500, y: 300, w: 100, h: 100 },
                layout(Some(g[1].id), "T1", 120),
                layout(None, "T3", 240),
            ],
        )
        .unwrap();
    assert_eq!(saved.len(), 3);
    let moved = saved.iter().find(|t| t.id == g[0].id).unwrap();
    assert_eq!((moved.name.as_str(), moved.shape, moved.x, moved.y), ("T2", TableShape::Round, 500, 300));

    // Removing a table = leaving it out of the layout.
    let saved = pos.save_floor_layout(&admin, ground.id, &[layout(Some(g[1].id), "T1", 120)]).unwrap();
    assert_eq!(saved.len(), 1);

    // Validation: duplicate names, off-canvas, a table from another floor.
    assert!(pos.save_floor_layout(&admin, ground.id, &[layout(None, "A", 0), layout(None, "a", 200)]).is_err());
    assert!(pos.save_floor_layout(&admin, ground.id, &[layout(None, "A", PLAN_WIDTH - 10)]).is_err());
    assert!(pos.save_floor_layout(&admin, ground.id, &[layout(Some(f[0].id), "X", 0)]).is_err());
    assert!(matches!(pos.create_floor(&admin, "ground"), Err(PosError::Conflict(_))));
    // Same table name is fine on a different floor.
    assert!(pos.save_floor_layout(&admin, first.id, &[layout(Some(f[0].id), "T1", 10)]).is_ok());
}

#[test]
fn busy_tables_and_floors_cannot_be_removed() {
    let (mut pos, admin) = setup();
    let (ground, g, _, _) = floors(&mut pos, &admin);
    pos.open_order(&admin, &dine_in(g[0].id)).unwrap();
    assert!(pos.save_floor_layout(&admin, ground.id, &[layout(Some(g[1].id), "T2", 120)]).is_err());
    assert!(pos.delete_floor(&admin, ground.id).is_err());
    // Moving the busy table is fine.
    assert!(pos
        .save_floor_layout(&admin, ground.id, &[layout(Some(g[0].id), "T1", 600), layout(Some(g[1].id), "T2", 120)])
        .is_ok());
}

// ---------------------------------------------------------------- orders

#[test]
fn dine_in_full_service() {
    let (mut pos, admin) = setup();
    let s = staff(&mut pos, &admin);
    let (ground, g, _, _) = floors(&mut pos, &admin);
    let burger = item(&mut pos, &admin, "Burger", 1000, 1500, None);
    let cola = item(&mut pos, &admin, "Cola", 250, 1500, Some(24));

    let o = pos.open_order(&s, &dine_in(g[0].id)).unwrap();
    assert_eq!((o.table_name.as_deref(), o.floor_name.as_deref()), (Some("T1"), Some("Ground")));
    // One open order per table.
    assert!(matches!(pos.open_order(&s, &dine_in(g[0].id)), Err(PosError::Conflict(_))));
    assert_eq!(pos.table_order(&s, g[0].id).unwrap().unwrap().id, o.id);

    // Round 1: two burgers (tapped twice -> one line), one cola, with a note.
    pos.add_item(&s, o.id, burger.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, burger.id, 1, None, None, &[]).unwrap();
    let o2 = pos.add_item(&s, o.id, cola.id, 1, Some("no ice"), None, &[]).unwrap();
    assert_eq!(o2.items.len(), 2);
    assert_eq!(o2.items[0].qty, 2);
    assert_eq!(o2.items[1].note.as_deref(), Some("no ice"));

    let status = pos.floor_status(&s, ground.id).unwrap();
    let t1 = status.iter().find(|t| t.table.id == g[0].id).unwrap();
    assert_eq!((t1.order_id, t1.item_count, t1.unsent_count), (Some(o.id), 3, 2));
    assert_eq!(t1.total_cents, o2.total_cents);
    assert!(status.iter().find(|t| t.table.id == g[1].id).unwrap().order_id.is_none());

    let ticket = pos.send_to_kitchen(&s, o.id).unwrap();
    assert_eq!(ticket.len(), 2);
    assert!(pos.send_to_kitchen(&s, o.id).unwrap().is_empty());

    // Round 2: dessert-ish cola. Only the new line goes on the next ticket.
    pos.add_item(&s, o.id, cola.id, 1, None, None, &[]).unwrap();
    let ticket = pos.send_to_kitchen(&s, o.id).unwrap();
    assert_eq!((ticket.len(), ticket[0].qty), (1, 1));

    // Move to another table, then pay.
    let o3 = pos.move_order(&s, o.id, g[1].id).unwrap();
    assert_eq!(o3.table_name.as_deref(), Some("T2"));
    // 2x10.00 + 2x2.50 = 25.00. Tax is rounded per line: 3.00 + 0.38 + 0.38.
    assert_eq!((o3.subtotal_cents, o3.tax_cents, o3.total_cents), (2500, 376, 2876));
    assert!(pos.pay_order(&s, o.id, PaymentMethod::Cash, 2000).is_err());
    let paid = pos.pay_order(&s, o.id, PaymentMethod::Cash, 3000).unwrap();
    assert_eq!((paid.status.as_str(), paid.change_cents), ("paid", Some(124)));
    assert_eq!(pos.get_product(&admin, cola.id).unwrap().stock_qty, 22);
    // Table is free again; paid order is closed for edits.
    assert!(pos.table_order(&s, g[1].id).unwrap().is_none());
    assert!(pos.add_item(&s, o.id, cola.id, 1, None, None, &[]).is_err());

    let sum = pos.sales_summary(&admin, 0, i64::MAX).unwrap();
    assert_eq!((sum.order_count, sum.total_cents), (1, 2876));
    assert_eq!(sum.by_type[0].key, "dine_in");

    // Admin voids it: stock comes back, reports drop it.
    pos.void_order(&admin, o.id, "complaint").unwrap();
    assert_eq!(pos.get_product(&admin, cola.id).unwrap().stock_qty, 24);
    let sum = pos.sales_summary(&admin, 0, i64::MAX).unwrap();
    assert_eq!((sum.order_count, sum.voided_count), (0, 1));
}

#[test]
fn sent_items_need_admin_to_remove() {
    let (mut pos, admin) = setup();
    let s = staff(&mut pos, &admin);
    let (_, g, _, _) = floors(&mut pos, &admin);
    let burger = item(&mut pos, &admin, "Burger", 1000, 0, None);
    let o = pos.open_order(&s, &dine_in(g[0].id)).unwrap();
    let o = pos.add_item(&s, o.id, burger.id, 2, None, None, &[]).unwrap();
    let line = o.items[0].id;

    // Unsent: staff can change freely.
    pos.update_item(&s, line, &ItemUpdate { qty: 3, discount_cents: 0, note: Some("well done".into()) }).unwrap();
    pos.send_to_kitchen(&s, o.id).unwrap();

    let keep_note = Some("well done".to_string());
    let reduce = ItemUpdate { qty: 1, discount_cents: 0, note: keep_note.clone() };
    assert!(matches!(pos.update_item(&s, line, &reduce), Err(PosError::Forbidden)));
    assert!(pos.update_item(&s, line, &ItemUpdate { qty: 4, discount_cents: 0, note: keep_note.clone() }).is_err());
    assert!(pos.update_item(&s, line, &ItemUpdate { qty: 3, discount_cents: 0, note: None }).is_err());
    // Discount on a sent item is fine for staff.
    let o = pos.update_item(&s, line, &ItemUpdate { qty: 3, discount_cents: 500, note: keep_note.clone() }).unwrap();
    assert_eq!(o.total_cents, 2500);
    // Admin can reduce.
    let o = pos.update_item(&admin, line, &reduce).unwrap();
    assert_eq!(o.items[0].qty, 1);

    // Cancelling an order with sent items: admin + reason only.
    assert!(matches!(pos.cancel_order(&s, o.id, "walked out"), Err(PosError::Forbidden)));
    assert!(pos.cancel_order(&admin, o.id, "").is_err());
    pos.cancel_order(&admin, o.id, "walked out").unwrap();
    assert_eq!(pos.sales_summary(&admin, 0, i64::MAX).unwrap().cancelled_count, 1);

    // An order with nothing sent can be cancelled by staff without a reason.
    let o = pos.open_order(&s, &dine_in(g[0].id)).unwrap();
    pos.add_item(&s, o.id, burger.id, 1, None, None, &[]).unwrap();
    pos.cancel_order(&s, o.id, "").unwrap();
}

#[test]
fn takeaway_collection_delivery() {
    let (mut pos, admin) = setup();
    let s = staff(&mut pos, &admin);
    let (_, g, _, _) = floors(&mut pos, &admin);
    let pizza = item(&mut pos, &admin, "Pizza", 1200, 0, None);

    let new = |t, name: Option<&str>, phone: Option<&str>, addr: Option<&str>| NewOrder {
        order_type: t,
        table_id: None, channel_id: None,
        details: OrderDetails {
            customer_name: name.map(Into::into),
            customer_phone: phone.map(Into::into),
            delivery_address: addr.map(Into::into),
            notes: None,
            external_ref: None,
            customer_id: None,
        },
    };
    // Takeaway needs nothing; collection needs a name; delivery needs name+phone+address.
    let ta = pos.open_order(&s, &new(OrderType::Takeaway, None, None, None)).unwrap();
    assert!(pos.open_order(&s, &new(OrderType::Collection, None, Some("0700"), None)).is_err());
    let col = pos.open_order(&s, &new(OrderType::Collection, Some("Ann"), None, None)).unwrap();
    assert!(pos.open_order(&s, &new(OrderType::Delivery, Some("Bob"), Some("0711"), None)).is_err());
    let del = pos.open_order(&s, &new(OrderType::Delivery, Some("Bob"), Some("0711"), Some("1 High St"))).unwrap();
    assert_eq!(del.delivery_address.as_deref(), Some("1 High St"));
    // Non dine-in orders cannot take a table; dine-in must.
    let mut bad = new(OrderType::Takeaway, None, None, None);
    bad.table_id = Some(g[0].id);
    assert!(pos.open_order(&s, &bad).is_err());
    assert!(pos.open_order(&s, &NewOrder { order_type: OrderType::DineIn, table_id: None, channel_id: None, details: Default::default() }).is_err());
    assert!(pos.move_order(&s, ta.id, g[0].id).is_err());

    // Editing details re-validates for the order type.
    let cleared = OrderDetails { customer_name: Some("Bob".into()), ..Default::default() };
    assert!(pos.update_order_details(&s, del.id, &cleared).is_err());

    assert_eq!(pos.list_open_orders(&s).unwrap().len(), 3);
    for o in [&ta, &col, &del] {
        pos.add_item(&s, o.id, pizza.id, 1, None, None, &[]).unwrap();
        pos.send_to_kitchen(&s, o.id).unwrap();
        pos.pay_order(&s, o.id, PaymentMethod::Card, 0).unwrap();
    }
    assert!(pos.list_open_orders(&s).unwrap().is_empty());
    let sum = pos.sales_summary(&admin, 0, i64::MAX).unwrap();
    assert_eq!(sum.order_count, 3);
    assert_eq!(sum.by_type.len(), 3);
    assert_eq!(pos.list_orders(&admin, 0, i64::MAX, 50, 0).unwrap().len(), 3);
    // Empty orders cannot be paid.
    let empty = pos.open_order(&s, &new(OrderType::Takeaway, None, None, None)).unwrap();
    assert!(pos.pay_order(&s, empty.id, PaymentMethod::Card, 0).is_err());
}

#[test]
fn staff_history_is_own_orders_only() {
    let (mut pos, admin) = setup();
    let s = staff(&mut pos, &admin);
    let p = item(&mut pos, &admin, "Tea", 200, 0, None);
    let ta = NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() };
    let mine = pos.open_order(&s, &ta).unwrap();
    let theirs = pos.open_order(&admin, &ta).unwrap();
    // Open orders are shared: staff can work on the admin's open order.
    pos.add_item(&s, theirs.id, p.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, mine.id, p.id, 1, None, None, &[]).unwrap();
    pos.pay_order(&admin, theirs.id, PaymentMethod::Card, 0).unwrap();
    pos.pay_order(&s, mine.id, PaymentMethod::Card, 0).unwrap();

    let hist = pos.list_orders(&s, 0, i64::MAX, 50, 0).unwrap();
    assert_eq!(hist.iter().map(|o| o.id).collect::<Vec<_>>(), [mine.id]);
    assert!(matches!(pos.get_order(&s, theirs.id), Err(PosError::Forbidden)));
    assert!(matches!(pos.void_order(&s, mine.id, "x"), Err(PosError::Forbidden)));
}

#[test]
fn price_is_fixed_when_item_is_added() {
    let (mut pos, admin) = setup();
    let p = item(&mut pos, &admin, "Soup", 500, 0, None);
    let o = pos.open_order(&admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    pos.add_item(&admin, o.id, p.id, 1, None, None, &[]).unwrap();
    let upd = ProductUpdate {
        category_id: None, sku: None, barcode: None, name: "Soup".into(), price_cents: 900,
        cost_cents: 0, tax_rate_bp: 0, track_stock: false, active: true, starter: false, name_alt: None, prep: false, team_id: None,
    };
    pos.update_product(&admin, p.id, upd).unwrap();
    // Existing line keeps 5.00; a new tap at 9.00 becomes its own line.
    let o = pos.add_item(&admin, o.id, p.id, 1, None, None, &[]).unwrap();
    assert_eq!(o.items.iter().map(|i| i.unit_price_cents).collect::<Vec<_>>(), [500, 900]);
    assert_eq!(o.total_cents, 1400);
}

// ------------------------------------------------------------------ menu

#[test]
fn categories_and_menu_order() {
    let (mut pos, admin) = setup();
    let drinks = pos.create_category(&admin, "Drinks", Some("#1d4ed8"), 2).unwrap();
    let mains = pos.create_category(&admin, "Mains", None, 1).unwrap();
    assert!(pos.create_category(&admin, "drinks", None, 3).is_err());
    assert!(pos.create_category(&admin, "Bad", Some("red"), 3).is_err());
    for (name, cat) in [("Water", Some(drinks.id)), ("Steak", Some(mains.id)), ("Bread", None), ("Cola", Some(drinks.id))] {
        pos.create_product(&admin, NewProduct {
            category_id: cat, sku: None, barcode: None, name: name.into(), price_cents: 100,
            cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        }).unwrap();
    }
    let names: Vec<_> = pos.menu(&admin).unwrap().into_iter().map(|p| p.name).collect();
    // Category order, then the order items were added (drag to change it).
    assert_eq!(names, ["Steak", "Water", "Cola", "Bread"]);
    assert_eq!(pos.list_categories(&admin, false).unwrap().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Mains", "Drinks"]);
    let bad = NewProduct {
        category_id: Some(999), sku: None, barcode: None, name: "X".into(), price_cents: 1,
        cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
    };
    assert!(pos.create_product(&admin, bad).is_err());
}

#[test]
fn search_and_barcode_lookup() {
    let (mut pos, admin) = setup();
    for (name, code) in [("Coca Cola 330ml", "500"), ("Chocolate Cake", "501"), ("100% Juice", "502")] {
        pos.create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: Some(code.into()), name: name.into(), price_cents: 100,
            cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        }).unwrap();
    }
    let names = |v: Vec<Product>| v.into_iter().map(|p| p.name).collect::<Vec<_>>();
    assert_eq!(names(pos.search_products(&admin, "co", 10).unwrap()), ["Coca Cola 330ml"]);
    let mut hits = names(pos.search_products(&admin, "cola", 10).unwrap());
    hits.sort();
    assert_eq!(hits, ["Chocolate Cake", "Coca Cola 330ml"]);
    assert_eq!(names(pos.search_products(&admin, "1%", 10).unwrap()), Vec::<String>::new());
    assert_eq!(names(pos.search_products(&admin, "10", 10).unwrap()), ["100% Juice"]);
    assert_eq!(pos.search_products(&admin, "\"oops", 10).unwrap().len(), 0);
    assert_eq!(pos.find_by_code(&admin, "501").unwrap().unwrap().name, "Chocolate Cake");
    assert!(pos.find_by_code(&admin, "nope").unwrap().is_none());
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
