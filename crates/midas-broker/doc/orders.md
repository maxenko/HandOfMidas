# Order Types & State Machine

## LocalOrder

Full lifecycle representation of an order, persisted to SQLite.

Key fields:
- `id: Uuid` — UUIDv7 (time-sortable)
- `ib_order_id: Option<i32>`, `ib_perm_id: Option<i64>` — assigned by IB on submission
- `symbol`, `con_id`, `sec_type: SecurityType`, `exchange`, `currency`
- `action: OrderAction`, `order_type: OrderKind`, `quantity`, `tif: TimeInForce`
- `status: OrderStatus` — current state machine position
- `filled_qty`, `remaining_qty`, `avg_fill_price`, `commission`
- `parent_id`, `oca_group`, `bracket_role` — for bracket/OCA grouping
- `created_at`, `updated_at` — `DateTime<Utc>`

Constructor: `LocalOrder::new_draft(symbol, action, order_type, quantity)` — defaults to STK/SMART/USD/DAY.

## OrderAction

```rust
pub enum OrderAction { Buy, Sell }
```
Display: `Buy` → `"BUY"`. FromStr: `"BUY"` → `Buy`.

## OrderKind

```rust
pub enum OrderKind { Market, Limit, Stop, StopLimit, TrailingStop }
```
Display: `Limit` → `"LMT"`, `StopLimit` → `"STP LMT"`, `TrailingStop` → `"TRAIL"`.

## TimeInForce

```rust
pub enum TimeInForce { Day, Gtc, Ioc, Gtd, Opg }
```
Display: `Day` → `"DAY"`, etc.

## OrderStatus State Machine (11 states)

```
Draft → Inactive → PendingSubmit → PreSubmitted → Submitted
                                                    ↓
                                              PartiallyFilled → Filled
                                                    ↓
                                              PendingCancel → Cancelled

Any non-terminal → Error → Inactive (retry) or PendingSubmit (retry)
Any live state → Rejected
```

Terminal states: `Filled`, `Cancelled`, `Rejected`.

Predicates:
- `is_terminal()` — Filled, Cancelled, Rejected
- `is_live_at_ib()` — PreSubmitted, Submitted, PartiallyFilled
- `can_activate()`, `can_deactivate()`, `can_cancel()`, `can_modify_locally()`, `can_modify_at_ib()`

`from_ib_status(s)` maps IB's status strings. Note: IB's `"Inactive"` maps to `Rejected`.

32 transition tests validate every edge of the state machine.
