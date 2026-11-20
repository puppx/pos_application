//! Kitchen screen: the dishes sent to the kitchen and not yet done, one card
//! per order, for a tablet in the kitchen (instead of, or as well as, paper).
//! Tapping a dish marks it done; "Done" on a card marks the whole order.

use std::collections::HashMap;

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
    /// "DINE-IN 4", "TAKEAWAY #12", "GRAB 123" (the order number when the
    /// channel gave no reference: "GRAB #12"). The customer's name is apart.
    pub heading: String,
    pub customer_name: Option<String>,
    /// When the oldest dish still to do was sent.
    pub since: i64,
    pub paid: bool,
    pub items: Vec<BoardItem>,
}

/// The orders on the kitchen screen: the open ones and those paid in the
/// last `?1` seconds (none when 0). Both parts use an index on `orders`.
const BOARD_ORDERS: &str = "SELECT id FROM orders WHERE status = 'open'
    UNION ALL SELECT id FROM orders WHERE ?1 > 0 AND paid_at > unixepoch() - ?1 AND status = 'paid'";

/// What a card's heading is made from.
struct OrderHead {
    kind: String,
    table: Option<String>,
    channel: Option<String>,
    external: Option<String>,
    customer: Option<String>,
    paid: bool,
}

impl Pos {
    /// Orders with dishes still to do. `filter`: "all", "team:<id>" (a dish
    /// moved to another team on its order shows under that team),
    /// "drinks", "desserts" or "food" (everything but drinks).
    pub fn kitchen_board(&self, s: &Session, filter: &str) -> Result<Vec<BoardOrder>> {
        s.require(Permission::Sell)?;
        let team: Option<i64> = filter.strip_prefix("team:").and_then(|t| t.parse().ok());
        // Paid orders stay on the screen this long (setting `kds_paid_hours`, 0..24, default 6).
        let paid_secs = self.kds_paid_hours()? * 3600;
        // Every screen asks every few seconds, under the database lock: start
        // from the open and recently paid orders (both indexed), never from
        // every dish ever sold. The order's own details come in the same row.
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT i.id, i.order_id, i.qty, i.name, i.name_alt, i.note, i.sent_at, i.with_name, par.name, i.kitchen_done_at IS NOT NULL,
                    COALESCE(i.team_id, p.team_id), p.drink, p.dessert,
                    o.order_type, t.name, ch.name, o.external_ref, o.customer_name, o.status = 'paid'
             FROM orders o
             JOIN order_items i ON i.order_id = o.id
             JOIN products p ON p.id = i.product_id
             LEFT JOIN order_items par ON par.id = i.parent_id
             LEFT JOIN dining_tables t ON t.id = o.table_id
             LEFT JOIN delivery_channels ch ON ch.id = o.channel_id
             WHERE o.id IN ({BOARD_ORDERS}) AND i.sent_at IS NOT NULL AND i.is_set = 0
             ORDER BY i.sent_at, i.id"
        ))?;
        type Row = (BoardItem, i64, Option<i64>, bool, bool, OrderHead);
        let rows: Vec<Row> = stmt
            .query_map([paid_secs], |r| {
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
                    OrderHead { kind: r.get(13)?, table: r.get(14)?, channel: r.get(15)?, external: r.get(16)?, customer: r.get(17)?, paid: r.get(18)? },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        // Every dish's options in one go (not one query per dish).
        let mut options: HashMap<i64, Vec<String>> = HashMap::new();
        let mut opts = self.conn.prepare_cached(&format!(
            "SELECT x.item_id, x.name FROM order_item_options x
             WHERE x.item_id IN (SELECT i.id FROM order_items i WHERE i.order_id IN ({BOARD_ORDERS}) AND i.sent_at IS NOT NULL AND i.is_set = 0)
             ORDER BY x.item_id, x.id"
        ))?;
        for row in opts.query_map([paid_secs], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (item, name) = row?;
            options.entry(item).or_default().push(name);
        }
        // The headings' labels, read once.
        let labels = [self.label("label_dine_in")?, self.label("label_collection")?, self.label("label_delivery")?, self.label("label_takeaway")?];
        let mut out: Vec<BoardOrder> = Vec::new();
        let mut at_of: HashMap<i64, usize> = HashMap::new();
        for (mut item, order_id, item_team, drink, dessert, head) in rows {
            let keep = match filter {
                "drinks" => drink,
                "desserts" => dessert,
                "food" => !drink,
                _ => team.is_none_or(|t| item_team == Some(t)),
            };
            if !keep {
                continue;
            }
            item.options = options.remove(&item.id).unwrap_or_default();
            let at = *at_of.entry(order_id).or_insert_with(|| {
                let OrderHead { kind, table, channel, external, customer, paid } = head;
                // Orders without a table carry their number, so three
                // takeaways are three different cards ("TAKEAWAY #12").
                let number = format!("#{order_id}");
                let external = external.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
                let heading = match (channel, kind.as_str()) {
                    (Some(ch), _) => format!("{} {}", ch.to_uppercase(), external.unwrap_or(number)),
                    (None, "dine_in") => format!("{} {}", labels[0], table.unwrap_or_default()).trim().to_string(),
                    (None, "collection") => format!("{} {number}", labels[1]),
                    (None, "delivery") => format!("{} {number}", labels[2]),
                    (None, _) => format!("{} {number}", labels[3]),
                };
                out.push(BoardOrder { order_id, heading, customer_name: customer, since: item.sent_at, paid, items: Vec::new() });
                out.len() - 1
            });
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

    /// Hours a paid order with dishes still to do stays on the kitchen screen
    /// (setting `kds_paid_hours`; 0 = it leaves at once; default 6).
    pub fn kds_paid_hours(&self) -> Result<i64> {
        Ok(self.setting("kds_paid_hours")?.and_then(|v| v.trim().parse::<i64>().ok()).map_or(6, |h| h.clamp(0, 24)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    /// The kitchen screen as it was worked out before 0.13.1 (every dish
    /// ever sent scanned, one query per dish and per order), kept to check
    /// the faster one gives exactly the same screen.
    fn old_board(pos: &Pos, s: &Session, filter: &str) -> Result<Vec<BoardOrder>> {
        s.require(Permission::Sell)?;
        let team: Option<i64> = filter.strip_prefix("team:").and_then(|t| t.parse().ok());
        let paid_secs = pos.kds_paid_hours()? * 3600;
        let mut stmt = pos.conn.prepare(
            "SELECT i.id, i.order_id, i.qty, i.name, i.name_alt, i.note, i.sent_at, i.with_name, par.name, i.kitchen_done_at IS NOT NULL,
                    COALESCE(i.team_id, p.team_id), p.drink, p.dessert
             FROM order_items i
             JOIN orders o ON o.id = i.order_id
             JOIN products p ON p.id = i.product_id
             LEFT JOIN order_items par ON par.id = i.parent_id
             WHERE i.sent_at IS NOT NULL AND i.is_set = 0
               AND (o.status = 'open' OR (?1 > 0 AND o.status = 'paid' AND o.paid_at > unixepoch() - ?1))
             ORDER BY i.sent_at, i.id",
        )?;
        type Row = (BoardItem, i64, Option<i64>, bool, bool);
        let rows: Vec<Row> = stmt
            .query_map([paid_secs], |r| {
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
        let mut opts = pos.conn.prepare("SELECT name FROM order_item_options WHERE item_id = ?1 ORDER BY id")?;
        let mut info = pos.conn.prepare(
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
                    let number = format!("#{order_id}");
                    let external = external.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
                    let heading = match (channel, kind.as_str()) {
                        (Some(ch), _) => format!("{} {}", ch.to_uppercase(), external.unwrap_or(number)),
                        (None, "dine_in") => format!("{} {}", pos.label("label_dine_in")?, table.unwrap_or_default()).trim().to_string(),
                        (None, "collection") => format!("{} {number}", pos.label("label_collection")?),
                        (None, "delivery") => format!("{} {number}", pos.label("label_delivery")?),
                        (None, _) => format!("{} {number}", pos.label("label_takeaway")?),
                    };
                    out.push(BoardOrder { order_id, heading, customer_name: customer, since: item.sent_at, paid, items: Vec::new() });
                    out.len() - 1
                }
            };
            out[at].items.push(item);
        }
        out.retain(|o| o.items.iter().any(|i| !i.done));
        for o in &mut out {
            o.since = o.items.iter().filter(|i| !i.done).map(|i| i.sent_at).min().unwrap_or(o.since);
        }
        out.sort_by_key(|o| o.since);
        Ok(out)
    }

    fn json(b: &[BoardOrder]) -> String {
        serde_json::to_string_pretty(b).unwrap()
    }

    #[test]
    fn the_faster_kitchen_screen_shows_exactly_what_it_did_before() {
        let mut pos = Pos::open_in_memory().unwrap();
        let s = pos.setup_first_admin("Owner", "1111").unwrap();
        pos.set_setting(&s, "prices_include_tax", "0").unwrap();
        let teams = pos.list_teams(&s).unwrap();
        let team = |n: &str| teams.iter().find(|t| t.name == n).map(|t| t.id);
        let cat = pos.create_category(&s, "Food", None, 1).unwrap().id;
        let item = |pos: &mut Pos, name: &str, starter: bool, team: Option<i64>, alt: Option<&str>| {
            pos.create_product(&s, NewProduct {
                category_id: Some(cat), sku: None, barcode: None, name: name.into(), price_cents: 1000, cost_cents: 0, tax_rate_bp: 1000,
                stock_qty: 0, track_stock: false, starter, name_alt: alt.map(Into::into), prep: false, team_id: team,
            })
            .unwrap()
            .id
        };
        let soup = item(&mut pos, "Soup", true, team("Starter"), Some("ซุป"));
        let steak = item(&mut pos, "Steak", false, team("Main"), None);
        let curry = item(&mut pos, "Green curry", false, team("Curry"), Some("แกงเขียวหวาน"));
        let rice = item(&mut pos, "Jasmine rice", false, team("Rice"), None);
        let cola = item(&mut pos, "Cola", false, None, None);
        let cake = item(&mut pos, "Cake", false, None, None);
        let set = item(&mut pos, "Dinner set", false, None, None);
        pos.set_drink(&s, cola, true).unwrap();
        pos.set_dessert(&s, cake, true).unwrap();
        let extras = pos.create_option_group(&s, "Extras", 0, 2).unwrap();
        let extras = pos
            .save_group_options(&s, extras.id, &[
                OptionInput { id: None, name: "Cheese".into(), price_cents: 100, name_alt: None },
                OptionInput { id: None, name: "Bacon".into(), price_cents: 150, name_alt: None },
            ])
            .unwrap();
        let (cheese, bacon) = (extras.options[0].id, extras.options[1].id);
        pos.set_product_option_groups(&s, steak, &[extras.id]).unwrap();
        let sections = pos
            .save_set(&s, set, &[
                SetSectionInput { name: "Starter".into(), name_alt: None, pick: 1, starter: true, product_ids: vec![soup], with_ids: vec![] },
                SetSectionInput { name: "Main".into(), name_alt: None, pick: 1, starter: false, product_ids: vec![steak, curry], with_ids: vec![] },
            ])
            .unwrap();
        let grab = pos.save_channels(&s, &[ChannelInput { id: None, name: "Grab".into(), color: None }]).unwrap();
        let grab = grab.iter().find(|c| c.name == "Grab").unwrap().id;
        let floor = pos.create_floor(&s, "Ground").unwrap();
        let layout: Vec<TableLayout> = ["4", "5", "6", "7"]
            .iter()
            .enumerate()
            .map(|(i, n)| TableLayout { is_table: Some(true), id: None, name: (*n).into(), shape: TableShape::Rect, x: i as i64 * 100, y: 0, w: 80, h: 80 })
            .collect();
        let tables: Vec<i64> = pos.save_floor_layout(&s, floor.id, &layout).unwrap().iter().map(|t| t.id).collect();
        let open = |pos: &Pos, t: OrderType, table: Option<i64>, channel: Option<i64>, details: OrderDetails| {
            pos.open_order(&s, &NewOrder { order_type: t, table_id: table, channel_id: channel, details }).unwrap().id
        };

        // Table 4: steak with options and a note, two curries, a cola; the steak is done.
        let t4 = open(&pos, OrderType::DineIn, Some(tables[0]), None, Default::default());
        pos.add_item(&s, t4, steak, 1, Some("rare"), None, &[cheese, bacon]).unwrap();
        let o = pos.add_item(&s, t4, curry, 2, None, None, &[]).unwrap();
        // The curries are made by another team.
        pos.set_item_team(&s, o.items[1].id, team("Main")).unwrap();
        pos.add_item(&s, t4, cola, 1, None, None, &[]).unwrap();
        let sent = pos.send_to_kitchen(&s, t4).unwrap();
        pos.kitchen_done(&s, &[sent[0].id], true).unwrap();
        // Table 5: a set and a cake; mains held behind the starter, then called.
        let t5 = open(&pos, OrderType::DineIn, Some(tables[1]), None, Default::default());
        let picks = [
            SetPick { section_id: sections[0].id, product_id: soup, with_id: None, ..Default::default() },
            SetPick { section_id: sections[1].id, product_id: curry, with_id: None, ..Default::default() },
        ];
        pos.add_set(&s, t5, set, &picks, Some("no chilli")).unwrap();
        pos.add_item(&s, t5, cake, 1, None, None, &[]).unwrap();
        pos.send_to_kitchen(&s, t5).unwrap();
        pos.call_mains(&s, t5).unwrap();
        // Table 6: all done (leaves the screen). Table 7: nothing sent yet.
        let t6 = open(&pos, OrderType::DineIn, Some(tables[2]), None, Default::default());
        pos.add_item(&s, t6, rice, 3, None, None, &[]).unwrap();
        let done: Vec<i64> = pos.send_to_kitchen(&s, t6).unwrap().iter().map(|i| i.id).collect();
        pos.kitchen_done(&s, &done, true).unwrap();
        let t7 = open(&pos, OrderType::DineIn, Some(tables[3]), None, Default::default());
        pos.add_item(&s, t7, soup, 1, None, None, &[]).unwrap();
        // Takeaways: paid just now, paid 7 hours ago, paid 30 hours ago, cancelled.
        let mut paid = Vec::new();
        for (n, dish) in [(0, soup), (1, rice), (2, steak), (3, curry)] {
            let o = open(&pos, OrderType::Takeaway, None, None, Default::default());
            let extra: &[i64] = if dish == steak { &[bacon] } else { &[] };
            pos.add_item(&s, o, dish, 1 + n, None, None, extra).unwrap();
            pos.add_item(&s, o, cola, 1, None, None, &[]).unwrap();
            pos.send_to_kitchen(&s, o).unwrap();
            if n < 3 {
                pos.pay_order(&s, o, PaymentMethod::Card, 0).unwrap();
                paid.push(o);
            } else {
                pos.cancel_order(&s, o, "mistake").unwrap();
            }
        }
        pos.conn.execute("UPDATE orders SET paid_at = paid_at - 7 * 3600 WHERE id = ?1", [paid[1]]).unwrap();
        pos.conn.execute("UPDATE orders SET paid_at = paid_at - 30 * 3600 WHERE id = ?1", [paid[2]]).unwrap();
        // Collection with a name; platform orders with and without their number.
        let details = |name: &str, r: Option<&str>| OrderDetails { customer_name: Some(name.into()), external_ref: r.map(Into::into), ..Default::default() };
        let col = open(&pos, OrderType::Collection, None, None, details("Ann", None));
        pos.add_item(&s, col, curry, 1, Some("mild"), None, &[]).unwrap();
        pos.send_to_kitchen(&s, col).unwrap();
        let g1 = open(&pos, OrderType::Delivery, None, Some(grab), details("Bo", Some("  A12 ")));
        pos.add_item(&s, g1, steak, 1, None, None, &[cheese]).unwrap();
        pos.add_item(&s, g1, cake, 2, None, None, &[]).unwrap();
        pos.send_to_kitchen(&s, g1).unwrap();
        let g2 = open(&pos, OrderType::Delivery, None, Some(grab), details("Cy", Some("   ")));
        pos.add_item(&s, g2, rice, 1, None, None, &[]).unwrap();
        pos.send_to_kitchen(&s, g2).unwrap();
        let own = open(&pos, OrderType::Delivery, None, None, OrderDetails {
            customer_name: Some("Di".into()), customer_phone: Some("0812345678".into()), delivery_address: Some("1 Road".into()), ..Default::default()
        });
        pos.add_item(&s, own, soup, 1, None, None, &[]).unwrap();
        pos.send_to_kitchen(&s, own).unwrap();
        // Sent at different times, some in the same second.
        pos.conn.execute("UPDATE order_items SET sent_at = sent_at - (id * 37) % 300 WHERE sent_at IS NOT NULL AND id % 4 <> 0", []).unwrap();

        let mut filters = vec!["all".to_string(), "drinks".into(), "desserts".into(), "food".into(), "team:999".into(), "team:x".into(), "".into()];
        filters.extend(teams.iter().map(|t| format!("team:{}", t.id)));
        let mut seen_paid = false;
        for (hours, labels) in [("6", false), ("0", false), ("24", true), ("8", true)] {
            pos.set_setting(&s, "kds_paid_hours", hours).unwrap();
            if labels {
                pos.set_setting(&s, "label_dine_in", "").unwrap();
                pos.set_setting(&s, "label_takeaway", "TA").unwrap();
                pos.set_setting(&s, "label_collection", "  ").unwrap();
                pos.set_setting(&s, "label_delivery", "DEL").unwrap();
            }
            for f in &filters {
                let new = pos.kitchen_board(&s, f).unwrap();
                assert_eq!(json(&new), json(&old_board(&pos, &s, f).unwrap()), "filter {f:?}, kds_paid_hours {hours}");
                seen_paid |= new.iter().any(|o| o.paid);
            }
        }
        // The data covers what matters: paid orders, sets, options, platforms.
        let all = pos.kitchen_board(&s, "all").unwrap();
        assert!(seen_paid);
        assert!(all.iter().any(|o| o.items.iter().any(|i| i.set_name.is_some())));
        assert!(all.iter().any(|o| o.items.iter().any(|i| i.options.len() == 2)));
        assert!(all.iter().any(|o| o.heading == "GRAB A12") && all.iter().any(|o| o.heading == format!("GRAB #{g2}")));
        assert!(all.iter().any(|o| o.heading == "4") && all.iter().any(|o| o.heading == format!("COLLECTION #{col}")));
        assert!(!all.iter().any(|o| o.order_id == t6 || o.order_id == t7 || o.order_id == paid[2]));
        assert!(all.iter().any(|o| o.order_id == paid[1]), "paid 7 hours ago, shown for 8");
    }
}
