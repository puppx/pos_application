//! Two tills on one order (QA round 31): a pay dialog or a dish shown from
//! before another till's change never takes a different amount or undoes
//! that change; a bill with nothing to pay closes; a bill never drops
//! below what is already paid; staff cancel a mistake order with a confirm.

use pos_core::*;

struct Kit {
    pos: Pos,
    admin: Session,
    staff: Session,
    tables: Vec<i64>,
    curry: Product,
    beer: Product,
    water: Product,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let item = |pos: &mut Pos, name: &str, price: i64| {
        pos.create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: name.into(), price_cents: price, cost_cents: 0, tax_rate_bp: Some(0),
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap()
    };
    let curry = item(&mut pos, "Curry", 1200);
    let beer = item(&mut pos, "Beer", 500);
    let water = item(&mut pos, "Tap water", 0);
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let layout: Vec<TableLayout> = (1..=3)
        .map(|n| TableLayout { is_table: Some(true), id: None, name: n.to_string(), shape: TableShape::Rect, x: n * 100, y: 0, w: 80, h: 80 })
        .collect();
    let tables = pos.save_floor_layout(&admin, floor.id, &layout).unwrap().into_iter().map(|t| t.id).collect();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    let staff = pos.login("2222").unwrap();
    Kit { pos, admin, staff, tables, curry, beer, water }
}

fn takeaway(k: &mut Kit) -> Order {
    k.pos.open_order(&k.admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap()
}

fn dine(k: &mut Kit, table: usize) -> Order {
    let t = k.tables[table];
    k.pos.open_order(&k.admin, &NewOrder { order_type: OrderType::DineIn, table_id: Some(t), channel_id: None, details: Default::default() }).unwrap()
}

fn card(amount: Option<i64>) -> PaymentInput {
    PaymentInput { method: PaymentMethod::Card, amount_cents: amount, tendered_cents: 0, tip_cents: 0, items: None }
}

fn msg(e: PosError) -> String {
    e.to_string()
}

#[test]
fn a_stale_pay_dialog_takes_nothing_and_says_the_bill_changed() {
    let mut k = setup();
    let o = takeaway(&mut k);
    let o = k.pos.add_item(&k.admin, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    // The dialog shows 12.00 left; another till adds a beer meanwhile.
    let shown = o.total_cents;
    k.pos.add_item(&k.staff, o.id, k.beer.id, 1, None, None, &[]).unwrap();
    let e = k.pos.add_payment_checked(&k.admin, None, o.id, &card(Some(shown)), Some(shown)).unwrap_err();
    assert!(matches!(e, PosError::Conflict(_)));
    assert_eq!(msg(e), BILL_CHANGED);
    let now = k.pos.get_order(&k.admin, o.id).unwrap();
    assert!(now.payments.is_empty() && now.status == "open");
    // "Everything" without an amount is checked too (pay_order's path).
    assert_eq!(msg(k.pos.add_payment_checked(&k.admin, None, o.id, &card(None), Some(shown)).unwrap_err()), BILL_CHANGED);
    // The dialog reloads and shows 17.00: that is taken, and the bill closes.
    let paid = k.pos.add_payment_checked(&k.admin, None, o.id, &card(Some(1700)), Some(1700)).unwrap();
    assert_eq!((paid.status.as_str(), paid.paid_cents), ("paid", 1700));
    // Left out: as before (the rest).
    let o = takeaway(&mut k);
    k.pos.add_item(&k.admin, o.id, k.beer.id, 1, None, None, &[]).unwrap();
    assert_eq!(k.pos.add_payment_checked(&k.admin, None, o.id, &card(None), None).unwrap().status, "paid");
}

#[test]
fn a_part_payment_by_amount_after_a_change_is_refused_too() {
    let mut k = setup();
    let o = takeaway(&mut k);
    k.pos.add_item(&k.admin, o.id, k.curry.id, 2, None, None, &[]).unwrap(); // 24.00
    let o = k.pos.add_payment_checked(&k.admin, None, o.id, &card(Some(1200)), Some(2400)).unwrap();
    assert_eq!(o.paid_cents, 1200);
    // Split dialog still shows 12.00 left; another till removes a curry (now 0 left: covered).
    let line = o.items[0].id;
    k.pos.update_item(&k.admin, line, &ItemUpdate { qty: 1, discount_cents: 0, note: None }).unwrap();
    assert_eq!(msg(k.pos.add_payment_checked(&k.admin, None, o.id, &card(Some(1200)), Some(1200)).unwrap_err()), BILL_CHANGED);
}

#[test]
fn a_bill_with_nothing_to_pay_closes_with_no_payment() {
    let mut k = setup();
    // A free dish.
    let o = takeaway(&mut k);
    let o = k.pos.add_item(&k.staff, o.id, k.water.id, 2, None, None, &[]).unwrap();
    assert_eq!(o.total_cents, 0);
    let paid = k.pos.add_payment_checked(&k.staff, None, o.id, &card(None), Some(0)).unwrap();
    assert_eq!((paid.status.as_str(), paid.payments.len(), paid.total_cents), ("paid", 0, 0));
    // Fully comped by a manager: the same, through pay_order's path, even by "Other".
    let o = takeaway(&mut k);
    let o = k.pos.add_item(&k.admin, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    k.pos.update_item(&k.admin, o.items[0].id, &ItemUpdate { qty: 1, discount_cents: 1200, note: None }).unwrap();
    let other = PaymentInput { method: PaymentMethod::Other, ..card(None) };
    let paid = k.pos.add_payment(&k.staff, o.id, &other).unwrap();
    assert_eq!((paid.status.as_str(), paid.payments.len()), ("paid", 0));
    // Logged, reports count the orders and no money, and the day can close.
    for id in [o.id - 1, o.id] {
        let log = k.pos.order_activity(&k.admin, id).unwrap();
        assert!(log.iter().any(|r| r.action == "order.closed_nothing_to_pay"), "{:?}", log.iter().map(|r| &r.action).collect::<Vec<_>>());
    }
    let sum = k.pos.sales_summary(&k.admin, 0, i64::MAX).unwrap();
    assert_eq!((sum.order_count, sum.total_cents), (2, 0));
    assert!(sum.by_payment.iter().all(|g| g.total_cents == 0));
    k.pos.close_day(&k.admin).unwrap();
    // A tip on a bill with nothing to pay: refused, not dropped.
    let o = takeaway(&mut k);
    k.pos.add_item(&k.admin, o.id, k.water.id, 1, None, None, &[]).unwrap();
    let tip = PaymentInput { tip_cents: 200, ..card(None) };
    assert!(k.pos.add_payment(&k.admin, o.id, &tip).unwrap_err().to_string().contains("tip"));
    assert_eq!(k.pos.get_order(&k.admin, o.id).unwrap().status, "open");
    // An empty order is still refused.
    let o = takeaway(&mut k);
    assert!(k.pos.add_payment(&k.admin, o.id, &card(None)).is_err());
}

#[test]
fn after_a_part_payment_the_bill_never_drops_below_what_is_paid() {
    let mut k = setup();
    let o = dine(&mut k, 0);
    k.pos.add_item(&k.admin, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    let o = k.pos.add_item(&k.admin, o.id, k.beer.id, 2, None, None, &[]).unwrap(); // 22.00
    let o = k.pos.add_payment(&k.admin, o.id, &card(Some(1500))).unwrap();
    let (curry, beer) = (o.items[0].clone(), o.items[1].clone());
    let below = |r: Result<Order>| assert_eq!(msg(r.unwrap_err()), BELOW_PAID);
    // Removing the curry (22 -> 10), or discounting it by 8.00 (-> 14).
    below(k.pos.update_item(&k.admin, curry.id, &ItemUpdate { qty: 0, discount_cents: 0, note: None }));
    below(k.pos.update_item(&k.admin, curry.id, &ItemUpdate { qty: 1, discount_cents: 800, note: None }));
    // A smaller change that still covers the payment is fine (-> 17.00).
    k.pos.update_item(&k.admin, beer.id, &ItemUpdate { qty: 1, discount_cents: 0, note: None }).unwrap();
    // Service charge down, tax for the order changed (prices before tax, so tax counts).
    k.pos.set_setting(&k.admin, "prices_include_tax", "0").unwrap();
    let o = dine(&mut k, 1);
    k.pos.add_item(&k.admin, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    k.pos.set_service_charge(&k.admin, o.id, 1000).unwrap(); // 13.20
    k.pos.set_order_tax(&k.admin, o.id, Some(2000)).unwrap(); // 12.00 + 2.40 tax + 1.20 = 15.60
    let o = k.pos.get_order(&k.admin, o.id).unwrap();
    let o = k.pos.add_payment(&k.admin, o.id, &card(Some(o.total_cents - 10))).unwrap();
    below(k.pos.set_service_charge(&k.admin, o.id, 0));
    below(k.pos.set_order_tax(&k.admin, o.id, Some(0)));
    below(k.pos.set_order_tax(&k.admin, o.id, None));
    let now = k.pos.get_order(&k.admin, o.id).unwrap();
    assert_eq!((now.service_bp, now.tax_bp, now.total_cents), (1000, Some(2000), o.total_cents));
    // Raising them is fine.
    k.pos.set_service_charge(&k.admin, o.id, 1250).unwrap();
    // Moving dishes off, or joining it onto another bill, is refused while part paid.
    assert!(k.pos.move_items(&k.admin, o.id, k.tables[2], &[(now.items[0].id, 1)]).is_err());
    assert!(k.pos.merge_into_table(&k.admin, o.id, k.tables[0]).is_err());
    // Dishes moved onto a part-paid bill only raise it.
    let t3 = dine(&mut k, 2);
    let t3 = k.pos.add_item(&k.admin, t3.id, k.beer.id, 1, None, None, &[]).unwrap();
    let joined = k.pos.move_items(&k.admin, t3.id, k.tables[1], &[(t3.items[0].id, 1)]).unwrap();
    assert!(joined.total_cents > joined.paid_cents);
}

#[test]
fn a_stale_line_change_is_refused_and_plus_minus_add_up() {
    let mut k = setup();
    let o = takeaway(&mut k);
    let o = k.pos.add_item(&k.admin, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    let line = o.items[0].clone();
    // Till B taps + twice (deltas, from whatever the line has).
    for _ in 0..2 {
        let check = ItemCheck { qty_delta: Some(1), ..Default::default() };
        // The update's own fields are stale (qty 1): the delta wins.
        let u = k.pos.resolve_item_update(line.id, &ItemUpdate { qty: 2, discount_cents: 0, note: None }, &check).unwrap();
        k.pos.update_item(&k.staff, line.id, &u).unwrap();
    }
    assert_eq!(k.pos.get_order(&k.admin, o.id).unwrap().items[0].qty, 3);
    // Till A still shows qty 1 and edits the note: refused, nothing lost.
    let stale = ItemCheck { expected_qty: Some(1), expected_discount_cents: Some(0), qty_delta: None };
    let e = k.pos.resolve_item_update(line.id, &ItemUpdate { qty: 1, discount_cents: 0, note: Some("mild".into()) }, &stale).unwrap_err();
    assert!(matches!(e, PosError::Conflict(_)));
    assert_eq!(msg(e), ITEM_CHANGED);
    // Up to date: saved.
    let fresh = ItemCheck { expected_qty: Some(3), ..Default::default() };
    let u = k.pos.resolve_item_update(line.id, &ItemUpdate { qty: 3, discount_cents: 0, note: Some("mild".into()) }, &fresh).unwrap();
    let o2 = k.pos.update_item(&k.admin, line.id, &u).unwrap();
    assert_eq!((o2.items[0].qty, o2.items[0].note.as_deref()), (3, Some("mild")));
    // − keeps the note and the discount's share; − on the last one removes it.
    let o2 = k.pos.update_item(&k.admin, line.id, &ItemUpdate { qty: 3, discount_cents: 900, note: Some("mild".into()) }).unwrap();
    assert_eq!(o2.items[0].discount_cents, 900);
    let minus = ItemCheck { qty_delta: Some(-1), ..Default::default() };
    let u = k.pos.resolve_item_update(line.id, &ItemUpdate { qty: 9, discount_cents: 0, note: None }, &minus).unwrap();
    assert_eq!((u.qty, u.discount_cents, u.note.as_deref()), (2, 600, Some("mild")));
    k.pos.update_item(&k.admin, line.id, &u).unwrap();
    for _ in 0..2 {
        let u = k.pos.resolve_item_update(line.id, &ItemUpdate { qty: 1, discount_cents: 0, note: None }, &minus).unwrap();
        k.pos.update_item(&k.admin, line.id, &u).unwrap();
    }
    assert!(k.pos.get_order(&k.admin, o.id).unwrap().items.is_empty());
    // A line that is gone: not found.
    assert!(matches!(k.pos.resolve_item_update(line.id, &ItemUpdate { qty: 1, discount_cents: 0, note: None }, &minus), Err(PosError::NotFound)));
}

#[test]
fn a_note_alone_goes_on_the_dish_as_it_is_now() {
    let mut k = setup();
    let o = takeaway(&mut k);
    let o = k.pos.add_item(&k.admin, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    let line = o.items[0].id;
    // Another till makes it 3; this screen still shows 1 and types a note.
    k.pos.update_item(&k.staff, line, &ItemUpdate { qty: 3, discount_cents: 0, note: None }).unwrap();
    let note_only = ItemCheck { qty_delta: Some(0), expected_discount_cents: Some(0), ..Default::default() };
    let u = k.pos.resolve_item_update(line, &ItemUpdate { qty: 1, discount_cents: 0, note: Some("no chilli".into()) }, &note_only).unwrap();
    let it = k.pos.update_item(&k.admin, line, &u).unwrap().items[0].clone();
    assert_eq!((it.qty, it.discount_cents, it.note.as_deref()), (3, 0, Some("no chilli")));
    // A discount sent with a note alone is refused, not dropped: nothing changes.
    let bare = ItemCheck { qty_delta: Some(0), ..Default::default() };
    let e = k.pos.resolve_item_update(line, &ItemUpdate { qty: 3, discount_cents: 900, note: Some("mild".into()) }, &bare).unwrap_err();
    assert!(e.to_string().contains("send the discount separately"), "{e}");
    // The discount this till showed is checked: changed on another till = a conflict.
    k.pos.update_item(&k.staff, line, &ItemUpdate { qty: 3, discount_cents: 300, note: Some("no chilli".into()) }).unwrap();
    assert_eq!(msg(k.pos.resolve_item_update(line, &ItemUpdate { qty: 3, discount_cents: 0, note: Some("mild".into()) }, &note_only).unwrap_err()), ITEM_CHANGED);
    let it = k.pos.get_order(&k.admin, o.id).unwrap().items[0].clone();
    assert_eq!((it.discount_cents, it.note.as_deref()), (300, Some("no chilli")));
}

#[test]
fn a_dishes_payment_for_another_share_than_shown_is_refused() {
    let mut k = setup();
    let o = takeaway(&mut k);
    k.pos.add_item(&k.admin, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    let o = k.pos.add_item(&k.admin, o.id, k.beer.id, 1, None, None, &[]).unwrap(); // 17.00
    let curry = o.items[0].id;
    // Another till discounts the curry by 1.00 and puts 1.00 on the beer: the same left, another share.
    k.pos.update_item(&k.admin, curry, &ItemUpdate { qty: 1, discount_cents: 100, note: None }).unwrap();
    k.pos.set_item_price(&k.admin, None, o.items[1].id, 600).unwrap();
    let pay = |amount| PaymentInput { items: Some(vec![(curry, 1)]), ..card(amount) };
    let e = k.pos.add_payment_checked(&k.admin, None, o.id, &pay(Some(1200)), Some(1700)).unwrap_err();
    assert_eq!(msg(e), BILL_CHANGED);
    assert!(k.pos.get_order(&k.admin, o.id).unwrap().payments.is_empty());
    // The share as it is now: taken. Without an amount (older tills): as before.
    let paid = k.pos.add_payment_checked(&k.admin, None, o.id, &pay(Some(1100)), Some(1700)).unwrap();
    assert_eq!(paid.payments[0].amount_cents, 1100);
}

#[test]
fn details_edited_on_two_tills_keep_both_changes() {
    let mut k = setup();
    let o = takeaway(&mut k);
    let name = OrderDetails { customer_name: Some("Ann".into()), ..Default::default() };
    k.pos.update_order_details_fields(&k.admin, o.id, &name, Some(&["customer_name".into()])).unwrap();
    // The other till's form still has no name; it only changed the phone.
    let phone = OrderDetails { customer_phone: Some("0812".into()), ..Default::default() };
    let o = k.pos.update_order_details_fields(&k.staff, o.id, &phone, Some(&["customer_phone".into()])).unwrap();
    assert_eq!((o.customer_name.as_deref(), o.customer_phone.as_deref()), (Some("Ann"), Some("0812")));
    // Left out: every field, as before.
    let o = k.pos.update_order_details_fields(&k.staff, o.id, &phone, None).unwrap();
    assert_eq!(o.customer_name, None);
    assert!(k.pos.update_order_details_fields(&k.staff, o.id, &phone, Some(&["table_id".into()])).is_err());
}

#[test]
fn a_joined_bill_says_where_it_went() {
    let mut k = setup();
    let a = dine(&mut k, 0);
    k.pos.add_item(&k.admin, a.id, k.curry.id, 1, None, None, &[]).unwrap();
    let b = dine(&mut k, 1);
    k.pos.add_item(&k.admin, b.id, k.beer.id, 1, None, None, &[]).unwrap();
    assert!(k.pos.moved_to(&k.staff, a.id).unwrap().is_none(), "still here");
    let joined = k.pos.merge_into_table(&k.admin, a.id, k.tables[1]).unwrap();
    assert_eq!(joined.id, b.id);
    let to = k.pos.moved_to(&k.staff, a.id).unwrap().expect("joined onto table 2");
    assert_eq!((to.id, to.table_name.as_deref()), (b.id, Some("2")));
    // All its dishes moved: the same.
    let c = dine(&mut k, 2);
    let c = k.pos.add_item(&k.admin, c.id, k.beer.id, 1, None, None, &[]).unwrap();
    k.pos.move_items(&k.admin, c.id, k.tables[1], &[(c.items[0].id, 1)]).unwrap();
    assert_eq!(k.pos.moved_to(&k.staff, c.id).unwrap().map(|o| o.id), Some(b.id));
    assert!(k.pos.moved_to(&k.staff, 9999).unwrap().is_none());
}

#[test]
fn staff_cancel_a_mistake_order_with_no_pin_but_not_once_sent_or_paid() {
    let mut k = setup();
    let sam = k.staff.clone();
    let o = takeaway(&mut k);
    k.pos.add_item(&sam, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    assert!(k.pos.cancel_order_confirmed(&sam, o.id, "", None).unwrap().is_none());
    assert_eq!(k.pos.get_order(&k.admin, o.id).unwrap().status, "cancelled");
    // Sent: a PIN, as before.
    let o = takeaway(&mut k);
    k.pos.add_item(&sam, o.id, k.curry.id, 1, None, None, &[]).unwrap();
    k.pos.send_to_kitchen(&sam, o.id).unwrap();
    assert!(matches!(k.pos.cancel_order_confirmed(&sam, o.id, "", None), Err(PosError::Forbidden)));
    // Part paid: refused too.
    let o = takeaway(&mut k);
    k.pos.add_item(&sam, o.id, k.curry.id, 2, None, None, &[]).unwrap();
    k.pos.add_payment(&sam, o.id, &card(Some(500))).unwrap();
    assert!(matches!(k.pos.cancel_order_confirmed(&sam, o.id, "", None), Err(PosError::Forbidden)));
    assert_eq!(k.pos.get_order(&k.admin, o.id).unwrap().status, "open");
}
