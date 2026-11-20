//! Load test: a busy restaurant with a long history. Not run by default:
//!   cargo test --release -p pos-core --test perf -- --ignored --nocapture

use std::time::Instant;

use pos_core::*;

#[test]
#[ignore]
fn busy_restaurant() {
    let dir = tempfile::tempdir().unwrap();
    let mut pos = Pos::open(dir.path().join("pos.db")).unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();

    // Menu: 20 categories x 100 items.
    let mut items = Vec::new();
    for c in 0..20 {
        let cat = pos.create_category(&admin, &format!("Category {c}"), None, c).unwrap();
        for i in 0..100 {
            let p = pos
                .create_product(&admin, NewProduct {
                    category_id: Some(cat.id),
                    sku: None,
                    barcode: None,
                    name: format!("Dish {c}-{i}"),
                    price_cents: 500 + i * 10,
                    cost_cents: 0,
                    tax_rate_bp: 1500,
                    stock_qty: 1_000,
                    track_stock: i % 10 == 0,
                    starter: false, name_alt: None, prep: false, team_id: None,
                })
                .unwrap();
            items.push(p.id);
        }
    }

    // Two floors of 30 tables.
    let mut tables = Vec::new();
    for f in 0..2 {
        let floor = pos.create_floor(&admin, &format!("Floor {f}")).unwrap();
        let layout: Vec<TableLayout> = (0..30)
            .map(|i| TableLayout {
                is_table: Some(true),
                id: None,
                name: format!("T{}", f * 100 + i),
                shape: TableShape::Rect,
                x: (i % 10) * 95,
                y: (i / 10) * 200,
                w: 80,
                h: 80,
            })
            .collect();
        tables.extend(pos.save_floor_layout(&admin, floor.id, &layout).unwrap().into_iter().map(|t| (floor.id, t.id)));
    }

    let t = Instant::now();
    let s = pos.login("1111").unwrap();
    println!("PIN login (argon2id 19 MiB):       {:?}", t.elapsed());

    let t = Instant::now();
    let menu = pos.menu(&s).unwrap();
    println!("load menu ({} items):           {:?}", menu.len(), t.elapsed());

    // History: 20k paid orders of 6 items each (months of trading).
    let history = 20_000;
    let t = Instant::now();
    let mut add_time = std::time::Duration::ZERO;
    let mut pay_time = std::time::Duration::ZERO;
    for n in 0..history {
        let o = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
        let a = Instant::now();
        for j in 0..6 {
            pos.add_item(&s, o.id, items[(n * 7 + j * 13) % items.len()], 1, None, None, &[]).unwrap();
        }
        add_time += a.elapsed();
        pos.send_to_kitchen(&s, o.id).unwrap();
        let p = Instant::now();
        pos.pay_order(&s, o.id, PaymentMethod::Card, 0).unwrap();
        pay_time += p.elapsed();
    }
    println!("{history} full orders:              {:?}", t.elapsed());
    println!("  add item (tap):                  {:?} avg", add_time / (history as u32 * 6));
    println!("  pay order (6 lines, fsync'd):    {:?} avg", pay_time / history as u32);

    // Full house: every table has an open order with 8 lines.
    for (i, (_, table)) in tables.iter().enumerate() {
        let o = pos.open_order(&s, &NewOrder { order_type: OrderType::DineIn, table_id: Some(*table), channel_id: None, details: Default::default() }).unwrap();
        for j in 0..8 {
            pos.add_item(&s, o.id, items[(i * 11 + j * 17) % items.len()], 1, None, None, &[]).unwrap();
        }
    }
    let floor_id = tables[0].0;
    let t = Instant::now();
    for _ in 0..100 {
        pos.floor_status(&s, floor_id).unwrap();
    }
    println!("floor view (30 busy tables):       {:?} avg", t.elapsed() / 100);

    let t = Instant::now();
    for _ in 0..100 {
        pos.list_open_orders(&s).unwrap();
    }
    println!("open orders board (60 orders):     {:?} avg", t.elapsed() / 100);

    let t = Instant::now();
    let sum = pos.sales_summary(&s, 0, i64::MAX).unwrap();
    let top = pos.top_products(&s, 0, i64::MAX, 20).unwrap();
    println!("report over all {} orders:      {:?}", sum.order_count, t.elapsed());
    assert_eq!(sum.order_count, history as i64);
    assert_eq!(top.len(), 20);

    let size = std::fs::metadata(dir.path().join("pos.db")).unwrap().len();
    println!("database size:                     {:.1} MB", size as f64 / 1e6);
}

/// The kitchen screen (every 4 s per screen) and the floor view (every 10 s
/// per till) must not slow down as the history grows: both start from the
/// open orders, never from every dish ever sold. Not run by default:
///   cargo test -p pos-core --test perf kitchen_and_floor -- --ignored --nocapture
/// On a copy of a real database (it is migrated, so never the live file):
///   POS_PERF_DB=/path/copy.db POS_PERF_PIN=1234 POS_PERF_USER=1 cargo test ...
#[test]
#[ignore]
fn kitchen_and_floor_with_a_long_history() {
    let (pos, s) = match std::env::var("POS_PERF_DB") {
        Ok(path) => {
            let mut pos = Pos::open(path).unwrap();
            let pin = std::env::var("POS_PERF_PIN").unwrap_or_else(|_| "1234".into());
            let user = std::env::var("POS_PERF_USER").ok().and_then(|u| u.parse().ok());
            let s = pos.login_as(user, &pin, &mut LoginThrottle::default()).unwrap();
            (pos, s)
        }
        Err(_) => synthetic_history(),
    };
    let floors = pos.list_floors(&s).unwrap();
    let time = |label: &str, runs: u32, f: &mut dyn FnMut() -> usize| {
        let n = f();
        let t = Instant::now();
        for _ in 0..runs {
            f();
        }
        let avg = t.elapsed() / runs;
        println!("{label:<34} {avg:>10.3?} avg ({n} rows)");
        avg
    };
    let board = time("kitchen screen, all", 50, &mut || pos.kitchen_board(&s, "all").unwrap().len());
    time("kitchen screen, drinks", 50, &mut || pos.kitchen_board(&s, "drinks").unwrap().len());
    let mut floor = std::time::Duration::ZERO;
    for f in &floors {
        floor = floor.max(time(&format!("floor view, {}", f.name), 50, &mut || pos.floor_status(&s, f.id).unwrap().len()));
    }
    time("open orders board", 50, &mut || pos.list_open_orders(&s).unwrap().len());
    // Generous even for a debug build on a slow machine; the old full scans
    // took 16-24 ms per call on a year of data in a release build.
    assert!(board < std::time::Duration::from_millis(15), "kitchen screen too slow: {board:?}");
    assert!(floor < std::time::Duration::from_millis(15), "floor view too slow: {floor:?}");
}

/// 4,000 paid orders of 5 dishes (all in the kitchen and done), and a full
/// floor of 30 open tables with dishes in the kitchen.
fn synthetic_history() -> (Pos, Session) {
    let mut pos = Pos::open_in_memory().unwrap();
    let s = pos.setup_first_admin("Owner", "1111").unwrap();
    // Paid orders leave the kitchen screen at once, as they would hours later.
    pos.set_setting(&s, "kds_paid_hours", "0").unwrap();
    let cat = pos.create_category(&s, "Mains", None, 0).unwrap();
    let items: Vec<i64> = (0..40)
        .map(|i| {
            pos.create_product(&s, NewProduct {
                category_id: Some(cat.id),
                sku: None,
                barcode: None,
                name: format!("Dish {i}"),
                price_cents: 500 + i * 10,
                cost_cents: 0,
                tax_rate_bp: 1500,
                stock_qty: 0,
                track_stock: false,
                starter: false, name_alt: None, prep: false, team_id: None,
            })
            .unwrap()
            .id
        })
        .collect();
    let floor = pos.create_floor(&s, "Main").unwrap();
    let layout: Vec<TableLayout> = (0..30)
        .map(|i| TableLayout { is_table: Some(true), id: None, name: format!("T{i}"), shape: TableShape::Rect, x: (i % 10) * 95, y: (i / 10) * 200, w: 80, h: 80 })
        .collect();
    let tables = pos.save_floor_layout(&s, floor.id, &layout).unwrap();
    for n in 0..4_000usize {
        let o = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
        for j in 0..5 {
            pos.add_item(&s, o.id, items[(n * 7 + j * 13) % items.len()], 1, None, None, &[]).unwrap();
        }
        let sent: Vec<i64> = pos.send_to_kitchen(&s, o.id).unwrap().iter().map(|i| i.id).collect();
        pos.kitchen_done(&s, &sent, true).unwrap();
        pos.pay_order(&s, o.id, PaymentMethod::Card, 0).unwrap();
    }
    for (i, t) in tables.iter().enumerate() {
        let o = pos.open_order(&s, &NewOrder { order_type: OrderType::DineIn, table_id: Some(t.id), channel_id: None, details: Default::default() }).unwrap();
        for j in 0..6 {
            pos.add_item(&s, o.id, items[(i * 11 + j * 17) % items.len()], 1, None, None, &[]).unwrap();
        }
        pos.send_to_kitchen(&s, o.id).unwrap();
    }
    (pos, s)
}
