//! Load test with a realistic large catalogue. Not run by default:
//!   cargo test --release -p pos-core --test perf -- --ignored --nocapture

use std::time::Instant;

use pos_core::*;

#[test]
#[ignore]
fn large_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let mut pos = Pos::open(dir.path().join("pos.db")).unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();

    const PRODUCTS: i64 = 50_000;
    let t = Instant::now();
    for i in 0..PRODUCTS {
        pos.create_product(
            &admin,
            NewProduct {
                sku: Some(format!("SKU{i:06}")),
                barcode: Some(format!("{:013}", 5_000_000_000_000i64 + i)),
                name: format!("Product {i} {}", ["Cola", "Bread", "Milk", "Soap", "Rice"][i as usize % 5]),
                price_cents: 100 + i % 900,
                cost_cents: 50,
                tax_rate_bp: 1500,
                stock_qty: 1_000,
                track_stock: true,
            },
        )
        .unwrap();
    }
    println!("insert {PRODUCTS} products: {:?}", t.elapsed());

    let t = Instant::now();
    let s = pos.login("1111").unwrap();
    println!("PIN login (argon2id 19 MiB): {:?}", t.elapsed());

    let n = 1_000;
    let t = Instant::now();
    for i in 0..n {
        let code = format!("{:013}", 5_000_000_000_000i64 + (i * 37) % PRODUCTS);
        assert!(pos.find_by_code(&s, &code).unwrap().is_some());
    }
    println!("barcode lookup: {:?} avg", t.elapsed() / n as u32);

    let t = Instant::now();
    for q in ["ri", "Mil", "cola", "Product 4999", "duct 12"] {
        for _ in 0..100 {
            pos.search_products(&s, q, 40).unwrap();
        }
    }
    println!("search (40 results): {:?} avg", t.elapsed() / 500);

    let sales = 2_000;
    let t = Instant::now();
    for i in 0..sales {
        let lines: Vec<CartLine> = (0..5)
            .map(|j| CartLine { product_id: 1 + (i * 5 + j) % PRODUCTS, qty: 1 + j % 3, discount_cents: 0 })
            .collect();
        pos.complete_sale(&s, &lines, PaymentMethod::Card, 0).unwrap();
    }
    println!("checkout (5 lines, fsync'd): {:?} avg", t.elapsed() / sales as u32);

    let t = Instant::now();
    let sum = pos.sales_summary(&s, 0, i64::MAX).unwrap();
    let top = pos.top_products(&s, 0, i64::MAX, 20).unwrap();
    println!("day report over {} sales: {:?}", sum.sale_count, t.elapsed());
    assert_eq!(sum.sale_count, sales);
    assert_eq!(top.len(), 20);

    let size = std::fs::metadata(dir.path().join("pos.db")).unwrap().len();
    println!("database size: {:.1} MB", size as f64 / 1e6);
}
