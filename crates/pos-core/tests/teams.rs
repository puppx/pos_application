use pos_core::*;

struct Kit {
    pos: Pos,
    s: Session,
    soup: Product,
    wings: Product,
    steak: Product,
    green: Product,
    jasmine: Product,
    cola: Product,
    table: i64,
    teams: Vec<Team>,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let s = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.set_setting(&s, "prices_include_tax", "0").unwrap();
    let teams = pos.list_teams(&s).unwrap();
    let cat = |pos: &Pos, name: &str, n: i64| pos.create_category(&s, name, None, n).unwrap().id;
    let (starters, mains, curries, rice, drinks) =
        (cat(&pos, "Starters", 1), cat(&pos, "Mains", 2), cat(&pos, "Curries", 3), cat(&pos, "Rice", 4), cat(&pos, "Drinks", 5));
    let item = |pos: &mut Pos, name: &str, c: i64, starter: bool, team: Option<i64>| {
        pos.create_product(&s, NewProduct {
            category_id: Some(c), sku: None, barcode: None, name: name.into(), price_cents: 1000, cost_cents: 0, tax_rate_bp: 0,
            stock_qty: 0, track_stock: false, starter, name_alt: None, prep: false, team_id: team,
        })
        .unwrap()
    };
    let by = |n: &str| teams.iter().find(|t| t.name == n).map(|t| t.id);
    let soup = item(&mut pos, "Soup", starters, true, by("Starter"));
    let wings = item(&mut pos, "Wings", starters, true, by("Starter"));
    let steak = item(&mut pos, "Steak", mains, false, by("Main"));
    let green = item(&mut pos, "Green curry", curries, false, by("Curry"));
    let jasmine = item(&mut pos, "Jasmine rice", rice, false, by("Rice"));
    let cola = item(&mut pos, "Cola", drinks, false, None);
    let floor = pos.create_floor(&s, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&s, floor.id, &[TableLayout { id: None, name: "4".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    pos.set_kitchen_printer(&s, &PrinterConfig {
        target: PrinterTarget::File { path: std::env::temp_dir().join("pos-teams-unused.bin").display().to_string() },
        copy_delay_secs: 0, call_table_only: false, ..Default::default()
    })
    .unwrap();
    Kit { pos, s, soup, wings, steak, green, jasmine, cola, table: t[0].id, teams }
}

fn dine_in(table: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table), channel_id: None, details: Default::default() }
}

fn heads(tickets: &[KitchenTicket]) -> Vec<Option<&str>> {
    tickets.iter().map(|t| t.banner.as_deref().map(|b| b.lines().next().unwrap())).collect()
}

#[test]
fn four_teams_are_there_to_start_with_and_can_be_edited() {
    let mut k = setup();
    let names: Vec<_> = k.teams.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Starter", "Main", "Curry", "Rice"]);
    assert_eq!(k.teams[2].name_alt.as_deref(), Some("แกง"));
    assert_eq!(k.pos.list_teams(&k.s).unwrap()[0].items, 2);

    let input = |t: &Team| TeamInput { id: Some(t.id), name: t.name.clone(), name_alt: t.name_alt.clone() };
    // Add one, rename one, reorder (Rice first), remove Main.
    let list = vec![
        input(&k.teams[3]),
        TeamInput { id: Some(k.teams[2].id), name: "Curries".into(), name_alt: None },
        input(&k.teams[0]),
        TeamInput { id: None, name: "Dessert".into(), name_alt: Some(" ของหวาน ".into()) },
    ];
    let saved = k.pos.save_teams(&k.s, &list).unwrap();
    assert_eq!(saved.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Rice", "Curries", "Starter", "Dessert"]);
    assert_eq!(saved[3].name_alt.as_deref(), Some("ของหวาน"));
    // The removed team's dishes are back on the general ticket.
    assert_eq!(k.pos.get_product(&k.s, k.steak.id).unwrap().team_id, None);
    assert_eq!(k.pos.get_product(&k.s, k.jasmine.id).unwrap().team_id, Some(k.teams[3].id));
    // Bad lists and staff.
    let dup = vec![TeamInput { id: None, name: "A".into(), name_alt: None }, TeamInput { id: None, name: "a".into(), name_alt: None }];
    assert!(k.pos.save_teams(&k.s, &dup).is_err());
    assert!(k.pos.save_teams(&k.s, &[TeamInput { id: None, name: " ".into(), name_alt: None }]).is_err());
    k.pos.create_user(&k.s, "Sam", Role::Staff, "2222").unwrap();
    let staff = k.pos.login("2222").unwrap();
    assert!(k.pos.save_teams(&staff, &list).is_err() && k.pos.list_teams(&staff).is_err());
    // A product can only join a team that exists.
    let p = k.pos.get_product(&k.s, k.cola.id).unwrap();
    let upd = |team| ProductUpdate {
        category_id: p.category_id, sku: None, barcode: None, name: p.name.clone(), price_cents: 1000, cost_cents: 0, tax_rate_bp: 0,
        track_stock: false, active: true, starter: false, name_alt: None, prep: false, team_id: team,
    };
    assert!(k.pos.update_product(&k.s, p.id, upd(Some(999))).is_err());
    assert_eq!(k.pos.update_product(&k.s, p.id, upd(Some(saved[0].id))).unwrap().team_id, Some(saved[0].id));
}

#[test]
fn one_ticket_per_team_each_with_its_own_header() {
    let mut k = setup();
    let o = k.pos.open_order(&k.s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    for (p, q) in [(&k.steak, 1), (&k.green, 2), (&k.jasmine, 2), (&k.cola, 1), (&k.soup, 1)] {
        k.pos.add_item(&k.s, o.id, p.id, q, None, None, &[]).unwrap();
    }
    let sent = k.pos.send_to_kitchen(&k.s, o.id).unwrap();
    let tickets = k.pos.kitchen_tickets(&k.s, o.id, &sent, false).unwrap();
    // Team order, then the general ticket; same title on all of them.
    assert_eq!(heads(&tickets), [Some("STARTER"), Some("MAIN"), Some("CURRY"), Some("RICE"), None]);
    assert!(tickets.iter().all(|t| t.title == "NEW ORDER"));
    let dishes = |t: &KitchenTicket| t.items.iter().map(|i| i.name.clone()).collect::<Vec<_>>();
    assert_eq!(dishes(&tickets[0]), ["Soup"]);
    assert_eq!(dishes(&tickets[2]), ["Green curry"]);
    assert_eq!(dishes(&tickets[4]), ["Cola"]);
    assert_eq!(tickets[2].banner.as_deref(), Some("CURRY\nแกง"), "the Thai name goes under it");
    let text = tickets[2].to_text(42);
    assert!(text.starts_with("[ CURRY ]") || text.trim_start().starts_with("[ CURRY ]"), "{text}");
    assert!(text.contains("2 x Green curry") && !text.contains("Steak"), "{text}");

    // One print job: every ticket back to back, each with its own header image and cut.
    let job = k.pos.kitchen_job(&k.s, o.id, &sent, false).unwrap().unwrap();
    let bytes = job.bytes();
    assert_eq!(bytes.windows(4).filter(|w| *w == [0x1D, b'V', 66, 0]).count(), 5, "five tickets, five cuts");
    assert!(bytes.windows(4).filter(|w| *w == [0x1D, b'v', b'0', 0]).count() >= 4, "header bars are images");
    assert_eq!(job.parts.len(), 1);

    // A reprint is split the same way, and a team with nothing in the send prints nothing.
    let reprint = k.pos.kitchen_reprint_job(&k.s, o.id).unwrap().unwrap();
    assert_eq!(reprint.bytes().windows(4).filter(|w| *w == [0x1D, b'V', 66, 0]).count(), 5);
    let only_curry: Vec<_> = sent.iter().filter(|i| i.name == "Green curry").cloned().collect();
    assert_eq!(heads(&k.pos.kitchen_tickets(&k.s, o.id, &only_curry, false).unwrap()), [Some("CURRY")]);
}

#[test]
fn nothing_changes_until_dishes_are_given_a_team() {
    let mut k = setup();
    for id in [k.soup.id, k.wings.id, k.steak.id, k.green.id, k.jasmine.id] {
        let p = k.pos.get_product(&k.s, id).unwrap();
        k.pos.update_product(&k.s, id, ProductUpdate {
            category_id: p.category_id, sku: None, barcode: None, name: p.name.clone(), price_cents: 1000, cost_cents: 0, tax_rate_bp: 0,
            track_stock: false, active: true, starter: p.starter, name_alt: None, prep: false, team_id: None,
        })
        .unwrap();
    }
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap();
    k.pos.add_item(&k.s, o.id, k.steak.id, 1, None, None, &[]).unwrap();
    k.pos.add_item(&k.s, o.id, k.cola.id, 1, None, None, &[]).unwrap();
    let sent = k.pos.send_to_kitchen(&k.s, o.id).unwrap();
    let tickets = k.pos.kitchen_tickets(&k.s, o.id, &sent, false).unwrap();
    assert_eq!(tickets.len(), 1);
    assert!(tickets[0].banner.is_none());
    assert_eq!(tickets[0], k.pos.kitchen_ticket(&k.s, o.id, &sent, false).unwrap());
}

#[test]
fn courses_prepare_and_main_course_are_split_by_team_too() {
    let mut k = setup();
    k.pos.set_prep(&k.s, k.green.id, true).unwrap();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap();
    for (p, q) in [(&k.soup, 1), (&k.steak, 1), (&k.green, 1), (&k.jasmine, 1)] {
        k.pos.add_item(&k.s, o.id, p.id, q, None, None, &[]).unwrap();
    }
    // Starters first: only the Starter team's ticket, and PREPARE NOW only for Curry.
    let sent = k.pos.send_to_kitchen(&k.s, o.id).unwrap();
    let t = k.pos.kitchen_tickets(&k.s, o.id, &sent, false).unwrap();
    assert_eq!((heads(&t), t[0].title.as_str()), (vec![Some("STARTER")], "NEW ORDER"));
    let prep = k.pos.send_prep(&k.s, o.id).unwrap();
    let job = k.pos.prep_job(&k.s, o.id, &prep).unwrap().unwrap();
    assert_eq!(job.bytes().windows(4).filter(|w| *w == [0x1D, b'V', 66, 0]).count(), 1);
    assert!(String::from_utf8_lossy(&job.bytes()).contains("PREPARE NOW"));
    // Call mains: MAINS AWAY for Main, Curry and Rice.
    let mains = k.pos.call_mains(&k.s, o.id).unwrap();
    let t = k.pos.kitchen_tickets(&k.s, o.id, &mains, false).unwrap();
    assert_eq!(heads(&t), [Some("MAIN"), Some("CURRY"), Some("RICE")]);
    assert!(t.iter().all(|t| t.title == "MAINS AWAY"));
    // Call mains again: one short ticket each.
    let job = k.pos.main_course_job(&k.s, o.id).unwrap().unwrap();
    assert_eq!(job.bytes().windows(4).filter(|w| *w == [0x1D, b'V', 66, 0]).count(), 3);
}

#[test]
fn a_category_can_hand_its_team_to_its_dishes() {
    let mut k = setup();
    let desserts = k.pos.create_category(&k.s, "Desserts", None, 9).unwrap();
    let a = k.pos.import_items(&k.s, &[ImportRow { name: "Cake".into(), price_cents: 500, category: Some("desserts".into()), tax_rate_bp: None, name_alt: None }]).unwrap();
    assert_eq!(a.items, 1);
    let rice = k.teams[3].id;
    // Set the team for new items only, then move the existing ones too.
    assert_eq!(k.pos.set_category_team(&k.s, desserts.id, Some(rice), false).unwrap(), 0);
    let cake = |k: &Kit| k.pos.list_products(&k.s, false, 100, 0).unwrap().into_iter().find(|p| p.name == "Cake").unwrap();
    assert_eq!(cake(&k).team_id, None);
    assert_eq!(k.pos.set_category_team(&k.s, desserts.id, Some(rice), true).unwrap(), 1);
    assert_eq!(cake(&k).team_id, Some(rice));
    k.pos.import_items(&k.s, &[ImportRow { name: "Pie".into(), price_cents: 500, category: Some("desserts".into()), tax_rate_bp: None, name_alt: None }]).unwrap();
    assert_eq!(k.pos.list_products(&k.s, false, 100, 0).unwrap().iter().find(|p| p.name == "Pie").unwrap().team_id, Some(rice));
    assert!(k.pos.set_category_team(&k.s, desserts.id, Some(999), true).is_err());
    assert_eq!(k.pos.duplicate_product(&k.s, k.steak.id).unwrap().team_id, k.pos.get_product(&k.s, k.steak.id).unwrap().team_id);
}

#[test]
fn teams_survive_a_menu_backup() {
    let mut k = setup();
    let backup = k.pos.export_menu(&k.s).unwrap();
    assert_eq!(backup.teams.len(), 4);
    assert!(backup.items.iter().any(|i| i.name == "Green curry" && i.team.as_deref() == Some("Curry")));
    let json = serde_json::to_string(&backup).unwrap();
    // Restore on a till that has lost its teams: they come back by name.
    k.pos.save_teams(&k.s, &[]).unwrap();
    let restored: MenuBackup = serde_json::from_str(&json).unwrap();
    k.pos.import_menu(&k.s, &restored).unwrap();
    let teams = k.pos.list_teams(&k.s).unwrap();
    assert_eq!(teams.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Starter", "Main", "Curry", "Rice"]);
    let curry = teams.iter().find(|t| t.name == "Curry").unwrap().id;
    let items = k.pos.list_products(&k.s, false, 100, 0).unwrap();
    assert_eq!(items.iter().find(|p| p.name == "Green curry").unwrap().team_id, Some(curry));
    assert_eq!(items.iter().find(|p| p.name == "Cola").unwrap().team_id, None);
    // Older backups (no teams) still load.
    let old: MenuBackup = serde_json::from_str(&json.replace("\"teams\"", "\"unused\"")).unwrap();
    assert!(old.teams.is_empty());
}

#[test]
fn sample_tickets_show_every_header() {
    let k = setup();
    let job = k.pos.test_teams_job(&k.s).unwrap().unwrap();
    assert_eq!(job.bytes().windows(4).filter(|w| *w == [0x1D, b'V', 66, 0]).count(), 4);
}
