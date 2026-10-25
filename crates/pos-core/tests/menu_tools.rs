use pos_core::*;

fn setup() -> (Pos, Session) {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    (pos, admin)
}

fn row(name: &str, price: i64, cat: Option<&str>) -> ImportRow {
    ImportRow { name: name.into(), price_cents: price, category: cat.map(str::to_string), tax_rate_bp: None, name_alt: None }
}

#[test]
fn import_uses_category_defaults_and_creates_categories() {
    let (mut pos, s) = setup();
    pos.set_setting(&s, "default_tax_bp", "2000").unwrap();
    assert_eq!(pos.default_tax_bp().unwrap(), 2000);
    let starters = pos.create_category(&s, "Starters", None, 1).unwrap();
    let starters = pos.set_category_defaults(&s, starters.id, Some(0), true, false, None).unwrap();
    assert_eq!((starters.tax_rate_bp, starters.starter), (Some(0), true));

    let r = pos
        .import_items(&s, &[row("Soup", 550, Some("starters")), row("Wings", 750, Some("Starters")), row("Steak", 2000, Some("Mains")), row("Bread", 200, None)])
        .unwrap();
    assert_eq!((r.items, r.new_categories.clone()), (4, vec!["Mains".to_string()]));
    let menu = pos.menu(&s).unwrap();
    let get = |n: &str| menu.iter().find(|p| p.name == n).unwrap().clone();
    // Category defaults: Starters are 0% tax and starters; Mains uses the main default.
    assert_eq!((get("Soup").tax_rate_bp, get("Soup").starter), (0, true));
    assert_eq!((get("Steak").tax_rate_bp, get("Steak").starter), (2000, false));
    assert_eq!(get("Bread").category_id, None);
    // Items keep the order they were pasted in.
    let names: Vec<_> = menu.iter().filter(|p| p.category_id == Some(starters.id)).map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Soup", "Wings"]);

    // One bad line rejects everything, with its line number.
    let err = pos.import_items(&s, &[row("Ok", 100, None), row("", 100, None)]).unwrap_err();
    assert!(err.to_string().starts_with("line 2:"), "{err}");
    assert_eq!(pos.menu(&s).unwrap().len(), 4);
    assert!(pos.set_category_defaults(&s, starters.id, Some(10_001), false, false, None).is_err());
}

#[test]
fn reorder_and_duplicate() {
    let (mut pos, s) = setup();
    pos.import_items(&s, &[row("A", 100, Some("Cat")), row("B", 200, Some("Cat")), row("C", 300, Some("Cat"))]).unwrap();
    let ids = |pos: &Pos| pos.menu(&s).unwrap().iter().map(|p| (p.id, p.name.clone())).collect::<Vec<_>>();
    let v = ids(&pos);
    pos.reorder_products(&s, &[v[2].0, v[0].0, v[1].0]).unwrap();
    assert_eq!(ids(&pos).iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["C", "A", "B"]);

    // Duplicate B with a choice and an option group: copy lands right after it.
    let b = v[1].0;
    pos.set_variants(&s, b, &[VariantInput { id: None, name: "Small".into(), price_cents: 150, name_alt: None }]).unwrap();
    let g = pos.create_option_group(&s, "Extras", 0, 2).unwrap();
    pos.set_product_option_groups(&s, b, &[g.id]).unwrap();
    let copy = pos.duplicate_product(&s, b).unwrap();
    assert_eq!((copy.name.as_str(), copy.price_cents), ("B (copy)", 200));
    assert_eq!(ids(&pos).iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["C", "A", "B", "B (copy)"]);
    assert_eq!(pos.list_variants(&s, Some(copy.id)).unwrap()[0].name, "Small");
    assert!(pos.menu_options(&s).unwrap().links.iter().any(|l| l.product_id == copy.id && l.group_id == g.id));

    // Categories reorder too.
    let x = pos.create_category(&s, "X", None, 9).unwrap();
    let cat = pos.list_categories(&s, false).unwrap()[0].id;
    pos.reorder_categories(&s, &[x.id, cat]).unwrap();
    assert_eq!(pos.list_categories(&s, false).unwrap()[0].name, "X");

    // A menu backup keeps the defaults and the order.
    pos.set_category_defaults(&s, cat, Some(500), true, false, None).unwrap();
    let backup = pos.export_menu(&s).unwrap();
    pos.import_menu(&s, &backup).unwrap();
    let c = pos.list_categories(&s, false).unwrap().into_iter().find(|c| c.name == "Cat").unwrap();
    assert_eq!((c.tax_rate_bp, c.starter), (Some(500), true));
    assert_eq!(ids(&pos).iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["C", "A", "B", "B (copy)"]);
}

#[test]
fn default_tax_fills_items_left_at_zero() {
    let mut pos = Pos::open_in_memory().unwrap();
    let s = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.create_category(&s, "Food", None, 1).unwrap();
    let cold = pos.create_category(&s, "Cold food", None, 2).unwrap();
    let wine = pos.create_category(&s, "Wine", None, 3).unwrap();
    pos.set_category_defaults(&s, cold.id, Some(0), false, false, None).unwrap();
    pos.set_category_defaults(&s, wine.id, Some(2500), false, false, None).unwrap();
    let rows = [
        row("Burger", 900, Some("Food")),
        row("Salad", 700, Some("Cold food")),
        row("Merlot", 2000, Some("Wine")),
        row("Water", 100, None),
    ];
    pos.import_items(&s, &rows).unwrap();
    let mut items = pos.list_products(&s, false, 100, 0).unwrap();
    let wine_id = items.iter().find(|p| p.name == "Merlot").unwrap().id;
    // Merlot got 25% from its category on import; set it back to 0% by hand.
    let m = items.iter_mut().find(|p| p.id == wine_id).unwrap();
    pos.update_product(&s, m.id, ProductUpdate {
        category_id: m.category_id, sku: None, barcode: None, name: m.name.clone(), price_cents: 2000, cost_cents: 0,
        tax_rate_bp: 0, track_stock: false, active: true, starter: false, name_alt: None, prep: false,
    })
    .unwrap();
    // A sale before the change keeps its tax.
    let o = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    let burger = items.iter().find(|p| p.name == "Burger").unwrap().id;
    pos.add_item(&s, o.id, burger, 1, None, None, &[]).unwrap();

    assert_eq!(pos.untaxed_items(&s).unwrap(), 1, "only Merlot: no main default yet");
    pos.set_setting(&s, "default_tax_bp", "2000").unwrap();
    assert_eq!(pos.untaxed_items(&s).unwrap(), 3);
    assert_eq!(pos.apply_default_tax(&s).unwrap(), 3);
    let tax = |pos: &Pos, name: &str| pos.list_products(&s, false, 100, 0).unwrap().into_iter().find(|p| p.name == name).unwrap().tax_rate_bp;
    assert_eq!(tax(&pos, "Burger"), 2000);
    assert_eq!(tax(&pos, "Water"), 2000);
    assert_eq!(tax(&pos, "Merlot"), 2500, "its category's own rate");
    assert_eq!(tax(&pos, "Salad"), 0, "category set to 0% on purpose");
    assert_eq!(pos.untaxed_items(&s).unwrap(), 0);
    assert_eq!(pos.get_order(&s, o.id).unwrap().items[0].tax_rate_bp, 0);
}
