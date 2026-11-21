//! Tickets ("Contact admin"): who sees, answers, closes and deletes them,
//! the unread badges, and what the server fills in itself.

use pos_core::*;

struct Kit {
    pos: Pos,
    admin: Session,
    quinn: Session,
    sam: Session,
    lee: Session,
}

fn setup() -> Kit {
    let mut pos = Pos::open_in_memory().unwrap();
    let admin = pos.setup_first_admin("Owner", "1234").unwrap();
    pos.create_user(&admin, "Quinn", Role::Manager, "4646").unwrap();
    pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
    pos.create_user(&admin, "Lee", Role::Staff, "3333").unwrap();
    let quinn = pos.login_as(None, "4646", &mut Default::default()).unwrap();
    let sam = pos.login_as(None, "2222", &mut Default::default()).unwrap();
    let lee = pos.login_as(None, "3333", &mut Default::default()).unwrap();
    Kit { pos, admin, quinn, sam, lee }
}

fn ticket(category: &str, urgency: &str, body: &str, order_id: Option<i64>) -> NewTicket {
    NewTicket {
        category: category.into(),
        urgency: Some(urgency.into()),
        body: body.into(),
        order_id,
        screen: Some("order".into()),
        screen_label: Some("Order #1".into()),
        device: Some("Bar tablet".into()),
    }
}

#[test]
fn staff_see_their_own_tickets_and_answerers_see_and_answer_all() {
    let mut k = setup();
    let floor = k.pos.create_floor(&k.admin, "Ground").unwrap();
    let t = k
        .pos
        .save_floor_layout(&k.admin, floor.id, &[TableLayout { is_table: Some(true), id: None, name: "5".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    let order = k.pos.open_order(&k.sam, &NewOrder { order_type: OrderType::DineIn, table_id: Some(t[0].id), channel_id: None, details: Default::default() }).unwrap();
    k.pos.set_terminal(Some("abcdef123456"));
    let tk = k.pos.create_ticket(&k.sam, &ticket("printer", "urgent", "  Kitchen printer not printing ", Some(order.id))).unwrap();
    k.pos.set_terminal(None);
    // Filled in by the server: the table from the order, the terminal, the version.
    assert_eq!((tk.table_name.as_deref(), tk.terminal.as_str(), tk.device.as_str()), (Some("5"), "web", "Bar tablet"));
    assert_eq!(tk.context["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(tk.context["terminal_label"], "Browser");
    assert_eq!(tk.context["screen_label"], "Order #1");
    assert_eq!((tk.status.as_str(), tk.messages[0].body.as_str(), tk.mine), ("open", "Kitchen printer not printing", true));

    // Lee (other staff) cannot see it: not found, never forbidden.
    assert!(matches!(k.pos.get_ticket(&k.lee, tk.id), Err(PosError::NotFound)));
    assert!(matches!(k.pos.reply_ticket(&k.lee, tk.id, "hi", None), Err(PosError::NotFound)));
    assert!(matches!(k.pos.set_ticket_status(&k.lee, tk.id, "done"), Err(PosError::NotFound)));
    assert!(k.pos.list_tickets(&k.lee, "open", 100, 0).unwrap().is_empty(), "staff asking for Open get their own");
    assert_eq!(k.pos.list_tickets(&k.sam, "open", 100, 0).unwrap().len(), 1);

    // The manager and the admin see it, urgent, unread.
    for who in [&k.quinn, &k.admin] {
        let b = k.pos.ticket_badges(who).unwrap();
        assert_eq!((b.unread, b.open, b.waiting, b.urgent.len(), b.answerer), (1, 1, 0, 1, true));
        assert_eq!(b.urgent[0].by, "Sam");
    }
    let open = k.pos.list_tickets(&k.quinn, "open", 100, 0).unwrap();
    assert_eq!((open.len(), open[0].unread, open[0].waiting), (1, true, false));
    // Sam has no badge for his own message.
    let b = k.pos.ticket_badges(&k.sam).unwrap();
    assert_eq!((b.unread, b.open, b.urgent.len(), b.latest.is_none(), b.answerer), (0, 0, 0, true, false));

    // Quinn replies: it goes In progress and moves to Waiting; Sam gets a badge.
    let after = k.pos.reply_ticket(&k.quinn, tk.id, "On my way", None).unwrap();
    assert_eq!(after.status, "in_progress");
    assert!(after.waiting);
    assert_eq!(k.pos.list_tickets(&k.admin, "waiting", 100, 0).unwrap().len(), 1);
    assert!(k.pos.list_tickets(&k.admin, "open", 100, 0).unwrap().is_empty());
    let b = k.pos.ticket_badges(&k.sam).unwrap();
    assert_eq!((b.unread, b.unread_mine), (1, 1));
    let latest = b.latest.unwrap();
    assert_eq!((latest.ticket_id, latest.by.as_str(), latest.preview.as_str()), (tk.id, "Quinn", "On my way"));
    assert_eq!(k.pos.ticket_badges(&k.quinn).unwrap().unread, 0, "Quinn wrote last");
    // Reading clears it (per user, so on every device).
    let read = k.pos.get_ticket(&k.sam, tk.id).unwrap();
    assert_eq!(read.messages.iter().map(|m| (m.kind.as_str(), m.body.as_str())).collect::<Vec<_>>(), [
        ("message", "Kitchen printer not printing"),
        ("message", "On my way"),
        ("status", "in_progress")
    ]);
    assert_eq!(k.pos.ticket_badges(&k.sam).unwrap().unread, 0);
    // The admin still has it unread until they open it.
    assert_eq!(k.pos.ticket_badges(&k.admin).unwrap().unread, 1);
    k.pos.get_ticket(&k.admin, tk.id).unwrap();
    assert_eq!(k.pos.ticket_badges(&k.admin).unwrap().urgent.len(), 0);

    // Sam: "Fixed now, thanks" closes it; staff cannot set In progress.
    assert!(matches!(k.pos.set_ticket_status(&k.sam, tk.id, "in_progress"), Err(PosError::Forbidden)));
    let done = k.pos.reply_ticket(&k.sam, tk.id, "Fixed now, thanks", Some("done")).unwrap();
    assert_eq!((done.status.as_str(), done.closed_by_name.as_deref(), done.can_reopen), ("done", Some("Sam"), true));
    assert_eq!(k.pos.list_tickets(&k.quinn, "done", 100, 0).unwrap().len(), 1);
    // "Still not working" reopens it within 7 days.
    let again = k.pos.reply_ticket(&k.sam, tk.id, "Still not working", Some("open")).unwrap();
    assert_eq!(again.status, "open");
    // An answerer reopens or closes any time.
    k.pos.set_ticket_status(&k.quinn, tk.id, "done").unwrap();
    k.pos.set_ticket_status(&k.quinn, tk.id, "in_progress").unwrap();

    // Only admins delete; messages and read markers go; the log says so.
    assert!(matches!(k.pos.delete_ticket(&k.quinn, tk.id), Err(PosError::Forbidden)));
    assert!(matches!(k.pos.delete_ticket(&k.sam, tk.id), Err(PosError::Forbidden)));
    k.pos.delete_ticket(&k.admin, tk.id).unwrap();
    assert!(matches!(k.pos.get_ticket(&k.admin, tk.id), Err(PosError::NotFound)));
    let log = k.pos.list_activity(&k.admin, &ActivityFilter { kind: Some("other".into()), ..Default::default() }, None, None).unwrap();
    let mine: Vec<(&str, &str)> = log.rows.iter().filter(|r| r.action.starts_with("ticket.")).map(|r| (r.action.as_str(), r.detail.as_str())).collect();
    assert_eq!(mine[0], ("ticket.delete", &*format!("#{} printer by Sam, 4 messages", tk.id)));
    assert!(mine.contains(&("ticket.create", &*format!("#{} printer urgent", tk.id))));
    assert!(mine.contains(&("ticket.status", &*format!("#{} open -> in_progress", tk.id))));
    // Ticket rows are about the order: they show in its activity.
    let on_order = k.pos.order_activity(&k.quinn, order.id).unwrap();
    assert!(on_order.iter().any(|r| r.action == "ticket.create"), "{on_order:?}");
    let created = on_order.iter().find(|r| r.action == "ticket.create").unwrap();
    assert_eq!(created.terminal.as_deref(), Some("web-abcdef"));
}

#[test]
fn tickets_are_checked_and_limited() {
    let mut k = setup();
    let bad = |c: &str, b: &str| NewTicket { category: c.into(), urgency: None, body: b.into(), order_id: None, screen: None, screen_label: None, device: None };
    assert_eq!(k.pos.create_ticket(&k.sam, &bad("coffee", "hi")).unwrap_err().to_string(), "Pick what it's about");
    assert_eq!(k.pos.create_ticket(&k.sam, &bad("till", "   ")).unwrap_err().to_string(), "Add a few words");
    assert!(k.pos.create_ticket(&k.sam, &bad("till", &"x".repeat(501))).is_err());
    // Without a device name: "Browser" (not the main till).
    let t = k.pos.create_ticket(&k.sam, &bad("till", "slow")).unwrap();
    assert_eq!((t.device.as_str(), t.order_id), ("Browser", None));
    // A made-up order: nothing attached.
    let t = k.pos.create_ticket(&k.sam, &NewTicket { order_id: Some(999), ..bad("order", "x") }).unwrap();
    assert_eq!((t.order_id, t.table_name), (None, None));
    for _ in 0..18 {
        k.pos.create_ticket(&k.sam, &bad("other", "again")).unwrap();
    }
    assert_eq!(k.pos.create_ticket(&k.sam, &bad("other", "again")).unwrap_err().to_string(), "Too many tickets, try again later");
    // A manager may open one too; other answerers see it like any other.
    let q = k.pos.create_ticket(&k.quinn, &bad("menu", "for the owner")).unwrap();
    assert!(k.pos.get_ticket(&k.admin, q.id).is_ok());
    assert_eq!(k.pos.ticket_badges(&k.quinn).unwrap().unread, 20, "Sam's 20, not her own");
}

#[test]
fn the_urgent_count_is_every_unread_urgent_ticket_not_just_the_three_listed() {
    let mut k = setup();
    for i in 0..5 {
        k.pos.create_ticket(&k.sam, &ticket("till", "urgent", &format!("frozen {i}"), None)).unwrap();
    }
    k.pos.create_ticket(&k.sam, &ticket("till", "normal", "slow", None)).unwrap();
    let b = k.pos.ticket_badges(&k.quinn).unwrap();
    assert_eq!((b.urgent.len(), b.urgent_count), (3, 5));
    // Read one: one fewer. Staff never get a count.
    let first = b.urgent[0].id;
    k.pos.get_ticket(&k.quinn, first).unwrap();
    assert_eq!(k.pos.ticket_badges(&k.quinn).unwrap().urgent_count, 4);
    assert_eq!(k.pos.ticket_badges(&k.admin).unwrap().urgent_count, 5, "read markers are per person");
    assert_eq!(k.pos.ticket_badges(&k.sam).unwrap().urgent_count, 0);
}

#[test]
fn void_and_cancel_rows_carry_the_orders_table_and_total() {
    let mut k = setup();
    let floor = k.pos.create_floor(&k.admin, "Ground").unwrap();
    let t = k
        .pos
        .save_floor_layout(&k.admin, floor.id, &[TableLayout { is_table: Some(true), id: None, name: "17".into(), shape: TableShape::Rect, x: 0, y: 0, w: 80, h: 80 }])
        .unwrap();
    let soup = k
        .pos
        .create_product(&k.admin, NewProduct {
            category_id: None, sku: None, barcode: None, name: "Soup".into(), price_cents: 650, cost_cents: 0, tax_rate_bp: Some(0),
            stock_qty: 0, track_stock: false, starter: false, name_alt: None, prep: false, team_id: None,
        })
        .unwrap();
    let dine_in = NewOrder { order_type: OrderType::DineIn, table_id: Some(t[0].id), channel_id: None, details: Default::default() };
    let paid = k.pos.open_order(&k.admin, &dine_in).unwrap().id;
    k.pos.add_item(&k.admin, paid, soup.id, 2, None, None, &[]).unwrap();
    k.pos.pay_order(&k.admin, paid, PaymentMethod::Cash, 2000).unwrap();
    k.pos.void_order(&k.admin, paid, "wrong table").unwrap();
    let open = k.pos.open_order(&k.admin, &dine_in).unwrap().id;
    k.pos.add_item(&k.admin, open, soup.id, 1, None, None, &[]).unwrap();
    k.pos.cancel_order(&k.admin, open, "left").unwrap();
    let rows = k.pos.list_activity(&k.admin, &ActivityFilter { kind: Some("void".into()), ..Default::default() }, None, None).unwrap().rows;
    let of = |action: &str| rows.iter().find(|r| r.action == action).and_then(|r| r.order.clone()).unwrap_or_else(|| panic!("{action}: {rows:?}"));
    let void = of("order.void");
    assert_eq!((void.table_name.as_deref(), void.total_cents, void.status.as_str()), (Some("17"), 1300, "voided"));
    let cancel = of("order.cancel");
    assert_eq!((cancel.table_name.as_deref(), cancel.status.as_str()), (Some("17"), "cancelled"));
}

fn ids(c: &rusqlite::Connection, sql: &str) -> Vec<i64> {
    let mut stmt = c.prepare(sql).unwrap();
    stmt.query_map([], |r| r.get(0)).unwrap().collect::<std::result::Result<_, _>>().unwrap()
}

#[test]
fn ticket_and_message_ids_are_never_used_again() {
    let mut k = setup();
    let a = k.pos.create_ticket(&k.sam, &ticket("printer", "normal", "jam", None)).unwrap();
    let a = k.pos.reply_ticket(&k.quinn, a.id, "on my way", None).unwrap();
    let last_msg = a.messages.last().unwrap().id;
    k.pos.delete_ticket(&k.admin, a.id).unwrap();
    let b = k.pos.create_ticket(&k.sam, &ticket("other", "normal", "hello", None)).unwrap();
    assert!(b.id > a.id, "a deleted ticket's id is not handed out again");
    assert!(b.messages[0].id > last_msg, "nor its messages' ids");
}

/// A 0.14.0 database (ids could be reused) is rebuilt with AUTOINCREMENT,
/// keeping every ticket, message and read marker, and the next ids come
/// after any the log or a till may still know.
#[test]
fn a_0_14_0_database_gets_ids_that_are_never_reused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pos.db");
    let (kept, gone) = {
        let mut pos = Pos::open(&path).unwrap();
        let admin = pos.setup_first_admin("Owner", "1234").unwrap();
        pos.create_user(&admin, "Sam", Role::Staff, "2222").unwrap();
        let sam = pos.login_as(None, "2222", &mut Default::default()).unwrap();
        let kept = pos.create_ticket(&sam, &ticket("printer", "urgent", "jam", None)).unwrap();
        pos.reply_ticket(&admin, kept.id, "coming", None).unwrap();
        pos.get_ticket(&sam, kept.id).unwrap();
        let gone = pos.create_ticket(&sam, &ticket("menu", "normal", "price", None)).unwrap();
        pos.reply_ticket(&admin, gone.id, "fixed", None).unwrap();
        pos.delete_ticket(&admin, gone.id).unwrap();
        (kept.id, gone.id)
    };
    {
        // Back to 0.14.0: the tables as 0032 made them, no sequence.
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!(
            "PRAGMA foreign_keys = OFF;
             BEGIN;
             CREATE TABLE t2 AS SELECT * FROM tickets; CREATE TABLE m2 AS SELECT * FROM ticket_messages; CREATE TABLE r2 AS SELECT * FROM ticket_reads;
             DROP TABLE ticket_reads; DROP TABLE ticket_messages; DROP TABLE tickets;
             {}
             INSERT INTO tickets SELECT * FROM t2; INSERT INTO ticket_messages SELECT * FROM m2; INSERT INTO ticket_reads SELECT * FROM r2;
             DROP TABLE t2; DROP TABLE m2; DROP TABLE r2;
             DELETE FROM sqlite_sequence WHERE name IN ('tickets', 'ticket_messages');
             COMMIT;
             PRAGMA user_version = 33;",
            include_str!("../src/migrations/0032_tickets.sql")
        ))
        .unwrap();
        assert!(!c.query_row("SELECT sql FROM sqlite_schema WHERE name = 'tickets'", [], |r| r.get::<_, String>(0)).unwrap().contains("AUTOINCREMENT"));
    }
    let before = rusqlite::Connection::open(&path).unwrap();
    let msgs = ids(&before, "SELECT id FROM ticket_messages ORDER BY id");
    let reads = ids(&before, "SELECT last_read_id FROM ticket_reads ORDER BY ticket_id, user_id");
    drop(before);
    let mut pos = Pos::open(&path).unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    for t in ["tickets", "ticket_messages"] {
        let sql: String = c.query_row("SELECT sql FROM sqlite_schema WHERE name = ?1", [t], |r| r.get(0)).unwrap();
        assert!(sql.contains("AUTOINCREMENT"), "{sql}");
    }
    assert_eq!(ids(&c, "SELECT id FROM tickets"), [kept]);
    assert_eq!(ids(&c, "SELECT id FROM ticket_messages ORDER BY id"), msgs, "every message kept");
    assert_eq!(ids(&c, "SELECT last_read_id FROM ticket_reads ORDER BY ticket_id, user_id"), reads, "and every read marker");
    let indexes = ids(&c, "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'index' AND name IN ('tickets_status', 'tickets_creator', 'tickets_closed', 'ticket_messages_ticket')");
    assert_eq!(indexes, [4]);
    let fk: Vec<i64> = ids(&c, "SELECT COUNT(*) FROM pragma_foreign_key_check");
    assert_eq!(fk, [0]);
    let admin = pos.login("1234").unwrap();
    let sam = pos.login_as(None, "2222", &mut Default::default()).unwrap();
    // The deleted ticket's number (only in the log now) is not used again.
    let new = pos.create_ticket(&sam, &ticket("other", "normal", "hi", None)).unwrap();
    assert!(new.id > gone, "{} after {gone}", new.id);
    assert!(new.messages[0].id > *msgs.iter().max().unwrap());
    // Deleting a ticket still takes its messages and read markers with it.
    pos.delete_ticket(&admin, kept).unwrap();
    assert_eq!(ids(&c, &format!("SELECT COUNT(*) FROM ticket_messages WHERE ticket_id = {kept}")), [0]);
    assert_eq!(ids(&c, &format!("SELECT COUNT(*) FROM ticket_reads WHERE ticket_id = {kept}")), [0]);
}
