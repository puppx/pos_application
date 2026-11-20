//! Shapes on the floor plan that are not tables (a bar, a window): they are
//! saved and drawn, but never take an order.

use pos_core::*;

fn shape(id: Option<i64>, name: &str, is_table: bool) -> TableLayout {
    TableLayout { is_table: Some(is_table), id, name: name.into(), shape: TableShape::Rect, x: 10, y: 10, w: 80, h: 80 }
}

fn order(table: i64) -> NewOrder {
    NewOrder { order_type: OrderType::DineIn, table_id: Some(table), channel_id: None, details: Default::default() }
}

#[test]
fn a_shape_that_is_not_a_table_is_kept_but_takes_no_order() {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let saved = pos.save_floor_layout(&admin, floor.id, &[shape(None, "1", true), shape(None, "Bar", false)]).unwrap();
    let bar = saved.iter().find(|t| t.name == "Bar").unwrap();
    assert!(!bar.is_table);
    assert!(saved.iter().find(|t| t.name == "1").unwrap().is_table);
    let status = pos.floor_status(&admin, floor.id).unwrap();
    assert!(!status.iter().find(|t| t.table.name == "Bar").unwrap().table.is_table);
    let err = pos.open_order(&admin, &order(bar.id)).unwrap_err().to_string();
    assert!(err.contains("not a table"), "{err}");

    // Back to a table: it takes orders again.
    pos.save_floor_layout(&admin, floor.id, &[shape(Some(saved[0].id), "1", true), shape(Some(bar.id), "Bar", true)]).unwrap();
    pos.open_order(&admin, &order(bar.id)).unwrap();
}

#[test]
fn a_table_with_an_open_bill_stays_a_table() {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let saved = pos.save_floor_layout(&admin, floor.id, &[shape(None, "1", true)]).unwrap();
    pos.open_order(&admin, &order(saved[0].id)).unwrap();
    let err = pos.save_floor_layout(&admin, floor.id, &[shape(Some(saved[0].id), "1", false)]).unwrap_err().to_string();
    assert!(err.contains("open order"), "{err}");
}

#[test]
fn a_layout_without_is_table_keeps_what_is_stored() {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1111").unwrap();
    let floor = pos.create_floor(&admin, "Ground").unwrap();
    let saved = pos.save_floor_layout(&admin, floor.id, &[shape(None, "1", true), shape(None, "Bar", false)]).unwrap();
    let id = |n: &str| saved.iter().find(|t| t.name == n).unwrap().id;
    // A till with an older page open sends no is_table: nothing turns back into a table.
    let stale: Vec<TableLayout> = serde_json::from_value(serde_json::json!([
        { "id": id("1"), "name": "1", "shape": "rect", "x": 20, "y": 10, "w": 80, "h": 80 },
        { "id": id("Bar"), "name": "Bar", "shape": "rect", "x": 200, "y": 10, "w": 80, "h": 80 },
        { "name": "2", "shape": "round", "x": 300, "y": 10, "w": 80, "h": 80 },
    ]))
    .unwrap();
    let after = pos.save_floor_layout(&admin, floor.id, &stale).unwrap();
    let is = |n: &str| after.iter().find(|t| t.name == n).unwrap().is_table;
    assert!(is("1"));
    assert!(!is("Bar"), "the bar stays not a table");
    assert!(is("2"), "a new shape is a table");
    assert_eq!(after.iter().find(|t| t.name == "1").unwrap().x, 20);
}
