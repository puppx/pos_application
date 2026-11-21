//! Kitchen tickets for several set menus sent with all their courses at
//! once: every starter together at the top, every main below, not set by
//! set. One set, or one course at a time, keeps the set-by-set layout.

use pos_core::*;

struct Kit {
    pos: Pos,
    s: Session,
    dinner: Product,
    dinner_secs: Vec<SetSection>,
    lunch: Product,
    lunch_secs: Vec<SetSection>,
    soup: Product,
    satay: Product,
    curry: Product,
    pad_thai: Product,
    rice: Product,
    table: i64,
}

fn setup() -> Kit {
    setup_with(false)
}

/// `teams`: the dishes cooked by the Starter, Curry and Rice teams.
fn setup_with(teams: bool) -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let s = pos.setup_first_admin("Owner", "1111").unwrap();
    pos.set_setting(&s, "prices_include_tax", "0").unwrap();
    let cat = pos.create_category(&s, "Food", None, 1).unwrap().id;
    let list = pos.list_teams(&s).unwrap();
    let team = |n: &str| if teams { list.iter().find(|t| t.name == n).map(|t| t.id) } else { None };
    let item = |pos: &mut Pos, name: &str, price: i64, team_id: Option<i64>| {
        pos.create_product(&s, NewProduct {
            category_id: Some(cat), sku: None, barcode: None, name: name.into(), price_cents: price, cost_cents: 0, tax_rate_bp: Some(0),
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id,
        })
        .unwrap()
    };
    let soup = item(&mut pos, "Soup", 500, team("Starter"));
    let satay = item(&mut pos, "Satay", 600, team("Starter"));
    let curry = item(&mut pos, "Green curry", 1200, team("Curry"));
    let pad_thai = item(&mut pos, "Pad Thai", 1100, team("Curry"));
    let rice = item(&mut pos, "Jasmine rice", 300, team("Rice"));
    let dinner = item(&mut pos, "Dinner set", 2500, None);
    let lunch = item(&mut pos, "Lunch set", 1900, None);
    let section = |name: &str, starter: bool, product_ids: Vec<i64>| SetSectionInput {
        name: name.into(), name_alt: None, pick: 1, starter, product_ids, with_ids: vec![],
    };
    let dinner_secs = pos
        .save_set(&s, dinner.id, &[
            section("Starter", true, vec![soup.id, satay.id]),
            section("Main", false, vec![curry.id]),
            section("Side", false, vec![rice.id]),
        ])
        .unwrap();
    let lunch_secs = pos
        .save_set(&s, lunch.id, &[
            section("Starter", true, vec![soup.id]),
            section("Main", false, vec![pad_thai.id, curry.id]),
            section("Side", false, vec![rice.id]),
        ])
        .unwrap();
    let floor = pos.create_floor(&s, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&s, floor.id, &[TableLayout { is_table: Some(true), id: None, name: "4".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    pos.set_kitchen_printer(&s, &PrinterConfig {
        target: PrinterTarget::File { path: std::env::temp_dir().join("pos-sets-by-course-unused.bin").display().to_string() },
        copy_delay_secs: 0, call_table_only: false, app_font: false, names: Some(PrintNames::English), ..Default::default()
    })
    .unwrap();
    Kit { pos, s, dinner, dinner_secs, lunch, lunch_secs, soup, satay, curry, pad_thai, rice, table: t[0].id }
}

fn dine_in(table: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table), channel_id: None, details: Default::default() }
}

fn pick(section: &SetSection, product: &Product) -> SetPick {
    SetPick { section_id: section.id, product_id: product.id, ..Default::default() }
}

/// A Dinner set: `starter`, Green curry, Jasmine rice.
fn dinner(k: &mut Kit, order: i64, starter: i64) {
    let secs = &k.dinner_secs;
    let picks = [
        SetPick { section_id: secs[0].id, product_id: starter, ..Default::default() },
        pick(&secs[1], &k.curry),
        pick(&secs[2], &k.rice),
    ];
    k.pos.add_set(&k.s, order, k.dinner.id, &picks, None).unwrap();
}

/// A Lunch set: Soup, `main`, Jasmine rice.
fn lunch(k: &mut Kit, order: i64, main: i64) {
    let secs = &k.lunch_secs;
    let picks = [pick(&secs[0], &k.soup), SetPick { section_id: secs[1].id, product_id: main, ..Default::default() }, pick(&secs[2], &k.rice)];
    k.pos.add_set(&k.s, order, k.lunch.id, &picks, None).unwrap();
}

/// The ticket's non-blank lines from the first dish heading to the last
/// dish, trimmed (rules keep their text: "--- STARTERS ---").
fn body(text: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    // After the order number line and its rule; up to the closing rule.
    let start = lines.iter().position(|l| l.starts_with('#')).unwrap() + 2;
    let end = lines.iter().rposition(|l| !l.is_empty() && l.chars().all(|c| c == '=')).unwrap();
    lines[start..end].to_vec()
}

/// "==== SET: X ====" -> "SET: X", "---- MAIN ----" -> "MAIN"; dishes as they are.
fn plain(lines: Vec<String>) -> Vec<String> {
    lines.into_iter().map(|l| l.trim_matches(|c| c == '=' || c == '-').trim().to_string()).collect()
}

fn ticket_text(k: &Kit, order: i64, sent: &[OrderItem]) -> String {
    let t = k.pos.kitchen_tickets(&k.s, order, sent, false).unwrap();
    assert_eq!(t.len(), 1, "one kitchen ticket");
    t[0].to_text(42)
}

#[test]
fn two_sets_sent_together_print_every_starter_above_every_main() {
    let mut k = setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.soup.id;
    dinner(&mut k, o, dish);
    let dish = k.satay.id;
    dinner(&mut k, o, dish);
    let dish = k.pad_thai.id;
    lunch(&mut k, o, dish);
    // "All courses together".
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    // The sets on top (two Dinner sets on one line), then the starters
    // (both Soups on one line), then the mains: mains before sides.
    assert_eq!(
        plain(body(&text)),
        [
            "SET: 2 x DINNER SET",
            "SET: LUNCH SET",
            "STARTERS",
            "2 x Soup",
            "1 x Satay",
            "MAINS",
            "2 x Green curry",
            "1 x Pad Thai",
            // A set's later section (sides) keeps its heading under the mains.
            "SIDE",
            "3 x Jasmine rice",
        ],
        "{text}"
    );
    // The headings are the ticket's usual rules.
    assert!(text.contains("========== SET: 2 x DINNER SET ===========\n"), "{text}");
    assert!(text.contains("---------------- STARTERS ----------------\n"), "{text}");
    assert!(text.contains("----------------- MAINS ------------------\n"), "{text}");
}

#[test]
fn only_identical_dishes_are_put_on_one_line() {
    let mut k = setup();
    let g = k.pos.create_option_group(&k.s, "Spice", 0, 1).unwrap();
    let g = k.pos.save_group_options(&k.s, g.id, &[OptionInput { id: None, name: "Extra spicy".into(), price_cents: 0, name_alt: None }]).unwrap();
    k.pos.set_product_option_groups(&k.s, k.curry.id, &[g.id]).unwrap();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.soup.id;
    dinner(&mut k, o, dish);
    // The second Dinner set's curry is extra spicy: its own line.
    let secs = k.dinner_secs.clone();
    let spicy = [
        pick(&secs[0], &k.soup),
        SetPick { section_id: secs[1].id, product_id: k.curry.id, option_ids: vec![g.options[0].id], ..Default::default() },
        pick(&secs[2], &k.rice),
    ];
    k.pos.add_set(&k.s, o, k.dinner.id, &spicy, None).unwrap();
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    assert_eq!(
        plain(body(&text)),
        ["SET: 2 x DINNER SET", "STARTERS", "2 x Soup", "MAINS", "1 x Green curry", "1 x Green curry", "+ Extra spicy", "SIDE", "2 x Jasmine rice"],
        "{text}"
    );
}

#[test]
fn one_set_sent_together_keeps_its_own_block() {
    let mut k = setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.soup.id;
    dinner(&mut k, o, dish);
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    assert_eq!(
        plain(body(&text)),
        ["SET: DINNER SET", "STARTER", "1 x Soup", "MAIN", "1 x Green curry", "SIDE", "1 x Jasmine rice"],
        "{text}"
    );
    k.pos.cancel_order(&k.s, o, "").unwrap();
    // One set line for two people is still one set: as before.
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap();
    let dish = k.soup.id;
    dinner(&mut k, o.id, dish);
    let parent = k.pos.get_order(&k.s, o.id).unwrap().items.into_iter().find(|i| i.is_set).unwrap();
    k.pos.update_item(&k.s, parent.id, &ItemUpdate { qty: 2, discount_cents: 0, note: None }).unwrap();
    let sent = k.pos.call_mains(&k.s, o.id).unwrap();
    let text = ticket_text(&k, o.id, &sent);
    assert_eq!(
        plain(body(&text)),
        ["SET: 2 x DINNER SET", "STARTER", "2 x Soup", "MAIN", "2 x Green curry", "SIDE", "2 x Jasmine rice"],
        "{text}"
    );
}

#[test]
fn two_sets_sent_course_by_course_keep_one_block_per_set() {
    let mut k = setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.satay.id;
    dinner(&mut k, o, dish);
    let dish = k.pad_thai.id;
    lunch(&mut k, o, dish);
    // Starters first...
    let sent = k.pos.send_to_kitchen(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    assert_eq!(plain(body(&text)), ["SET: DINNER SET", "STARTER", "1 x Satay", "SET: LUNCH SET", "STARTER", "1 x Soup"], "{text}");
    // ...then "Call mains".
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    assert!(text.contains("MAINS AWAY"), "{text}");
    assert_eq!(
        plain(body(&text)),
        [
            "SET: DINNER SET", "MAIN", "1 x Green curry", "SIDE", "1 x Jasmine rice",
            "SET: LUNCH SET", "MAIN", "1 x Pad Thai", "SIDE", "1 x Jasmine rice",
        ],
        "{text}"
    );
}

#[test]
fn dishes_off_the_set_stay_first_and_drinks_keep_their_own_ticket() {
    let mut k = setup();
    let cola = k.pos.create_product(&k.s, NewProduct {
        category_id: None, sku: None, barcode: None, name: "Cola".into(), price_cents: 0, cost_cents: 0, tax_rate_bp: Some(0),
        stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
    }).unwrap();
    k.pos.set_drink(&k.s, cola.id, true).unwrap();
    let secs = k.pos.save_set(&k.s, k.lunch.id, &[
        SetSectionInput { name: "Starter".into(), name_alt: None, pick: 1, starter: true, product_ids: vec![k.soup.id], with_ids: vec![] },
        SetSectionInput { name: "Main".into(), name_alt: None, pick: 1, starter: false, product_ids: vec![k.pad_thai.id], with_ids: vec![] },
        SetSectionInput { name: "Drink".into(), name_alt: None, pick: 1, starter: false, product_ids: vec![cola.id], with_ids: vec![] },
    ]).unwrap();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let picks = [pick(&secs[0], &k.soup), pick(&secs[1], &k.pad_thai), pick(&secs[2], &cola)];
    k.pos.add_set(&k.s, o, k.lunch.id, &picks, None).unwrap();
    k.pos.add_set(&k.s, o, k.lunch.id, &picks, None).unwrap();
    k.pos.add_item(&k.s, o, k.satay.id, 1, None, None, &[]).unwrap();
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let t = k.pos.kitchen_tickets(&k.s, o, &sent, false).unwrap();
    assert_eq!(t.iter().map(|t| t.banner.as_deref().map(|b| b.split(" / ").next().unwrap())).collect::<Vec<_>>(), [Some("DRINKS"), None]);
    let drinks = t[0].to_text(42);
    assert_eq!(plain(body(&drinks)), ["SET: 2 x LUNCH SET", "DRINKS", "2 x Cola"], "{drinks}");
    let food = t[1].to_text(42);
    assert_eq!(plain(body(&food)), ["1 x Satay", "SET: 2 x LUNCH SET", "STARTERS", "2 x Soup", "MAINS", "2 x Pad Thai"], "{food}");

    // Drinks off their own ticket: they head the sets' dishes.
    k.pos.set_setting(&k.s, "drinks_split", "0").unwrap();
    let text = ticket_text(&k, o, &sent);
    assert_eq!(
        plain(body(&text)),
        ["1 x Satay", "SET: 2 x LUNCH SET", "DRINKS", "2 x Cola", "STARTERS", "2 x Soup", "MAINS", "2 x Pad Thai"],
        "{text}"
    );
}

#[test]
fn each_teams_ticket_is_laid_out_by_course_too() {
    // Starters cooked by the Starter team, curries by the Curry team.
    let mut k = setup_with(true);
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.soup.id;
    dinner(&mut k, o, dish);
    let dish = k.pad_thai.id;
    lunch(&mut k, o, dish);
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let t = k.pos.kitchen_tickets(&k.s, o, &sent, false).unwrap();
    let texts: Vec<Vec<String>> = t.iter().map(|t| plain(body(&t.to_text(42)))).collect();
    assert_eq!(texts[0], ["SET: DINNER SET", "SET: LUNCH SET", "STARTERS", "2 x Soup"], "{texts:?}");
    assert_eq!(texts[1], ["SET: DINNER SET", "SET: LUNCH SET", "MAINS", "1 x Green curry", "1 x Pad Thai"], "{texts:?}");
    // A team with only the sides still prints them as SIDE, not MAINS.
    assert_eq!(texts[2], ["SET: DINNER SET", "SET: LUNCH SET", "SIDE", "2 x Jasmine rice"], "{texts:?}");

    // One shared ticket (team tickets off): each team's bar, then its dishes
    // under the sets' names again.
    k.pos.set_setting(&k.s, "teams_split", "0").unwrap();
    let text = ticket_text(&k, o, &sent);
    let lines = plain(body(&text));
    let sets = lines.iter().filter(|l| l.starts_with("SET: DINNER SET")).count();
    assert_eq!(sets, 3, "{text}");
}

#[test]
fn cancelling_two_sets_sent_together_prints_them_by_course() {
    let mut k = setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.soup.id;
    dinner(&mut k, o, dish);
    let dish = k.curry.id;
    lunch(&mut k, o, dish);
    k.pos.call_mains(&k.s, o).unwrap();
    let job = k.pos.cancel_order_for_kitchen(&k.s, o, "").unwrap().expect("a CANCELLED ticket");
    let text = String::from_utf8_lossy(&job.bytes()).to_string();
    let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s} missing: {text}"));
    assert!(at("CANCELLED") < at("SET: DINNER SET") && at("SET: DINNER SET") < at("SET: LUNCH SET"), "{text}");
    assert!(at("SET: LUNCH SET") < at("STARTERS") && at("STARTERS") < at("2 x Soup") && at("2 x Soup") < at("MAINS"), "{text}");
    assert!(at("MAINS") < at("2 x Green curry") && at("2 x Green curry") < at("2 x Jasmine rice"), "{text}");

    // Removing one of two sets: that set's own block, as before.
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.soup.id;
    dinner(&mut k, o, dish);
    let dish = k.curry.id;
    lunch(&mut k, o, dish);
    k.pos.call_mains(&k.s, o).unwrap();
    let lunch_line = k.pos.get_order(&k.s, o).unwrap().items.into_iter().find(|i| i.is_set && i.product_id == k.lunch.id).unwrap();
    let (_, job) = k.pos.update_item_for_kitchen(&k.s, lunch_line.id, &ItemUpdate { qty: 0, discount_cents: 0, note: None }).unwrap();
    let text = String::from_utf8_lossy(&job.expect("a CANCELLED ticket").bytes()).to_string();
    assert!(text.contains("SET: LUNCH SET") && !text.contains("DINNER") && !text.contains("STARTERS"), "{text}");
    assert!(text.find("STARTER").unwrap() < text.find("1 x Soup").unwrap(), "{text}");
}

/// A Dinner set with a note on the set line: `starter`, Green curry, Jasmine rice.
fn dinner_noted(k: &mut Kit, order: i64, starter: i64, note: &str) {
    let secs = &k.dinner_secs;
    let picks = [SetPick { section_id: secs[0].id, product_id: starter, ..Default::default() }, pick(&secs[1], &k.curry), pick(&secs[2], &k.rice)];
    k.pos.add_set(&k.s, order, k.dinner.id, &picks, Some(note)).unwrap();
}

#[test]
fn a_set_lines_note_prints_under_the_set_on_every_kind_of_ticket() {
    let mut k = setup();
    // All courses together, one set: the note under the set's name.
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.soup.id;
    dinner_noted(&mut k, o, dish, "no peanuts");
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    assert_eq!(
        plain(body(&text)),
        ["SET: DINNER SET", "** no peanuts", "STARTER", "1 x Soup", "MAIN", "1 x Green curry", "SIDE", "1 x Jasmine rice"],
        "{text}"
    );
    // Reprint.
    let order = k.pos.get_order(&k.s, o).unwrap();
    let t = k.pos.kitchen_tickets(&k.s, o, &order.items, true).unwrap();
    let text = t[0].to_text(42);
    assert!(text.contains("REPRINT") && text.contains("** no peanuts"), "{text}");
    // Cancelled.
    let job = k.pos.cancel_order_for_kitchen(&k.s, o, "").unwrap().expect("a CANCELLED ticket");
    let text = String::from_utf8_lossy(&job.bytes()).to_string();
    assert!(text.contains("CANCELLED") && text.contains("no peanuts"), "{text}");

    // Course by course: the starters, then "call mains", each with the note.
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.satay.id;
    dinner_noted(&mut k, o, dish, "nut allergy");
    let sent = k.pos.send_to_kitchen(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    assert_eq!(plain(body(&text)), ["SET: DINNER SET", "** nut allergy", "STARTER", "1 x Satay"], "{text}");
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    assert!(text.contains("MAINS AWAY"), "{text}");
    assert_eq!(plain(body(&text)), ["SET: DINNER SET", "** nut allergy", "MAIN", "1 x Green curry", "SIDE", "1 x Jasmine rice"], "{text}");
    // Added later: another set with its own note.
    let dish = k.soup.id;
    dinner_noted(&mut k, o, dish, "mild");
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    assert!(text.contains("ADDITIONAL") && text.contains("** mild") && !text.contains("nut allergy"), "{text}");
}

#[test]
fn several_sets_by_course_name_each_sets_note() {
    let mut k = setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.soup.id;
    dinner_noted(&mut k, o, dish, "no peanuts");
    let dish = k.pad_thai.id;
    lunch(&mut k, o, dish);
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    let lines = plain(body(&text));
    assert_eq!(lines[..2], ["SET: DINNER SET", "SET: LUNCH SET"], "{text}");
    // The note (wrapped on the double-height line) before the starters.
    assert_eq!(format!("{} {}", lines[2], lines[3]), "** DINNER SET: no peanuts", "{text}");
    assert_eq!(lines[4], "STARTERS", "{text}");
}

#[test]
fn notes_on_a_sets_dishes_and_a_changed_set_note_are_printed() {
    let mut k = setup();
    let o = k.pos.open_order(&k.s, &dine_in(k.table)).unwrap().id;
    let dish = k.soup.id;
    dinner(&mut k, o, dish);
    let items = k.pos.get_order(&k.s, o).unwrap().items;
    let curry = items.iter().find(|i| i.product_id == k.curry.id).unwrap().clone();
    let set = items.iter().find(|i| i.is_set).unwrap().clone();
    // A note on one dish of the set, before sending.
    k.pos.update_item(&k.s, curry.id, &ItemUpdate { qty: curry.qty, discount_cents: 0, note: Some("extra hot".into()) }).unwrap();
    let sent = k.pos.call_mains(&k.s, o).unwrap();
    let text = ticket_text(&k, o, &sent);
    assert!(text.find("1 x Green curry").unwrap() < text.find("** extra hot").unwrap(), "{text}");
    // A note added to the set line after it was sent: a CHANGED ticket with it.
    let (_, job) = k.pos.update_item_for_kitchen(&k.s, set.id, &ItemUpdate { qty: set.qty, discount_cents: 0, note: Some("no chilli".into()) }).unwrap();
    let text = String::from_utf8_lossy(&job.expect("a CHANGED ticket").bytes()).to_string();
    assert!(text.contains("CHANGED") && text.contains("SET: DINNER SET") && text.contains("no chilli"), "{text}");
    // A note changed on a sent dish of the set.
    let (_, job) = k.pos.update_item_for_kitchen(&k.s, curry.id, &ItemUpdate { qty: curry.qty, discount_cents: 0, note: Some("very hot".into()) }).unwrap();
    let text = String::from_utf8_lossy(&job.expect("a CHANGED ticket").bytes()).to_string();
    assert!(text.contains("very hot") && text.contains("no chilli"), "{text}");
}
