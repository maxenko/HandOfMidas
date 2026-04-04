# Market Order Bracket — Implementation Plan

> Full-stack implementation of Market Order brackets for Hand of Midas.
> Covers broker engine, chart visualization, UI order entry, and IB API submission.
>
> Status: IN PROGRESS — Phase 1 ~90% complete, Phase 2 signatures done, Phase 3 types done
> Date: 2026-04-01 (updated 2026-04-02)
> Documents: 5
>
> **Reference**: TradingView's bracket order model (1 TP + 1 SL per bracket)
> served as the UX baseline. IB's native bracket order API (parent + children
> linked via `parentId` + `transmit` flag) is the execution mechanism.
>
> **Scope**: Market Order entry with attached Take Profit (Limit) and
> Stop Loss (Stop). Single TP + single SL per bracket (TradingView model).
> Multi-TP via OCA groups is deferred to Stop Limit bracket plan.

---

## Documents

| # | Document | Lines | Description |
|---|----------|-------|-------------|
| 01 | [Data Model](01-data-model.md) | ~500 | Type changes to `midas-broker` and `midas-chart` for market bracket support |
| 02 | [Broker Engine](02-broker-engine.md) | ~600 | Engine command handling, IB API submission, bracket lifecycle management |
| 03 | [Chart Visualization](03-chart-visualization.md) | ~500 | Chart-level bracket rendering, interaction, and status-driven styling |
| 04 | [Order Entry UI](04-order-entry-ui.md) | ~400 | Order panel widget, TradingView-style input, broker bridge |
| 05 | [Testing & Rollout](05-testing-and-rollout.md) | ~400 | Test strategy, phased implementation, acceptance criteria |

---

## Architecture Context

```
┌─────────────┐     BrokerCommand      ┌──────────────┐     rust-ibapi     ┌─────┐
│  midas-app  │ ──────────────────────> │ midas-broker │ ──────────────── > │ IB  │
│  (UI layer) │ <────────────────────── │   (engine)   │ <──────────────── │ TWS │
└──────┬──────┘     BrokerEvent         └──────────────┘                   └─────┘
       │
       │  Annotation ↔ Order bridge
       │
┌──────┴──────┐
│ midas-chart │   Sans-IO chart core
│  (widgets)  │   OrderBracket annotation
└─────────────┘
```

**Key boundary**: `midas-chart` knows nothing about broker orders. It produces
visual geometry (OrderBracket). `midas-app` is the bridge that maps brackets
to LocalOrder trios and routes BrokerEvents back to bracket status updates.

---

## TradingView Reference

TradingView's Market Order bracket (the UX baseline for this plan):

- **Order ticket**: BUY/SELL toggle + quantity + optional TP checkbox + optional SL checkbox
- **TP input**: Absolute price, offset (ticks/pips), or percentage from entry
- **SL input**: Absolute price, offset, or percentage. Optional trailing stop toggle
- **Risk/Reward**: Calculated dynamically as TP and SL are adjusted
- **Chart lines**: Entry (blue/green dashed), TP (green solid), SL (red solid)
- **Draggable**: TP and SL lines can be dragged on chart to modify live orders
- **P&L labels**: Projected profit/loss shown next to each line
- **OCO**: When TP fills, SL auto-cancels (and vice versa)
- **Limitations**: Only 1 TP + 1 SL per bracket (no multi-TP in standard UI)

> **Reference URLs** (verify in browser — may have changed):
> - TradingView TP/SL docs: `tradingview.com/support/solutions/43000540498`
> - TradingView order types: `tradingview.com/support/solutions/43000540497`
> - Pine Script `strategy.exit()`: `tradingview.com/pine-script-reference/v5/#fun_strategy.exit`

---

## Non-Goals

These are explicitly excluded from this plan's scope:

| Non-Goal | Why Excluded | Reconsider When |
|----------|-------------|-----------------|
| Multi-TP / multi-SL brackets (OCA groups) | Significant additional complexity; deferred to Stop Limit bracket plan | Stop Limit plan begins |
| Limit entry brackets | Already designed in `broker/plan/02-order-management.md` §6 | Need arises to revise existing design |
| Trailing stop as SL type | Requires additional state tracking for trail distance | User demand after v1 market brackets ship |
| Bracket templates / presets | UX convenience, not core functionality | After order panel is validated by users |
| Automatic position sizing (% of account) | Requires account data integration not yet built | After `RequestAccountSummary` is implemented |
| Real-time P&L streaming on bracket lines | Requires live tick feed integrated with chart annotations | After `midas-feed` streaming is built |
| Close Position shortcut | Convenience feature, reserved in context menu but not implemented | Phase 6 or later |

---

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Bracket model | 1 TP + 1 SL (TradingView) | Simple, proven UX. Multi-TP deferred to Stop Limit plan |
| Entry type | Market only | This plan's scope. Limit entry covered by existing bracket system |
| IB submission | Native bracket (parentId + transmit) | IB manages implicit OCA between TP/SL children automatically |
| Bracket creation | Order panel widget (not chart drawing) | Market orders fill instantly — no entry price to draw |
| TP/SL visualization | Reuse existing OrderBracket widget | Already has BracketLeg, BracketStatus, zone fills, R:R display |
| Price input | Absolute + offset + percentage modes | Matches TradingView. Percentage is most useful for quick R:R |
| SL type | Stop (default), StopLimit (optional) | Stop is simpler; StopLimit adds slippage protection |
| TP/SL required? | Both optional, but validation warns if neither set | Naked market orders are valid but risky |
| Bracket storage | 3 rows in `orders` table (existing schema) | No schema changes needed — `parent_id`, `bracket_role` already exist |
| Chart display | Show TP/SL lines after fill, no entry line | Market orders fill instantly — entry line would be at fill price |

---

## Alternatives Considered

### Market Order Submission: Immediate vs Draft→Activate

Two credible approaches for submitting market brackets:

| Approach | Pros | Cons |
|----------|------|------|
| **Immediate submission** (chosen) | Matches user intent — market orders are "act now"; no stale draft state; simpler command API | No review step for large orders; harder to add pre-flight checks later |
| **Draft → Inactive → Activate** | Existing workflow for limit brackets; user can review before submitting | Market orders fill instantly — reviewing a draft at an unknown entry price is meaningless; adds latency to a time-sensitive action |

**Decision**: Immediate submission. Market orders have no entry price to review. The order panel's confirmation dialog (§5 in `04-order-entry-ui.md`) provides the review opportunity. If Draft→Activate is later needed (e.g., for very large orders), `CreateMarketBracket` can be extended with an optional `defer_submit: bool` flag without breaking the existing API.

### IB Bracket Mechanism: Native parentId+transmit vs Manual OCA

| Approach | Pros | Cons |
|----------|------|------|
| **Native bracket** (parentId + transmit, chosen) | IB manages child auto-cancellation on fill atomically; proven mechanism; fewer API calls | Limited to 1 TP + 1 SL per parent; children must share the same contract |
| **Manual OCA group** | Supports multi-TP / multi-SL; more flexible grouping | Requires explicit OCA management; more error-prone; IB OCA has documented quirks with partial fills |

**Decision**: Native bracket via `parentId` + `transmit` flag. This is IB's intended mechanism for single-TP/single-SL brackets and handles child auto-cancellation atomically. Multi-TP via OCA is deferred to the Stop Limit bracket plan.

> **Terminology note**: Throughout this plan, "implicit OCA" refers informally to
> IB's parent-child auto-cancellation behavior (when one child fills, IB cancels
> the sibling). This is **not** an explicit OCA group — it is a feature of the
> `parentId` relationship. The distinction matters for the future Multi-TP plan,
> which will use actual OCA groups (`ocaGroup` field) for a different mechanism.
