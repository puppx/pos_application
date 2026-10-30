use pos_core::*;

fn setup() -> (Pos, Session, Session, Product, Product, Category) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    let drinks = pos.create_category(&admin, "Drinks", None, 1).unwrap();
    let mut item = |name: &str, sku: &str, stock| {
        pos.create_product(&admin, NewProduct {
            category_id: Some(drinks.id), sku: Some(sku.into()), barcode: None, name: name.into(), price_cents: 300,
            cost_cents: 0, tax_rate_bp: 0, stock_qty: stock, track_stock: stock > 0, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap()
    };
    let cola = item("Cola", "C1", 10);
    let tea = item("Tea", "T1", 0);
    (pos, admin, staff, cola, tea, drinks)
}

fn takeaway() -> NewOrder {
    NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }
}

#[test]
fn admin_deletes_an_order_completely() {
    let (mut pos, admin, staff, cola, _, _) = setup();
    let o = pos.open_order(&staff, &takeaway()).unwrap();
    pos.add_item(&staff, o.id, cola.id, 3, None, None, &[]).unwrap();
    pos.pay_order(&staff, o.id, PaymentMethod::Cash, 900).unwrap();
    assert_eq!(pos.get_product(&admin, cola.id).unwrap().stock_qty, 7);
    let now = chrono::Utc::now().timestamp();
    assert_eq!(pos.sales_summary(&admin, 0, now + 10).unwrap().order_count, 1);

    assert!(matches!(pos.delete_order(&staff, o.id, "mistake"), Err(PosError::Forbidden)));
    pos.delete_order(&admin, o.id, "wrong table").unwrap();
    assert!(matches!(pos.get_order(&admin, o.id), Err(PosError::NotFound)));
    assert!(pos.list_orders(&admin, 0, now + 10, 50, 0).unwrap().is_empty());
    assert_eq!(pos.sales_summary(&admin, 0, now + 10).unwrap().order_count, 0);
    // Stock taken by the sale is back.
    assert_eq!(pos.get_product(&admin, cola.id).unwrap().stock_qty, 10);
    assert!(matches!(pos.delete_order(&admin, o.id, ""), Err(PosError::NotFound)));

    // Open and voided orders can be deleted too; a voided one already gave its stock back.
    let open = pos.open_order(&staff, &takeaway()).unwrap();
    pos.add_item(&staff, open.id, cola.id, 1, None, None, &[]).unwrap();
    pos.delete_order(&admin, open.id, "").unwrap();
    assert!(pos.list_open_orders(&admin).unwrap().is_empty());
    let v = pos.open_order(&staff, &takeaway()).unwrap();
    pos.add_item(&staff, v.id, cola.id, 2, None, None, &[]).unwrap();
    pos.pay_order(&staff, v.id, PaymentMethod::Card, 0).unwrap();
    pos.void_order(&admin, v.id, "refund").unwrap();
    pos.delete_order(&admin, v.id, "").unwrap();
    assert_eq!(pos.get_product(&admin, cola.id).unwrap().stock_qty, 10);
}

#[test]
fn menu_items_and_categories_are_deleted_for_good() {
    let (mut pos, admin, staff, cola, tea, drinks) = setup();
    // Tea was never sold: its row is removed.
    assert!(pos.delete_product(&admin, tea.id).unwrap());
    assert!(matches!(pos.get_product(&admin, tea.id), Err(PosError::NotFound)));

    // Cola is on a past order: hidden everywhere, the order keeps its line.
    let o = pos.open_order(&staff, &takeaway()).unwrap();
    pos.add_item(&staff, o.id, cola.id, 1, None, None, &[]).unwrap();
    pos.pay_order(&staff, o.id, PaymentMethod::Card, 0).unwrap();
    assert!(matches!(pos.delete_product(&staff, cola.id), Err(PosError::Forbidden)));
    assert!(!pos.delete_product(&admin, cola.id).unwrap());
    assert!(pos.menu(&admin).unwrap().is_empty());
    assert!(pos.list_products(&admin, true, 100, 0).unwrap().is_empty(), "not even under Show removed");
    assert!(pos.search_products(&admin, "Cola", 10).unwrap().is_empty());
    assert_eq!(pos.get_order(&admin, o.id).unwrap().items[0].name, "Cola");
    assert!(matches!(pos.delete_product(&admin, cola.id), Err(PosError::NotFound)));
    // Its SKU is free again.
    pos.create_product(&admin, NewProduct {
        category_id: Some(drinks.id), sku: Some("C1".into()), barcode: None, name: "Cola".into(), price_cents: 350,
        cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
    })
    .unwrap();

    // Deleting a category keeps its items, without a category.
    pos.delete_category(&admin, drinks.id).unwrap();
    assert!(pos.list_categories(&admin, true).unwrap().is_empty());
    let menu = pos.menu(&admin).unwrap();
    assert_eq!((menu.len(), menu[0].category_id), (1, None));
    pos.create_category(&admin, "Drinks", None, 1).unwrap();
}
