# Persistence Layer

## BrokerDb

```rust
pub struct BrokerDb {
    conn: Arc<Mutex<rusqlite::Connection>>,
}
```

- `open(path)` — file-based, applies pragmas, runs migrations
- `open_in_memory()` — for tests
- `conn()` — returns `Arc<Mutex<Connection>>`

### SQLite Pragmas

```sql
PRAGMA journal_mode = WAL;        -- concurrent readers
PRAGMA synchronous = NORMAL;      -- safe with WAL, faster
PRAGMA cache_size = -8000;        -- ~8 MB page cache
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;       -- 5s lock wait
```

### Default Path

- Windows: `%LOCALAPPDATA%\midas\broker.db`
- Linux/macOS: `~/.local/share/midas/broker.db`

## Schema (6 tables)

Defined in `migrations/001_initial.sql`.

| Table | Purpose | Key Indexes |
|---|---|---|
| `orders` | Full order lifecycle (31 columns) | status, symbol, ib_order_id, ib_perm_id, parent_id |
| `order_audit` | Append-only status transition log | order_local_id |
| `fills` | Execution reports (one per partial fill) | order_local_id, UNIQUE(ib_exec_id) |
| `positions` | Current position cache | PRIMARY KEY(account, con_id) |
| `account_values` | Account summary (cash, margin, NLV) | UNIQUE(account, tag, currency) |
| `contracts` | Contract qualification cache | symbol |

## Repository Functions (`persist/order_repo.rs`)

All synchronous, designed for `spawn_blocking`.

| Function | Purpose |
|---|---|
| `insert_order(conn, order)` | 37-column INSERT |
| `update_order_status(conn, id, status, updated_at)` | Atomic status + timestamp |
| `get_order(conn, id)` | Fetch by local UUID |
| `get_orders_by_status(conn, status)` | Filter by status |
| `write_audit(conn, id, from, to, details, source)` | Append audit trail |
| `insert_fill(conn, fill)` | Idempotent via INSERT OR IGNORE on ib_exec_id |

## Write Policy (Two-Tier)

- **Critical** (awaited): orders, fills, audit
- **Non-critical** (fire-and-forget): positions, account values — refreshed on reconnect
