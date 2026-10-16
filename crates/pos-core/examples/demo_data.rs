//! Fill a new database with a demo restaurant, for trying the app out.
//!
//!   cargo run -p pos-core --example demo_data -- <path to pos.db>
//!
//! Creates: admin PIN 1234, staff PIN 2222, two floors with tables, a menu
//! with categories, and option groups (burger size, extras, drink size).
//! Refuses to touch a database that has already been set up.

use pos_core::*;

/// (category, colour, [(item, price in cents, option group ids)])
type MenuSpec<'a> = &'a [(&'a str, &'a str, &'a [(&'a str, i64, &'a [i64])])];

fn main() -> Result<()> {
    let path = std::env::args().nth(1).expect("usage: demo_data <path to pos.db>");
    let mut pos = Pos::open(&path)?;
    if !pos.needs_setup()? {
        eprintln!("{path} is already set up; use a new file");
        std::process::exit(1);
    }
    let admin = pos.setup_first_admin("Owner", "1234")?;
    pos.create_user(&admin, "Sam", Role::Staff, "2222")?;
    pos.set_setting(&admin, "store_name", "Demo Bistro")?;

    let t = |name: &str, shape, x, y, w, h| TableLayout { id: None, name: name.into(), shape, x, y, w, h };
    let ground = pos.create_floor(&admin, "Ground")?;
    pos.save_floor_layout(&admin, ground.id, &[
        t("1", TableShape::Rect, 60, 60, 110, 110),
        t("2", TableShape::Rect, 230, 60, 110, 110),
        t("3", TableShape::Rect, 400, 60, 110, 110),
        t("4", TableShape::Round, 80, 300, 100, 100),
        t("5", TableShape::Round, 250, 300, 100, 100),
        t("Window", TableShape::Rect, 600, 60, 300, 90),
        t("Bar", TableShape::Rect, 600, 480, 340, 80),
    ])?;
    let terrace = pos.create_floor(&admin, "Terrace")?;
    pos.save_floor_layout(&admin, terrace.id, &[
        t("T1", TableShape::Round, 100, 100, 110, 110),
        t("T2", TableShape::Round, 300, 100, 110, 110),
        t("T3", TableShape::Round, 500, 100, 110, 110),
    ])?;

    let mut group = |name: &str, min, max, opts: &[(&str, i64)]| -> Result<i64> {
        let g = pos.create_option_group(&admin, name, min, max)?;
        let opts: Vec<OptionInput> =
            opts.iter().map(|(n, p)| OptionInput { id: None, name: (*n).into(), price_cents: *p, name_alt: None }).collect();
        Ok(pos.save_group_options(&admin, g.id, &opts)?.id)
    };
    let burger_size = group("Burger size", 1, 1, &[("Regular", 0), ("Double", 350)])?;
    let cooking = group("Cooking", 0, 1, &[("Medium", 0), ("Well done", 0)])?;
    let extras = group("Extras", 0, 5, &[("Cheese", 100), ("Bacon", 150), ("Egg", 80), ("Avocado", 120)])?;
    let drink_size = group("Drink size", 1, 1, &[("Small", -50), ("Regular", 0), ("Large", 70)])?;
    let pizza_size = group("Pizza size", 1, 1, &[("10 inch", 0), ("14 inch", 400)])?;

    let menu: MenuSpec = &[
        ("Starters", "#f59e0b", &[("Soup of the day", 550, &[]), ("Garlic bread", 450, &[]), ("Chicken wings", 750, &[])]),
        ("Burgers", "#dc2626", &[("Classic burger", 1150, &[burger_size, cooking, extras]), ("Chicken burger", 1100, &[burger_size, extras])]),
        ("Pizza", "#16a34a", &[("Margherita", 950, &[pizza_size, extras]), ("Pepperoni", 1150, &[pizza_size, extras])]),
        ("Sides", "#a16207", &[("Fries", 350, &[]), ("Salad", 400, &[])]),
        ("Drinks", "#2563eb", &[("Cola", 250, &[drink_size]), ("Lemonade", 280, &[drink_size]), ("Water", 150, &[])]),
    ];
    for (i, (cat, color, items)) in menu.iter().enumerate() {
        let c = pos.create_category(&admin, cat, Some(color), i as i64)?;
        for (name, price, groups) in items.iter() {
            let p = pos.create_product(&admin, NewProduct {
                category_id: Some(c.id),
                sku: None,
                barcode: None,
                name: (*name).into(),
                price_cents: *price,
                cost_cents: 0,
                tax_rate_bp: 1000,
                stock_qty: 0,
                track_stock: false,
                starter: *cat == "Starters", name_alt: None,
            })?;
            if !groups.is_empty() {
                pos.set_product_option_groups(&admin, p.id, groups)?;
            }
        }
    }
    println!("Demo restaurant created in {path}. Admin PIN 1234, staff PIN 2222.");
    Ok(())
}
