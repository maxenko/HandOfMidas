# Interactive Brokers API — Complete Reference for Programmatic Trading

> Research compiled March 2026. Verify pricing and features at interactivebrokers.com before relying on specific numbers.

---

## Table of Contents

- [Account & Prerequisites](#account--prerequisites)
- [API Architecture Overview](#api-architecture-overview)
- [Python Ecosystem](#python-ecosystem)
- [Connection & Setup](#connection--setup)
- [Contract Specification](#contract-specification)
- [Order Placement](#order-placement)
- [Order Modification](#order-modification)
- [Order Cancellation](#order-cancellation)
- [Order Activation & Deactivation](#order-activation--deactivation)
- [Order Status & Monitoring](#order-status--monitoring)
- [Bracket Orders](#bracket-orders)
- [OCA (One-Cancels-All) Groups](#oca-one-cancels-all-groups)
- [Conditional Orders](#conditional-orders)
- [IB Algo Orders](#ib-algo-orders)
- [Client Portal REST API](#client-portal-rest-api)
- [Market Data](#market-data)
- [Historical Data](#historical-data)
- [Scanners & Screening](#scanners--screening)
- [Portfolio & Account Data](#portfolio--account-data)
- [Data Caching Strategies](#data-caching-strategies)
- [Commissions](#commissions)
- [Rate Limits & Pacing Rules](#rate-limits--pacing-rules)
- [Practical Gotchas](#practical-gotchas)
- [Production Deployment](#production-deployment)
- [Error Handling Reference](#error-handling-reference)

---

## Account & Prerequisites

### IBKR Lite vs Pro

**IBKR Lite does NOT support API access.** This is a hard restriction — no TWS API, no Client Portal API, no Web API. You must use **IBKR Pro**.

| Feature | IBKR Lite | IBKR Pro |
|---|---|---|
| API access | No | Yes (TWS, Client Portal, Web API) |
| US stock commissions | $0 (PFOF) | $0.005/share fixed or tiered |
| SmartRouting | No (PFOF routing) | Yes (best execution) |
| Advanced order types | No | Yes (all) |

Upgrading from Lite to Pro takes effect after the next daily server reset.

### Account Types

All account types (Individual, Joint, IRA, Trust, LLC) support API access under IBKR Pro. IRA accounts have trading restrictions (no margin, no shorting) but the API interface is identical — violating orders get rejected server-side.

### 2FA and API Access

IB requires 2FA for all logins, which complicates headless API operation.

**Solution — Secondary User:** Create a second username in Account Management that does NOT require 2FA. Use the primary (2FA-protected) username for web/portal access; use the secondary username solely for TWS/Gateway API connections.

**Caveat:** Each username needs its own market data subscriptions (you pay for data twice). Whitelist the secondary user's IP in Settings > Security > IP Restrictions for compensating security.

### API Permissions to Enable

In TWS: **File > Global Configuration > API > Settings** — enable "Enable ActiveX and Socket Clients." IB Gateway accepts socket connections by default.

---

## API Architecture Overview

### Two APIs, One Protocol

IBKR offers two primary API surfaces:

| Aspect | TWS API (Socket) | Client Portal API (REST) |
|---|---|---|
| Protocol | TCP socket (binary) | REST HTTP + WebSocket |
| Languages | Python, Java, C++, C# | Any (HTTP-based) |
| Infrastructure | TWS or IB Gateway | Client Portal Gateway (Java app) |
| Feature completeness | Most complete | Growing, some gaps |
| Best for | Full-featured trading systems | Web apps, lightweight integrations |
| Account type | All IBKR Pro | IBKR Pro only |

**You cannot run both with the same username simultaneously.** An IB username can only have one brokerage session open. Workaround: create a second username.

### TWS vs IB Gateway

| Feature | TWS (Trader Workstation) | IB Gateway |
|---|---|---|
| GUI | Full trading GUI with charts | Minimal — no trading GUI |
| Resource usage | ~2+ GB RAM | ~512 MB RAM (40% less) |
| Default API ports | 7496 (live), 7497 (paper) | 4001 (live), 4002 (paper) |
| Purpose | Desktop trading + API | Dedicated API gateway |
| Headless operation | Possible but not ideal | Designed for it (with IBC) |
| Max API connections | 32 (clientId 0-31) | 32 (clientId 0-31) |

**For production, always use IB Gateway.** For development, TWS is useful for visual confirmation.

From the API's perspective, they are functionally identical. Equivalent versions are always released together.

---

## Python Ecosystem

### Recommended: `ib_async` (successor to `ib_insync`)

The original `ib_insync` was created by Ewald de Wit in 2017. After his passing in early 2024, the project was forked as **`ib_async`** under [ib-api-reloaded](https://github.com/ib-api-reloaded/ib_async), maintained by Matt Stancliff.

```bash
pip install ib_async
```

- Implements the full IBKR API binary protocol internally — does NOT require the official `ibapi` package
- Built on asyncio with both sync and async interfaces
- Automatic state synchronization (orders, positions, tickers kept in sync)
- Automatic order ID management
- First-class Jupyter notebook support via `util.startLoop()`
- Python 3.10+

### Official: `ibapi`

```bash
pip install ibapi
```

The official IB package using EClient/EWrapper callback pattern. More verbose, requires manual threading and state management. Most developers prefer `ib_async`.

| Aspect | `ibapi` (official) | `ib_async` |
|---|---|---|
| API style | Callback-driven, verbose | Pythonic, concise |
| Threading | Manual (background thread for `app.run()`) | Single-threaded asyncio |
| Order IDs | Manual tracking of `nextValidId` | Automatic |
| State sync | Manual via callbacks | Auto-sync (Trade, Ticker objects) |
| Code for basic order | ~50+ lines | ~10 lines |
| Pandas support | None | Built-in `util.df()` |

### Key Classes (ib_async)

| Class | Purpose |
|---|---|
| `IB` | Central class — connection, requests, events |
| `Stock` | Equity contract: `Stock('AAPL', 'SMART', 'USD')` |
| `Option` | Options contract |
| `Future` | Futures contract |
| `Forex` | Forex pair: `Forex('EURUSD')` |
| `MarketOrder` | Market order |
| `LimitOrder` | Limit order with price |
| `StopOrder` | Stop order |
| `StopLimitOrder` | Stop-limit order |
| `Trade` | Live-updated placed order object |
| `Ticker` | Live-updated market data container |

### Other Frameworks with IBKR Integration

| Framework | Notes |
|---|---|
| **Backtrader** | Built-in IB broker/data feed. Mature, widely used. Not actively developed but stable. |
| **NautilusTrader** | Native IB adapter. High-performance Rust core. Modern, production-grade. |
| **QuantConnect/LEAN** | IB brokerage integration. Cloud-based with open-source engine. |
| **Zipline** | Via zipline-live fork. Community-maintained. |

---

## Connection & Setup

### Basic Connection (ib_async)

```python
from ib_async import *

ib = IB()
ib.connect('127.0.0.1', 4002, clientId=1)  # Gateway paper trading
# 4001 = Gateway live, 7496 = TWS live, 7497 = TWS paper

print(ib.managedAccounts())
ib.disconnect()
```

### Async Connection

```python
import asyncio
from ib_async import *

async def main():
    ib = IB()
    await ib.connectAsync('127.0.0.1', 4002, clientId=1)
    print(ib.managedAccounts())
    ib.disconnect()

asyncio.run(main())
```

### Connection (official ibapi)

```python
from ibapi.client import EClient
from ibapi.wrapper import EWrapper
import threading

class IBApp(EWrapper, EClient):
    def __init__(self):
        EClient.__init__(self, self)
        self.nextOrderId = None

    def nextValidId(self, orderId: int):
        self.nextOrderId = orderId  # Connection ready

app = IBApp()
app.connect('127.0.0.1', 4002, clientId=1)
api_thread = threading.Thread(target=app.run, daemon=True)
api_thread.start()
```

### Port Reference

| Platform | Live | Paper |
|---|---|---|
| TWS | 7496 | 7497 |
| IB Gateway | 4001 | 4002 |

### Client IDs

- Each API connection needs a unique `clientId` (0-31)
- **clientId 0 is special** — receives order updates for manually-placed TWS orders too
- Max 32 simultaneous connections per TWS/Gateway instance
- Best practice: assign dedicated clientIds per module (e.g., 1=data, 2=orders, 3=historical)

---

## Contract Specification

### Stocks

```python
# ib_async shorthand
contract = Stock('AAPL', 'SMART', 'USD')

# Or explicit (works with both ibapi and ib_async)
contract = Contract()
contract.symbol = "AAPL"
contract.secType = "STK"
contract.exchange = "SMART"       # IB SmartRouting
contract.currency = "USD"
contract.primaryExchange = "NASDAQ"  # Disambiguate if needed
```

### Options

```python
contract = Option('AAPL', '20260320', 150.0, 'C', 'SMART')
# Or:
contract = Contract()
contract.symbol = "AAPL"
contract.secType = "OPT"
contract.exchange = "SMART"
contract.currency = "USD"
contract.lastTradeDateOrExpiry = "20260320"  # YYYYMMDD
contract.strike = 150.0
contract.right = "C"           # "C" = Call, "P" = Put
contract.multiplier = "100"
```

### Futures

```python
contract = Future('ES', '202606', 'CME')
# Or:
contract = Contract()
contract.symbol = "ES"
contract.secType = "FUT"
contract.exchange = "CME"
contract.currency = "USD"
contract.lastTradeDateOrExpiry = "202606"  # YYYYMM
```

### Forex

```python
contract = Forex('EURUSD')
# Or:
contract = Contract()
contract.symbol = "EUR"
contract.secType = "CASH"
contract.exchange = "IDEALPRO"
contract.currency = "USD"
```

### Contract Qualification

**Always qualify contracts before use** — this resolves ambiguities and fills in the `conId`:

```python
# ib_async
ib.qualifyContracts(contract)

# ibapi
app.reqContractDetails(reqId, contract)
# Results arrive via contractDetails() callback
```

### Options Chain Discovery

Use `reqSecDefOptParams` (not `reqContractDetails`) for options chains:

```python
# ibapi
app.reqSecDefOptParams(reqId, "AAPL", "", "STK", conId)
# Returns valid expiries and strikes via securityDefinitionOptionParameter callback
```

---

## Order Placement

### ib_async (Recommended)

```python
from ib_async import *

ib = IB()
ib.connect('127.0.0.1', 4002, clientId=1)

contract = Stock('AAPL', 'SMART', 'USD')
ib.qualifyContracts(contract)

# Market order
trade = ib.placeOrder(contract, MarketOrder('BUY', 100))

# Limit order
trade = ib.placeOrder(contract, LimitOrder('BUY', 100, 150.00))

# Stop order
trade = ib.placeOrder(contract, StopOrder('SELL', 100, 145.00))

# Wait for fill
while not trade.isDone():
    ib.waitOnUpdate()
print(f"Filled at {trade.orderStatus.avgFillPrice}")
```

### ibapi (Official)

```python
from ibapi.order import Order

# Market order
order = Order()
order.action = "BUY"
order.orderType = "MKT"
order.totalQuantity = 100
app.placeOrder(app.nextOrderId, contract, order)
app.nextOrderId += 1

# Limit order
order = Order()
order.action = "BUY"
order.orderType = "LMT"
order.totalQuantity = 100
order.lmtPrice = 150.00
app.placeOrder(app.nextOrderId, contract, order)
app.nextOrderId += 1

# Stop order
order = Order()
order.action = "SELL"
order.orderType = "STP"
order.totalQuantity = 100
order.auxPrice = 145.00
app.placeOrder(app.nextOrderId, contract, order)
app.nextOrderId += 1
```

### All Supported Order Types

**Basic:**
| `orderType` | Description |
|---|---|
| `MKT` | Market |
| `LMT` | Limit |
| `STP` | Stop |
| `STP LMT` | Stop Limit |
| `MIT` | Market-if-Touched |
| `LIT` | Limit-if-Touched |
| `MTL` | Market-to-Limit |
| `MOC` | Market-on-Close |
| `LOC` | Limit-on-Close |

**Trailing:**
| `orderType` | Description |
|---|---|
| `TRAIL` | Trailing Stop |
| `TRAIL LIMIT` | Trailing Stop Limit |

**Pegged:**
| `orderType` | Description |
|---|---|
| `REL` | Relative / Pegged-to-Primary |
| `PEG MKT` | Pegged-to-Market |
| `PEG MID` | Pegged-to-Midpoint |
| `PEG BENCH` | Pegged-to-Benchmark |
| `PASSV REL` | Passive Relative |

**Other:**
| `orderType` | Description |
|---|---|
| `VOL` | Volatility |
| `VWAP` | Volume-Weighted Average Price |
| `STP PRT` | Stop with Protection |
| `MKT PRT` | Market with Protection |

### Time-in-Force (TIF)

| TIF | Description |
|---|---|
| `DAY` | Valid for trading day only (default) |
| `GTC` | Good-Til-Canceled |
| `IOC` | Immediate-or-Cancel |
| `FOK` | Fill-or-Kill |
| `OPG` | At-the-Open (opening auction) |
| `GTD` | Good-Til-Date (set `order.goodTillDate`) |

```python
order = LimitOrder('BUY', 100, 150.00)
order.tif = 'GTC'
# For GTD:
order.tif = 'GTD'
order.goodTillDate = '20260401 16:00:00 US/Eastern'
```

### Extended Hours Trading

```python
order.outsideRth = True   # Allow fills in pre-market / after-hours
```

---

## Order Modification

### How It Works

Call `placeOrder()` with the **same orderId** as the existing order, with updated fields.

**ib_async:**
```python
# Place
order = LimitOrder('BUY', 100, 145.00)
trade = ib.placeOrder(contract, order)

# Modify price — reuse same order object
order.lmtPrice = 147.00
trade = ib.placeOrder(contract, order)  # Same orderId → modification
```

**ibapi:**
```python
# Modify — same orderId
order.lmtPrice = 151.00
app.placeOrder(100, contract, order)  # orderId 100 = modification, not new order
```

### Rules

- `orderId` must match the original — modifications do NOT get a new ID
- The API client must use the **same clientId** that placed the original
- IB recommends limiting modifications to: **price**, **quantity**, **TIF**
- For other changes, cancel and place a new order

---

## Order Cancellation

### Cancel Single Order

```python
# ib_async
ib.cancelOrder(order)
# or by trade object:
ib.cancelOrder(trade.order)

# ibapi
app.cancelOrder(orderId, "")
```

### Cancel All Orders

```python
# ib_async
ib.reqGlobalCancel()

# ibapi
app.reqGlobalCancel()
```

Cancels ALL open orders regardless of which client placed them, including manual TWS orders.

### Cancel by Symbol (Not Built-In)

No direct API method exists. You must:
1. Get open orders: `ib.openTrades()` or `app.reqOpenOrders()`
2. Filter by symbol
3. Cancel each matching order individually

---

## Order Activation & Deactivation

### The `transmit` Flag — Place Without Activating

Setting `transmit = False` sends the order to TWS but does **not** transmit it to IB's servers:

```python
# ibapi
order.transmit = False
app.placeOrder(orderId, contract, order)  # Held in TWS, not live

# Later, activate it:
order.transmit = True
app.placeOrder(orderId, contract, order)  # Now transmitted
```

**Caveats:** Untransmitted orders exist only in the TWS session and are cleared on restart.

### Deactivating a Live Order

There is **no direct "deactivate" API function** equivalent to TWS's right-click > Deactivate. Options for similar behavior:

1. **Cancel and re-place later** — simplest approach
2. **Use `activeStartTime` / `activeStopTime`** (GTC orders only):
   ```python
   order.tif = "GTC"
   order.activeStartTime = "20260325 09:30:00 US/Eastern"
   order.activeStopTime = "20260325 16:00:00 US/Eastern"
   ```
3. **Local caching workaround** — cancel the order, store its parameters locally, re-place when needed. This is the recommended approach for implementing "deactivate/reactivate" in your application.

### GoodAfterTime — Delayed Activation

```python
order.goodAfterTime = "20260325 09:30:00 US/Eastern"
# Order activates at the specified time
```

### GoodTillDate — Auto-Expiration

```python
order.tif = "GTD"
order.goodTillDate = "20260401 16:00:00 US/Eastern"
# Order expires at the specified time
```

---

## Order Status & Monitoring

### Order States

| State | Description |
|---|---|
| `ApiPending` | Not yet sent to IB server |
| `PendingSubmit` | Sent from TWS, awaiting destination confirmation |
| `PreSubmitted` | Simulated order accepted, not yet triggered |
| `Submitted` | Working at the exchange |
| `Filled` | Completely filled |
| `PendingCancel` | Cancel request sent, not yet confirmed |
| `Cancelled` | Confirmed cancelled |
| `Inactive` | Not working (invalid, error, or conditions not met) |
| `ApiCancelled` | Cancelled via API |

### Event Handling (ib_async)

```python
def on_order_status(trade):
    print(f"{trade.contract.symbol}: {trade.orderStatus.status}")
    print(f"  Filled: {trade.orderStatus.filled}/{trade.order.totalQuantity}")
    print(f"  Avg price: {trade.orderStatus.avgFillPrice}")

def on_exec_details(trade, fill):
    print(f"FILL: {fill.execution.shares} @ {fill.execution.price}")

ib.orderStatusEvent += on_order_status
ib.execDetailsEvent += on_exec_details
ib.newOrderEvent += lambda trade: print(f"NEW: {trade.order.orderId}")
ib.orderModifyEvent += lambda trade: print(f"MODIFIED: {trade.order.orderId}")
ib.cancelOrderEvent += lambda trade: print(f"CANCEL: {trade.order.orderId}")
```

### Checking Order Status

```python
# ib_async — Trade object is live-updated
print(trade.orderStatus.status)
print(trade.orderStatus.filled)
print(trade.orderStatus.remaining)
print(trade.orderStatus.avgFillPrice)
print(trade.fills)    # List of Fill objects
print(trade.log)      # List of TradeLogEntry with timestamps

# All open trades
for t in ib.openTrades():
    print(f"{t.contract.symbol}: {t.order.action} {t.order.totalQuantity} - {t.orderStatus.status}")
```

### Callbacks (ibapi)

```python
def orderStatus(self, orderId, status, filled, remaining,
                avgFillPrice, permId, parentId, lastFillPrice,
                clientId, whyHeld, mktCapPrice):
    print(f"Order {orderId}: {status}, filled={filled}")

def openOrder(self, orderId, contract, order, orderState):
    print(f"Open: {orderId} {contract.symbol} {order.action}")

def execDetails(self, reqId, contract, execution):
    print(f"Fill: {execution.shares} @ {execution.price}")

def commissionReport(self, commissionReport):
    print(f"Commission: {commissionReport.commission}")
```

**Important:** `orderStatus` callbacks are NOT guaranteed for every state change (fast fills may skip intermediate states). Always monitor `execDetails` for reliable fill detection.

---

## Bracket Orders

A bracket order = parent entry + take-profit limit + stop-loss, linked together.

### ib_async

```python
bracket = ib.bracketOrder('BUY', 100, limitPrice=150, takeProfitPrice=160, stopLossPrice=140)
for o in bracket:
    ib.placeOrder(contract, o)
```

### ibapi — Manual Construction

```python
def bracket_order(parentId, action, qty, entryPrice, tpPrice, slPrice):
    # PARENT
    parent = Order()
    parent.orderId = parentId
    parent.action = action
    parent.orderType = "LMT"
    parent.totalQuantity = qty
    parent.lmtPrice = entryPrice
    parent.transmit = False          # Hold — don't transmit yet

    # TAKE PROFIT
    tp = Order()
    tp.orderId = parentId + 1
    tp.action = "SELL" if action == "BUY" else "BUY"
    tp.orderType = "LMT"
    tp.totalQuantity = qty
    tp.lmtPrice = tpPrice
    tp.parentId = parentId           # Link to parent
    tp.transmit = False              # Hold

    # STOP LOSS
    sl = Order()
    sl.orderId = parentId + 2
    sl.action = "SELL" if action == "BUY" else "BUY"
    sl.orderType = "STP"
    sl.totalQuantity = qty
    sl.auxPrice = slPrice
    sl.parentId = parentId           # Link to parent
    sl.transmit = True               # Transmit ALL — triggers entire bracket

    return [parent, tp, sl]

for o in bracket_order(app.nextOrderId, "BUY", 100, 150.0, 160.0, 140.0):
    app.placeOrder(o.orderId, contract, o)
```

**How it works:**
- `parentId` links children to the parent
- `transmit = False` holds orders locally until the last child sets `transmit = True`
- Children activate only after the parent fills
- The two children form an implicit OCA group — when one fills, the other cancels

**Modifying legs:** Call `placeOrder()` with the same `orderId` of the leg you want to change. Other legs remain unchanged.

---

## OCA (One-Cancels-All) Groups

OCA groups link unrelated orders so that when one fills, the others are handled automatically.

```python
# All orders share the same ocaGroup string
order1 = LimitOrder('BUY', 100, 150.0)
order1.ocaGroup = "MyOCA1"
order1.ocaType = 1    # CancelWithBlock
order1.transmit = False

order2 = LimitOrder('BUY', 200, 145.0)
order2.ocaGroup = "MyOCA1"
order2.ocaType = 1
order2.transmit = False

order3 = LimitOrder('BUY', 150, 148.0)
order3.ocaGroup = "MyOCA1"
order3.ocaType = 1
order3.transmit = True  # Transmit entire group

ib.placeOrder(contract1, order1)
ib.placeOrder(contract2, order2)
ib.placeOrder(contract3, order3)
```

### OCA Types

| `ocaType` | Name | Behavior |
|---|---|---|
| 1 | CancelWithBlock | One fills → all others cancelled. One routed at a time (overfill protection). |
| 2 | ReduceWithBlock | One fills → others' quantities reduced. One routed at a time. |
| 3 | ReduceWithoutBlock | Same as 2 but multiple can be live simultaneously (overfill risk). |

### ib_async Shorthand

```python
orders = [order1, order2, order3]
Order.oneCancelsAll(orders, ocaGroup="MyOCA1", ocaType=1)
```

---

## Conditional Orders

Conditions control when an order gets submitted or cancelled.

### Condition Types

| Type | Triggers On |
|---|---|
| `Price` | Another instrument hits a price level |
| `Time` | Specified date/time reached |
| `Volume` | Trading volume exceeds threshold |
| `Margin` | Account margin cushion hits threshold |
| `PercentChange` | Price change percentage exceeded |
| `Execution` | A trade occurs on a specific product |

### Price Condition Example

```python
from ibapi.order_condition import Create, OrderCondition

cond = Create(OrderCondition.Price)
cond.conId = 265598               # conId of reference instrument (e.g., SPY)
cond.exchange = "SMART"
cond.isMore = True                # Trigger when price > threshold
cond.triggerMethod = cond.TriggerMethodEnum.Last
cond.price = 450.0
cond.isConjunctionConnection = True  # AND with next condition (False = OR)

order.conditions.append(cond)
```

### Time Condition Example

```python
cond = Create(OrderCondition.Time)
cond.isMore = True                # After this time
cond.time = "20260324 09:30:00 US/Eastern"
order.conditions.append(cond)
```

### Volume Condition Example

```python
cond = Create(OrderCondition.Volume)
cond.conId = 265598
cond.exchange = "SMART"
cond.isMore = True
cond.volume = 1000000             # Volume threshold
order.conditions.append(cond)
```

### Combining Conditions

- `isConjunctionConnection = True` → AND with next condition
- `isConjunctionConnection = False` → OR with next condition
- `order.conditionsCancelOrder = True` → Cancel (instead of submit) when conditions met
- `order.conditionsIgnoreRth = True` → Evaluate conditions outside regular hours

---

## IB Algo Orders

Submit via `algoStrategy` and `algoParams` fields on the Order object.

### Adaptive (Most Common)

```python
order = LimitOrder('BUY', 1000, 50.00)
order.algoStrategy = "Adaptive"
order.algoParams = [TagValue("adaptivePriority", "Patient")]
# Priority: "Urgent" | "Normal" | "Patient"
```

### VWAP

```python
order.algoStrategy = "Vwap"
order.algoParams = [
    TagValue("maxPctVol", "0.2"),
    TagValue("startTime", "09:30:00 US/Eastern"),
    TagValue("endTime", "16:00:00 US/Eastern"),
    TagValue("allowPastEndTime", "1"),
    TagValue("noTakeLiq", "0"),
    TagValue("speedUp", "1"),
]
```

### TWAP

```python
order.algoStrategy = "Twap"
order.algoParams = [
    TagValue("strategyType", "Marketable"),  # or "Matching"
    TagValue("startTime", "09:30:00 US/Eastern"),
    TagValue("endTime", "16:00:00 US/Eastern"),
    TagValue("allowPastEndTime", "1"),
]
```

### All Available IB Algos

| Strategy | `algoStrategy` | Key Parameters |
|---|---|---|
| **Adaptive** | `"Adaptive"` | `adaptivePriority` (Urgent/Normal/Patient) |
| **VWAP** | `"Vwap"` | `maxPctVol`, `startTime`, `endTime`, `noTakeLiq`, `speedUp` |
| **TWAP** | `"Twap"` | `strategyType` (Marketable/Matching), `startTime`, `endTime` |
| **Arrival Price** | `"ArrivalPx"` | `maxPctVol`, `riskAversion` (Get Done/Aggressive/Neutral/Passive) |
| **Close Price** | `"ClosePx"` | `maxPctVol`, `riskAversion`, `forceCompletion` |
| **Dark Ice** | `"DarkIce"` | `displaySize`, `startTime`, `endTime` |
| **Pct of Volume** | `"PctVol"` | `pctVol` (e.g., "0.25" = 25%), `startTime`, `endTime` |
| **Min Impact** | `"MinImpact"` | `maxPctVol` |
| **Balance Impact/Risk** | `"BalanceImpactRisk"` | `maxPctVol`, `riskAversion` |
| **Accumulate/Distribute** | `"AD"` | GUI-only configuration |

Third-party algos also supported: Jefferies (`"JEFF_VWAP"`, etc.), Quantitative Brokers (`"STROBE"`).

---

## Client Portal REST API

### Setup

1. Download the Client Portal Gateway from IBKR
2. Configure `root/conf.yaml` (default port: 5000)
3. Launch: `bin/run.bat root/conf.yaml` (Windows)
4. Login at `https://localhost:5000` in browser
5. Make REST calls to `https://localhost:5000/v1/api/...`

### Key Order Endpoints

**Initialize session (REQUIRED before trading):**
```
GET /iserver/accounts
```

**Place order:**
```
POST /iserver/account/{accountId}/orders
```
```json
{
  "orders": [{
    "conid": 265598,
    "orderType": "LMT",
    "price": 165.00,
    "side": "BUY",
    "tif": "DAY",
    "quantity": 100
  }]
}
```

**Confirm order (mandatory — most orders trigger a confirmation):**
```
POST /iserver/reply/{replyId}
```
```json
{ "confirmed": true }
```

**Modify order:**
```
POST /iserver/account/{accountId}/order/{orderId}
```

**Cancel order:**
```
DELETE /iserver/account/{accountId}/order/{orderId}
```

**List open orders:**
```
GET /iserver/account/orders
```
Note: First call initializes subscription; second call returns actual data.

**Order preview (what-if):**
```
POST /iserver/account/{accountId}/orders/whatif
```

### WebSocket Streaming

Connect to `wss://localhost:5000/v1/api/ws`. Send heartbeat `"tic"` every 10 seconds.

| Topic | Subscribe Message | Description |
|---|---|---|
| Live Orders | `sor+{}` | Real-time order status updates |
| Market Data | `smd+CONID+{"fields":["31","84","86"]}` | Top-of-book quotes |
| P&L | `spl+{}` | Real-time P&L |
| Account Summary | `sad+{}` | Account data |
| Trades | `str+{}` | Execution updates |
| Market Depth | `sbd+CONID+{}` | Book data |

Unsubscribe by changing `s` prefix to `u` (e.g., `umd+CONID+{}`).

### Authentication & Session

- Session timeout: ~5 minutes without activity. Send `POST /tickle` every 60 seconds.
- Sessions expire daily at midnight (regional).
- Reauthenticate: `POST /iserver/reauthenticate` or `POST /iserver/auth/ssodh/init`
- New OAuth 2.0 unified API in beta (uses `private_key_jwt`).

### Headless Operation: IBeam

[IBeam](https://github.com/Voyz/ibeam) automates browser login using Selenium + virtual display:
```yaml
# docker-compose.yml
services:
  ibeam:
    image: voyz/ibeam
    environment:
      IBEAM_ACCOUNT: ${IB_USER}
      IBEAM_PASSWORD: ${IB_PASS}
    ports:
      - "5000:5000"
```

### REST API Limitations vs TWS API

Missing: Market scanners, advanced algo parameters, conditional orders (limited), real-time 5-second bars, news endpoints. Rate limit: 10 req/sec (gateway), 50 req/sec (direct).

---

## Market Data

### Level 1 Streaming — `reqMktData`

Delivers aggregated snapshots at intra-second intervals (not every tick).

**Default fields returned:**

| Tick ID | Field |
|---|---|
| 0/1/2/3 | BidSize, Bid, Ask, AskSize |
| 4/5 | Last, LastSize |
| 6/7 | High, Low |
| 8 | Volume |
| 9 | Previous Close |
| 14 | Open |

**Useful generic tick types** (request via `genericTickList` parameter):

| ID | Data |
|---|---|
| 233 | RT Volume (last trade + VWAP) |
| 236 | Shortable shares |
| 258 | Fundamental ratios (75+ ratios) |
| 104 | Historical volatility |
| 106 | Option implied volatility + Greeks |
| 225 | Auction values |
| 588 | Futures open interest |

### Tick-by-Tick — `reqTickByTickData`

Every individual trade/quote, not aggregated.

| Type | Data |
|---|---|
| `"Last"` | Last trade |
| `"AllLast"` | All last trades (including outside RTH) |
| `"BidAsk"` | Every bid/ask change |
| `"MidPoint"` | Every midpoint change |

**Limits:** Max **5 simultaneous US securities**. Not available for options in real-time. 1 request per instrument per 15 seconds.

### Level 2 / Depth — `reqMktDepth`

```python
# ib_async
ib.reqMktDepth(contract, numRows=10, isSmartDepth=True)
```

- `isSmartDepth=True` aggregates depth across all subscribed exchanges
- Requires L2 subscription (NASDAQ TotalView, NYSE ArcaBook, etc.)
- Min 3, max 60 simultaneous depth subscriptions

### Market Data Types

```python
# Switch between live and delayed
ib.reqMarketDataType(1)  # 1=Live, 2=Frozen, 3=Delayed (free), 4=Delayed-Frozen (free)
```

### Streaming Lines

Default: 100 concurrent streaming lines. Formula (recalculated monthly):
```
Lines = MAX(commissions/8, (equity × 100)/1,000,000, 100)
```

Quote Booster Packs: $30/month each, +100 L1 lines + 1 L2 symbol. Max 10 packs.

### Market Data Subscriptions (Non-Professional)

| Package | Cost/Month | Contents |
|---|---|---|
| US Securities Snapshot & Futures Value Bundle | $10 | Full NBBO for NYSE/NASDAQ/AMEX + CME futures. Waived with $30+/mo commissions. |
| US Equity & Options Streaming Add-On | $4.50 | Streaming for additional exchanges. Requires the $10 bundle. |
| NASDAQ TotalView (L2) | ~$1.50 | Full NASDAQ depth of book |
| NYSE ArcaBook (L2) | ~$1.50 | NYSE Arca depth |
| NYSE OpenBook (L2) | ~$1.50 | NYSE depth |
| OPRA (options) | ~$1.50 | Real-time options quotes |

**Free tier:** Cboe One + IEX streaming (non-consolidated, NOT true NBBO). Plus 15-min delayed data for everything else.

**Snapshots:** $0.01/request US equities. 100 free/month. Auto-upgrade to streaming if monthly snapshot fees reach subscription cost.

---

## Historical Data

### Bar Sizes Available

`1 secs`, `5 secs`, `10 secs`, `15 secs`, `30 secs`, `1 min`, `2 mins`, `3 mins`, `5 mins`, `10 mins`, `15 mins`, `20 mins`, `30 mins`, `1 hour`, `2 hours`, `3 hours`, `4 hours`, `8 hours`, `1 day`, `1 week`, `1 month`

### Data Types (`whatToShow`)

`TRADES`, `MIDPOINT`, `BID`, `ASK`, `BID_ASK`, `ADJUSTED_LAST`, `HISTORICAL_VOLATILITY`, `OPTION_IMPLIED_VOLATILITY`, `REBATE_RATE`, `FEE_RATE`

### Example (ib_async)

```python
bars = ib.reqHistoricalData(
    contract,
    endDateTime='',           # '' = now
    durationStr='30 D',
    barSizeSetting='1 hour',
    whatToShow='TRADES',
    useRTH=True
)
df = util.df(bars)
```

### Live-Updating Bars (`keepUpToDate`)

```python
bars = ib.reqHistoricalData(
    contract,
    endDateTime='',           # Must be empty string
    durationStr='1 D',
    barSizeSetting='5 mins',
    whatToShow='TRADES',
    useRTH=True,
    keepUpToDate=True         # Streams updates to most recent bar
)
```

### Real-Time 5-Second Bars

```python
bars = ib.reqRealTimeBars(contract, 5, 'TRADES', False)
# Fixed at 5-second intervals. No other sizes available via this function.
# Use keepUpToDate=True on reqHistoricalData for other intervals.
```

### Historical Tick Data

```python
# ibapi
app.reqHistoricalTicks(reqId, contract, startDateTime, "", 1000, "TRADES", 1, True, [])
# Max 1000 ticks per request
```

---

## Scanners & Screening

```python
# Discover all available scan codes
xml = ib.reqScannerParameters()  # Returns XML with all scan types and filters

# Create a live scanner
sub = ScannerSubscription()
sub.instrument = "STK"
sub.locationCode = "STK.US.MAJOR"
sub.scanCode = "TOP_PERC_GAIN"
sub.abovePrice = 5.0
sub.aboveVolume = 100000
sub.numberOfRows = 50           # Max 50 results

results = ib.reqScannerSubscription(sub)
```

Common scan codes: `TOP_PERC_GAIN`, `TOP_PERC_LOSE`, `MOST_ACTIVE`, `HOT_BY_OPT_VOLUME`, `HIGH_OPEN_GAP`, `HIGH_OPT_IMP_VOLAT`, `HALTED`, `HIGH_VS_52W_HL`

**Limits:** Max 50 results per scan, max 10 active scanner subscriptions.

---

## Portfolio & Account Data

### Current Positions

```python
# ib_async
positions = ib.positions()
for p in positions:
    print(f"{p.contract.symbol}: {p.position} @ avg {p.avgCost}")
```

### Account Values

```python
# ib_async
account_values = ib.accountValues()
for v in account_values:
    if v.tag in ('NetLiquidation', 'BuyingPower', 'TotalCashValue', 'AvailableFunds'):
        print(f"{v.tag}: {v.value} {v.currency}")
```

### Account Summary

```python
# ibapi
app.reqAccountSummary(9001, "All",
    "NetLiquidation,BuyingPower,TotalCashValue,AvailableFunds,MaintMarginReq")
```

Available tags: `NetLiquidation`, `TotalCashValue`, `BuyingPower`, `AvailableFunds`, `MaintMarginReq`, `ExcessLiquidity`, `Cushion`, `DayTradesRemaining`, `Leverage`, and many more.

**Limit:** Max 2 active account summary subscriptions.

### P&L

```python
# ib_async
pnl = ib.reqPnL(account)       # Account-level P&L
pnl_single = ib.reqPnLSingle(account, "", conId)  # Per-position P&L
```

---

## Data Caching Strategies

### What to Cache Locally

| Data | Cache? | Why |
|---|---|---|
| Contract IDs (conIds) | Yes | Persistent for life of instrument |
| Daily/weekly bars | Yes | Rarely change; append incrementally |
| Intraday bars (1 min+) | Yes | Bulk download, then use `keepUpToDate` |
| Fundamental data | Yes | Changes quarterly at most |
| Scanner parameters XML | Yes | Changes infrequently |
| Real-time quotes | No | Always request live |
| Tick-by-tick | No | Always request live |
| Market depth | No | Always request live |

### Recommended Time-Series Databases

| Database | Best For |
|---|---|
| **TimescaleDB** | Lowest friction if you use PostgreSQL. Full SQL, compression, retention policies. |
| **QuestDB** | Highest raw performance (6-36x faster ingestion). Best for tick data at scale. |
| **InfluxDB** | Good write performance, strong ecosystem, real-time dashboards. |
| **Parquet files** | Simplest. Fast, works with pandas/polars. No server needed. |
| **DuckDB** | In-process, no server, reads Parquet natively. Great for analytics. |
| **Arctic (MongoDB)** | DataFrame-oriented. Used by pysystemtrade. |

### Best Practice

Use IB for live/recent data and supplement with other providers (Polygon, Databento) for deep historical data. IB's pacing rules make bulk downloading slow.

---

## Commissions

### US Stocks — Fixed Pricing

$0.005/share. Min $1.00/order. Max 1% of trade value. All-in (no separate exchange/clearing fees).

### US Stocks — Tiered Pricing

| Monthly Volume | Rate/Share |
|---|---|
| ≤ 300,000 | $0.0035 |
| 300,001 – 3,000,000 | $0.0020 |
| 3,000,001 – 20,000,000 | $0.0015 |
| > 20,000,000 | $0.0010 |

Min $0.35/order. Plus exchange fees, clearing fees, regulatory fees on top.

**Tiered is cheaper for most retail traders** (lower minimum, potential exchange rebates with limit orders). You can switch between Fixed and Tiered once per day.

### US Options

Fixed: $0.65/contract, min $1.00/order.

Tiered: $0.25-$0.65/contract depending on premium and volume, plus exchange/clearing/regulatory fees.

---

## Rate Limits & Pacing Rules

### Message Rates

| Limit | Value | Consequence |
|---|---|---|
| TWS API messages/sec | 50 | Connection dropped |
| Web API requests/sec | 10 (gateway) / 50 (direct) | HTTP 429, 10-min penalty box |
| Simultaneous API connections | 32 per TWS/Gateway | Connection refused |

### Historical Data Pacing

| Rule | Limit |
|---|---|
| Identical request repeat | Wait 15 seconds |
| Same contract/exchange/type | Max 6 in 2 seconds |
| Total historical requests | Max 60 in any 10-minute window |
| BID_ASK requests | Count as 2 each |
| Max concurrent historical | Market Data Lines / 2 |
| Historical ticks per request | Max 1,000 |

### Market Data

| Limit | Value |
|---|---|
| Default streaming lines | 100 (scales with equity/commissions) |
| Depth of book symbols | Min 3, max 60 |
| Tick-by-tick (US) | Max 5 simultaneous |
| Active scanners | Max 10 |
| Account summary subscriptions | Max 2 |

### Order Limits

- 20 active orders per contract per side per account
- **Order Efficiency Ratio (OER):** IB monitors fill rate. If below ~2% (spam/cancel pattern), warnings → potential account disable.

---

## Practical Gotchas

### Daily Restart

TWS/Gateway performs a **mandatory daily restart** (~11:45 PM ET on weekdays). With auto-restart enabled (File > Global Configuration > Lock and Exit), it can run Sunday-to-Sunday without manual intervention.

**Your code must handle disconnection and reconnection gracefully.** Re-subscribe to market data, re-check open orders, rebuild state.

### Sunday Authentication

Weekly server reset on Saturday night requires **manual re-authentication** on Sunday (2FA). Auto-restart handles Mon-Sat but not Sunday.

### Market Data Farm Messages

These fire as `error()` callbacks with `orderId = -1`:

| Code | Meaning | Action |
|---|---|---|
| 1100 | Connection lost | Pause trading logic |
| 1101 | Connection restored, data lost | Re-subscribe to all market data |
| 1102 | Connection restored, data maintained | Resume normally |
| 2104 | Market data farm OK | Safe to request data |
| 2106 | Historical data farm OK | Safe to request historical |
| 2108 | Market data farm inactive | Wait for 2104 |

**Do not send data requests until you receive 2104/2106.**

### Critical Rules

1. **Never use `time.sleep()`** with ib_async — use `ib.sleep()` instead (yields to event loop)
2. **Always qualify contracts** before use — call `ib.qualifyContracts(contract)`
3. **Call `util.startLoop()`** in Jupyter notebooks (not in regular scripts)
4. **clientId 0 sees all manual TWS orders** — use intentionally or avoid
5. **Implement message throttling** — stay under 45 msg/sec to avoid connection drops
6. **Design for idempotent reconnection** — re-subscribe, re-check state on every connect

### Paper Trading Differences

- Fills are simulated (no real exchange routing) — more optimistic than live
- No commissions simulated
- Some order types unavailable (VWAP, Auction, RFQ, Pegged to Market)
- Real-time market data is live (real quotes)
- Can run paper and live simultaneously (different ports)

---

## Production Deployment

### Recommended Stack

```
ib_async (pip install ib_async)
  + IB Gateway (not TWS)
  + IBC for automated login
  + Docker (gnzsnz/ib-gateway) for headless deployment
  + Paper trading on port 4002 for testing
```

### Docker Setup

```yaml
# docker-compose.yml
services:
  ib-gateway:
    image: ghcr.io/gnzsnz/ib-gateway:latest
    restart: always
    environment:
      TWS_USERID: ${TWS_USERID}
      TWS_PASSWORD: ${TWS_PASSWORD}
      TRADING_MODE: paper          # 'live', 'paper', or 'both'
      VNC_SERVER_PASSWORD: ${VNC_PASSWORD}
      READ_ONLY_API: "no"
      AUTO_RESTART_TIME: "11:59 PM"
      TWOFA_TIMEOUT_ACTION: restart
      RELOGIN_AFTER_TWOFA_TIMEOUT: "yes"
    ports:
      - "127.0.0.1:4001:4001"    # Live
      - "127.0.0.1:4002:4002"    # Paper
      - "127.0.0.1:5900:5900"    # VNC (debugging)
```

Since version 10.26.1k, `TRADING_MODE=both` runs live AND paper simultaneously.

### Watchdog (ib_async)

For production without Docker:

```python
from ib_async import *

ibc = IBC(twsVersion=1019, gateway=True, tradingMode='paper',
          userid='your_user', password='your_pass')

watchdog = Watchdog(ibc, ib=IB(), port=4002, clientId=1,
                    appStartupTime=30, appTimeout=20, retryDelay=5)

def on_startup():
    print("Connected — setting up subscriptions...")

watchdog.startedEvent += on_startup
watchdog.start()
IB.run()
```

### Reconnection Pattern (Manual)

```python
def on_disconnected():
    print("Disconnected — reconnecting in 10s...")
    ib.sleep(10)
    try:
        ib.connect('127.0.0.1', 4002, clientId=1)
    except Exception as e:
        print(f"Reconnect failed: {e}")

ib.disconnectedEvent += on_disconnected
```

---

## Error Handling Reference

### Order Errors

| Code | Meaning |
|---|---|
| 103 | Duplicate order ID |
| 104 | Can't modify a filled order |
| 105 | Modified order doesn't match original |
| 110 | Price doesn't conform to minimum tick |
| 161 | Cancel attempted on non-cancellable order |
| 200 | No security definition found (wrong contract spec) |
| 201 | Order rejected (reason in message) |
| 202 | Order cancelled (reason in message) |
| 203 | Security not available for this account |
| 399 | Order won't be placed (warning before submission) |

### Connection Errors

| Code | Meaning |
|---|---|
| 1100 | Connectivity lost between IB and TWS |
| 1101 | Connectivity restored, data lost |
| 1102 | Connectivity restored, data maintained |
| 2103 | Market data farm connection broken |
| 2104 | Market data farm connection OK |
| 2105 | Historical data farm connection broken |
| 2106 | Historical data farm connection OK |
| 2108 | Market data farm inactive |
| 2158 | Sec-def data farm connection OK |

### Error Handling Pattern

```python
def on_error(reqId, errorCode, errorString, contract):
    if errorCode == 1100:
        pause_trading()
    elif errorCode in (1101, 1102):
        resubscribe_all_data()
        if errorCode == 1101:
            rebuild_state()
    elif errorCode in (2104, 2106):
        data_ready = True
    elif errorCode == 201:
        log(f"ORDER REJECTED {reqId}: {errorString}")
    elif errorCode == 202:
        log(f"ORDER CANCELLED {reqId}: {errorString}")

ib.errorEvent += on_error
```

---

## Quick Reference: Local Caching for Unsupported Features

For features IB doesn't natively support, implement locally:

| Feature | IB Support | Local Workaround |
|---|---|---|
| Order deactivation/reactivation | No direct API | Cancel order, cache params locally, re-place on demand |
| Cancel orders by symbol | No direct API | Query open orders, filter, cancel individually |
| Order templates/presets | GUI only | Build factory functions returning pre-configured Order objects |
| Complex order scheduling | Limited (`goodAfterTime`) | Local scheduler + `placeOrder()` at scheduled times |
| Multi-leg strategy management | Manual bracket/OCA only | State machine tracking legs with local persistence |
| Historical data bulk download | Pacing-limited | Incremental downloads with local DB, supplement with Polygon/Databento |
| Order audit trail | Current day only | Log all `orderStatus` and `execDetails` events to local DB |
