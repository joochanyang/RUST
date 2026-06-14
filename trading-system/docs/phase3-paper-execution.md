# Phase 3 Strategy, Risk, and Paper Execution

## Implemented

- Common order request, fill, protected order, and TP/SL plan types.
- RSI + Bollinger technical strategy skeleton.
- Basic risk gate with the project default rules:
  - Default leverage 3x.
  - Max leverage 5x.
  - Max entry notional 5% of equity.
  - Stop loss 1%.
  - Take profit 2%.
  - Minimum reward/risk ratio 1:2.
  - Entry blocked when market data latency exceeds 2 seconds.
- Paper broker that refuses non-paper requests and simulates immediate market fills.
- Persistence helpers for signals, orders, fills, positions, and protection orders.
- API-level helper for signal -> risk gate -> paper broker -> database recording.
- Market ingestion forwarding into a paper strategy loop when `PAPER_TRADING_ENABLED=true`.
- Virtual TP/SL tracker that closes paper positions on best bid/ask updates and records `paper_exits`.
- Duplicate signal suppression for repeated updates to the same candle open time.
- Same exchange/symbol paper position lockout until the virtual TP/SL exit is persisted.
- Paper strategy runtime rebuilds its open-position lockout from stored open positions on startup.
- Paper strategy runtime restores open protected orders into the TP/SL tracker on startup.
- Runtime lock state blocks new paper entries through the risk gate.
- Paper entry risk state uses stored paper PnL and open-position unrealized PnL instead of only static env defaults.
- Open paper positions are marked to market (`mark_price`/`unrealized_pnl`) on every order-book tick, so manual/panic/dashboard closes realize the true marked PnL. Previously these positions were frozen at entry price, making every non-SL/TP close realize ~0 PnL and starving the daily-loss kill switch — fixed.
- Paper exit persistence is idempotent: `persist_paper_exit` closes the position under a `closed_at IS NULL` guard and skips recording a duplicate exit if the position was already closed out-of-band (dashboard/panic), preventing double-counted PnL. It returns `Ok(true)` when recorded, `Ok(false)` when already closed.
- Manual close and panic close record `paper_exits` with realized PnL computed from the marked exit price.

## Safety Gate

The paper broker explicitly rejects any `TradingMode::Live` request. Live order routing remains unimplemented in exchange adapters and should not be added before the Phase 6 live-readiness checklist is satisfied.

## Remaining Work

1. Add integration tests after the Rust toolchain issue is resolved.
