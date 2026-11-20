use std::collections::HashSet;

use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{invalid, map_unique};
use crate::{Permission, Pos, PosError, Result, Session};

/// Plan coordinates live on a fixed canvas; the UI scales it to the screen.
pub const PLAN_WIDTH: i64 = 1000;
pub const PLAN_HEIGHT: i64 = 700;
const MIN_SIZE: i64 = 30;
const MAX_SIZE: i64 = 400;
const MAX_TABLES_PER_FLOOR: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct Floor {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableShape {
    Rect,
    Round,
}

impl TableShape {
    fn as_str(self) -> &'static str {
        match self {
            TableShape::Rect => "rect",
            TableShape::Round => "round",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiningTable {
    pub id: i64,
    pub floor_id: i64,
    pub name: String,
    pub shape: TableShape,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    /// False for a shape that only shows where something is (a bar, a window):
    /// it never takes an order and is not counted as a table.
    pub is_table: bool,
}

/// One table as drawn in the floor-plan editor. `id` is `None` for new tables.
#[derive(Debug, Clone, Deserialize)]
pub struct TableLayout {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    pub shape: TableShape,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    /// See `DiningTable::is_table`. Missing (None), e.g. from a till with an
    /// older page open: a kept shape stays as it is and a new one is a table.
    #[serde(default)]
    pub is_table: Option<bool>,
}

/// A table plus its open order, if any, for the live floor view.
#[derive(Debug, Clone, Serialize)]
pub struct TableStatus {
    #[serde(flatten)]
    pub table: DiningTable,
    pub order_id: Option<i64>,
    pub opened_at: Option<i64>,
    pub total_cents: i64,
    pub item_count: i64,
    /// New lines not yet sent (mains waiting for "Call mains" excluded).
    pub unsent_count: i64,
    /// Mains waiting for "Call mains".
    pub held_count: i64,
}

fn table_from_row(r: &Row) -> rusqlite::Result<DiningTable> {
    let shape: String = r.get(3)?;
    Ok(DiningTable {
        id: r.get(0)?,
        floor_id: r.get(1)?,
        name: r.get(2)?,
        shape: if shape == "round" { TableShape::Round } else { TableShape::Rect },
        x: r.get(4)?,
        y: r.get(5)?,
        w: r.get(6)?,
        h: r.get(7)?,
        is_table: r.get("is_table")?,
    })
}

fn clean_floor_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 30 {
        return invalid("floor name must be 1 to 30 characters");
    }
    Ok(name.to_string())
}

fn validate_layout(t: &TableLayout) -> Result<String> {
    let name = t.name.trim();
    if name.is_empty() || name.chars().count() > 12 {
        return invalid("table name must be 1 to 12 characters");
    }
    if !(MIN_SIZE..=MAX_SIZE).contains(&t.w) || !(MIN_SIZE..=MAX_SIZE).contains(&t.h) {
        return invalid(format!("table {name}: size must be between {MIN_SIZE} and {MAX_SIZE}"));
    }
    if t.x < 0 || t.y < 0 || t.x + t.w > PLAN_WIDTH || t.y + t.h > PLAN_HEIGHT {
        return invalid(format!("table {name} is outside the floor plan"));
    }
    Ok(name.to_string())
}

const FLOOR_TAKEN: &str = "a floor with that name already exists";

impl Pos {
    pub fn list_floors(&self, s: &Session) -> Result<Vec<Floor>> {
        s.require(Permission::Sell)?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, name, sort_order FROM floors WHERE active = 1 ORDER BY sort_order, id")?;
        let rows = stmt.query_map([], |r| Ok(Floor { id: r.get(0)?, name: r.get(1)?, sort_order: r.get(2)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_floor(&self, s: &Session, name: &str) -> Result<Floor> {
        s.require(Permission::ManageFloorPlan)?;
        let name = clean_floor_name(name)?;
        self.conn
            .execute(
                "INSERT INTO floors (name, sort_order)
                 VALUES (?1, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM floors WHERE active = 1))",
                [&name],
            )
            .map_err(|e| map_unique(e, FLOOR_TAKEN))?;
        let id = self.conn.last_insert_rowid();
        self.audit(Some(s.user_id), "floor.create", &format!("{id} {name}"))?;
        self.get_floor(id)
    }

    pub fn update_floor(&self, s: &Session, id: i64, name: &str, sort_order: i64) -> Result<Floor> {
        s.require(Permission::ManageFloorPlan)?;
        let name = clean_floor_name(name)?;
        let changed = self
            .conn
            .execute(
                "UPDATE floors SET name = ?2, sort_order = ?3 WHERE id = ?1 AND active = 1",
                params![id, name, sort_order],
            )
            .map_err(|e| map_unique(e, FLOOR_TAKEN))?;
        if changed == 0 {
            return Err(PosError::NotFound);
        }
        self.audit(Some(s.user_id), "floor.update", &format!("{id} {name}"))?;
        self.get_floor(id)
    }

    /// Remove a floor and its tables. Refused while any table has an open order.
    pub fn delete_floor(&mut self, s: &Session, id: i64) -> Result<()> {
        s.require(Permission::ManageFloorPlan)?;
        s.require(Permission::Delete)?;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let busy: Option<String> = tx
            .query_row(
                "SELECT t.name FROM dining_tables t JOIN orders o ON o.table_id = t.id AND o.status = 'open'
                 WHERE t.floor_id = ?1 LIMIT 1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(t) = busy {
            return invalid(format!("table {t} on this floor has an open order"));
        }
        if tx.execute("UPDATE floors SET active = 0 WHERE id = ?1 AND active = 1", [id])? == 0 {
            return Err(PosError::NotFound);
        }
        tx.execute("UPDATE dining_tables SET active = 0 WHERE floor_id = ?1", [id])?;
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'floor.delete', ?2)",
            params![s.user_id, id.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn get_floor(&self, id: i64) -> Result<Floor> {
        self.conn
            .prepare_cached("SELECT id, name, sort_order FROM floors WHERE id = ?1 AND active = 1")?
            .query_row([id], |r| Ok(Floor { id: r.get(0)?, name: r.get(1)?, sort_order: r.get(2)? }))
            .optional()?
            .ok_or(PosError::NotFound)
    }

    pub fn list_tables(&self, s: &Session, floor_id: i64) -> Result<Vec<DiningTable>> {
        s.require(Permission::Sell)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, floor_id, name, shape, x, y, w, h, is_table FROM dining_tables
             WHERE floor_id = ?1 AND active = 1 ORDER BY id",
        )?;
        let rows = stmt.query_map([floor_id], table_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Live floor view: every table with its open order's running total.
    /// Each till asks every few seconds: the totals are worked out per open
    /// order from the order_items index, never over every dish ever sold.
    pub fn floor_status(&self, s: &Session, floor_id: i64) -> Result<Vec<TableStatus>> {
        s.require(Permission::Sell)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.id, t.floor_id, t.name, t.shape, t.x, t.y, t.w, t.h, o.id, o.opened_at,
                    COALESCE((
                        SELECT SUM(i.unit_price_cents * i.qty - i.discount_cents
                                   + CASE WHEN o.tax_incl = 1 THEN 0
                                          ELSE ((i.unit_price_cents * i.qty - i.discount_cents) * i.tax_rate_bp + 5000) / 10000 END)
                               + (SUM(i.unit_price_cents * i.qty - i.discount_cents) * COALESCE(o.service_bp, 0) + 5000) / 10000
                        FROM order_items i WHERE i.order_id = o.id), 0),
                    COALESCE((SELECT SUM(i.qty) FROM order_items i WHERE i.order_id = o.id), 0),
                    (SELECT COUNT(*) FROM order_items i WHERE i.order_id = o.id AND i.sent_at IS NULL AND i.held = 0 AND i.is_set = 0),
                    (SELECT COUNT(*) FROM order_items i WHERE i.order_id = o.id AND i.sent_at IS NULL AND i.held = 1),
                    t.is_table AS is_table
             FROM dining_tables t
             LEFT JOIN orders o ON o.table_id = t.id AND o.status = 'open'
             WHERE t.floor_id = ?1 AND t.active = 1
             ORDER BY t.id",
        )?;
        let rows = stmt.query_map([floor_id], |r| {
            Ok(TableStatus {
                table: table_from_row(r)?,
                order_id: r.get(8)?,
                opened_at: r.get(9)?,
                total_cents: r.get(10)?,
                item_count: r.get(11)?,
                unsent_count: r.get(12)?,
                held_count: r.get(13)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replace a floor's tables with the editor's layout in one transaction:
    /// existing ids are moved/renamed, new ones created, missing ones removed
    /// (refused if a removed table has an open order).
    pub fn save_floor_layout(&mut self, s: &Session, floor_id: i64, tables: &[TableLayout]) -> Result<Vec<DiningTable>> {
        s.require(Permission::ManageFloorPlan)?;
        if tables.len() > MAX_TABLES_PER_FLOOR {
            return invalid(format!("a floor can have at most {MAX_TABLES_PER_FLOOR} tables"));
        }
        let mut names = HashSet::new();
        let mut cleaned = Vec::with_capacity(tables.len());
        for t in tables {
            let name = validate_layout(t)?;
            if !names.insert(name.to_lowercase()) {
                return invalid(format!("two tables are named {name}"));
            }
            cleaned.push((t, name));
        }

        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let floor_ok: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM floors WHERE id = ?1 AND active = 1)",
            [floor_id],
            |r| r.get(0),
        )?;
        if !floor_ok {
            return Err(PosError::NotFound);
        }
        let existing: Vec<(i64, String, bool)> = tx
            .prepare(
                "SELECT t.id, t.name, o.id IS NOT NULL FROM dining_tables t
                 LEFT JOIN orders o ON o.table_id = t.id AND o.status = 'open'
                 WHERE t.floor_id = ?1 AND t.active = 1",
            )?
            .query_map([floor_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let keep: HashSet<i64> = tables.iter().filter_map(|t| t.id).collect();
        for id in &keep {
            if !existing.iter().any(|(e, _, _)| e == id) {
                return invalid(format!("table {id} is not on this floor"));
            }
        }
        for (id, name, busy) in &existing {
            // A table with an open bill stays a table until the bill is closed.
            if *busy && tables.iter().any(|t| t.id == Some(*id) && t.is_table == Some(false)) {
                return invalid(format!("table {name} has an open order: it stays a table until that order is paid or moved"));
            }
            if !keep.contains(id) {
                if *busy {
                    return invalid(format!("table {name} has an open order and cannot be removed"));
                }
                tx.execute("UPDATE dining_tables SET active = 0 WHERE id = ?1", [id])?;
            }
        }
        // Park kept tables under unique temporary names so swaps (A<->B) do
        // not trip the unique-name index halfway through.
        tx.execute(
            "UPDATE dining_tables SET name = char(1) || id WHERE floor_id = ?1 AND active = 1",
            [floor_id],
        )?;
        {
            let mut upd = tx.prepare(
                "UPDATE dining_tables SET name = ?2, shape = ?3, x = ?4, y = ?5, w = ?6, h = ?7, is_table = COALESCE(?8, is_table) WHERE id = ?1",
            )?;
            let mut ins = tx.prepare(
                "INSERT INTO dining_tables (floor_id, name, shape, x, y, w, h, is_table) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (t, name) in &cleaned {
                match t.id {
                    Some(id) => upd.execute(params![id, name, t.shape.as_str(), t.x, t.y, t.w, t.h, t.is_table])?,
                    None => ins.execute(params![floor_id, name, t.shape.as_str(), t.x, t.y, t.w, t.h, t.is_table.unwrap_or(true)])?,
                };
            }
        }
        tx.execute(
            "INSERT INTO audit_log (user_id, action, detail) VALUES (?1, 'floor.layout', ?2)",
            params![s.user_id, format!("{floor_id} tables={}", tables.len())],
        )?;
        tx.commit()?;
        self.list_tables(s, floor_id)
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    /// The floor view's query before 0.13.1 (totals of every open order
    /// worked out over all of order_items), to check the faster one matches.
    fn old_status(pos: &Pos, floor_id: i64) -> Vec<TableStatus> {
        let mut stmt = pos
            .conn
            .prepare(
                "SELECT t.id, t.floor_id, t.name, t.shape, t.x, t.y, t.w, t.h, o.id, o.opened_at,
                    COALESCE(i.total, 0) + (COALESCE(i.net, 0) * COALESCE(o.service_bp, 0) + 5000) / 10000,
                    COALESCE(i.items, 0), COALESCE(i.unsent, 0), COALESCE(i.held, 0), t.is_table AS is_table
             FROM dining_tables t
             LEFT JOIN orders o ON o.table_id = t.id AND o.status = 'open'
             LEFT JOIN (
                 SELECT oi.order_id,
                        SUM(oi.unit_price_cents * oi.qty - oi.discount_cents
                            + CASE WHEN oo.tax_incl = 1 THEN 0
                                   ELSE ((oi.unit_price_cents * oi.qty - oi.discount_cents) * oi.tax_rate_bp + 5000) / 10000 END) AS total,
                        SUM(oi.unit_price_cents * oi.qty - oi.discount_cents) AS net,
                        SUM(oi.qty) AS items,
                        SUM(oi.sent_at IS NULL AND oi.held = 0 AND oi.is_set = 0) AS unsent,
                        SUM(oi.sent_at IS NULL AND oi.held = 1) AS held
                 FROM order_items oi JOIN orders oo ON oo.id = oi.order_id
                 WHERE oo.status = 'open'
                 GROUP BY oi.order_id
             ) i ON i.order_id = o.id
             WHERE t.floor_id = ?1 AND t.active = 1
             ORDER BY t.id",
            )
            .unwrap();
        stmt.query_map([floor_id], |r| {
            Ok(TableStatus {
                table: super::table_from_row(r)?,
                order_id: r.get(8)?,
                opened_at: r.get(9)?,
                total_cents: r.get(10)?,
                item_count: r.get(11)?,
                unsent_count: r.get(12)?,
                held_count: r.get(13)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    #[test]
    fn the_faster_floor_view_shows_exactly_what_it_did_before() {
        let mut pos = Pos::open_in_memory().unwrap();
        let s = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.set_setting(&s, "prices_include_tax", "0").unwrap();
        let cat = pos.create_category(&s, "Food", None, 1).unwrap().id;
        let dishes: Vec<i64> = [(333, 1500, true), (1250, 700, false), (999, 0, false), (45, 1000, true)]
            .iter()
            .enumerate()
            .map(|(n, (price, tax, starter))| {
                pos.create_product(&s, NewProduct {
                    category_id: Some(cat), sku: None, barcode: None, name: format!("Dish {n}"), price_cents: *price, cost_cents: 0,
                    tax_rate_bp: *tax, stock_qty: 0, track_stock: false, starter: *starter, name_alt: None, prep: false, team_id: None,
                })
                .unwrap()
                .id
            })
            .collect();
        let mut floors = Vec::new();
        for f in 0..2 {
            let floor = pos.create_floor(&s, &format!("Floor {f}")).unwrap();
            let layout: Vec<TableLayout> = (0..8)
                .map(|i| TableLayout {
                    is_table: Some(i != 7), id: None, name: format!("{f}-{i}"), shape: if i % 2 == 0 { TableShape::Round } else { TableShape::Rect },
                    x: i * 100, y: f * 100, w: 80, h: 80,
                })
                .collect();
            let tables = pos.save_floor_layout(&s, floor.id, &layout).unwrap();
            floors.push((floor.id, tables));
        }
        let mut n = 0;
        for (_, tables) in &floors {
            for t in tables.iter().filter(|t| t.is_table).take(6) {
                n += 1;
                let o = pos.open_order(&s, &NewOrder { order_type: OrderType::DineIn, table_id: Some(t.id), channel_id: None, details: Default::default() }).unwrap().id;
                // Every fifth order stays empty; the rest get a mix of dishes.
                for k in 0..(n % 5) {
                    let ord = pos.add_item(&s, o, dishes[(n + k) % dishes.len()], 1 + (k as i64 % 3), None, None, &[]).unwrap();
                    if k == 1 {
                        let line = ord.items.last().unwrap().id;
                        pos.update_item(&s, line, &ItemUpdate { qty: 2, discount_cents: 77, note: None }).unwrap();
                    }
                }
                if n % 3 == 0 {
                    pos.send_to_kitchen(&s, o).unwrap();
                    pos.add_item(&s, o, dishes[1], 1, None, None, &[]).unwrap();
                }
                pos.conn
                    .execute("UPDATE orders SET service_bp = ?2, tax_incl = ?3 WHERE id = ?1", rusqlite::params![o, [0, 1000, 1250][n % 3], n % 2])
                    .unwrap();
                if n == 4 {
                    // Mains waiting for "Call mains".
                    pos.conn.execute("UPDATE order_items SET held = 1 WHERE order_id = ?1 AND sent_at IS NULL", [o]).unwrap();
                }
                if n == 7 {
                    // Paid: the table is free again.
                    pos.pay_order(&s, o, PaymentMethod::Card, 0).unwrap();
                }
            }
        }
        // A table removed from the plan is left out.
        pos.conn.execute("UPDATE dining_tables SET active = 0 WHERE id = ?1", [floors[1].1[6].id]).unwrap();
        let mut busy = 0;
        for (floor, _) in &floors {
            let new = pos.floor_status(&s, *floor).unwrap();
            assert_eq!(format!("{new:?}"), format!("{:?}", old_status(&pos, *floor)));
            busy += new.iter().filter(|t| t.order_id.is_some()).count();
            assert!(new.iter().any(|t| t.order_id.is_some() && t.total_cents == 0), "an empty open order");
        }
        assert_eq!(busy, 11);
        let all: Vec<TableStatus> = floors.iter().flat_map(|(f, _)| pos.floor_status(&s, *f).unwrap()).collect();
        assert!(all.iter().any(|t| t.held_count > 0) && all.iter().any(|t| t.unsent_count > 0 && t.item_count > t.unsent_count));
        assert!(pos.floor_status(&s, 999).unwrap().is_empty());
    }
}
