# Broker Bridge: Wire BrokerEngine into Desktop UI

Connect the `midas-broker` engine (TestBroker) to the desktop app so market order
brackets placed in the UI flow through the real engine, get simulated fills, and
update chart annotations with live status.

## Status: Planned

## Documents

| Doc | Contents |
|-----|----------|
| [01-architecture.md](01-architecture.md) | Component diagram, BrokerBridge adapter design, event/command flow, lifecycle, config, type translation layer |
| [02-implementation.md](02-implementation.md) | 11 vertical slices (0-10) with code, tests, and done criteria |

## Slice Overview

```
S0  BracketCreated prices + re-exports    (root workspace)
S1  Cargo.toml dependency                 (desktop workspace)
S2  BrokerBridge adapter                  (new file: broker_bridge.rs)
S3  Start engine on app startup           (MidasApp::new)
S4  Wire OrderPanelConfirmYes             (local annotation + engine command)
S5  Broker event subscription             (iced Subscription::run_with)
S6  Reconcile engine UUIDs                (update OrderAnnotationLink)
S7  Status updates from fills             (Pending -> Active -> Closed)
S8  Drag-to-modify bracket legs           (BrokerCommand::ModifyBracketLeg)
S9  Context menu cancel                   (BrokerCommand::CancelBracket)
S10 Connection state in status bar        (watch channel -> colored dot)
```

Slices 0-6 are sequential. Slices 7-10 depend on S6 but are independent of each other.

## Key Design Decisions

- **No mailbox processor** -- BrokerEngine is already its own actor via tokio::spawn
- **No feature gating** -- broker is core functionality, always compiled
- **Local annotation + UUID reconciliation** -- instant UX, engine UUIDs patched in async
- **iced 0.14 `Subscription::run_with`** -- correct API for streaming broker events
- **`try_send` (non-blocking)** -- OrderBroker trait is sync, mpsc capacity 256 is ample
- **Boxed BrokerEvent in Message** -- avoids bloating the iced Message enum
