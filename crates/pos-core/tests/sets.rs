use pos_core::*;

struct Kit {
    pos: Pos,
    s: Session,
    set: Product,
    sections: Vec<SetSection>,
    soup: Product,
    satay: Product,
    curry: Product,
    rice: Product,
    table: i64,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let s = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.set_setting(&s, "prices_include_tax", "0").unwrap();
    let cat = pos.create_category(&s, "Food", None, 1).unwrap().id;
    let item = |pos: &mut Pos, name: &str, price: i64| {
        pos.create_product(&s, NewProduct {
            category_id: Some(cat), sku: None, barcode: None, name: name.into(), price_cents: price, cost_cents: 0, tax_rate_bp: 1000,
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap()
    };
    let soup = item(&mut pos, "Soup", 500);
    let satay = item(&mut pos, "Satay", 600);
    let curry = item(&mut pos, "Green curry", 1200);
    let rice = item(&mut pos, "Jasmine rice", 300);
    let set = item(&mut pos, "Dinner set", 2500);
    let sections = pos
        .save_set(&s, set.id, &[
            SetSectionInput { name: "Starter".into(), name_alt: Some("ของเรียกน้ำย่อย".into()), pick: 1, starter: true, product_ids: vec![soup.id, satay.id] },
            SetSectionInput { name: "Main".into(), name_alt: None, pick: 1, starter: false, product_ids: vec![curry.id] },
            SetSectionInput { name: "Side".into(), name_alt: None, pick: 1, starter: false, product_ids: vec![rice.id] },
        ])
        .unwrap();
    let set = pos.get_product(&s, set.id).unwrap();
    let floor = pos.create_floor(&s, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&s, floor.id, &[TableLayout { id: None, name: "4".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    pos.set_kitchen_printer(&s, &PrinterConfig {
        target: PrinterTarget::File { path: std::env::temp_dir().join("pos-sets-unused.bin").display().to_string() },
        copy_delay_secs: 0, call_table_only: false, ..Default::default()
    })
    .unwrap();
    Kit { pos, s, set, sections, soup, satay, curry, rice, table: t[0].id }
}

fn dine_in(table: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table), channel_id: None, details: Default::default() }
}

fn picks(k: &Kit, starter: i64) -> Vec<SetPick> {
    vec![
        SetPick { section_id: k.sections[0].id, product_id: starter },
        SetPick { section_id: k.sections[1].id, product_id: k.curry.id },
        SetPick { section_id: k.sections[2].id, product_id: k.rice.id },
    ]
}

#[test]
fn a_set_is_a_product_with_sections() {
    let k = setup();
    assert!(k.set.is_set);
    let names: Vec<_> = k.sections.iter().map(|s| (s.name.as_str(), s.pick, s.starter, s.items.len())).collect();
    assert_eq!(names, [("Starter", 1, true, 2), ("Main", 1, false, 1), ("Side", 1, false, 1)]);
    // A set cannot hold itself or another set.
    let err = k.pos.get_set(&k.s, k.set.id).unwrap();
    assert_eq!(err.len(), 3);
}

#[test]
fn ordering_a_set_bills_one_line_and_sends_the_dishes_by_course() {
    let mut k = setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap();
    let o = k.pos.add_set(&k.s, o.id, k.set.id, &picks(&k, k.satay.id), None).unwrap();
    assert_eq!(o.items.len(), 4);
    assert_eq!(o.subtotal_cents, 2500, "the set's own price only");
    let parent = o.items.iter().find(|i| i.is_set).unwrap();
    assert!(o.items.iter().filter(|i| !i.is_set).all(|i| i.parent_id == Some(parent.id) && i.unit_price_cents == 0));

    // Starter section first; the set's own line never prints.
    let sent = k.pos.send_to_kitchen(&k.s, o.id).unwrap();
    let t = k.pos.kitchen_ticket(&k.s, o.id, &sent, false).unwrap();
    let text = t.to_text(42);
    assert_eq!(t.title, "NEW ORDER");
    assert!(text.contains("SET: DINNER SET") && text.contains("STARTER") && text.contains("1 x Satay"), "{text}");
    assert!(!text.contains("Dinner set\n") && !text.contains("Green curry"), "{text}");
    assert_eq!(t.footer.as_deref(), Some("MAINS TO FOLLOW: 2"));
    // Then main and side, under the set's name again.
    let sent = k.pos.call_mains(&k.s, o.id).unwrap();
    let t = k.pos.kitchen_ticket(&k.s, o.id, &sent, false).unwrap();
    let text = t.to_text(42);
    assert_eq!(t.title, "MAINS AWAY");
    assert!(text.contains("SET: DINNER SET") && text.find("MAIN ").unwrap() < text.find("Green curry").unwrap(), "{text}");
    assert!(text.find("Green curry").unwrap() < text.find("SIDE").unwrap() && text.contains("1 x Jasmine rice"), "{text}");

    // The bill: one line at the set's price, one tax class.
    let r = k.pos.receipt(&k.s, o.id).unwrap();
    assert_eq!(r.lines.len(), 1);
    assert_eq!((r.lines[0].name.as_str(), r.lines[0].amount_cents), ("Dinner set", 2500));
    assert_eq!(r.tax_classes.len(), 1);
    assert_eq!(r.total_cents, 2750);
    assert!(!r.to_text(42).contains("Satay"));
    assert!(!r.staff_to_text(42).contains("Satay"));
}

#[test]
fn sets_are_checked_and_removed_whole() {
    let mut k = setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap();
    // Every section must be chosen, from its own list.
    let mut short = picks(&k, k.soup.id);
    short.pop();
    assert!(k.pos.add_set(&k.s, o.id, k.set.id, &short, None).unwrap_err().to_string().contains("Side"));
    let mut wrong = picks(&k, k.soup.id);
    wrong[1].product_id = k.rice.id;
    assert!(k.pos.add_set(&k.s, o.id, k.set.id, &wrong, None).is_err());

    let o = k.pos.add_set(&k.s, o.id, k.set.id, &picks(&k, k.soup.id), None).unwrap();
    // A plain Soup is its own line, never merged into the set's.
    let o2 = k.pos.add_item(&k.s, o.id, k.soup.id, 1, None, None, &[]).unwrap();
    assert_eq!(o2.items.len(), o.items.len() + 1);
    // Dishes in a set only change with the set.
    let child = o2.items.iter().find(|i| i.parent_id.is_some()).unwrap();
    let upd = ItemUpdate { qty: 2, discount_cents: 0, note: None };
    assert!(k.pos.update_item(&k.s, child.id, &upd).is_err());
    // Two sets: every dish doubles; none: all gone.
    let parent = o2.items.iter().find(|i| i.is_set).unwrap().id;
    let o3 = k.pos.update_item(&k.s, parent, &ItemUpdate { qty: 2, discount_cents: 0, note: None }).unwrap();
    assert!(o3.items.iter().filter(|i| i.parent_id == Some(parent)).all(|i| i.qty == 2));
    assert_eq!(o3.subtotal_cents, 5000 + 500);
    let o4 = k.pos.update_item(&k.s, parent, &ItemUpdate { qty: 0, discount_cents: 0, note: None }).unwrap();
    assert_eq!(o4.items.len(), 1);
}

#[test]
fn set_only_items_stay_off_the_order_screen_and_sets_survive_backups() {
    let mut k = setup();
    k.pos.set_set_only(&k.s, k.rice.id, true).unwrap();
    let menu = k.pos.menu(&k.s).unwrap();
    assert!(!menu.iter().any(|p| p.id == k.rice.id) && menu.iter().any(|p| p.id == k.set.id));
    // Duplicate copies the sections.
    let copy = k.pos.duplicate_product(&k.s, k.set.id).unwrap();
    assert!(copy.is_set);
    assert_eq!(k.pos.get_set(&k.s, copy.id).unwrap().len(), 3);
    // Backup and restore keep the set and its dishes.
    let backup = k.pos.export_menu(&k.s).unwrap();
    k.pos.import_menu(&k.s, &backup).unwrap();
    let products = k.pos.list_products(&k.s, false, 100, 0).unwrap();
    let set = products.iter().find(|p| p.name == "Dinner set").unwrap();
    let rice = products.iter().find(|p| p.name == "Jasmine rice").unwrap();
    assert!(set.is_set && rice.set_only);
    let secs = k.pos.get_set(&k.s, set.id).unwrap();
    assert_eq!(secs[0].items.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Soup", "Satay"]);
    assert_eq!(secs[2].items[0].product_id, rice.id);
}
