//! Broker client abstraction. Decouples the engine from ibapi for testing.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

/// Result of placing an order with the broker.
#[derive(Debug, Clone)]
pub struct PlaceOrderResult {
    pub ib_order_id: i32,
}

/// Result of cancelling an order.
#[derive(Debug, Clone)]
pub struct CancelOrderResult {
    pub ib_order_id: i32,
}

/// A single position as reported by the broker.
#[derive(Debug, Clone)]
pub struct PositionRecord {
    pub symbol: String,
    pub quantity: f64,
    pub avg_cost: f64,
}

/// Snapshot of account values.
#[derive(Debug, Clone, Default)]
pub struct AccountSummary {
    pub cash_balance: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
}

/// Callbacks produced by the broker client (IB status updates, fills, etc.).
/// The engine polls these and translates to BrokerEvents.
#[derive(Debug, Clone)]
pub enum BrokerCallback {
    /// Order status changed.
    OrderStatus {
        ib_order_id: i32,
        status: String,
        filled: f64,
        remaining: f64,
        avg_fill_price: f64,
    },
    /// An execution (fill) occurred.
    Execution {
        ib_order_id: i32,
        exec_id: String,
        shares: f64,
        price: f64,
        commission: f64,
        side: String,
    },
    /// Order was rejected.
    OrderRejected {
        ib_order_id: i32,
        reason: String,
    },
    /// Connection status changed.
    ConnectionStatus {
        connected: bool,
        server_version: Option<i32>,
    },
    /// Level-1 tick update (bid/ask/last/volume).
    Tick {
        symbol: String,
        con_id: i32,
        bid: Option<f64>,
        ask: Option<f64>,
        last: Option<f64>,
        volume: Option<i64>,
    },
    /// A real-time bar was updated (in-progress bar).
    BarUpdated {
        symbol: String,
        timestamp: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: i64,
    },
    /// A real-time bar has closed (complete bar).
    BarClosed {
        symbol: String,
        timestamp: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: i64,
    },
    /// Position snapshot (one per symbol).
    Position {
        symbol: String,
        quantity: f64,
        avg_cost: f64,
    },
    /// Account summary snapshot.
    Account {
        cash_balance: f64,
        unrealized_pnl: f64,
        realized_pnl: f64,
    },
}

/// Abstraction over IB API client. Allows test/null implementations.
///
/// Required methods: `next_order_id`, `place_order`, `cancel_order`, `name`.
/// Optional methods have default no-op implementations so that simple
/// test stubs (like `TestBrokerClient`) don't need to implement them.
#[allow(clippy::too_many_arguments)]
pub trait BrokerClient: Send + Sync {
    /// Get the next available order ID from the broker.
    fn next_order_id(&self) -> i32;

    /// Place an order. Returns the assigned IB order ID.
    fn place_order(
        &self,
        ib_order_id: i32,
        symbol: &str,
        action: &str,
        order_type: &str,
        quantity: f64,
        limit_price: Option<f64>,
        stop_price: Option<f64>,
        parent_id: Option<i32>,
        transmit: bool,
        tif: &str,
        outside_rth: bool,
    ) -> Result<PlaceOrderResult, String>;

    /// Cancel an order by its IB order ID.
    fn cancel_order(&self, ib_order_id: i32) -> Result<CancelOrderResult, String>;

    /// Name of this client implementation.
    fn name(&self) -> &str;

    // -- Optional methods with defaults --

    /// Connect to the broker. Returns server version.
    fn connect(&self) -> Result<i32, String> { Ok(176) }

    /// Disconnect from the broker.
    fn disconnect(&self) {}

    /// Whether the client is currently connected.
    fn is_connected(&self) -> bool { true }

    // ── Market data subscriptions ─────────────────────────────────

    /// Subscribe to streaming L1 market data for a symbol.
    fn subscribe_market_data(&self, _symbol: &str, _con_id: i32) {}

    /// Unsubscribe from streaming market data.
    fn unsubscribe_market_data(&self, _symbol: &str) {}

    // ── Account queries ───────────────────────────────────────────

    /// Request current positions. Returns a snapshot.
    fn request_positions(&self) -> Vec<PositionRecord> { Vec::new() }

    /// Request account summary (cash, P&L). Returns a snapshot.
    fn request_account_summary(&self) -> AccountSummary { AccountSummary::default() }

    // ── Polling ───────────────────────────────────────────────────

    /// Poll for pending callbacks (status changes, fills, ticks).
    fn poll_callbacks(&self) -> Vec<BrokerCallback> { Vec::new() }
}

/// A placed order record for inspection in tests.
#[derive(Debug, Clone)]
pub struct PlacedOrder {
    pub ib_order_id: i32,
    pub symbol: String,
    pub action: String,
    pub order_type: String,
    pub quantity: f64,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub parent_id: Option<i32>,
    pub transmit: bool,
    pub tif: String,
    pub outside_rth: bool,
}

/// Test broker client that accepts all orders and auto-assigns IDs.
/// Simulates instant acceptance for all order types.
pub struct TestBrokerClient {
    next_id: AtomicI32,
    /// All orders placed through this client (for test inspection).
    pub orders_placed: Arc<Mutex<Vec<PlacedOrder>>>,
    /// All orders cancelled through this client.
    pub orders_cancelled: Arc<Mutex<Vec<i32>>>,
}

impl Default for TestBrokerClient {
    fn default() -> Self {
        Self {
            next_id: AtomicI32::new(1000),
            orders_placed: Arc::new(Mutex::new(Vec::new())),
            orders_cancelled: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl TestBrokerClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a snapshot of all placed orders for assertions.
    pub fn placed_orders(&self) -> Vec<PlacedOrder> {
        self.orders_placed.lock().clone()
    }

    /// Get a snapshot of all cancelled order IDs.
    pub fn cancelled_orders(&self) -> Vec<i32> {
        self.orders_cancelled.lock().clone()
    }
}

impl BrokerClient for TestBrokerClient {
    fn next_order_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    fn place_order(
        &self,
        ib_order_id: i32,
        symbol: &str,
        action: &str,
        order_type: &str,
        quantity: f64,
        limit_price: Option<f64>,
        stop_price: Option<f64>,
        parent_id: Option<i32>,
        transmit: bool,
        tif: &str,
        outside_rth: bool,
    ) -> Result<PlaceOrderResult, String> {
        self.orders_placed.lock().push(PlacedOrder {
            ib_order_id,
            symbol: symbol.to_string(),
            action: action.to_string(),
            order_type: order_type.to_string(),
            quantity,
            limit_price,
            stop_price,
            parent_id,
            transmit,
            tif: tif.to_string(),
            outside_rth,
        });
        Ok(PlaceOrderResult { ib_order_id })
    }

    fn cancel_order(&self, ib_order_id: i32) -> Result<CancelOrderResult, String> {
        self.orders_cancelled.lock().push(ib_order_id);
        Ok(CancelOrderResult { ib_order_id })
    }

    fn name(&self) -> &str {
        "test"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_order_id_increments() {
        let client = TestBrokerClient::new();
        let id1 = client.next_order_id();
        let id2 = client.next_order_id();
        assert_eq!(id1, 1000);
        assert_eq!(id2, id1 + 1);
    }

    #[test]
    fn test_place_order_records() {
        let client = TestBrokerClient::new();
        client
            .place_order(1, "AAPL", "BUY", "MKT", 100.0, None, None, None, true, "DAY", false)
            .unwrap();
        let orders = client.placed_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].symbol, "AAPL");
        assert_eq!(orders[0].action, "BUY");
        assert_eq!(orders[0].order_type, "MKT");
        assert_eq!(orders[0].quantity, 100.0);
        assert!(orders[0].transmit);
        assert!(orders[0].parent_id.is_none());
    }

    #[test]
    fn test_cancel_order_records() {
        let client = TestBrokerClient::new();
        client.cancel_order(42).unwrap();
        assert_eq!(client.cancelled_orders(), vec![42]);
    }

    #[test]
    fn test_bracket_transmit_flags() {
        let client = TestBrokerClient::new();
        let parent_id = client.next_order_id();
        // Parent: transmit=false
        client
            .place_order(
                parent_id, "AAPL", "BUY", "MKT", 100.0, None, None, None, false, "DAY", false,
            )
            .unwrap();
        // TP: transmit=false
        let tp_id = client.next_order_id();
        client
            .place_order(
                tp_id,
                "AAPL",
                "SELL",
                "LMT",
                100.0,
                Some(192.0),
                None,
                Some(parent_id),
                false,
                "GTC",
                false,
            )
            .unwrap();
        // SL: transmit=true (triggers all)
        let sl_id = client.next_order_id();
        client
            .place_order(
                sl_id,
                "AAPL",
                "SELL",
                "STP",
                100.0,
                None,
                Some(182.0),
                Some(parent_id),
                true,
                "GTC",
                false,
            )
            .unwrap();

        let orders = client.placed_orders();
        assert_eq!(orders.len(), 3);
        assert!(!orders[0].transmit); // parent
        assert!(!orders[1].transmit); // TP
        assert!(orders[2].transmit); // SL (last child)
        assert_eq!(orders[1].parent_id, Some(parent_id));
        assert_eq!(orders[2].parent_id, Some(parent_id));
    }

    #[test]
    fn test_client_name() {
        let client = TestBrokerClient::new();
        assert_eq!(client.name(), "test");
    }

    #[test]
    fn test_multiple_place_and_cancel() {
        let client = TestBrokerClient::new();
        let id1 = client.next_order_id();
        let id2 = client.next_order_id();
        client
            .place_order(id1, "AAPL", "BUY", "MKT", 50.0, None, None, None, true, "DAY", false)
            .unwrap();
        client
            .place_order(id2, "MSFT", "SELL", "LMT", 25.0, Some(400.0), None, None, true, "DAY", false)
            .unwrap();
        client.cancel_order(id1).unwrap();

        assert_eq!(client.placed_orders().len(), 2);
        assert_eq!(client.cancelled_orders(), vec![id1]);
    }
}
