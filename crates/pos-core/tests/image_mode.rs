//! The per-printer "Image mode" (how Thai text and other images are sent)
//! reaches every print, and the Thai test page.

use pos_core::*;

const FS_DOT: [u8; 2] = [0x1C, b'.'];

fn count(b: &[u8], needle: &[u8]) -> usize {
    b.windows(needle.len()).filter(|w| *w == needle).count()
}

/// GS v 0 commands and ESC * 33 stripes in `b`.
fn images(b: &[u8]) -> (usize, usize) {
    (count(b, &[0x1D, b'v', b'0', 0]), count(b, &[0x1B, b'*', 33]))
}

fn setup() -> (Pos, Session, i64) {
    let mut pos = Pos::open_in_memory().unwrap();
    let s = pos.setup_first_admin("Owner", "1111").unwrap();
    let cat = pos.create_category(&s, "Mains", None, 1).unwrap();
    let mk = |pos: &mut Pos, name: &str, alt: Option<&str>, drink: bool| {
        let p = pos
            .create_product(&s, NewProduct {
                category_id: Some(cat.id), sku: None, barcode: None, name: name.into(), price_cents: 1200, cost_cents: 0, tax_rate_bp: Some(0),
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

#[test]
fn kitchen_and_route_printers_each_use_their_own_mode() {
    let (pos, s, order) = setup();
    let file = |n: &str| PrinterTarget::File { path: std::env::temp_dir().join(n).display().to_string() };
    let kitchen = PrinterConfig { target: file("pos-image-mode-k.bin"), copy_delay_secs: 0, image_mode: ImageMode::Column, ..Default::default() };
    pos.set_kitchen_printer(&s, &kitchen).unwrap();
    assert_eq!(pos.kitchen_printer().unwrap().image_mode, ImageMode::Column);
    let bar = KitchenRoute {
        label: "Bar".into(), target: file("pos-image-mode-b.bin"), width: 32, copies: 1, team_ids: vec![], drinks: true, desserts: false,
        image_mode: ImageMode::RasterSmall, slow_send: false,
    };
    pos.set_kitchen_routes(&s, std::slice::from_ref(&bar)).unwrap();
    assert_eq!(pos.route_config(&bar).unwrap().image_mode, ImageMode::RasterSmall);
    let items = pos.get_order(&s, order).unwrap().items;
    let job = pos.kitchen_job(&s, order, &items, false).unwrap().unwrap();
    // Main kitchen printer: column images only (the Thai name, app font).
    let main = job.parts.concat();
    assert_eq!(&main[8..10], &FS_DOT);
    let (raster, column) = images(&main);
    assert_eq!(raster, 0);
    assert!(column > 0);
    // Bar: small raster bands only.
    let bar_bytes = job.more[0].bytes();
    assert_eq!(&bar_bytes[8..10], &FS_DOT);
    let (raster, column) = images(&bar_bytes);
    assert!(raster > 0 && column == 0);
    // The test ticket follows the form's mode.
    let test = pos.test_print_job(&s, &kitchen).unwrap().bytes();
    assert!(images(&test).0 == 0 && images(&test).1 > 0);
}

#[test]
fn receipts_staff_copy_and_shift_report_use_the_receipt_printers_mode() {
    let (mut pos, s, order) = setup();
    // A Thai restaurant name: every print below has Thai in it.
    pos.set_setting(&s, "store_name", "ครัวไทย").unwrap();
    let mut cfg = pos.receipt_settings().unwrap();
    cfg.printer = PrinterConfig { target: PrinterTarget::File { path: std::env::temp_dir().join("pos-image-mode-r.bin").display().to_string() }, copy_delay_secs: 0, ..Default::default() };
    cfg.printer.names = Some(PrintNames::Both);
    pos.set_receipt_settings(&s, &cfg).unwrap();
    // Standard mode: exactly the bytes receipts have always had.
    let r = pos.receipt(&s, order).unwrap();
    let bill = pos.receipt_job(&s, order).unwrap().bytes();
    assert_eq!(bill, r.to_escpos(cfg.printer.width, cfg.printer.bold));
    assert!(images(&bill).0 > 0 && !bill.windows(2).any(|w| w == FS_DOT));

    cfg.printer.image_mode = ImageMode::Column;
    pos.set_receipt_settings(&s, &cfg).unwrap();
    assert_eq!(pos.receipt_settings().unwrap().printer.image_mode, ImageMode::Column);
    let bill = pos.receipt_job(&s, order).unwrap().bytes();
    assert_eq!(&bill[8..10], &FS_DOT);
    assert!(images(&bill).0 == 0 && images(&bill).1 > 0);
    pos.open_shift(&s, 0).unwrap();
    let paid = pos.pay_order(&s, order, PaymentMethod::Card, 0).unwrap();
    let job = pos.payment_print_job(&s, &paid).unwrap().unwrap();
    // The bill was given, so only the staff copy prints at payment.
    assert_eq!(job.parts.len(), 1);
    for part in &job.parts {
        assert!(images(part).0 == 0 && images(part).1 > 0);
    }
    let staff = pos.staff_copy_job(&s, order).unwrap().bytes();
    assert!(images(&staff).0 == 0 && images(&staff).1 > 0);
    let shift = pos.close_shift(&s, 0, "นับเงินแล้ว").unwrap();
    let report = pos.shift_job(&s, &shift).unwrap().unwrap().bytes();
    assert_eq!(&report[8..10], &FS_DOT);
    assert!(images(&report).0 == 0 && images(&report).1 > 0);
    let day = pos.close_day(&s).unwrap();
    let report = pos.day_close_job(&s, &day).unwrap().unwrap().bytes();
    assert_eq!(&report[8..10], &FS_DOT);
    assert!(images(&report).0 == 0 && images(&report).1 > 0);
}

#[test]
fn thai_test_page_job() {
    let (mut pos, s, _) = setup();
    pos.create_user(&s, "Sam", Role::Staff, "2222").unwrap();
    let sam = pos.login("2222").unwrap();
    let cfg = PrinterConfig { target: PrinterTarget::Network { host: "192.168.1.50".into(), port: 9100 }, ..Default::default() };
    assert!(matches!(pos.thai_test_job(&sam, &cfg), Err(PosError::Forbidden)));
    assert!(pos.thai_test_job(&s, &PrinterConfig { width: 10, ..cfg.clone() }).is_err());
    let job = pos.thai_test_job(&s, &cfg).unwrap();
    assert_eq!(job.parts.len(), 1, "one page, whatever the copies");
    let b = job.bytes();
    // All three modes, whatever the printer's own mode is.
    let (raster, column) = images(&b);
    assert!(raster > 0 && column > 0);
    assert_eq!(b, pos.thai_test_job(&s, &PrinterConfig { image_mode: ImageMode::Column, copies: 3, ..cfg }).unwrap().bytes());
    let text = String::from_utf8_lossy(&b);
    let at = |t: &str| text.find(t).unwrap_or_else(|| panic!("{t}"));
    assert!(at("Thai test - pick the mode that prints") < at("1. Compatible (older printers)"));
    assert!(at("1. Compatible (older printers)") < at("2. Small pieces") && at("2. Small pieces") < at("3. Standard"));
    assert!(b.ends_with(&[0x1D, b'V', 66, 0]));
}
