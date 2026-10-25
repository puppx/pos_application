use pos_core::*;

struct Menu {
    pad_thai: Product,
    cola: Product,
    chicken: i64,
    egg: i64,
}

fn setup() -> (Pos, Session, Menu, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let s = pos.setup_first_admin("Owner", "1111").unwrap();
    let mains = pos.create_category(&s, "Mains", None, 1).unwrap();
    let mains = pos.set_category_defaults(&s, mains.id, None, false, false, Some(" อาหารจานหลัก ")).unwrap();
    assert_eq!(mains.name_alt.as_deref(), Some("อาหารจานหลัก"));
    let item = |pos: &mut Pos, name: &str, alt: Option<&str>| {
        pos.create_product(&s, NewProduct {
            category_id: Some(mains.id), sku: None, barcode: None, name: name.into(), price_cents: 1000,
            cost_cents: 0, tax_rate_bp: 0, stock_qty: 0, track_stock: false, starter: false, name_alt: alt.map(str::to_string), prep: false,
        })
        .unwrap()
    };
    let pad_thai = item(&mut pos, "Pad Thai", Some("ผัดไทย"));
    let cola = item(&mut pos, "Cola", Some("  "));
    assert_eq!((pad_thai.name_alt.as_deref(), cola.name_alt.as_deref()), (Some("ผัดไทย"), None));
    let v = pos
        .set_variants(&s, pad_thai.id, &[
            VariantInput { id: None, name: "Chicken".into(), price_cents: 1000, name_alt: Some("ไก่".into()) },
            VariantInput { id: None, name: "Tofu".into(), price_cents: 900, name_alt: None },
        ])
        .unwrap();
    let g = pos.create_option_group(&s, "Extras", 0, 2).unwrap();
    let g = pos
        .save_group_options(&s, g.id, &[OptionInput { id: None, name: "Fried egg".into(), price_cents: 100, name_alt: Some("ไข่ดาว".into()) }])
        .unwrap();
    pos.set_product_option_groups(&s, pad_thai.id, &[g.id]).unwrap();
    pos.set_kitchen_printer(&s, &PrinterConfig { target: PrinterTarget::File { path: "unused".into() }, ..Default::default() }).unwrap();
    let o = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    (pos, s, Menu { pad_thai, cola, chicken: v[0].id, egg: g.options[0].id }, o.id)
}

#[test]
fn order_lines_keep_the_thai_name() {
    let (mut pos, s, m, order) = setup();
    let o = pos.add_item(&s, order, m.pad_thai.id, 1, None, Some(m.chicken), &[m.egg]).unwrap();
    let tofu = pos.list_variants(&s, Some(m.pad_thai.id)).unwrap()[1].id;
    let o2 = pos.add_item(&s, order, m.pad_thai.id, 1, None, Some(tofu), &[]).unwrap();
    pos.add_item(&s, order, m.cola.id, 2, None, None, &[]).unwrap();
    assert_eq!(o.items[0].name_alt.as_deref(), Some("ผัดไทย (ไก่)"));
    assert_eq!(o.items[0].options[0].name_alt.as_deref(), Some("ไข่ดาว"));
    // A choice without Thai keeps its English part.
    assert_eq!(o2.items[1].name_alt.as_deref(), Some("ผัดไทย (Tofu)"));

    // Renaming the menu later does not change the order.
    let p = &m.pad_thai;
    pos.update_product(&s, p.id, ProductUpdate {
        category_id: p.category_id, sku: None, barcode: None, name: p.name.clone(), price_cents: 1000, cost_cents: 0,
        tax_rate_bp: 0, track_stock: false, active: true, starter: false, name_alt: Some("ผัดไท".into()), prep: false,
    })
    .unwrap();
    let o = pos.get_order(&s, order).unwrap();
    assert_eq!(o.items[0].name_alt.as_deref(), Some("ผัดไทย (ไก่)"));

    // Kitchen: English with Thai under it, by default.
    let t = pos.kitchen_ticket(&s, order, &o.items, false).unwrap().to_text(42);
    assert!(t.contains("-- MAINS / อาหารจานหลัก --"), "{t}");
    assert!(t.contains("1 x Pad Thai\n  (Chicken)\n  ผัดไทย (ไก่)\n  + Fried egg / ไข่ดาว\n"), "{t}");
    assert!(t.contains("2 x Cola\n-"), "{t}");

    // Thai only.
    let mut cfg = pos.kitchen_printer().unwrap();
    cfg.names = Some(PrintNames::Thai);
    pos.set_kitchen_printer(&s, &cfg).unwrap();
    let t = pos.kitchen_ticket(&s, order, &o.items, false).unwrap().to_text(42);
    assert!(t.contains("-- อาหารจานหลัก --") && t.contains("1 x ผัดไทย (ไก่)\n  + ไข่ดาว\n") && t.contains("2 x Cola"), "{t}");
    assert!(!t.contains("Chicken"), "{t}");

    // English only.
    cfg.names = Some(PrintNames::English);
    pos.set_kitchen_printer(&s, &cfg).unwrap();
    let t = pos.kitchen_ticket(&s, order, &o.items, false).unwrap().to_text(42);
    assert!(!t.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c)), "{t}");

    // Receipts: English by default; English and Thai when chosen.
    let r = pos.receipt(&s, order).unwrap().to_text(42);
    assert!(!r.contains("ผัด"), "{r}");
    let mut rc = pos.receipt_settings().unwrap();
    rc.printer.names = Some(PrintNames::Both);
    pos.set_receipt_settings(&s, &rc).unwrap();
    let r = pos.receipt(&s, order).unwrap().to_text(42);
    assert!(r.contains("1 x Pad Thai (Chicken)") && r.contains("\n    ผัดไทย (ไก่)\n"), "{r}");
}

#[test]
fn thai_prints_as_an_image() {
    let (mut pos, s, m, order) = setup();
    pos.add_item(&s, order, m.pad_thai.id, 1, None, Some(m.chicken), &[]).unwrap();
    let o = pos.get_order(&s, order).unwrap();
    let job = pos.kitchen_job(&s, order, &o.items, false).unwrap().unwrap();
    let b = job.bytes();
    let images = b.windows(4).filter(|w| *w == [0x1D, b'v', b'0', 0]).count();
    assert_eq!(images, 2, "category heading and the Thai name");
    assert!(b.windows(12).any(|w| w == b"1 x Pad Thai"));
    assert!(!b.windows(3).any(|w| w == "ผ".as_bytes()), "no raw UTF-8 Thai");
}

#[test]
fn backups_imports_and_copies_keep_thai() {
    let (mut pos, s, m, _) = setup();
    let copy = pos.duplicate_product(&s, m.pad_thai.id).unwrap();
    assert_eq!(copy.name_alt.as_deref(), Some("ผัดไทย"));
    assert_eq!(pos.list_variants(&s, Some(copy.id)).unwrap()[0].name_alt.as_deref(), Some("ไก่"));

    let backup = pos.export_menu(&s).unwrap();
    let json = serde_json::to_string(&backup).unwrap();
    assert!(json.contains("ผัดไทย") && json.contains("ไข่ดาว") && json.contains("อาหารจานหลัก"));
    // Older backups without Thai still load.
    let old = json.replace(",\"name_alt\":\"ผัดไทย\"", "");
    let mut restored: MenuBackup = serde_json::from_str(&old).unwrap();
    assert!(restored.items[0].name_alt.is_none());
    restored.items[0].name_alt = Some("ผัดไทยกุ้ง".into());
    pos.import_menu(&s, &restored).unwrap();
    let again = pos.export_menu(&s).unwrap();
    assert_eq!(again.items[0].name_alt.as_deref(), Some("ผัดไทยกุ้ง"));
    assert_eq!(again.categories[0].name_alt.as_deref(), Some("อาหารจานหลัก"));
    assert_eq!(again.option_groups[0].options[0].name_alt.as_deref(), Some("ไข่ดาว"));
    assert_eq!(again.items[0].choices[0].name_alt.as_deref(), Some("ไก่"));

    let rows = [ImportRow { name: "Green curry".into(), price_cents: 1200, category: None, tax_rate_bp: None, name_alt: Some("แกงเขียวหวาน".into()) }];
    pos.import_items(&s, &rows).unwrap();
    let items = pos.list_products(&s, false, 100, 0).unwrap();
    assert_eq!(items.iter().find(|p| p.name == "Green curry").unwrap().name_alt.as_deref(), Some("แกงเขียวหวาน"));
    let too_long = [ImportRow { name_alt: Some("ก".repeat(121)), ..rows[0].clone() }];
    assert!(pos.import_items(&s, &too_long).unwrap_err().to_string().contains("line 1"));
}
