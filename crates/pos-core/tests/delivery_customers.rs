use pos_core::*;

fn setup() -> (Pos, Session) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    (pos, admin)
}

fn staff(pos: &mut Pos, admin: &Session) -> Session {
    pos.create_user(admin, "Sam", Role::Staff, "2222").unwrap();
    pos.login("2222").unwrap()
}

fn item(pos: &mut Pos, admin: &Session) -> Product {
    pos.create_product(admin, NewProduct {
        category_id: None, sku: None, barcode: None, name: "Pizza".into(), price_cents: 1000,
        cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false,
    })
    .unwrap()
}

fn cust(name: &str, phone: &str, line1: &str, postcode: &str) -> CustomerInput {
    CustomerInput {
        name: name.into(),
        phone: Some(phone.into()),
        address_line1: Some(line1.into()),
        town: Some("London".into()),
        postcode: Some(postcode.into()),
        ..Default::default()
    }
}

#[test]
fn default_platforms_and_editing() {
    let (mut pos, admin) = setup();
    let list = pos.list_channels(&admin).unwrap();
    assert_eq!(list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Uber Eats", "Just Eat", "Deliveroo"]);

    // Rename one, drop one, add one, reorder: one save.
    let edited = pos
        .save_channels(&admin, &[
            ChannelInput { id: Some(list[2].id), name: "Deliveroo".into(), color: None },
            ChannelInput { id: Some(list[0].id), name: "UberEats".into(), color: Some("#06c167".into()) },
            ChannelInput { id: None, name: "Phone app".into(), color: Some("#123abc".into()) },
        ])
        .unwrap();
    assert_eq!(edited.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Deliveroo", "UberEats", "Phone app"]);
    assert_eq!(edited[1].id, list[0].id);

    assert!(pos.save_channels(&admin, &[ChannelInput { id: None, name: "A".into(), color: None }, ChannelInput { id: None, name: "a".into(), color: None }]).is_err());
    assert!(pos.save_channels(&admin, &[ChannelInput { id: None, name: "A".into(), color: Some("orange".into()) }]).is_err());

    let s = staff(&mut pos, &admin);
    assert!(pos.list_channels(&s).is_ok());
    assert!(matches!(pos.save_channels(&s, &[]), Err(PosError::Forbidden)));
}

#[test]
fn platform_orders_need_no_customer_details() {
    let (mut pos, admin) = setup();
    let s = staff(&mut pos, &admin);
    let pizza = item(&mut pos, &admin);
    let uber = pos.list_channels(&s).unwrap()[0].clone();

    let o = pos
        .open_order(&s, &NewOrder {
            order_type: OrderType::Delivery,
            table_id: None,
            channel_id: Some(uber.id),
            details: OrderDetails { external_ref: Some("#A1B2C".into()), ..Default::default() },
        })
        .unwrap();
    assert_eq!((o.channel_name.as_deref(), o.external_ref.as_deref()), (Some("Uber Eats"), Some("#A1B2C")));

    // Same order type without a platform still needs the full details.
    let direct = NewOrder { order_type: OrderType::Delivery, table_id: None, channel_id: None, details: Default::default() };
    assert!(pos.open_order(&s, &direct).is_err());
    // Dine-in cannot be a platform order; unknown platforms are refused.
    let floor = pos.create_floor(&admin, "G").unwrap();
    let t = pos.save_floor_layout(&admin, floor.id, &[TableLayout { id: None, name: "1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 50, h: 50 }]).unwrap();
    assert!(pos.open_order(&s, &NewOrder { order_type: OrderType::DineIn, table_id: Some(t[0].id), channel_id: Some(uber.id), details: Default::default() }).is_err());
    assert!(pos.open_order(&s, &NewOrder { order_type: OrderType::Collection, table_id: None, channel_id: Some(999), details: Default::default() }).is_err());

    // Kitchen ticket heading shows the platform and its order number.
    pos.add_item(&s, o.id, pizza.id, 1, None, None, &[]).unwrap();
    let sent = pos.send_to_kitchen(&s, o.id).unwrap();
    let text = pos.kitchen_ticket(&s, o.id, &sent, false).unwrap().to_text(42);
    assert!(text.contains("UBER EATS #A1B2C"), "{text}");

    pos.pay_order(&s, o.id, PaymentMethod::Other, 0).unwrap();
    let board = pos.list_orders(&admin, 0, i64::MAX, 10, 0).unwrap();
    assert_eq!(board[0].channel_name.as_deref(), Some("Uber Eats"));

    // Reports split by platform, with direct orders under "Direct".
    let direct = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    pos.add_item(&s, direct.id, pizza.id, 2, None, None, &[]).unwrap();
    pos.pay_order(&s, direct.id, PaymentMethod::Card, 0).unwrap();
    let sum = pos.sales_summary(&admin, 0, i64::MAX).unwrap();
    let by: Vec<(&str, i64)> = sum.by_channel.iter().map(|g| (g.key.as_str(), g.total_cents)).collect();
    assert_eq!(by, [("Direct", 2000), ("Uber Eats", 1000)]);

    // A retired platform still shows on old orders and in reports.
    pos.save_channels(&admin, &[]).unwrap();
    assert!(pos.list_channels(&admin).unwrap().is_empty());
    assert_eq!(pos.get_order(&admin, o.id).unwrap().channel_name.as_deref(), Some("Uber Eats"));
}

#[test]
fn saved_customers_search_and_postcode_list() {
    let (mut pos, admin) = setup();
    let s = staff(&mut pos, &admin);
    let ann = pos.create_customer(&s, &cust("Ann Smith", "07700 900123", "12 High Street", "sw1a2aa")).unwrap();
    assert_eq!(ann.postcode.as_deref(), Some("SW1A 2AA"));
    assert_eq!(ann.address().as_deref(), Some("12 High Street, London, SW1A 2AA"));
    pos.create_customer(&s, &cust("Bob Jones", "+44 7700 900456", "3 High Street", "SW1A 2AA")).unwrap();
    pos.create_customer(&s, &cust("Cat Lee", "020 7946 0018", "Flat 2, 10 Low Road", "M1 1AE")).unwrap();

    let names = |v: Vec<Customer>| v.into_iter().map(|c| c.name).collect::<Vec<_>>();
    // Phone in any format, partial digits, name, postcode with or without space.
    assert_eq!(names(pos.search_customers(&s, "7700900456", 10).unwrap()), ["Bob Jones"]);
    assert_eq!(names(pos.search_customers(&s, "+447700900123", 10).unwrap()), ["Ann Smith"]);
    assert_eq!(names(pos.search_customers(&s, "smith", 10).unwrap()), ["Ann Smith"]);
    assert_eq!(names(pos.search_customers(&s, "m11", 10).unwrap()), ["Cat Lee"]);
    assert_eq!(pos.search_customers(&s, "", 10).unwrap().len(), 3);

    // Postcode list in door-number order (3 before 12).
    let at = pos.customers_at_postcode(&s, "sw1a 2aa").unwrap();
    assert_eq!(at.iter().map(|c| c.address_line1.as_deref().unwrap()).collect::<Vec<_>>(), ["3 High Street", "12 High Street"]);
    assert!(pos.customers_at_postcode(&s, "nonsense").unwrap().is_empty());

    // Edit; validation.
    let mut edit = cust("Ann Smith-Jones", "07700 900123", "14 High Street", "SW1A 2AA");
    edit.notes = Some("Gate code 1234".into());
    let ann = pos.update_customer(&s, ann.id, &edit).unwrap();
    assert_eq!((ann.name.as_str(), ann.notes.as_deref()), ("Ann Smith-Jones", Some("Gate code 1234")));
    assert!(pos.create_customer(&s, &cust("X", "1", "1 Road", "not a postcode")).is_err());
    assert!(pos.create_customer(&s, &CustomerInput::default()).is_err());

    // Ordering for a saved customer counts their orders.
    let new = NewOrder {
        order_type: OrderType::Delivery,
        table_id: None,
        channel_id: None,
        details: OrderDetails {
            customer_name: Some(ann.name.clone()),
            customer_phone: ann.phone.clone(),
            delivery_address: ann.address(),
            customer_id: Some(ann.id),
            ..Default::default()
        },
    };
    let o = pos.open_order(&s, &new).unwrap();
    assert_eq!(o.customer_id, Some(ann.id));
    assert_eq!(pos.get_customer(&s, ann.id).unwrap().order_count, 1);
    assert_eq!(pos.customer_orders(&s, ann.id, 10).unwrap()[0].id, o.id);
    // Most recent customer comes first in the default list.
    assert_eq!(pos.search_customers(&s, "", 1).unwrap()[0].id, ann.id);

    // Only admins delete; the order keeps its copy of the details.
    assert!(matches!(pos.delete_customer(&s, ann.id), Err(PosError::Forbidden)));
    pos.delete_customer(&admin, ann.id).unwrap();
    assert!(pos.search_customers(&s, "smith", 10).unwrap().is_empty());
    assert_eq!(pos.get_order(&s, o.id).unwrap().customer_name.as_deref(), Some("Ann Smith-Jones"));
    let mut bad = new.clone();
    bad.details.customer_id = Some(ann.id);
    assert!(pos.open_order(&s, &bad).is_err());
}

/// A database created by v0.1.0 (schema 1 only) upgrades in place and keeps
/// its data.
#[test]
fn upgrades_a_v0_1_0_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pos.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("../src/schema.sql")).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 1;
             INSERT INTO settings (key, value) VALUES ('store_name', 'Old Shop');
             INSERT INTO users (name, role, pin_hash) VALUES ('Owner', 'admin', x'00');
             INSERT INTO orders (order_type, customer_name, customer_phone, delivery_address, opened_by)
             VALUES ('delivery', 'Old Customer', '0700', '1 Old Road', 1);",
        )
        .unwrap();
    }
    let pos = Pos::open(&path).unwrap();
    assert_eq!(pos.setting("store_name").unwrap().as_deref(), Some("Old Shop"));
    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 10);
    let (name, channel): (String, Option<i64>) = conn
        .query_row("SELECT customer_name, channel_id FROM orders WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!((name.as_str(), channel), ("Old Customer", None));
    let platforms: i64 = conn.query_row("SELECT COUNT(*) FROM delivery_channels", [], |r| r.get(0)).unwrap();
    assert_eq!(platforms, 3);
}
