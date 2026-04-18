#!/usr/bin/env bash
# Orders-panel validation journey (Slice 7 of plan/order-store.md).
#
# Usage:
#   Terminal 1:  cd desktop/win && cargo run -p midas-app --features dev_harness -- --fixture empty-aapl-d1
#   Terminal 2:  cd desktop/win && ./scripts/devloop-orders-journey.sh
#
# Preconditions:
#   1. Launch the app manually with --features dev_harness
#   2. Add an "Orders" pane via the toolbar button
#   3. Have an AAPL chart showing so `inject_ticker_msg` routes correctly
#
# What it does:
#   - Drives a full bracket submission through inject_ticker_msg
#   - Uses InjectBrokerEvent (D3) to fabricate a full fill chain that
#     TestBroker's default "instant" timing doesn't fire for Limit legs
#   - Screenshots the populated blotter
#   - Verifies the rows exist via dump_state

set -e

PORT="${DEVLOOP_PORT:-9898}"
HOST="127.0.0.1"

send() { printf '%s\n' "$1" | nc -w 2 "$HOST" "$PORT"; }
hr()   { printf -- '---- %s ----\n' "$1"; }

# Use the same uuids throughout so BracketCreated → OrderStatusChanged →
# OrderFilled all reference the same entry-leg row.
PARENT_ID="550e8400-e29b-41d4-a716-446655440000"
TP_ID="550e8400-e29b-41d4-a716-446655440001"
SL_ID="550e8400-e29b-41d4-a716-446655440002"

hr "ping"
send '{"cmd":"ping"}'
echo

hr "set up a Market bracket for AAPL (skips Limit-fill-via-tick path)"
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"SetBracketMode","side":"Buy"}}'
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"EnsureDraftBracket","side":"Buy","entry_type":"Market"}}'
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"SetQuantity","quantity":100.0}}'
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"SetLegPrice","role":"TakeProfit","price":195.00}}'
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"SetLegPrice","role":"StopLoss","price":178.00}}'
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"SetTpEnabled","enabled":true}}'
send '{"cmd":"inject_ticker_msg","symbol":"AAPL","msg_json":{"type":"SetSlEnabled","enabled":true}}'
echo

hr "inject a BracketCreated so the blotter gets three rows"
send "{\"cmd\":\"inject_broker_event\",\"event_json\":{\"type\":\"BracketCreated\",\"parent_id\":\"$PARENT_ID\",\"take_profit_id\":\"$TP_ID\",\"stop_loss_id\":\"$SL_ID\",\"symbol\":\"AAPL\",\"action\":\"Buy\",\"quantity\":100.0,\"tp_price\":195.00,\"sl_price\":178.00,\"reference_price\":184.50,\"entry_kind\":\"Market\",\"entry_limit_price\":null,\"entry_stop_price\":null,\"sl_limit_price\":null,\"tp_tif\":\"Day\",\"sl_tif\":\"Gtc\"}}"
echo

hr "wait for the BracketCreated log entry"
send '{"cmd":"wait_for_event","event_type":"BracketCreated","timeout_ms":2000}'
echo

hr "fill the entry leg"
send "{\"cmd\":\"inject_broker_event\",\"event_json\":{\"type\":\"OrderStatusChanged\",\"order_id\":\"$PARENT_ID\",\"old_status\":\"Submitted\",\"new_status\":\"Filled\",\"filled_qty\":100.0,\"remaining_qty\":0.0,\"avg_fill_price\":184.53}}"
echo

hr "wait for OrderStatusChanged"
send '{"cmd":"wait_for_event","event_type":"OrderStatusChanged","timeout_ms":2000}'
echo

hr "dump blotter rows (expect 3 rows: Entry/TP/SL)"
send '{"cmd":"dump_state","path":"order_blotter"}'
echo

hr "screenshot the populated blotter"
send '{"cmd":"screenshot","out_path":".devloop/shots/orders-panel-filled.png"}'
echo

hr "wait for idle"
send '{"cmd":"wait_for_idle","timeout_ms":500}'
echo

echo
echo "Journey complete. Inspect .devloop/shots/orders-panel-filled.png."
echo "To tear down: send  '{\"cmd\":\"shutdown\"}'."
