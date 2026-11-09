//! Kitchen screen: the dishes sent to the kitchen and not yet done, one card
//! per order, for a tablet in the kitchen (instead of, or as well as, paper).
//! Tapping a dish marks it done; "Done" on a card marks the whole order.

use rusqlite::params;
use serde::Serialize;

use crate::error::invalid;
use crate::{Permission, Pos, Result, Session};

#[derive(Debug, Clone, Serialize)]
pub struct BoardItem {
    pub id: i64,
    pub qty: i64,
    pub name: String,
    pub name_alt: Option<String>,
    pub options: Vec<String>,
    pub note: Option<String>,
    /// What a set's dish comes with.
    pub with_name: Option<String>,
    /// The set it is part of, if any.
    pub set_name: Option<String>,
    pub sent_at: i64,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardOrder {
    pub order_id: i64,
    /// "DINE-IN 4", "TAKEAWAY", "GRAB 123"...
    pub heading: String,
    pub customer_name: Option<String>,
    /// When the oldest dish still to do was sent.
    pub since: i64,
    pub paid: bool,
    pub items: Vec<BoardItem>,
}

impl Pos {
    /// Orders with dishes still to do. `filter`: "all", "team:<id>",
    /// "drinks", "desserts" or "food" (everything but drinks).
    pub fn kitchen_board(&self, s: &Session, filter: &str) -> Result<Vec<BoardOrder>> {
        s.require(Permission::Sell)?;
        let team: Option<i64> = filter.strip_prefix("team:").and_then(|t| t.parse().ok());
        let mut stmt = self.conn.prepare_cached(
            "SELECT i.id, i.order_id, i.qty, i.name, i.name_alt, i.note, i.sent_at, i.with_name, par.name, i.kitchen_done_at IS NOT NULL,
                    p.team_id, p.drink, p.dessert
             FROM order_items i
             JOIN orders o ON o.id = i.order_id
             JOIN products p ON p.id = i.product_id
             LEFT JOIN order_items par ON par.id = i.parent_id
             WHERE i.sent_at IS NOT NULL AND i.is_set = 0
               AND (o.status = 'open' OR (o.status = 'paid' AND o.paid_at > unixepoch() - 6 * 3600))
             ORDER BY i.sent_at, i.id",
        )?;
        type Row = (BoardItem, i64, Option<i64>, bool, bool);
        let rows: Vec<Row> = stmt
            .query_map([], |r| {
                Ok((
                    BoardItem {
                        id: r.get(0)?,
                        qty: r.get(2)?,
                        name: r.get(3)?,
                        name_alt: r.get(4)?,
                        options: Vec::new(),
                        note: r.get(5)?,
                        sent_at: r.get(6)?,
                        with_name: r.get(7)?,
                        set_name: r.get(8)?,
                        done: r.get(9)?,
                    },
                    r.get(1)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut opts = self.conn.prepare_cached("SELECT name FROM order_item_options WHERE item_id = ?1 ORDER BY id")?;
        let mut info = self.conn.prepare_cached(
            "SELECT o.order_type, t.name, ch.name, o.external_ref, o.customer_name, o.status = 'paid'
             FROM orders o LEFT JOIN dining_tables t ON t.id = o.table_id LEFT JOIN delivery_channels ch ON ch.id = o.channel_id
             WHERE o.id = ?1",
        )?;
        let mut out: Vec<BoardOrder> = Vec::new();
        for (mut item, order_id, item_team, drink, dessert) in rows {
            let keep = match filter {
                "drinks" => drink,
                "desserts" => dessert,
                "food" => !drink,
                _ => team.is_none_or(|t| item_team == Some(t)),
            };
            if !keep {
                continue;
            }
            item.options = opts.query_map([item.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            let at = match out.iter().position(|o| o.order_id == order_id) {
                Some(i) => i,
                None => {
                    let (kind, table, channel, external, customer, paid): (String, Option<String>, Option<String>, Option<String>, Option<String>, bool) =
                        info.query_row([order_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
                    let heading = match (channel, kind.as_str()) {
                        (Some(ch), _) => format!("{} {}", ch.to_uppercase(), external.unwrap_or_default()).trim().to_string(),
                        (None, "dine_in") => format!("{} {}", self.label("label_dine_in")?, table.unwrap_or_default()).trim().to_string(),
                        (None, "collection") => self.label("label_collection")?,
                        (None, "delivery") => self.label("label_delivery")?,
                        (None, _) => self.label("label_takeaway")?,
                    };
                    out.push(BoardOrder { order_id, heading, customer_name: customer, since: item.sent_at, paid, items: Vec::new() });
                    out.len() - 1
                }
            };
            out[at].items.push(item);
        }
        // Only orders with something still to do; oldest first.
        out.retain(|o| o.items.iter().any(|i| !i.done));
        for o in &mut out {
            o.since = o.items.iter().filter(|i| !i.done).map(|i| i.sent_at).min().unwrap_or(o.since);
        }
        out.sort_by_key(|o| o.since);
        Ok(out)
    }

    /// Mark dishes done (or not done, to undo).
    pub fn kitchen_done(&self, s: &Session, item_ids: &[i64], done: bool) -> Result<()> {
        s.require(Permission::Sell)?;
        if item_ids.is_empty() || item_ids.len() > 500 {
            return invalid("choose the dishes");
        }
        let mut stmt = self.conn.prepare_cached(
            "UPDATE order_items SET kitchen_done_at = CASE WHEN ?2 THEN COALESCE(kitchen_done_at, unixepoch()) ELSE NULL END WHERE id = ?1",
        )?;
        for id in item_ids {
            stmt.execute(params![id, done])?;
        }
        Ok(())
    }
}
