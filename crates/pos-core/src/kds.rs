//! Kitchen screen: the dishes sent to the kitchen and not yet done, one card
//! per order, for a tablet in the kitchen (instead of, or as well as, paper).
//! Tapping a dish marks it done; "Done" on a card marks the whole order.
//! A dish taken off an order after it was sent stays a short while, struck
//! through and marked cancelled, so the kitchen notices (the same dishes as
//! the CANCELLED ticket; kept in memory only, lost on restart).

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
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
    /// Taken off the order after it was sent (see [`KDS_CANCELLED_SECS`]):
    /// shown struck through, never to do, and not for "Done". Its `id` is
    /// then negative (not a dish on the order).
    pub cancelled: bool,
}

/// How long a dish cancelled after it was sent stays on the kitchen screen.
pub const KDS_CANCELLED_SECS: i64 = 120;

/// A dish cancelled after it was sent, as the kitchen screen shows it.
#[derive(Debug, Clone)]
pub(crate) struct CancelledDish {
    /// When it was cancelled (unix seconds).
    at: i64,
    order_id: i64,
    head: OrderHead,
    item: BoardItem,
    team: Option<i64>,
    drink: bool,
    dessert: bool,
}

/// The dishes cancelled in the last [`KDS_CANCELLED_SECS`], on one [`Pos`].
#[derive(Debug, Default)]
pub(crate) struct CancelledDishes {
    list: Vec<CancelledDish>,
    /// Numbers the entries (their `id` on the screen is minus this).
    seq: i64,
    /// Tests: a shorter or longer window than [`KDS_CANCELLED_SECS`].
    window: Option<i64>,
}

impl CancelledDishes {
    fn window(&self) -> i64 {
        self.window.unwrap_or(KDS_CANCELLED_SECS)
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
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
#[derive(Debug, Clone)]
struct OrderHead {
    kind: String,
    table: Option<String>,
    channel: Option<String>,
    external: Option<String>,
    customer: Option<String>,
    paid: bool,
}

/// "DINE-IN 4", "TAKEAWAY #12", "GRAB A12" (see [`BoardOrder::heading`]).
fn heading_of(labels: &[String; 4], order_id: i64, head: &OrderHead) -> String {
    // Orders without a table carry their number, so three
    // takeaways are three different cards ("TAKEAWAY #12").
    let number = format!("#{order_id}");
    let external = head.external.as_deref().map(str::trim).filter(|r| !r.is_empty()).map(str::to_string);
    match (&head.channel, head.kind.as_str()) {
        (Some(ch), _) => format!("{} {}", ch.to_uppercase(), external.unwrap_or(number)),
        (None, "dine_in") => format!("{} {}", labels[0], head.table.as_deref().unwrap_or_default()).trim().to_string(),
        (None, "collection") => format!("{} {number}", labels[1]),
        (None, "delivery") => format!("{} {number}", labels[2]),
        (None, _) => format!("{} {number}", labels[3]),
    }
}

/// Whether a dish shows under `filter` (see [`Pos::kitchen_board`]).
fn shows(filter: &str, team: Option<i64>, item_team: Option<i64>, drink: bool, dessert: bool) -> bool {
    match filter {
        "drinks" => drink,
        "desserts" => dessert,
        "food" => !drink,
        _ => team.is_none_or(|t| item_team == Some(t)),
    }
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
                        cancelled: false,
                    },
                    r.get(1)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                    OrderHead { kind: r.get(13)?, table: r.get(14)?, channel: r.get(15)?, external: r.get(16)?, customer: r.get(17)?, paid: r.get(18)? },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let cancelled = self.kds_cancelled_now(filter, team, paid_secs)?;
        if rows.is_empty() && cancelled.is_empty() {
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
            if !shows(filter, team, item_team, drink, dessert) {
                continue;
            }
            item.options = options.remove(&item.id).unwrap_or_default();
            let at = *at_of.entry(order_id).or_insert_with(|| {
                let heading = heading_of(&labels, order_id, &head);
                out.push(BoardOrder { order_id, heading, customer_name: head.customer, since: item.sent_at, paid: head.paid, items: Vec::new() });
                out.len() - 1
            });
            out[at].items.push(item);
        }
        // Dishes cancelled a moment ago: on their order's card, or on a card
        // of their own (headed as the order was) when nothing else shows.
        let mut with_cancelled = Vec::new();
        for (c, paid) in cancelled {
            let at = *at_of.entry(c.order_id).or_insert_with(|| {
                let heading = heading_of(&labels, c.order_id, &c.head);
                out.push(BoardOrder { order_id: c.order_id, heading, customer_name: c.head.customer.clone(), since: c.item.sent_at, paid, items: Vec::new() });
                out.len() - 1
            });
            out[at].items.push(c.item);
            with_cancelled.push(at);
        }
        for at in with_cancelled {
            // Next to the dishes sent with it (a stable sort: the rest keep their order).
            out[at].items.sort_by_key(|i| i.sent_at);
        }
        // Only orders with something still to do (or just cancelled); oldest first.
        out.retain(|o| o.items.iter().any(|i| !i.done || i.cancelled));
        for o in &mut out {
            let todo = o.items.iter().filter(|i| !i.done && !i.cancelled).map(|i| i.sent_at).min();
            o.since = todo.or_else(|| o.items.iter().filter(|i| i.cancelled).map(|i| i.sent_at).min()).unwrap_or(o.since);
        }
        out.sort_by_key(|o| o.since);
        Ok(out)
    }

    /// The dishes cancelled within the window that show under `filter`, each
    /// with whether its order is paid. A paid order's leave the screen with
    /// it (`kds_paid_hours`).
    fn kds_cancelled_now(&self, filter: &str, team: Option<i64>, paid_secs: i64) -> Result<Vec<(CancelledDish, bool)>> {
        if self.kds_cancelled.list.is_empty() {
            return Ok(Vec::new());
        }
        let now = unix_now();
        let window = self.kds_cancelled.window();
        let mut out = Vec::new();
        let mut status: HashMap<i64, Option<(bool, bool)>> = HashMap::new();
        let mut stmt = self.conn.prepare_cached("SELECT status = 'paid', status = 'paid' AND NOT (?2 > 0 AND paid_at > unixepoch() - ?2) FROM orders WHERE id = ?1")?;
        for c in &self.kds_cancelled.list {
            if now - c.at >= window || !shows(filter, team, c.team, c.drink, c.dessert) {
                continue;
            }
            let st = match status.get(&c.order_id) {
                Some(st) => *st,
                None => {
                    let st = stmt.query_row(params![c.order_id, paid_secs], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
                    status.insert(c.order_id, st);
                    st
                }
            };
            // Paid and gone from the screen: so are its cancelled dishes.
            if st.is_some_and(|(_, gone)| gone) {
                continue;
            }
            out.push((c.clone(), st.is_some_and(|(paid, _)| paid)));
        }
        Ok(out)
    }

    /// What the kitchen screen will show for `lines` (dish id, and how many
    /// were cancelled; None = all of it) of `order_id`, read before they
    /// change. Sets' own lines (never on the screen) are left out. Kept by
    /// [`Pos::kds_note_cancelled`] once the change is made.
    pub(crate) fn kds_cancel_snapshot(&self, order_id: i64, lines: &[(i64, Option<i64>)]) -> Result<Vec<CancelledDish>> {
        if lines.is_empty() {
            return Ok(Vec::new());
        }
        let head = self.conn.prepare_cached(
            "SELECT o.order_type, t.name, ch.name, o.external_ref, o.customer_name, o.status = 'paid'
             FROM orders o LEFT JOIN dining_tables t ON t.id = o.table_id LEFT JOIN delivery_channels ch ON ch.id = o.channel_id
             WHERE o.id = ?1",
        )?
        .query_row([order_id], |r| Ok(OrderHead { kind: r.get(0)?, table: r.get(1)?, channel: r.get(2)?, external: r.get(3)?, customer: r.get(4)?, paid: r.get(5)? }))
        .optional()?;
        let Some(head) = head else { return Ok(Vec::new()) };
        let mut dish = self.conn.prepare_cached(
            "SELECT i.qty, i.name, i.name_alt, i.note, COALESCE(i.sent_at, i.prep_sent_at, unixepoch()), i.with_name, par.name,
                    COALESCE(i.team_id, p.team_id), p.drink, p.dessert
             FROM order_items i
             JOIN products p ON p.id = i.product_id
             LEFT JOIN order_items par ON par.id = i.parent_id
             WHERE i.id = ?1 AND i.order_id = ?2 AND i.is_set = 0",
        )?;
        let mut opts = self.conn.prepare_cached("SELECT name FROM order_item_options WHERE item_id = ?1 ORDER BY id")?;
        let mut out = Vec::new();
        for &(id, qty) in lines {
            let row = dish
                .query_row(params![id, order_id], |r| {
                    Ok((
                        BoardItem {
                            id: 0,
                            qty: r.get(0)?,
                            name: r.get(1)?,
                            name_alt: r.get(2)?,
                            options: Vec::new(),
                            note: r.get(3)?,
                            sent_at: r.get(4)?,
                            with_name: r.get(5)?,
                            set_name: r.get(6)?,
                            done: false,
                            cancelled: true,
                        },
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                    ))
                })
                .optional()?;
            let Some((mut item, team, drink, dessert)) = row else { continue };
            if let Some(q) = qty {
                item.qty = q;
            }
            if item.qty <= 0 {
                continue;
            }
            item.options = opts.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            out.push(CancelledDish { at: 0, order_id, head: head.clone(), item, team, drink, dessert });
        }
        Ok(out)
    }

    /// Keep `dishes` (from [`Pos::kds_cancel_snapshot`]) on the kitchen
    /// screen from now, and forget those whose time is up.
    pub(crate) fn kds_note_cancelled(&mut self, dishes: Vec<CancelledDish>) {
        let now = unix_now();
        let list = &mut self.kds_cancelled;
        let window = list.window();
        list.list.retain(|c| now - c.at < window);
        for mut c in dishes {
            list.seq += 1;
            c.at = now;
            c.item.id = -list.seq;
            list.list.push(c);
        }
        // A runaway (hundreds of dishes in two minutes): the newest stay.
        let extra = list.list.len().saturating_sub(1000);
        list.list.drain(..extra);
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
        // Cancelled dishes on the screen (negative ids) are not on the order: nothing to mark.
        let item_ids: Vec<i64> = item_ids.iter().copied().filter(|&id| id > 0).collect();
        let mut stmt = self.conn.prepare_cached(
            "UPDATE order_items SET kitchen_done_at = CASE WHEN ?2 THEN COALESCE(kitchen_done_at, unixepoch()) ELSE NULL END WHERE id = ?1",
        )?;
        for id in &item_ids {
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
                        cancelled: false,
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
                category_id: Some(cat), sku: None, barcode: None, name: name.into(), price_cents: 1000, cost_cents: 0, tax_rate_bp: Some(1000),
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

    /// A small menu: steak (Main team), cola (a drink), cake (a dessert).
    struct Kit {
        pos: Pos,
        s: Session,
        steak: i64,
        cola: i64,
        cake: i64,
        main: Option<i64>,
    }

    fn kit() -> Kit {
        let mut pos = Pos::open_in_memory().unwrap();
        let s = pos.setup_first_admin("Owner", "1111").unwrap();
        let teams = pos.list_teams(&s).unwrap();
        let main = teams.iter().find(|t| t.name == "Main").map(|t| t.id);
        let cat = pos.create_category(&s, "Food", None, 1).unwrap().id;
        let mut item = |name: &str, team: Option<i64>| {
            pos.create_product(&s, NewProduct {
                category_id: Some(cat), sku: None, barcode: None, name: name.into(), price_cents: 1000, cost_cents: 0, tax_rate_bp: Some(1000),
                stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: team,
            })
            .unwrap()
            .id
        };
        let (steak, cola, cake) = (item("Steak", main), item("Cola", None), item("Cake", None));
        pos.set_drink(&s, cola, true).unwrap();
        pos.set_dessert(&s, cake, true).unwrap();
        Kit { pos, s, steak, cola, cake, main }
    }

    impl Kit {
        fn takeaway(&mut self) -> i64 {
            self.pos.open_order(&self.s, &NewOrder { order_type: OrderType::Takeaway, table_id: None, channel_id: None, details: Default::default() }).unwrap().id
        }
        fn add(&mut self, order: i64, product: i64, qty: i64, note: Option<&str>) -> i64 {
            let o = self.pos.add_item(&self.s, order, product, qty, note, None, &[]).unwrap();
            o.items.iter().filter(|i| i.product_id == product).map(|i| i.id).max().unwrap()
        }
        fn set_qty(&mut self, item: i64, qty: i64) {
            let note = self.pos.get_order(&self.s, self.pos.order_of_item(item).unwrap()).unwrap().items.into_iter().find(|i| i.id == item).unwrap().note;
            self.pos.update_item_for_kitchen(&self.s, item, &ItemUpdate { qty, discount_cents: 0, note }).unwrap();
        }
        fn board(&self, filter: &str) -> Vec<BoardOrder> {
            self.pos.kitchen_board(&self.s, filter).unwrap()
        }
        /// Make every cancelled dish `secs` older.
        fn age(&mut self, secs: i64) {
            for c in &mut self.pos.kds_cancelled.list {
                c.at -= secs;
            }
        }
    }

    #[test]
    fn a_dish_reduced_after_sending_shows_struck_through_next_to_the_rest() {
        let mut k = kit();
        let o = k.takeaway();
        let steak = k.add(o, k.steak, 3, Some("rare"));
        let cake = k.add(o, k.cake, 1, None);
        k.pos.send_to_kitchen(&k.s, o).unwrap();
        k.set_qty(steak, 1);
        let b = k.board("all");
        assert_eq!(b.len(), 1);
        let card = &b[0];
        assert_eq!(card.heading, format!("TAKEAWAY #{o}"));
        let live: Vec<_> = card.items.iter().filter(|i| !i.cancelled).collect();
        let gone: Vec<_> = card.items.iter().filter(|i| i.cancelled).collect();
        assert_eq!(live.iter().map(|i| (i.id, i.qty)).collect::<Vec<_>>(), vec![(steak, 1), (cake, 1)]);
        assert_eq!(gone.len(), 1);
        let g = gone[0];
        assert_eq!((g.qty, g.name.as_str(), g.note.as_deref(), g.done), (2, "Steak", Some("rare"), false));
        assert!(g.id < 0, "not a dish on the order");
        // Next to the dish it was sent with.
        assert!(card.items.iter().position(|i| i.cancelled).unwrap() < card.items.iter().position(|i| i.id == cake).unwrap() || g.sent_at == live[1].sent_at);

        // Done (every id the card has) marks the real dishes only, and the
        // card stays while the cancelled dish shows, then goes.
        let ids: Vec<i64> = card.items.iter().map(|i| i.id).collect();
        k.pos.kitchen_done(&k.s, &ids, true).unwrap();
        let b = k.board("all");
        assert_eq!(b.len(), 1);
        assert!(b[0].items.iter().all(|i| i.done != i.cancelled), "the real dishes done, the cancelled one not");
        k.pos.kitchen_done(&k.s, &[g.id], true).unwrap();
        k.age(KDS_CANCELLED_SECS);
        assert!(k.board("all").is_empty());
    }

    #[test]
    fn a_whole_order_cancelled_shows_as_a_cancelled_card_with_its_heading() {
        let mut k = kit();
        let floor = k.pos.create_floor(&k.s, "Ground").unwrap();
        let layout = [TableLayout { is_table: Some(true), id: None, name: "9".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }];
        let table = k.pos.save_floor_layout(&k.s, floor.id, &layout).unwrap()[0].id;
        let o = k.pos.open_order(&k.s, &NewOrder { order_type: OrderType::DineIn, table_id: Some(table), channel_id: None, details: Default::default() }).unwrap().id;
        let steak = k.add(o, k.steak, 2, None);
        let cola = k.add(o, k.cola, 1, None);
        k.pos.send_to_kitchen(&k.s, o).unwrap();
        // The cola is out already: not cancelled on the screen (nor on the ticket).
        k.pos.kitchen_done(&k.s, &[cola], true).unwrap();
        let before = k.board("all");
        k.pos.cancel_order_for_kitchen(&k.s, o, "mistake").unwrap();
        let b = k.board("all");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].heading, before[0].heading);
        assert_eq!(b[0].heading, "DINE-IN 9");
        assert_eq!(b[0].items.len(), 1);
        let i = &b[0].items[0];
        assert!(i.cancelled && !i.done && i.id < 0 && i.id != steak);
        assert_eq!((i.qty, i.name.as_str()), (2, "Steak"));
        // Another instance (another database) sees none of it.
        let other = kit();
        assert!(other.board("all").is_empty());
        // Gone after two minutes, not before.
        k.age(KDS_CANCELLED_SECS - 5);
        assert_eq!(k.board("all").len(), 1);
        k.age(5);
        assert!(k.board("all").is_empty());
    }

    #[test]
    fn the_cancelled_window_can_be_changed_and_expired_entries_are_forgotten() {
        let mut k = kit();
        k.pos.kds_cancelled.window = Some(30);
        let o = k.takeaway();
        let steak = k.add(o, k.steak, 2, None);
        k.pos.send_to_kitchen(&k.s, o).unwrap();
        k.set_qty(steak, 1);
        assert!(k.board("all")[0].items.iter().any(|i| i.cancelled));
        k.age(30);
        assert!(!k.board("all")[0].items.iter().any(|i| i.cancelled));
        // The next cancel forgets the old one.
        k.set_qty(steak, 0);
        assert_eq!(k.pos.kds_cancelled.list.len(), 1);
        let b = k.board("all");
        assert_eq!(b.len(), 1);
        assert!(b[0].items.iter().all(|i| i.cancelled));
    }

    #[test]
    fn cancelled_dishes_follow_the_screen_filters_and_paid_hours() {
        let mut k = kit();
        let o = k.takeaway();
        let steak = k.add(o, k.steak, 1, None);
        let cola = k.add(o, k.cola, 2, None);
        let cake = k.add(o, k.cake, 1, None);
        k.pos.send_to_kitchen(&k.s, o).unwrap();
        k.set_qty(cola, 1);
        k.set_qty(cake, 0);
        k.set_qty(steak, 0);
        let cancelled = |b: &[BoardOrder]| -> Vec<String> {
            let mut v: Vec<String> = b.iter().flat_map(|o| o.items.iter().filter(|i| i.cancelled).map(|i| i.name.clone())).collect();
            v.sort();
            v
        };
        assert_eq!(cancelled(&k.board("all")), ["Cake", "Cola", "Steak"]);
        assert_eq!(cancelled(&k.board("drinks")), ["Cola"]);
        assert_eq!(cancelled(&k.board("desserts")), ["Cake"]);
        assert_eq!(cancelled(&k.board("food")), ["Cake", "Steak"]);
        assert_eq!(cancelled(&k.board(&format!("team:{}", k.main.unwrap()))), ["Steak"]);
        assert!(k.board("team:999").is_empty());
        // A card made of cancelled dishes only, under the Main team's filter.
        let main = k.board(&format!("team:{}", k.main.unwrap()));
        assert_eq!(main.len(), 1);
        assert!(main[0].items.iter().all(|i| i.cancelled));
        // Paid: shown as long as a paid order is (kds_paid_hours), then gone with it.
        k.pos.pay_order(&k.s, o, PaymentMethod::Card, 0).unwrap();
        let b = k.board("all");
        assert!(b[0].paid);
        assert_eq!(cancelled(&b), ["Cake", "Cola", "Steak"]);
        k.pos.set_setting(&k.s, "kds_paid_hours", "0").unwrap();
        assert!(k.board("all").is_empty());
    }

    #[test]
    fn dishes_never_sent_do_not_show_as_cancelled() {
        let mut k = kit();
        let o = k.takeaway();
        let sent = k.add(o, k.steak, 1, None);
        k.pos.send_to_kitchen(&k.s, o).unwrap();
        let unsent = k.add(o, k.cake, 2, None);
        k.set_qty(unsent, 1);
        k.set_qty(unsent, 0);
        let b = k.board("all");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].items.iter().map(|i| (i.id, i.cancelled)).collect::<Vec<_>>(), vec![(sent, false)]);
        // A whole order never sent: nothing either.
        let o2 = k.takeaway();
        k.add(o2, k.steak, 1, None);
        k.pos.cancel_order_for_kitchen(&k.s, o2, "mistake").unwrap();
        assert!(k.pos.kds_cancelled.list.is_empty());
        assert_eq!(k.board("all").len(), 1);
    }

    #[test]
    fn a_set_removed_after_sending_shows_its_dishes_cancelled() {
        let mut k = kit();
        let set = k.pos.create_product(&k.s, NewProduct {
            category_id: None, sku: None, barcode: None, name: "Steak set".into(), price_cents: 2000, cost_cents: 0, tax_rate_bp: Some(1000),
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap()
        .id;
        let sections = k.pos
            .save_set(&k.s, set, &[
                SetSectionInput { name: "Main".into(), name_alt: None, pick: 1, starter: false, product_ids: vec![k.steak], with_ids: vec![] },
                SetSectionInput { name: "Drink".into(), name_alt: None, pick: 1, starter: false, product_ids: vec![k.cola], with_ids: vec![] },
            ])
            .unwrap();
        let o = k.takeaway();
        let picks = [
            SetPick { section_id: sections[0].id, product_id: k.steak, with_id: None, ..Default::default() },
            SetPick { section_id: sections[1].id, product_id: k.cola, with_id: None, ..Default::default() },
        ];
        let order = k.pos.add_set(&k.s, o, set, &picks, None).unwrap();
        let line = order.items.iter().find(|i| i.is_set).unwrap().id;
        k.pos.send_to_kitchen(&k.s, o).unwrap();
        k.set_qty(line, 0);
        let b = k.board("all");
        assert_eq!(b.len(), 1);
        let mut names: Vec<(&str, Option<&str>, bool)> = b[0].items.iter().map(|i| (i.name.as_str(), i.set_name.as_deref(), i.cancelled)).collect();
        names.sort();
        assert_eq!(names, vec![("Cola", Some("Steak set"), true), ("Steak", Some("Steak set"), true)]);
    }
}
