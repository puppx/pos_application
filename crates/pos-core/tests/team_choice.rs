//! "Which team makes this?": a dish moved to another kitchen team on its
//! order (`order_items.team_id`) prints under that team, on that team's
//! printer, and shows under it on the kitchen screen; the setting
//! `team_choice` lists the teams to choose from.

use pos_core::*;

struct Kit {
    pos: Pos,
    s: Session,
    soup: Product,
    steak: Product,
    starter: i64,
    main: i64,
    curry: i64,
    tables: Vec<i64>,
    dir: tempfile::TempDir,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let s = pos.setup_first_admin("Owner", "1111").unwrap();
    let teams = pos.list_teams(&s).unwrap();
    let by = |n: &str| teams.iter().find(|t| t.name == n).unwrap().id;
    let (starter, main, curry) = (by("Starter"), by("Main"), by("Curry"));
    let cat = pos.create_category(&s, "Food", None, 1).unwrap().id;
    let item = |pos: &mut Pos, name: &str, is_starter: bool, team: i64| {
        pos.create_product(&s, NewProduct {
            category_id: Some(cat), sku: None, barcode: None, name: name.into(), price_cents: 1000, cost_cents: 0, tax_rate_bp: Some(0),
            stock_qty: 0, track_stock: false, starter: is_starter, name_alt: None, prep: false, team_id: Some(team),
        })
        .unwrap()
    };
    let soup = item(&mut pos, "Soup", true, starter);
    let steak = item(&mut pos, "Steak", false, main);
    let floor = pos.create_floor(&s, "Ground").unwrap();
    let layout: Vec<TableLayout> =
        ["1", "2", "3"].iter().map(|n| TableLayout { is_table: Some(true), id: None, name: (*n).into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }).collect();
    let tables = pos.save_floor_layout(&s, floor.id, &layout).unwrap().into_iter().map(|t| t.id).collect();
    let dir = tempfile::tempdir().unwrap();
    pos.set_kitchen_printer(&s, &PrinterConfig {
        target: PrinterTarget::File { path: dir.path().join("kitchen.bin").display().to_string() },
        copy_delay_secs: 0, app_font: false, slow_send: false, ..Default::default()
    })
    .unwrap();
    Kit { pos, s, soup, steak, starter, main, curry, tables, dir }
}

fn dine_in(table: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table), channel_id: None, details: Default::default() }
}

/// Each ticket's team bar and its dishes: [(Some("MAIN"), ["Soup"])...].
fn tickets(k: &Kit, order: i64, items: &[OrderItem], reprint: bool) -> Vec<(Option<String>, Vec<String>)> {
    k.pos
        .kitchen_tickets(&k.s, order, items, reprint)
        .unwrap()
        .into_iter()
        .map(|t| (t.banner.map(|b| b.split(" / ").next().unwrap().to_string()), t.items.into_iter().map(|i| i.name).collect()))
        .collect()
}

fn ticket(team: &str, dishes: &[&str]) -> (Option<String>, Vec<String>) {
    (Some(team.to_string()), dishes.iter().map(|d| d.to_string()).collect())
}

fn soup_line(o: &Order) -> &OrderItem {
    o.items.iter().find(|i| i.name == "Soup").unwrap()
}

#[test]
fn a_dish_moved_to_another_team_prints_routes_and_shows_under_that_team() {
    let k = &mut setup();
    // A second printer for the Main team.
    let line = k.dir.path().join("line.bin");
    k.pos
        .set_kitchen_routes(&k.s, &[KitchenRoute {
            label: "Grill".into(), target: PrinterTarget::File { path: line.display().to_string() }, width: 42, copies: 1,
            team_ids: vec![k.main], drinks: false, desserts: false, image_mode: ImageMode::Raster, slow_send: false,
        }])
        .unwrap();
    let o = k.pos.open_order(&k.s, &dine_in(k.tables[0])).unwrap();
    // The soup, a starter of the Starter team, made by the Main team this time.
    let o = k.pos.add_item_for(&k.s, o.id, k.soup.id, 1, None, None, &[], None, Some(k.main)).unwrap();
    let soup = soup_line(&o).clone();
    assert_eq!(soup.team_id, Some(k.main));
    assert!(soup.starter, "the course stays as the menu sets it");
    // Its own team is no move.
    let o = k.pos.add_item_for(&k.s, o.id, k.steak.id, 1, None, None, &[], None, Some(k.main)).unwrap();
    assert_eq!(o.items.iter().find(|i| i.name == "Steak").unwrap().team_id, None);

    let items = k.pos.get_order(&k.s, o.id).unwrap().items;
    assert_eq!(tickets(k, o.id, &items, false), [ticket("MAIN", &["Soup", "Steak"])]);
    // Both go to the Main team's printer, none to the kitchen printer.
    let sent = k.pos.send_to_kitchen(&k.s, o.id).unwrap();
    assert_eq!(sent.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["Soup"], "a starter still goes first");
    let job = k.pos.kitchen_job(&k.s, o.id, &sent, false).unwrap().unwrap();
    job.send().unwrap();
    assert!(!k.dir.path().join("kitchen.bin").exists());
    let grill = String::from_utf8_lossy(&std::fs::read(&line).unwrap()).to_string();
    assert!(grill.contains("Soup") && grill.contains("MAIN"), "{grill}");
    // Kitchen screen: under Main, not under Starter.
    let board = |f: String| -> Vec<String> {
        k.pos.kitchen_board(&k.s, &f).unwrap().into_iter().flat_map(|o| o.items).map(|i| i.name).collect()
    };
    assert_eq!(board(format!("team:{}", k.main)), ["Soup"]);
    assert!(board(format!("team:{}", k.starter)).is_empty());
    assert_eq!(board("all".into()), ["Soup"]);
}

#[test]
fn lines_for_different_teams_stay_apart_and_the_line_editor_can_move_one() {
    let k = &mut setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.tables[0])).unwrap();
    k.pos.add_item(&k.s, o.id, k.soup.id, 1, None, None, &[]).unwrap();
    k.pos.add_item_for(&k.s, o.id, k.soup.id, 1, None, None, &[], None, Some(k.main)).unwrap();
    k.pos.add_item_for(&k.s, o.id, k.soup.id, 1, None, None, &[], None, Some(k.main)).unwrap();
    let o = k.pos.add_item_for(&k.s, o.id, k.soup.id, 1, None, None, &[], None, Some(k.starter)).unwrap();
    // One line per team: 2 for the Starter team (its own), 2 for Main.
    let lines: Vec<(i64, Option<i64>)> = o.items.iter().map(|i| (i.qty, i.team_id)).collect();
    assert_eq!(lines, [(2, None), (2, Some(k.main))]);
    assert_eq!(tickets(k, o.id, &o.items, false), [ticket("STARTER", &["Soup"]), ticket("MAIN", &["Soup"])]);

    // ✎ Team: Curry, then back to its own team (no move left).
    let id = o.items[0].id;
    let o = k.pos.set_item_team(&k.s, id, Some(k.curry)).unwrap();
    assert_eq!(o.items[0].team_id, Some(k.curry));
    let o = k.pos.set_item_team(&k.s, id, Some(k.starter)).unwrap();
    assert_eq!(o.items[0].team_id, None);
    let o = k.pos.set_item_team(&k.s, o.items[1].id, None).unwrap();
    assert_eq!(o.items[1].team_id, None);
    // Not a team, or already sent: refused.
    assert!(k.pos.set_item_team(&k.s, id, Some(9999)).is_err());
    assert!(k.pos.add_item_for(&k.s, o.id, k.soup.id, 1, None, None, &[], None, Some(9999)).is_err());
    k.pos.send_to_kitchen(&k.s, o.id).unwrap();
    let err = k.pos.set_item_team(&k.s, id, Some(k.main)).unwrap_err();
    assert!(err.to_string().contains("not yet sent"), "{err}");
}

#[test]
fn the_team_survives_moving_merging_and_reprinting() {
    let k = &mut setup();
    // The Main team's own printer: where a moved dish's tickets must go.
    let grill = PrinterTarget::File { path: k.dir.path().join("grill.bin").display().to_string() };
    k.pos
        .set_kitchen_routes(&k.s, &[KitchenRoute {
            label: "Grill".into(), target: grill.clone(), width: 42, copies: 1, team_ids: vec![k.main], drinks: false, desserts: false,
            image_mode: ImageMode::Raster, slow_send: false,
        }])
        .unwrap();
    let o = k.pos.open_order(&k.s, &dine_in(k.tables[0])).unwrap();
    let o = k.pos.add_item_for(&k.s, o.id, k.soup.id, 3, None, None, &[], None, Some(k.main)).unwrap();
    let soup = soup_line(&o).id;
    k.pos.send_to_kitchen(&k.s, o.id).unwrap();
    // One soup to table 2 (a copy of the line), then the rest to table 2 as well.
    let at2 = k.pos.move_items(&k.s, o.id, k.tables[1], &[(soup, 1)]).unwrap();
    assert_eq!((soup_line(&at2).qty, soup_line(&at2).team_id), (1, Some(k.main)));
    let o = k.pos.get_order(&k.s, o.id).unwrap();
    assert_eq!((soup_line(&o).qty, soup_line(&o).team_id), (2, Some(k.main)));
    // Table 1 joins table 3's bill; then table 3 joins table 2's.
    let o3 = k.pos.open_order(&k.s, &dine_in(k.tables[2])).unwrap();
    k.pos.add_item(&k.s, o3.id, k.steak.id, 1, None, None, &[]).unwrap();
    let joined = k.pos.merge_into_table(&k.s, o.id, k.tables[2]).unwrap();
    assert_eq!(soup_line(&joined).team_id, Some(k.main));
    let all = k.pos.merge_into_table(&k.s, joined.id, k.tables[1]).unwrap();
    let soups: Vec<(i64, Option<i64>)> = all.items.iter().filter(|i| i.name == "Soup").map(|i| (i.qty, i.team_id)).collect();
    assert_eq!(soups, [(2, Some(k.main)), (1, Some(k.main))]);
    // Reprint: still under MAIN, on the Main team's printer only.
    let job = k.pos.kitchen_reprint_job(&k.s, all.id).unwrap().unwrap();
    assert_eq!((&job.target, job.more.len()), (&grill, 0));
    let text = String::from_utf8_lossy(&job.bytes()).to_string();
    assert!(text.contains("REPRINT") && text.contains("Soup") && !text.contains("Steak"), "{text}");
    let sent: Vec<OrderItem> = all.items.iter().filter(|i| i.sent_at.is_some()).cloned().collect();
    assert_eq!(tickets(k, all.id, &sent, true), [ticket("MAIN", &["Soup", "Soup"])]);
    // A CANCELLED ticket for one of them goes to the Main team too.
    let one = all.items.iter().find(|i| i.name == "Soup" && i.qty == 2).unwrap();
    let upd = ItemUpdate { qty: 1, discount_cents: one.discount_cents, note: None };
    let (_, job) = k.pos.update_item_for_kitchen(&k.s, one.id, &upd).unwrap();
    let job = job.unwrap();
    assert_eq!((&job.target, job.more.len()), (&grill, 0));
    assert!(String::from_utf8_lossy(&job.bytes()).contains("CANCELLED"));
}

#[test]
fn dishes_without_a_move_follow_the_menu_as_before() {
    let k = &mut setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.tables[0])).unwrap();
    k.pos.add_item(&k.s, o.id, k.soup.id, 1, None, None, &[]).unwrap();
    let o = k.pos.add_item_for(&k.s, o.id, k.steak.id, 1, None, None, &[], None, Some(k.curry)).unwrap();
    assert_eq!(soup_line(&o).team_id, None);
    assert_eq!(tickets(k, o.id, &o.items, false), [ticket("STARTER", &["Soup"]), ticket("CURRY", &["Steak"])]);
    // The menu moves the soup to Main: an open order's soup follows; the moved steak stays.
    let mut p = k.soup.clone();
    p.team_id = Some(k.main);
    k.pos
        .update_product(&k.s, p.id, ProductUpdate {
            category_id: p.category_id, sku: p.sku.clone(), barcode: p.barcode.clone(), name: p.name.clone(), price_cents: p.price_cents,
            cost_cents: p.cost_cents, tax_rate_bp: Some(p.tax_rate_bp), track_stock: p.track_stock, active: true, starter: p.starter,
            name_alt: None, prep: false, team_id: Some(k.main),
        })
        .unwrap();
    assert_eq!(tickets(k, o.id, &o.items, false), [ticket("MAIN", &["Soup"]), ticket("CURRY", &["Steak"])]);
    // Teams switched off: one plain ticket, moves or not.
    k.pos.set_setting(&k.s, "teams_on", "0").unwrap();
    assert_eq!(tickets(k, o.id, &o.items, false), [(None, vec!["Soup".to_string(), "Steak".to_string()])]);
}

#[test]
fn the_teams_to_choose_from_are_a_kitchen_setting() {
    let k = &mut setup();
    let quinn = k.pos.create_user(&k.s, "Quinn", Role::Manager, "4646").unwrap();
    k.pos.create_user(&k.s, "Sam", Role::Staff, "2222").unwrap();
    let _ = quinn;
    let quinn = k.pos.login("4646").unwrap();
    let sam = k.pos.login("2222").unwrap();
    assert!(k.pos.team_choice().unwrap().is_empty(), "off until teams are ticked");
    // Saved tidy, in team order.
    k.pos.set_setting(&quinn, TEAM_CHOICE_KEY, &format!(" {}, {} ", k.main, k.starter)).unwrap();
    assert_eq!(k.pos.setting(TEAM_CHOICE_KEY).unwrap(), Some(format!("{},{}", k.starter, k.main)));
    assert_eq!(k.pos.team_choice().unwrap(), [k.starter, k.main]);
    assert!(matches!(k.pos.set_setting(&sam, TEAM_CHOICE_KEY, ""), Err(PosError::Forbidden)));
    for bad in ["abc".to_string(), "9999,1".into(), k.main.to_string(), format!("{0},{0}", k.main)] {
        assert!(matches!(k.pos.set_setting(&quinn, TEAM_CHOICE_KEY, &bad), Err(PosError::Validation(_))), "{bad}");
    }
    assert_eq!(k.pos.team_choice().unwrap(), [k.starter, k.main], "unchanged by the refused values");
    // Kitchen teams switched off: nobody is asked.
    k.pos.set_setting(&quinn, "teams_on", "0").unwrap();
    assert!(k.pos.team_choice().unwrap().is_empty());
    k.pos.set_setting(&quinn, "teams_on", "1").unwrap();
    assert_eq!(k.pos.team_choice().unwrap(), [k.starter, k.main]);
    // A removed team leaves the list; one left is no choice.
    let teams = k.pos.list_teams(&k.s).unwrap();
    let keep: Vec<TeamInput> =
        teams.iter().filter(|t| t.id != k.main).map(|t| TeamInput { id: Some(t.id), name: t.name.clone(), name_alt: t.name_alt.clone() }).collect();
    k.pos.save_teams(&k.s, &keep).unwrap();
    assert!(k.pos.team_choice().unwrap().is_empty());
    // Switched off with an empty value.
    k.pos.set_setting(&quinn, TEAM_CHOICE_KEY, "").unwrap();
    assert_eq!(k.pos.setting(TEAM_CHOICE_KEY).unwrap().as_deref(), Some(""));
}
