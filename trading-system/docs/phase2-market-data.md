# Phase 2 Market Data Pipeline

## Implemented

- Standard `ObservedMarketEvent` wrapper with received time and latency.
- Exchange parser modules:
  - Binance kline and book ticker payloads.
  - Bybit kline and ticker payloads.
  - Bitget candlestick and ticker payloads.
- PostgreSQL persistence helper for `candles` and `order_books`.
- Ingestion loop that consumes `MarketStream` and persists standard events.
- Public WebSocket subscriptions for 1m candles and best bid/ask ticker data, with reconnect backoff.
- Optional API startup wiring via `MARKET_DATA_ENABLED=true` and `MARKET_DATA_EXCHANGES=binance,bybit,bitget`.
- Market data latency above the 2-second entry gate threshold records a `market_data_latency` risk event.

## API References Checked

- Binance USD-M Futures WebSocket connect docs: combined streams use `/stream?streams=...`, and market/public data now require routed paths.
- Binance kline stream docs: `<symbol>@kline_<interval>` uses the `/market` route.
- Binance book ticker docs: `<symbol>@bookTicker` uses the `/public` route.
- Bybit V5 ticker docs: public linear ticker topic is `tickers.{symbol}` with `bid1Price`, `ask1Price`, `bid1Size`, and `ask1Size`.
- Bybit V5 kline docs: public kline topic is `kline.{interval}.{symbol}`.
- Bitget futures ticker docs: subscription uses `instType=USDT-FUTURES`, `channel=ticker`, and `instId`.
- Bitget futures candlestick docs: subscription uses `channel=candle1m` for 1-minute candles.

## Next Implementation Order

1. Add an integration test that feeds recorded payload fixtures through parser -> repository -> database.
2. Verify live BTCUSDT streams against the three exchanges with PostgreSQL persistence enabled.

## Completion Gate

Phase 2 is complete only after BTCUSDT real-time data is collected from Binance, Bybit, and Bitget, persisted to PostgreSQL, and reconnection/latency behavior is verified.
