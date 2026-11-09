use pos_core::*;

struct Kit {
    pos: Pos,
    admin: Session,
    table: i64,
    curry: Product,
    beer: Product,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    // Prices include tax (the default): the menu price is what is paid.
    let item = |pos: &mut Pos, name: &str, price: i64| {
        pos.create_product(&admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: name.into(), price_cents: price, cost_cents: 0, tax_rate_bp: 2000,
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap()
    };
    let curry = item(&mut pos, "Curry", 1200);
    let beer = item(&mut pos, "Beer", 500);
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let t = pos
        .save_floor_layout(&admin, floor.id, &[TableLayout { id: None, name: "1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    Kit { pos, admin, table: t[0].id, curry, beer }
}

fn open(k: &mut Kit) -> Order {
    let o = k.pos.open_order(&k.admin, &NewOrder { order_type: OrderType::DineIn, table_id: Some(k.table), channel_id: None, details: Default::default() }).unwrap();
    k.pos.add_item(&k.admin, o.id, k.curry.id, 2, None, None, &[]).unwrap();
    k.pos.add_item(&k.admin, o.id, k.beer.id, 2, None, None, &[]).unwrap() // 34.00
}

fn pay(method: PaymentMethod, amount: Option<i64>, tendered: i64, tip: i64, items: Option<Vec<(i64, i64)>>) -> PaymentInput {
    PaymentInput { method, amount_cents: amount, tendered_cents: tendered, tip_cents: tip, items }
}

#[test]
fn split_evenly_cash_and_card_with_a_tip() {
    let mut k = setup();
    let o = open(&mut k);
    assert_eq!(o.total_cents, 3400);
    // Half in cash (20.00 handed over), half by card with a 2.00 tip.
    let o = k.pos.add_payment(&k.admin, o.id, &pay(PaymentMethod::Cash, Some(1700), 2000, 0, None)).unwrap();
    assert_eq!((o.status.as_str(), o.paid_cents, o.payments[0].change_cents), ("open", 1700, 300));
    // More than is left: refused.
    assert!(k.pos.add_payment(&k.admin, o.id, &pay(PaymentMethod::Card, Some(1800), 0, 0, None)).is_err());
    // A part-paid order cannot be cancelled.
    assert!(k.pos.cancel_order(&k.admin, o.id, "").is_err());
    let o = k.pos.add_payment(&k.admin, o.id, &pay(PaymentMethod::Card, None, 0, 200, None)).unwrap();
    assert_eq!(o.status, "paid");
    assert_eq!((o.paid_cents, o.tip_cents, o.payment_method.clone()), (3400, 200, None));
    assert_eq!(o.payments[1].tendered_cents, 1900);

    let sum = k.pos.sales_summary(&k.admin, 0, i64::MAX).unwrap();
    assert_eq!((sum.order_count, sum.total_cents, sum.tip_cents), (1, 3400, 200));
    let by: Vec<_> = sum.by_payment.iter().map(|g| (g.key.as_str(), g.total_cents)).collect();
    assert!(by.contains(&("cash", 1700)) && by.contains(&("card", 1700)));

    // The receipt lists both payments, the tip and the change.
    let r = k.pos.receipt(&k.admin, o.id).unwrap();
    let labels: Vec<_> = r.payment.iter().map(|(l, _)| l.trim()).collect();
    assert_eq!(labels, ["Cash", "Change", "Card", "Tip"]);
}

#[test]
fn pay_for_chosen_dishes() {
    let mut k = setup();
    let o = open(&mut k);
    let (curry, beer) = (o.items[0].id, o.items[1].id);
    // One curry and one beer: their share, 17.00.
    let o = k.pos.add_payment(&k.admin, o.id, &pay(PaymentMethod::Card, None, 0, 0, Some(vec![(curry, 1), (beer, 1)]))).unwrap();
    assert_eq!(o.paid_cents, 1700);
    assert_eq!((o.items[0].paid_qty, o.items[1].paid_qty), (1, 1));
    // Paying for more than is left of a dish is refused.
    assert!(k.pos.add_payment(&k.admin, o.id, &pay(PaymentMethod::Card, None, 0, 0, Some(vec![(beer, 2)]))).is_err());
    // A payment taken by mistake can be taken back.
    let o2 = k.pos.add_payment(&k.admin, o.id, &pay(PaymentMethod::Card, None, 0, 0, Some(vec![(beer, 1)]))).unwrap();
    assert_eq!(o2.paid_cents, 2200);
    let o2 = k.pos.remove_payment(&k.admin, o.id, o2.payments[1].id).unwrap();
    assert_eq!((o2.paid_cents, o2.items[1].paid_qty), (1700, 1));
    // The rest: exactly what is left, and the order closes.
    let o = k.pos.add_payment(&k.admin, o.id, &pay(PaymentMethod::Cash, None, 5000, 0, Some(vec![(curry, 1), (beer, 1)]))).unwrap();
    assert_eq!((o.status.as_str(), o.paid_cents), ("paid", 3400));
    assert_eq!(o.payments[1].change_cents, 3300);
}

#[test]
fn one_tap_payment_still_works() {
    let mut k = setup();
    let o = open(&mut k);
    assert!(k.pos.pay_order(&k.admin, o.id, PaymentMethod::Cash, 3000).is_err());
    let o = k.pos.pay_order(&k.admin, o.id, PaymentMethod::Cash, 5000).unwrap();
    assert_eq!((o.status.as_str(), o.payment_method.as_deref(), o.change_cents), ("paid", Some("cash"), Some(1600)));
    assert_eq!(o.payments.len(), 1);
}

#[test]
fn shift_cash_up_expects_float_cash_tips_moves_less_refunds() {
    let mut k = setup();
    assert!(k.pos.current_shift(&k.admin).unwrap().is_none());
    assert!(k.pos.cash_move(&k.admin, "in", 1000, "").is_err()); // no shift yet
    k.pos.open_shift(&k.admin, 10_000).unwrap();
    assert!(k.pos.open_shift(&k.admin, 0).is_err()); // one at a time
    // Order 1: 34.00 cash (50.00 handed over, 16.00 change) + 1.00 cash tip.
    let o = open(&mut k);
    k.pos.add_payment(&k.admin, o.id, &pay(PaymentMethod::Cash, None, 5000, 100, None)).unwrap();
    // Order 2: card with a 3.00 tip; then voided? No: order 3 is voided.
    k.pos.add_item(&k.admin, k.pos.open_order(&k.admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap().id, k.beer.id, 1, None, None, &[]).unwrap();
    let o2 = k.pos.list_open_orders(&k.admin).unwrap()[0].id;
    k.pos.add_payment(&k.admin, o2, &pay(PaymentMethod::Card, None, 0, 300, None)).unwrap();
    let o3 = k.pos.open_order(&k.admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    k.pos.add_item(&k.admin, o3.id, k.curry.id, 1, None, None, &[]).unwrap();
    k.pos.pay_order(&k.admin, o3.id, PaymentMethod::Cash, 1200).unwrap();
    k.pos.void_order(&k.admin, o3.id, "wrong order").unwrap();
    // Money in and out.
    assert!(k.pos.cash_move(&k.admin, "out", 500, " ").is_err()); // needs a reason
    k.pos.cash_move(&k.admin, "in", 2000, "more change").unwrap();
    let sh = k.pos.cash_move(&k.admin, "out", 700, "milk").unwrap();
    assert_eq!((sh.cash_cents, sh.cash_tip_cents, sh.card_cents, sh.card_tip_cents), (3400 + 1200, 100, 500, 300));
    assert_eq!(sh.refund_cents, 1200);
    // 100 + 34 + 1 + 12 - 12 + 20 - 7 = 148.00
    assert_eq!(sh.expected_cents, 14_800);
    let closed = k.pos.close_shift(&k.admin, 14_500, "").unwrap();
    assert_eq!((closed.counted_cents, closed.difference_cents), (Some(14_500), Some(-300)));
    assert!(k.pos.current_shift(&k.admin).unwrap().is_none());
    let text = k.pos.shift_text(&closed).unwrap();
    assert!(text.contains("EXPECTED CASH") && text.contains("SHORT"));
    assert_eq!(k.pos.list_shifts(&k.admin, 10).unwrap().len(), 1);
}

#[test]
fn sold_out_until_end_of_day() {
    let mut k = setup();
    k.pos.set_sold_out(&k.admin, k.curry.id, true).unwrap();
    assert!(k.pos.get_product(&k.admin, k.curry.id).unwrap().sold_out);
    let o = k.pos.open_order(&k.admin, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    let err = k.pos.add_item(&k.admin, o.id, k.curry.id, 1, None, None, &[]).unwrap_err();
    assert!(err.to_string().contains("sold out"));
    k.pos.add_item(&k.admin, o.id, k.beer.id, 1, None, None, &[]).unwrap();
    k.pos.pay_order(&k.admin, o.id, PaymentMethod::Card, 0).unwrap();
    k.pos.close_day(&k.admin).unwrap();
    assert!(!k.pos.get_product(&k.admin, k.curry.id).unwrap().sold_out);
}

#[test]
fn move_dishes_and_join_tables() {
    let mut k = setup();
    let floor = k.pos.list_floors(&k.admin).unwrap()[0].id;
    let tables = k.pos
        .save_floor_layout(&k.admin, floor, &[
            TableLayout { id: Some(k.table), name: "1".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 },
            TableLayout { id: None, name: "2".into(), shape: TableShape::Rect, x: 100, y: 0, w: 80, h: 80 },
            TableLayout { id: None, name: "3".into(), shape: TableShape::Rect, x: 200, y: 0, w: 80, h: 80 },
        ])
        .unwrap();
    let (t2, t3) = (tables[1].id, tables[2].id);
    let o = open(&mut k); // 2 curry, 2 beer on table 1
    k.pos.send_to_kitchen(&k.admin, o.id).unwrap();
    let (curry, beer) = (o.items[0].id, o.items[1].id);
    // One curry to table 2 (free): a new order there, still marked sent.
    let o2 = k.pos.move_items(&k.admin, o.id, t2, &[(curry, 1)]).unwrap();
    assert_eq!(o2.table_id, Some(t2));
    assert_eq!((o2.items.len(), o2.items[0].qty, o2.items[0].sent_at.is_some()), (1, 1, true));
    let left = k.pos.get_order(&k.admin, o.id).unwrap();
    assert_eq!((left.items[0].qty, left.total_cents), (1, 2200));
    // The rest of table 1 joins table 2: one bill, table 1 is free.
    let joined = k.pos.merge_into_table(&k.admin, o.id, t2).unwrap();
    assert_eq!(joined.id, o2.id);
    assert_eq!(joined.total_cents, 3400);
    assert!(k.pos.table_order(&k.admin, k.table).unwrap().is_none());
    assert!(matches!(k.pos.get_order(&k.admin, o.id), Err(PosError::NotFound)));
    // Joining onto a free table just moves the order.
    let moved = k.pos.merge_into_table(&k.admin, joined.id, t3).unwrap();
    assert_eq!((moved.id, moved.table_id), (joined.id, Some(t3)));
    // Moving every dish away removes the empty order.
    let _ = beer;
    let all: Vec<_> = moved.items.iter().map(|i| (i.id, i.qty)).collect();
    let back = k.pos.move_items(&k.admin, moved.id, k.table, &all).unwrap();
    assert_eq!(back.total_cents, 3400);
    assert!(k.pos.get_order(&k.admin, moved.id).is_err());
    // Nothing cancelled in the reports.
    assert_eq!(k.pos.sales_summary(&k.admin, 0, i64::MAX).unwrap().cancelled_count, 0);
}
