# Commands & Events

## BrokerCommand (17 variants)

Sent from UI to engine via `handle.commands` (mpsc).

### Connection
- `Connect` — initiate TWS/Gateway connection
- `Disconnect` — graceful disconnect
- `Reconnect` — force reconnection cycle

### Orders
- `CreateOrder(CreateOrderParams)` — create locally (not submitted to IB)
- `ActivateOrder { order_id }` — submit to IB
- `DeactivateOrder { order_id }` — pull back from IB to local staging
- `CancelOrder { order_id }` — cancel at IB
- `ModifyOrder { order_id, new_price, new_qty }` — modify existing

### Brackets
- `CreateBracketOrder { entry, take_profit_price, stop_loss_price }`

### Market Data
- `SubscribeMarketData { symbol, con_id }`
- `UnsubscribeMarketData { symbol }`
- `RequestHistoricalData { symbol, con_id, duration, bar_size, request_id }`

### Account
- `RequestPositions`
- `RequestAccountSummary`

### Recovery
- `RequestOrderSnapshot`

### System
- `Shutdown`

## CreateOrderParams

```rust
pub struct CreateOrderParams {
    pub symbol: String,
    pub con_id: Option<i32>,
    pub sec_type: SecurityType,    // enum, not string
    pub exchange: String,
    pub currency: String,
    pub action: String,            // "BUY" / "SELL"
    pub order_type: String,        // "MKT" / "LMT" / "STP" / "TRAIL"
    pub quantity: f64,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub trail_amount: Option<f64>,
    pub trail_percent: Option<f64>,
    pub tif: String,               // "DAY" / "GTC" / "IOC"
    pub outside_rth: bool,
    pub algo_strategy: Option<String>,
    pub algo_params: Option<serde_json::Value>,
    pub tag: Option<String>,
    pub strategy: Option<String>,
}
```

## BrokerEvent (22 variants)

Emitted by engine on broadcast channels.

### Connection
- `Connected { server_version }`
- `Disconnected { reason }`
- `Reconnecting { attempt, next_retry_secs }`
- `Reconnected`

### Orders (on `order_events` channel)
- `OrderCreated { order_id }`
- `OrderSubmitted { order_id, ib_order_id, ib_perm_id }`
- `OrderStatusChanged { order_id, old_status, new_status, filled_qty, remaining_qty, avg_fill_price }`
- `OrderFilled { order_id, ib_exec_id, shares, price, commission }`
- `OrderRejected { order_id, reason }`
- `OrderCancelled { order_id, reason }`
- `OrderError { order_id, code, message }`

### Market Data (on `market_events` channel)
- `Tick { symbol: SymbolKey, bid, ask, last, volume, timestamp }`
- `RealtimeBar { symbol, timestamp, open, high, low, close, volume }`
- `BarClosed { symbol, timestamp, open, high, low, close, volume }`
- `BarUpdated { symbol, timestamp, open, high, low, close, volume }`
- `HistoricalDataComplete { request_id, symbol }`
- `DepthUpdate { symbol, position, side: DepthSide, price, size }`

### Account
- `PositionUpdate { account, symbol, con_id, quantity, avg_cost }`
- `AccountValueUpdate { account, key, value, currency }`
- `PnlUpdate { daily_pnl, unrealized_pnl, realized_pnl }`

### System
- `Warning { code, message }`
- `DataFarmStatus { farm, ok }`
- `Error { code, message }`

### Recovery
- `OrderSnapshot { orders: Vec<OrderSnapshotEntry> }`
