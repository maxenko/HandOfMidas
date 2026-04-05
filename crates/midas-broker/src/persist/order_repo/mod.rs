//! Repository functions for the `orders`, `order_audit`, and `fills` tables.
//!
//! All functions are synchronous and take a `&rusqlite::Connection` obtained
//! from [`BrokerDb::conn()`](crate::db::BrokerDb::conn). They are designed to
//! be called inside `spawn_blocking`.
//!
//! **Note**: `OrderRow.bracket_role` is stored as TEXT. The `orders::types`
//! module defines a typed `BracketRole` enum with `Display`/`FromStr` impls
//! that handle the conversion. The caller is responsible for mapping between
//! `OrderRow` (String) and `LocalOrder` (typed enum).

use std::str::FromStr;

use chrono::{DateTime, Utc};
use midas_core::SecurityType;
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::orders::bracket::BracketGroup;
use crate::orders::state::OrderStatus;
use crate::orders::types::{BracketRole, LocalOrder, OrderAction, OrderKind, TimeInForce};

// ── Row structs ──────────────────────────────────────────────────────
// Types that mirror the DB schema. `bracket_role` is stored as TEXT;
// the caller converts via `BracketRole::Display`/`FromStr`.

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
            outside_rth,
            avg_fill_price, last_fill_price, commission,
            activation_count, last_activated_at, last_deactivated_at,
            created_at, updated_at
        ) VALUES (
            ?1,  ?2,  ?3,  ?4,  ?5,  ?6,
            ?7,  ?8,  ?9,  ?10, ?11, ?12,
            ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24, ?25, ?26,
            ?27,
            ?28, ?29, ?30,
            ?31, ?32, ?33,
            ?34, ?35
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

/// Update fill-tracking fields on an order row after a fill event.
/// Returns `true` if exactly one row was modified, `false` if the order was
/// not found.
pub fn update_order_fill(
    conn: &Connection,
    local_id: &str,
    filled_qty: f64,
    remaining_qty: f64,
    avg_fill_price: f64,
    last_fill_price: f64,
    commission: Option<f64>,
    updated_at: &str,
) -> Result<bool, rusqlite::Error> {
    let changed = conn.execute(
        "UPDATE orders SET filled_qty = ?1, remaining_qty = ?2, avg_fill_price = ?3, \
         last_fill_price = ?4, commission = ?5, updated_at = ?6 \
         WHERE local_id = ?7",
        params![
            filled_qty,
            remaining_qty,
            avg_fill_price,
            last_fill_price,
            commission,
            updated_at,
            local_id
        ],
    )?;
    Ok(changed > 0)
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

/// Fetch all orders whose `parent_id` matches the given local UUID.
/// Used to load bracket children for a parent order.
pub fn get_orders_by_parent_id(
    conn: &Connection,
    parent_id: &str,
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
        FROM orders WHERE parent_id = ?1",
    )?;

    let rows = stmt.query_map(params![parent_id], row_to_order)?;
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
    let outside_rth_int: i32 = row.get::<_, i32>("outside_rth")?;
    Ok(OrderRow {
        local_id: row.get::<_, String>("local_id")?,
        ib_order_id: row.get::<_, Option<i32>>("ib_order_id")?,
        ib_perm_id: row.get::<_, Option<i64>>("ib_perm_id")?,
        status: row.get::<_, String>("status")?,
        symbol: row.get::<_, String>("symbol")?,
        sec_type: row.get::<_, String>("sec_type")?,
        exchange: row.get::<_, String>("exchange")?,
        currency: row.get::<_, String>("currency")?,
        con_id: row.get::<_, Option<i32>>("con_id")?,
        action: row.get::<_, String>("action")?,
        order_type: row.get::<_, String>("order_type")?,
        quantity: row.get::<_, f64>("quantity")?,
        filled_qty: row.get::<_, f64>("filled_qty")?,
        remaining_qty: row.get::<_, f64>("remaining_qty")?,
        limit_price: row.get::<_, Option<f64>>("limit_price")?,
        stop_price: row.get::<_, Option<f64>>("stop_price")?,
        trail_amount: row.get::<_, Option<f64>>("trail_amount")?,
        trail_percent: row.get::<_, Option<f64>>("trail_percent")?,
        tif: row.get::<_, String>("tif")?,
        parent_id: row.get::<_, Option<String>>("parent_id")?,
        oca_group: row.get::<_, Option<String>>("oca_group")?,
        bracket_role: row.get::<_, Option<String>>("bracket_role")?,
        strategy: row.get::<_, Option<String>>("strategy")?,
        tags: row.get::<_, Option<String>>("tags")?,
        algo_strategy: row.get::<_, Option<String>>("algo_strategy")?,
        algo_params: row.get::<_, Option<String>>("algo_params")?,
        outside_rth: outside_rth_int != 0,
        avg_fill_price: row.get::<_, Option<f64>>("avg_fill_price")?,
        last_fill_price: row.get::<_, Option<f64>>("last_fill_price")?,
        commission: row.get::<_, Option<f64>>("commission")?,
        activation_count: row.get::<_, i32>("activation_count")?,
        last_activated_at: row.get::<_, Option<String>>("last_activated_at")?,
        last_deactivated_at: row.get::<_, Option<String>>("last_deactivated_at")?,
        created_at: row.get::<_, String>("created_at")?,
        updated_at: row.get::<_, String>("updated_at")?,
    })
}

// ── Conversion layer ────────────────────────────────────────────────

/// Error converting between [`OrderRow`] and [`LocalOrder`].
#[derive(Debug)]
pub enum ConversionError {
    /// A critical field could not be parsed.
    Field {
        field: &'static str,
        value: String,
        reason: String,
    },
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Field {
                field,
                value,
                reason,
            } => {
                write!(
                    f,
                    "conversion error on '{field}': value '{value}' — {reason}"
                )
            }
        }
    }
}

impl std::error::Error for ConversionError {}

/// Helper: parse a required field via `FromStr`, returning a `ConversionError`
/// on failure.
fn parse_required<T: FromStr>(
    field: &'static str,
    value: &str,
) -> Result<T, ConversionError>
where
    T::Err: std::fmt::Display,
{
    value.parse::<T>().map_err(|e| ConversionError::Field {
        field,
        value: value.to_string(),
        reason: e.to_string(),
    })
}

/// Convert an [`OrderRow`] (stringly-typed SQLite mirror) to a [`LocalOrder`]
/// (typed Rust enums).
///
/// # Hard failures
/// `status`, `action`, `order_type`, `tif`, `sec_type`, and `local_id` must
/// parse correctly or the function returns `Err`.
///
/// # Soft failures
/// `bracket_role`, `parent_id`, `tags`, `algo_params`, timestamps, and
/// optional datetime fields log a warning and fall back to a default.
pub fn order_row_to_local(row: &OrderRow) -> Result<LocalOrder, ConversionError> {
    // -- Hard-fail fields --
    let status: OrderStatus = parse_required("status", &row.status)?;
    let action: OrderAction = parse_required("action", &row.action)?;
    let order_type: OrderKind = parse_required("order_type", &row.order_type)?;
    let tif: TimeInForce = parse_required("tif", &row.tif)?;
    let sec_type: SecurityType = parse_required("sec_type", &row.sec_type)?;
    let local_id: Uuid = Uuid::parse_str(&row.local_id).map_err(|e| ConversionError::Field {
        field: "local_id",
        value: row.local_id.clone(),
        reason: e.to_string(),
    })?;

    // -- Soft-fail fields --
    let bracket_role: Option<BracketRole> = row.bracket_role.as_deref().and_then(|s| {
        match BracketRole::from_str(s) {
            Ok(role) => Some(role),
            Err(e) => {
                tracing::warn!(
                    "order_row_to_local: bad bracket_role '{s}': {e}, defaulting to None"
                );
                None
            }
        }
    });

    let parent_id: Option<Uuid> = row.parent_id.as_deref().and_then(|s| {
        match Uuid::parse_str(s) {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(
                    "order_row_to_local: bad parent_id '{s}': {e}, defaulting to None"
                );
                None
            }
        }
    });

    let tags: Vec<String> = row.tags.as_deref().map_or_else(Vec::new, |s| {
        match serde_json::from_str::<Vec<String>>(s) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "order_row_to_local: bad tags JSON '{s}': {e}, defaulting to empty"
                );
                Vec::new()
            }
        }
    });

    let algo_params: Option<serde_json::Value> = row.algo_params.as_deref().and_then(|s| {
        match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(
                    "order_row_to_local: bad algo_params JSON '{s}': {e}, defaulting to None"
                );
                None
            }
        }
    });

    let created_at: DateTime<Utc> = DateTime::parse_from_rfc3339(&row.created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|e| {
            tracing::warn!(
                "order_row_to_local: bad created_at '{}': {e}, defaulting to now",
                row.created_at
            );
            Utc::now()
        });

    let updated_at: DateTime<Utc> = DateTime::parse_from_rfc3339(&row.updated_at)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|e| {
            tracing::warn!(
                "order_row_to_local: bad updated_at '{}': {e}, defaulting to now",
                row.updated_at
            );
            Utc::now()
        });

    let last_activated_at: Option<DateTime<Utc>> =
        row.last_activated_at.as_deref().and_then(|s| {
            DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .inspect_err(|e| {
                    tracing::warn!(
                        "order_row_to_local: bad last_activated_at '{s}': {e}, defaulting to None"
                    );
                })
                .ok()
        });

    let last_deactivated_at: Option<DateTime<Utc>> =
        row.last_deactivated_at.as_deref().and_then(|s| {
            DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .inspect_err(|e| {
                    tracing::warn!(
                        "order_row_to_local: bad last_deactivated_at '{s}': {e}, \
                         defaulting to None"
                    );
                })
                .ok()
        });

    let mut order = LocalOrder {
        id: local_id,
        ib_order_id: row.ib_order_id,
        ib_perm_id: row.ib_perm_id,

        symbol: row.symbol.clone(),
        con_id: row.con_id,
        sec_type,
        exchange: row.exchange.clone(),
        currency: row.currency.clone(),

        action,
        order_type,
        quantity: row.quantity,
        limit_price: row.limit_price,
        stop_price: row.stop_price,
        trail_amount: row.trail_amount,
        trail_percent: row.trail_percent,
        tif,

        status,
        parent_id,
        oca_group: row.oca_group.clone(),
        bracket_role,
        strategy: row.strategy.clone(),
        tags,

        algo_strategy: row.algo_strategy.clone(),
        algo_params,

        outside_rth: row.outside_rth,

        filled_qty: row.filled_qty,
        remaining_qty: row.remaining_qty,
        avg_fill_price: row.avg_fill_price,
        last_fill_price: row.last_fill_price,
        commission: row.commission,

        activation_count: row.activation_count,
        last_activated_at,
        last_deactivated_at,

        created_at,
        updated_at,
    };

    // W13: Maintain invariant for child legs: parent_id must be present.
    // Parent legs legitimately have parent_id = None.
    if order.parent_id.is_none() {
        if let Some(ref role) = order.bracket_role {
            if *role != crate::orders::types::BracketRole::Parent {
                tracing::warn!(
                    "Clearing bracket_role {:?} for order {} — parent_id is missing",
                    role,
                    order.id
                );
                order.bracket_role = None;
            }
        }
    }

    Ok(order)
}

/// Convert a [`LocalOrder`] (typed Rust enums) to an [`OrderRow`] (stringly-typed
/// SQLite mirror). This conversion is infallible.
pub fn local_to_order_row(order: &LocalOrder) -> OrderRow {
    OrderRow {
        local_id: order.id.to_string(),
        ib_order_id: order.ib_order_id,
        ib_perm_id: order.ib_perm_id,
        status: order.status.to_string(),
        symbol: order.symbol.clone(),
        sec_type: order.sec_type.to_string(),
        exchange: order.exchange.clone(),
        currency: order.currency.clone(),
        con_id: order.con_id,
        action: order.action.to_string(),
        order_type: order.order_type.to_string(),
        quantity: order.quantity,
        filled_qty: order.filled_qty,
        remaining_qty: order.remaining_qty,
        limit_price: order.limit_price,
        stop_price: order.stop_price,
        trail_amount: order.trail_amount,
        trail_percent: order.trail_percent,
        tif: order.tif.to_string(),
        parent_id: order.parent_id.map(|id| id.to_string()),
        oca_group: order.oca_group.clone(),
        bracket_role: order.bracket_role.map(|r| r.to_string()),
        strategy: order.strategy.clone(),
        tags: if order.tags.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&order.tags).unwrap_or_else(|_| "[]".to_string()))
        },
        algo_strategy: order.algo_strategy.clone(),
        algo_params: order.algo_params.as_ref().and_then(|v| {
            serde_json::to_string(v)
                .inspect_err(|e| {
                    tracing::warn!(
                        "local_to_order_row: failed to serialize algo_params for order {}: {e}",
                        order.id
                    );
                })
                .ok()
        }),
        outside_rth: order.outside_rth,
        avg_fill_price: order.avg_fill_price,
        last_fill_price: order.last_fill_price,
        commission: order.commission,
        activation_count: order.activation_count,
        last_activated_at: order.last_activated_at.map(|dt| dt.to_rfc3339()),
        last_deactivated_at: order.last_deactivated_at.map(|dt| dt.to_rfc3339()),
        created_at: order.created_at.to_rfc3339(),
        updated_at: order.updated_at.to_rfc3339(),
    }
}

/// Persist all legs of a bracket group and transition each from `Inactive` to
/// `PendingSubmit` inside a single SQLite transaction.
///
/// After success the in-memory `group` struct is updated to reflect the new
/// statuses.
pub fn persist_and_transition_to_pending_submit(
    conn: &Connection,
    group: &mut BracketGroup,
) -> Result<(), rusqlite::Error> {
    conn.execute_batch("BEGIN")?;

    // Helper closure to persist one leg and transition its DB status without
    // mutating the in-memory order. In-memory mutation happens after COMMIT.
    let persist_leg = |conn: &Connection, order: &LocalOrder| -> Result<(), rusqlite::Error> {
        // Insert the row with its current (Inactive) status first.
        let row = local_to_order_row(order);
        insert_order(conn, &row)?;

        // Update the DB row to PendingSubmit.
        let now = Utc::now().to_rfc3339();
        update_order_status(
            conn,
            &order.id.to_string(),
            &OrderStatus::PendingSubmit.to_string(),
            &now,
        )?;

        // Record the transition in the audit log.
        write_audit(
            conn,
            &order.id.to_string(),
            &OrderStatus::Inactive.to_string(),
            &OrderStatus::PendingSubmit.to_string(),
            None,
            "persist_and_transition_to_pending_submit",
        )?;

        Ok(())
    };

    persist_leg(conn, &group.parent)?;

    if let Some(ref tp) = group.take_profit {
        persist_leg(conn, tp)?;
    }

    if let Some(ref sl) = group.stop_loss {
        persist_leg(conn, sl)?;
    }

    conn.execute_batch("COMMIT")?;

    // Only NOW update in-memory state — DB transaction succeeded.
    group.parent.status = OrderStatus::PendingSubmit;
    if let Some(ref mut tp) = group.take_profit {
        tp.status = OrderStatus::PendingSubmit;
    }
    if let Some(ref mut sl) = group.stop_loss {
        sl.status = OrderStatus::PendingSubmit;
    }

    Ok(())
}

/// Transition every non-terminal leg of a bracket group to `Error` status in
/// the database, writing an audit row with the given reason. Legs that are
/// already terminal are skipped. The entire operation runs in a transaction.
pub fn transition_bracket_to_error(
    conn: &Connection,
    group: &BracketGroup,
    reason: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute_batch("BEGIN")?;
    let now = chrono::Utc::now().to_rfc3339();
    for leg in group.legs() {
        if leg.status.is_terminal() {
            tracing::debug!(
                "Skipping terminal leg {} (status: {})",
                leg.id,
                leg.status
            );
            continue;
        }
        let id_str = leg.id.to_string();
        update_order_status(conn, &id_str, &OrderStatus::Error.to_string(), &now)?;
        write_audit(
            conn,
            &id_str,
            &leg.status.to_string(),
            &OrderStatus::Error.to_string(),
            Some(reason),
            "transition_bracket_to_error",
        )?;
    }
    conn.execute_batch("COMMIT")?;
    Ok(())
}

/// Transition every non-terminal leg of a bracket group to `Rejected` status in
/// the database, writing an audit row with the given reason. Legs that are
/// already terminal are skipped. The entire operation runs in a transaction.
pub fn transition_bracket_to_rejected(
    conn: &Connection,
    group: &BracketGroup,
    reason: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute_batch("BEGIN")?;
    let now = chrono::Utc::now().to_rfc3339();
    for leg in group.legs() {
        if leg.status.is_terminal() {
            tracing::debug!(
                "Skipping terminal leg {} (status: {})",
                leg.id,
                leg.status
            );
            continue;
        }
        let id_str = leg.id.to_string();
        update_order_status(conn, &id_str, &OrderStatus::Rejected.to_string(), &now)?;
        write_audit(
            conn,
            &id_str,
            &leg.status.to_string(),
            &OrderStatus::Rejected.to_string(),
            Some(reason),
            "transition_bracket_to_rejected",
        )?;
    }
    conn.execute_batch("COMMIT")?;
    Ok(())
}

#[cfg(test)]
mod tests;
