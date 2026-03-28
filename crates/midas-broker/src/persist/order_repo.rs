//! Repository functions for the `orders`, `order_audit`, and `fills` tables.
//!
//! All functions are synchronous and take a `&rusqlite::Connection` obtained
//! from [`BrokerDb::conn()`](crate::db::BrokerDb::conn). They are designed to
//! be called inside `spawn_blocking`.

use rusqlite::{params, Connection};

// ── Row structs ──────────────────────────────────────────────────────
// Self-contained types that mirror the DB schema. They intentionally do NOT
// depend on the `orders` module so this crate can be built in parallel.

/// A row in the `orders` table.
#[derive(Debug, Clone)]
pub struct OrderRow {
    pub local_id: String,
    pub ib_order_id: Option<i32>,
    pub ib_perm_id: Option<i64>,
    pub status: String,
    pub symbol: String,
    pub sec_type: String,
    pub exchange: String,
    pub currency: String,
    pub con_id: Option<i32>,
    pub action: String,
    pub order_type: String,
    pub quantity: f64,
    pub filled_qty: f64,
    pub remaining_qty: f64,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub trail_amount: Option<f64>,
    pub trail_percent: Option<f64>,
    pub tif: String,
    pub parent_id: Option<String>,
    pub oca_group: Option<String>,
    pub bracket_role: Option<String>,
    pub strategy: Option<String>,
    pub tags: Option<String>,
    pub algo_strategy: Option<String>,
    pub algo_params: Option<String>,
    pub outside_rth: bool,
    pub avg_fill_price: Option<f64>,
    pub last_fill_price: Option<f64>,
    pub commission: Option<f64>,
    pub activation_count: i32,
    pub last_activated_at: Option<String>,
    pub last_deactivated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A row in the `fills` table.
#[derive(Debug, Clone)]
pub struct FillRow {
    pub order_local_id: String,
    pub ib_exec_id: String,
    pub timestamp: String,
    pub shares: f64,
    pub price: f64,
    pub commission: Option<f64>,
    pub exchange: Option<String>,
    pub side: String,
}

// ── Repository functions ─────────────────────────────────────────────

/// Insert a new order row. Fails if `local_id` already exists.
pub fn insert_order(conn: &Connection, order: &OrderRow) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO orders (
            local_id, ib_order_id, ib_perm_id, status, symbol, sec_type,
            exchange, currency, con_id, action, order_type, quantity,
            filled_qty, remaining_qty, limit_price, stop_price,
            trail_amount, trail_percent, tif, parent_id, oca_group,
            bracket_role, strategy, tags, algo_strategy, algo_params,
            outside_rth, good_after_time, good_till_date,
            avg_fill_price, last_fill_price, commission,
            activation_count, last_activated_at, last_deactivated_at,
            created_at, updated_at
        ) VALUES (
            ?1,  ?2,  ?3,  ?4,  ?5,  ?6,
            ?7,  ?8,  ?9,  ?10, ?11, ?12,
            ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24, ?25, ?26,
            ?27, ?28, ?29,
            ?30, ?31, ?32,
            ?33, ?34, ?35,
            ?36, ?37
        )",
        params![
            order.local_id,
            order.ib_order_id,
            order.ib_perm_id,
            order.status,
            order.symbol,
            order.sec_type,
            order.exchange,
            order.currency,
            order.con_id,
            order.action,
            order.order_type,
            order.quantity,
            order.filled_qty,
            order.remaining_qty,
            order.limit_price,
            order.stop_price,
            order.trail_amount,
            order.trail_percent,
            order.tif,
            order.parent_id,
            order.oca_group,
            order.bracket_role,
            order.strategy,
            order.tags,
            order.algo_strategy,
            order.algo_params,
            order.outside_rth as i32,
            None::<String>,  // good_after_time
            None::<String>,  // good_till_date
            order.avg_fill_price,
            order.last_fill_price,
            order.commission,
            order.activation_count,
            order.last_activated_at,
            order.last_deactivated_at,
            order.created_at,
            order.updated_at,
        ],
    )?;
    Ok(())
}

/// Update the status and `updated_at` of an order. Returns `true` if exactly
/// one row was modified, `false` if the order was not found.
pub fn update_order_status(
    conn: &Connection,
    local_id: &str,
    new_status: &str,
    updated_at: &str,
) -> Result<bool, rusqlite::Error> {
    let rows = conn.execute(
        "UPDATE orders SET status = ?1, updated_at = ?2 WHERE local_id = ?3",
        params![new_status, updated_at, local_id],
    )?;
    Ok(rows == 1)
}

/// Fetch a single order by its local UUID. Returns `None` if not found.
pub fn get_order(conn: &Connection, local_id: &str) -> Result<Option<OrderRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            local_id, ib_order_id, ib_perm_id, status, symbol, sec_type,
            exchange, currency, con_id, action, order_type, quantity,
            filled_qty, remaining_qty, limit_price, stop_price,
            trail_amount, trail_percent, tif, parent_id, oca_group,
            bracket_role, strategy, tags, algo_strategy, algo_params,
            outside_rth, avg_fill_price, last_fill_price, commission,
            activation_count, last_activated_at, last_deactivated_at,
            created_at, updated_at
        FROM orders WHERE local_id = ?1",
    )?;

    let mut rows = stmt.query_map(params![local_id], row_to_order)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Fetch all orders matching a given status string.
pub fn get_orders_by_status(
    conn: &Connection,
    status: &str,
) -> Result<Vec<OrderRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            local_id, ib_order_id, ib_perm_id, status, symbol, sec_type,
            exchange, currency, con_id, action, order_type, quantity,
            filled_qty, remaining_qty, limit_price, stop_price,
            trail_amount, trail_percent, tif, parent_id, oca_group,
            bracket_role, strategy, tags, algo_strategy, algo_params,
            outside_rth, avg_fill_price, last_fill_price, commission,
            activation_count, last_activated_at, last_deactivated_at,
            created_at, updated_at
        FROM orders WHERE status = ?1",
    )?;

    let rows = stmt.query_map(params![status], row_to_order)?;
    rows.collect()
}

/// Append an entry to the `order_audit` table.
pub fn write_audit(
    conn: &Connection,
    order_local_id: &str,
    from_status: &str,
    to_status: &str,
    details: Option<&str>,
    source: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO order_audit (order_local_id, timestamp, from_status, to_status, details, source)
         VALUES (?1, datetime('now'), ?2, ?3, ?4, ?5)",
        params![order_local_id, from_status, to_status, details, source],
    )?;
    Ok(())
}

/// Insert a fill. Uses `INSERT OR IGNORE` so that a duplicate `ib_exec_id`
/// (UNIQUE constraint) is silently skipped, making the operation idempotent.
pub fn insert_fill(conn: &Connection, fill: &FillRow) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR IGNORE INTO fills (
            order_local_id, ib_exec_id, timestamp, shares, price,
            commission, exchange, side
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            fill.order_local_id,
            fill.ib_exec_id,
            fill.timestamp,
            fill.shares,
            fill.price,
            fill.commission,
            fill.exchange,
            fill.side,
        ],
    )?;
    Ok(())
}

// ── private helpers ──────────────────────────────────────────────────

/// Map a row from a SELECT on the `orders` table into an `OrderRow`.
fn row_to_order(row: &rusqlite::Row<'_>) -> Result<OrderRow, rusqlite::Error> {
    let outside_rth_int: i32 = row.get(26)?;
    Ok(OrderRow {
        local_id: row.get(0)?,
        ib_order_id: row.get(1)?,
        ib_perm_id: row.get(2)?,
        status: row.get(3)?,
        symbol: row.get(4)?,
        sec_type: row.get(5)?,
        exchange: row.get(6)?,
        currency: row.get(7)?,
        con_id: row.get(8)?,
        action: row.get(9)?,
        order_type: row.get(10)?,
        quantity: row.get(11)?,
        filled_qty: row.get(12)?,
        remaining_qty: row.get(13)?,
        limit_price: row.get(14)?,
        stop_price: row.get(15)?,
        trail_amount: row.get(16)?,
        trail_percent: row.get(17)?,
        tif: row.get(18)?,
        parent_id: row.get(19)?,
        oca_group: row.get(20)?,
        bracket_role: row.get(21)?,
        strategy: row.get(22)?,
        tags: row.get(23)?,
        algo_strategy: row.get(24)?,
        algo_params: row.get(25)?,
        outside_rth: outside_rth_int != 0,
        avg_fill_price: row.get(27)?,
        last_fill_price: row.get(28)?,
        commission: row.get(29)?,
        activation_count: row.get(30)?,
        last_activated_at: row.get(31)?,
        last_deactivated_at: row.get(32)?,
        created_at: row.get(33)?,
        updated_at: row.get(34)?,
    })
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::BrokerDb;

    /// Helper: create a minimal `OrderRow` with required fields filled in.
    fn make_order(local_id: &str, status: &str, symbol: &str) -> OrderRow {
        OrderRow {
            local_id: local_id.to_string(),
            ib_order_id: None,
            ib_perm_id: None,
            status: status.to_string(),
            symbol: symbol.to_string(),
            sec_type: "STK".to_string(),
            exchange: "SMART".to_string(),
            currency: "USD".to_string(),
            con_id: None,
            action: "BUY".to_string(),
            order_type: "LMT".to_string(),
            quantity: 100.0,
            filled_qty: 0.0,
            remaining_qty: 100.0,
            limit_price: Some(150.0),
            stop_price: None,
            trail_amount: None,
            trail_percent: None,
            tif: "DAY".to_string(),
            parent_id: None,
            oca_group: None,
            bracket_role: None,
            strategy: None,
            tags: None,
            algo_strategy: None,
            algo_params: None,
            outside_rth: false,
            avg_fill_price: None,
            last_fill_price: None,
            commission: None,
            activation_count: 0,
            last_activated_at: None,
            last_deactivated_at: None,
            created_at: "2026-03-25T12:00:00Z".to_string(),
            updated_at: "2026-03-25T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_insert_and_get_order() {
        let db = BrokerDb::open_in_memory().unwrap();
        let conn = db.conn().lock().unwrap();

        let order = make_order("order-001", "Draft", "AAPL");
        insert_order(&conn, &order).unwrap();

        let fetched = get_order(&conn, "order-001").unwrap().expect("order should exist");

        assert_eq!(fetched.local_id, "order-001");
        assert_eq!(fetched.status, "Draft");
        assert_eq!(fetched.symbol, "AAPL");
        assert_eq!(fetched.quantity, 100.0);
        assert_eq!(fetched.limit_price, Some(150.0));
        assert_eq!(fetched.outside_rth, false);
        assert_eq!(fetched.tif, "DAY");
        assert_eq!(fetched.action, "BUY");
        assert_eq!(fetched.order_type, "LMT");
        assert_eq!(fetched.filled_qty, 0.0);
        assert_eq!(fetched.remaining_qty, 100.0);
    }

    #[test]
    fn test_get_order_not_found() {
        let db = BrokerDb::open_in_memory().unwrap();
        let conn = db.conn().lock().unwrap();

        let result = get_order(&conn, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_update_order_status() {
        let db = BrokerDb::open_in_memory().unwrap();
        let conn = db.conn().lock().unwrap();

        let order = make_order("order-002", "Draft", "MSFT");
        insert_order(&conn, &order).unwrap();

        let updated = update_order_status(&conn, "order-002", "Submitted", "2026-03-25T12:01:00Z")
            .unwrap();
        assert!(updated);

        let fetched = get_order(&conn, "order-002").unwrap().unwrap();
        assert_eq!(fetched.status, "Submitted");
        assert_eq!(fetched.updated_at, "2026-03-25T12:01:00Z");
    }

    #[test]
    fn test_update_order_status_not_found() {
        let db = BrokerDb::open_in_memory().unwrap();
        let conn = db.conn().lock().unwrap();

        let updated =
            update_order_status(&conn, "nonexistent", "Submitted", "2026-03-25T12:01:00Z")
                .unwrap();
        assert!(!updated);
    }

    #[test]
    fn test_write_audit() {
        let db = BrokerDb::open_in_memory().unwrap();
        let conn = db.conn().lock().unwrap();

        write_audit(
            &conn,
            "order-003",
            "Draft",
            "Submitted",
            Some(r#"{"reason":"user click"}"#),
            "user",
        )
        .unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT order_local_id, from_status, to_status, details, source
                 FROM order_audit WHERE order_local_id = ?1",
            )
            .unwrap();

        let row = stmt
            .query_row(params!["order-003"], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .unwrap();

        assert_eq!(row.0, "order-003");
        assert_eq!(row.1, "Draft");
        assert_eq!(row.2, "Submitted");
        assert_eq!(row.3, Some(r#"{"reason":"user click"}"#.to_string()));
        assert_eq!(row.4, "user");
    }

    #[test]
    fn test_insert_fill_idempotent() {
        let db = BrokerDb::open_in_memory().unwrap();
        let conn = db.conn().lock().unwrap();

        let fill = FillRow {
            order_local_id: "order-004".to_string(),
            ib_exec_id: "exec-abc-123".to_string(),
            timestamp: "2026-03-25T12:05:00Z".to_string(),
            shares: 50.0,
            price: 151.25,
            commission: Some(1.0),
            exchange: Some("ARCA".to_string()),
            side: "BOT".to_string(),
        };

        // First insert succeeds.
        insert_fill(&conn, &fill).unwrap();

        // Second insert with same ib_exec_id should silently do nothing.
        insert_fill(&conn, &fill).unwrap();

        // Only one row should exist.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fills WHERE ib_exec_id = ?1",
                params!["exec-abc-123"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_get_orders_by_status() {
        let db = BrokerDb::open_in_memory().unwrap();
        let conn = db.conn().lock().unwrap();

        insert_order(&conn, &make_order("o1", "Draft", "AAPL")).unwrap();
        insert_order(&conn, &make_order("o2", "Submitted", "MSFT")).unwrap();
        insert_order(&conn, &make_order("o3", "Draft", "GOOG")).unwrap();
        insert_order(&conn, &make_order("o4", "Filled", "TSLA")).unwrap();

        let drafts = get_orders_by_status(&conn, "Draft").unwrap();
        assert_eq!(drafts.len(), 2);
        let ids: Vec<&str> = drafts.iter().map(|o| o.local_id.as_str()).collect();
        assert!(ids.contains(&"o1"));
        assert!(ids.contains(&"o3"));

        let submitted = get_orders_by_status(&conn, "Submitted").unwrap();
        assert_eq!(submitted.len(), 1);
        assert_eq!(submitted[0].local_id, "o2");

        let cancelled = get_orders_by_status(&conn, "Cancelled").unwrap();
        assert!(cancelled.is_empty());
    }
}
