-- 001_initial.sql
-- Canonical schema for the midas-broker persistence layer.
-- Tables: orders, order_audit, fills, positions, account_values, contracts

-- orders table
CREATE TABLE orders (
    local_id        TEXT    NOT NULL PRIMARY KEY,  -- UUIDv7
    ib_order_id     INTEGER,
    ib_perm_id      INTEGER,
    status          TEXT    NOT NULL DEFAULT 'Draft',
    symbol          TEXT    NOT NULL,
    sec_type        TEXT    NOT NULL DEFAULT 'STK',
    exchange        TEXT    NOT NULL DEFAULT 'SMART',
    currency        TEXT    NOT NULL DEFAULT 'USD',
    con_id          INTEGER,
    action          TEXT    NOT NULL,
    order_type      TEXT    NOT NULL,
    quantity        REAL    NOT NULL,
    filled_qty      REAL    NOT NULL DEFAULT 0.0,
    remaining_qty   REAL    NOT NULL,
    limit_price     REAL,
    stop_price      REAL,
    trail_amount    REAL,
    trail_percent   REAL,
    tif             TEXT    NOT NULL DEFAULT 'DAY',
    parent_id       TEXT,
    oca_group       TEXT,
    bracket_role    TEXT,
    strategy        TEXT,
    tags            TEXT,  -- JSON array
    algo_strategy   TEXT,
    algo_params     TEXT,  -- JSON
    outside_rth     INTEGER NOT NULL DEFAULT 0,
    good_after_time TEXT,
    good_till_date  TEXT,
    avg_fill_price  REAL,
    last_fill_price REAL,
    commission      REAL,
    activation_count INTEGER NOT NULL DEFAULT 0,
    last_activated_at TEXT,
    last_deactivated_at TEXT,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL
);

-- indexes for orders
CREATE INDEX idx_orders_status ON orders(status);
CREATE INDEX idx_orders_symbol ON orders(symbol);
CREATE INDEX idx_orders_ib_order_id ON orders(ib_order_id);
CREATE INDEX idx_orders_ib_perm_id ON orders(ib_perm_id);
CREATE INDEX idx_orders_parent ON orders(parent_id);

-- order_audit (append-only audit trail)
CREATE TABLE order_audit (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    order_local_id  TEXT    NOT NULL,
    timestamp       TEXT    NOT NULL,
    from_status     TEXT    NOT NULL,
    to_status       TEXT    NOT NULL,
    details         TEXT,  -- JSON
    source          TEXT    NOT NULL DEFAULT 'engine'  -- 'engine', 'ib', 'user', 'system'
);
CREATE INDEX idx_audit_order ON order_audit(order_local_id);

-- fills
CREATE TABLE fills (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    order_local_id  TEXT    NOT NULL,
    ib_exec_id      TEXT    NOT NULL UNIQUE,
    timestamp       TEXT    NOT NULL,
    shares          REAL    NOT NULL,
    price           REAL    NOT NULL,
    commission      REAL,
    exchange        TEXT,
    side            TEXT    NOT NULL
);
CREATE INDEX idx_fills_order ON fills(order_local_id);

-- positions
CREATE TABLE positions (
    account         TEXT    NOT NULL,
    con_id          INTEGER NOT NULL,
    symbol          TEXT    NOT NULL,
    sec_type        TEXT    NOT NULL DEFAULT 'STK',
    exchange        TEXT    NOT NULL,
    currency        TEXT    NOT NULL DEFAULT 'USD',
    quantity        REAL    NOT NULL,
    avg_cost        REAL    NOT NULL,
    market_value    REAL,
    unrealized_pnl  REAL,
    realized_pnl    REAL,
    updated_at      TEXT    NOT NULL,
    PRIMARY KEY (account, con_id)
);

-- account_values
CREATE TABLE account_values (
    account         TEXT    NOT NULL,
    tag             TEXT    NOT NULL,
    value           TEXT    NOT NULL,
    currency        TEXT    NOT NULL DEFAULT 'USD',
    updated_at      TEXT    NOT NULL,
    UNIQUE(account, tag, currency)
);

-- contracts cache
CREATE TABLE contracts (
    con_id          INTEGER PRIMARY KEY,
    symbol          TEXT    NOT NULL,
    sec_type        TEXT    NOT NULL,
    exchange        TEXT    NOT NULL,
    primary_exchange TEXT,
    currency        TEXT    NOT NULL,
    local_symbol    TEXT,
    multiplier      TEXT,
    last_trade_date TEXT,
    strike          REAL,
    right_          TEXT,
    details_json    TEXT,
    cached_at       TEXT    NOT NULL
);
CREATE INDEX idx_contracts_symbol ON contracts(symbol);
