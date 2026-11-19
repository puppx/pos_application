//! "Send slowly" (`slow_send`): every print to a printer follows that
//! printer's setting, and a paced send delivers exactly the same bytes.

use pos_core::*;

fn setup() -> (Pos, Session, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let s = pos.setup_first_admin("Owner", "1111").unwrap();
    let cat = pos.create_category(&s, "Mains", None, 1).unwrap();
    let mk = |pos: &mut Pos, name: &str, alt: Option<&str>, drink: bool| {
        let p = pos
            .create_product(&s, NewProduct {
                category_id: Some(cat.id), sku: None, barcode: None, name: name.into(), price_cents: 1200, cost_cents: 0, tax_rate_bp: 0,
                stock_qty: 0, track_stock: false, starter: false, name_alt: alt.map(Into::into), prep: false, team_id: None,
            })
            .unwrap();
        if drink {
            pos.set_drink(&s, p.id, true).unwrap();
        }
        p
    };
    let pad = mk(&mut pos, "Pad Thai", Some("ผัดไทย"), false);
    let tea = mk(&mut pos, "Thai tea", Some("ชาไทย"), true);
    let o = pos.open_order(&s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap();
    pos.add_item(&s, o.id, pad.id, 1, None, None, &[]).unwrap();
    pos.add_item(&s, o.id, tea.id, 1, None, None, &[]).unwrap();
    (pos, s, o.id)
}

fn file(dir: &tempfile::TempDir, n: &str) -> PrinterTarget {
    PrinterTarget::File { path: dir.path().join(n).display().to_string() }
}

#[test]
fn every_print_follows_its_printers_send_slowly_setting() {
    let (mut pos, s, order) = setup();
    let dir = tempfile::tempdir().unwrap();
    for slow in [true, false] {
        let kitchen = PrinterConfig { target: file(&dir, "k.bin"), copy_delay_secs: 0, slow_send: slow, ..Default::default() };
        pos.set_kitchen_printer(&s, &kitchen).unwrap();
        assert_eq!(pos.kitchen_printer().unwrap().slow_send, slow);
        // The bar printer has its own setting.
        let bar = KitchenRoute {
            label: "Bar".into(), target: file(&dir, "b.bin"), width: 42, copies: 1, team_ids: vec![], drinks: true, desserts: false,
            image_mode: ImageMode::Raster, slow_send: !slow,
        };
        pos.set_kitchen_routes(&s, std::slice::from_ref(&bar)).unwrap();
        assert_eq!(pos.kitchen_routes().unwrap()[0].slow_send, !slow);
        assert_eq!(pos.route_config(&bar).unwrap().slow_send, !slow);
        let items = pos.get_order(&s, order).unwrap().items;
        let job = pos.kitchen_job(&s, order, &items, false).unwrap().unwrap();
        assert_eq!((job.slow, job.more[0].slow), (slow, !slow));
        assert_eq!(pos.test_print_job(&s, &kitchen).unwrap().slow, slow);
        assert_eq!(pos.thai_test_job(&s, &kitchen).unwrap().slow, slow);
        let mut r = pos.receipt_settings().unwrap();
        r.printer = PrinterConfig { target: file(&dir, "r.bin"), copy_delay_secs: 0, slow_send: slow, ..Default::default() };
        pos.set_receipt_settings(&s, &r).unwrap();
        assert_eq!(pos.receipt_job(&s, order).unwrap().slow, slow);
        assert_eq!(pos.drawer_job(&s).unwrap().slow, slow);
        assert_eq!(pos.test_receipt_job(&s, &r, false).unwrap().slow, slow);
        assert_eq!(pos.test_receipt_job(&s, &r, true).unwrap().slow, slow);
    }
    // After payment: the receipt, the staff copy, the shift and day reports.
    pos.open_shift(&s, 0).unwrap();
    let mut r = pos.receipt_settings().unwrap();
    r.printer.slow_send = true;
    r.staff_copy = true;
    pos.set_receipt_settings(&s, &r).unwrap();
    r.drawer_on_cash = true;
    r.auto_print = true;
    pos.set_receipt_settings(&s, &r).unwrap();
    let paid = pos.pay_order(&s, order, PaymentMethod::Cash, 5000).unwrap();
    let job = pos.payment_print_job(&s, &paid).unwrap().unwrap();
    // Sent slowly, the drawer pulse goes on its own, ahead of anything still
    // waiting for the printer; the staff copy follows it (the bill was given).
    assert!(job.slow && job.urgent && job.parts.len() == 1 && job.parts[0].len() < 10);
    assert_eq!((job.more.len(), job.more[0].slow, job.more[0].urgent, job.more[0].parts.len()), (1, true, false, 1));
    assert_eq!(job.what, "Receipt #1");
    // The same bytes as without it, where the pulse leads the receipt.
    r.printer.slow_send = false;
    pos.set_receipt_settings(&s, &r).unwrap();
    let fast = pos.payment_print_job(&s, &paid).unwrap().unwrap();
    assert!(fast.more.is_empty());
    assert_eq!(fast.bytes(), job.bytes());
    r.printer.slow_send = true;
    pos.set_receipt_settings(&s, &r).unwrap();
    assert!(pos.drawer_job(&s).unwrap().urgent);
    assert!(pos.staff_copy_job(&s, order).unwrap().slow);
    let shift = pos.close_shift(&s, 0, "").unwrap();
    assert!(pos.shift_job(&s, &shift).unwrap().unwrap().slow);
    let day = pos.close_day(&s).unwrap();
    assert!(pos.day_close_job(&s, &day).unwrap().unwrap().slow);
}

#[test]
fn a_slow_send_to_a_port_or_path_writes_exactly_the_job() {
    let (pos, s, order) = setup();
    let dir = tempfile::tempdir().unwrap();
    pos.set_kitchen_printer(&s, &PrinterConfig { target: file(&dir, "k.bin"), copy_delay_secs: 0, copies: 2, ..Default::default() }).unwrap();
    let items = pos.get_order(&s, order).unwrap().items;
    let job = pos.kitchen_job(&s, order, &items, false).unwrap().unwrap();
    assert!(job.is_paced(), "on by default");
    assert!(job.parts[0].len() > 2 * SLOW_CHUNK);
    let started = std::time::Instant::now();
    job.send().unwrap();
    assert!(started.elapsed() >= paced_duration(job.parts[0].len()) * 2);
    assert_eq!(std::fs::read(dir.path().join("k.bin")).unwrap(), job.bytes());
}
