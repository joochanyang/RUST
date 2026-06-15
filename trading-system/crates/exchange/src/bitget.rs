use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use tokio::{sync::mpsc, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use trading_core::{ExchangeId, MarketEvent, OrderBookTop, Result, Symbol, TradingError};

use crate::{
    AccountSnapshot, CancelAck, ExchangeAdapter, MarketOrderRequest, MarketStream, OrderAck,
    ProtectionAck, ProtectionOrderRequest,
};

/// Maximum gap between market-data frames before the WebSocket is treated as
/// stalled and reconnected. A silent gap this long means the connection has
/// half-opened; without this bound `read.next()` hangs forever and the strategy
/// freezes on stale data.
const MARKET_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct BitgetAdapter {
    pub ws_base_url: String,
    pub rest_base_url: String,
}

impl Default for BitgetAdapter {
    fn default() -> Self {
        Self {
            ws_base_url: "wss://ws.bitget.com/v2/ws/public".to_owned(),
            rest_base_url: "https://api.bitget.com".to_owned(),
        }
    }
}

#[async_trait]
impl ExchangeAdapter for BitgetAdapter {
    fn exchange_id(&self) -> ExchangeId {
        ExchangeId::Bitget
    }

    async fn subscribe_market_stream(&self, symbols: &[Symbol]) -> Result<MarketStream> {
        let args = symbols
            .iter()
            .flat_map(|symbol| {
                [
                    json!({
                        "instType": "USDT-FUTURES",
                        "channel": "ticker",
                        "instId": symbol.as_str()
                    }),
                    json!({
                        "instType": "USDT-FUTURES",
                        "channel": "candle1m",
                        "instId": symbol.as_str()
                    }),
                ]
            })
            .collect::<Vec<_>>();
        let (sender, receiver) = mpsc::channel(1024);
        let url = self.ws_base_url.clone();

        tokio::spawn(async move {
            if let Err(error) = run_public_market_stream_with_reconnect(url, args, sender).await {
                tracing::warn!(%error, "Bitget market stream stopped");
            }
        });

        Ok(MarketStream::new(receiver))
    }

    async fn fetch_account_snapshot(&self) -> Result<AccountSnapshot> {
        Err(TradingError::Exchange(
            "Bitget account snapshot is not implemented yet".to_owned(),
        ))
    }

    async fn place_market_order(&self, _request: MarketOrderRequest) -> Result<OrderAck> {
        Err(TradingError::Exchange(
            "Bitget live order routing is not implemented yet".to_owned(),
        ))
    }

    async fn place_protection_orders(
        &self,
        _request: ProtectionOrderRequest,
    ) -> Result<ProtectionAck> {
        Err(TradingError::Exchange(
            "Bitget protection orders are not implemented yet".to_owned(),
        ))
    }

    async fn cancel_order(&self, _order_id: String) -> Result<CancelAck> {
        Err(TradingError::Exchange(
            "Bitget cancel order is not implemented yet".to_owned(),
        ))
    }

    async fn query_order(
        &self,
        _symbol: &Symbol,
        _client_order_id: &str,
    ) -> Result<Option<OrderAck>> {
        Err(TradingError::Exchange(
            "Bitget order query is not implemented yet".to_owned(),
        ))
    }
}

async fn run_public_market_stream_with_reconnect(
    url: String,
    args: Vec<Value>,
    sender: mpsc::Sender<Result<trading_core::ObservedMarketEvent>>,
) -> Result<()> {
    let mut backoff = Duration::from_secs(1);

    loop {
        if sender.is_closed() {
            return Ok(());
        }

        match run_public_market_stream_once(&url, &args, sender.clone(), MARKET_STREAM_IDLE_TIMEOUT)
            .await
        {
            Ok(()) => {
                backoff = Duration::from_secs(1);
            }
            Err(error) => {
                let _ = sender
                    .send(Err(TradingError::Exchange(format!(
                        "Bitget market stream reconnecting after error: {error}"
                    ))))
                    .await;
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn run_public_market_stream_once(
    url: &str,
    args: &[Value],
    sender: mpsc::Sender<Result<trading_core::ObservedMarketEvent>>,
    idle_timeout: Duration,
) -> Result<()> {
    let (stream, _) = connect_async(url).await.map_err(|error| {
        TradingError::Exchange(format!("Bitget WebSocket connect failed: {error}"))
    })?;
    let (mut write, mut read) = stream.split();
    let subscribe = json!({ "op": "subscribe", "args": args });
    write
        .send(Message::Text(subscribe.to_string().into()))
        .await
        .map_err(|error| TradingError::Exchange(format!("Bitget subscribe failed: {error}")))?;

    // Receive-time of the most recent order-book frame; see binance.rs for why
    // the whole-socket idle timeout can't see a ticker-only stall. Bitget
    // multiplexes ticker + candle on one socket, so a trickling candle frame
    // masks a dead ticker stream on the shared idle timer. Anchored to the
    // connect instant (this socket always subscribes the ticker channel), so a
    // ticker channel that never (re)starts after a reconnect — while candles
    // keep arriving — is still caught instead of silently streaming candle-only.
    let mut last_orderbook_at: Option<chrono::DateTime<Utc>> = Some(Utc::now());
    let staleness = chrono::Duration::seconds(trading_core::ORDERBOOK_STREAM_STALENESS_SECS);

    loop {
        // A silent connection must not hang the loop forever: time the read out so
        // the reconnect loop can re-establish it.
        let message = match tokio::time::timeout(idle_timeout, read.next()).await {
            Ok(Some(message)) => message,
            Ok(None) => return Ok(()),
            Err(_elapsed) => {
                return Err(TradingError::Exchange(format!(
                    "Bitget WebSocket stalled: no frame within {idle_timeout:?}"
                )));
            }
        };

        match message {
            Ok(Message::Text(text)) => {
                if !is_market_payload(text.as_ref()) {
                    continue;
                }
                let event = parse_market_payload(text.as_ref())?;
                let now = Utc::now();
                if matches!(event, MarketEvent::OrderBook(_)) {
                    last_orderbook_at = Some(now);
                }
                let observed = trading_core::ObservedMarketEvent::new(event, now);
                if sender.send(Ok(observed)).await.is_err() {
                    return Ok(());
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            // A read error means the connection is broken; return so the reconnect
            // loop re-establishes it instead of spinning on a dead socket.
            Err(error) => {
                return Err(TradingError::Exchange(format!(
                    "Bitget WebSocket read failed: {error}"
                )));
            }
        }

        // Partial stall: order-book frames stopped while other frames keep the
        // socket alive. Reconnect rather than stream stale data.
        if trading_core::orderbook_stream_is_stale(last_orderbook_at, Utc::now(), staleness) {
            return Err(TradingError::Exchange(format!(
                "Bitget order-book stream stalled: no order-book frame within {staleness}"
            )));
        }
    }
}

fn is_market_payload(payload: &str) -> bool {
    serde_json::from_str::<Value>(payload)
        .ok()
        .is_some_and(|value| {
            value
                .get("data")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
        })
}

pub fn parse_market_payload(payload: &str) -> Result<MarketEvent> {
    let value: Value = serde_json::from_str(payload)
        .map_err(|error| TradingError::Exchange(format!("invalid Bitget payload JSON: {error}")))?;
    let channel = value
        .get("arg")
        .and_then(|arg| arg.get("channel"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    if channel == "ticker" {
        return parse_ticker(payload);
    }

    if channel.starts_with("candle") {
        return parse_kline(payload);
    }

    Err(TradingError::Exchange(format!(
        "unsupported Bitget market channel: {channel}"
    )))
}

#[derive(Debug, Deserialize)]
struct BitgetTickerEnvelope {
    data: Vec<BitgetTicker>,
}

#[derive(Debug, Deserialize)]
struct BitgetTicker {
    #[serde(rename = "instId")]
    symbol: String,
    #[serde(rename = "bidPr")]
    bid_price: Decimal,
    #[serde(rename = "askPr")]
    ask_price: Decimal,
    #[serde(rename = "bidSz")]
    bid_size: Decimal,
    #[serde(rename = "askSz")]
    ask_size: Decimal,
    #[serde(rename = "ts", deserialize_with = "deserialize_i64_from_string")]
    timestamp: i64,
}

#[derive(Debug, Deserialize)]
struct BitgetKlineEnvelope {
    arg: BitgetArg,
    data: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct BitgetArg {
    channel: String,
    #[serde(rename = "instId")]
    symbol: String,
}

pub fn parse_ticker(payload: &str) -> Result<MarketEvent> {
    let envelope: BitgetTickerEnvelope = serde_json::from_str(payload).map_err(|error| {
        TradingError::Exchange(format!("invalid Bitget ticker payload: {error}"))
    })?;
    let ticker =
        envelope.data.into_iter().next().ok_or_else(|| {
            TradingError::Exchange("Bitget ticker payload has no data".to_owned())
        })?;

    Ok(MarketEvent::OrderBook(OrderBookTop {
        exchange: ExchangeId::Bitget,
        symbol: Symbol::new(ticker.symbol),
        event_time: millis_to_utc(ticker.timestamp)?,
        best_bid: ticker.bid_price,
        best_ask: ticker.ask_price,
        bid_size: ticker.bid_size,
        ask_size: ticker.ask_size,
    }))
}

pub fn parse_kline(payload: &str) -> Result<MarketEvent> {
    let envelope: BitgetKlineEnvelope = serde_json::from_str(payload).map_err(|error| {
        TradingError::Exchange(format!("invalid Bitget kline payload: {error}"))
    })?;
    // On subscribe Bitget pushes an `action:"snapshot"` batch of up to 500
    // historical candles ordered oldest-first; live `update` frames carry a
    // single current candle. Take the LAST row so the snapshot yields the newest
    // candle instead of an ~8h-stale one (which inflates latency and poisons the
    // candle buffer). For a single-row update, last == the only row.
    let row = envelope
        .data
        .into_iter()
        .next_back()
        .ok_or_else(|| TradingError::Exchange("Bitget kline payload has no data".to_owned()))?;

    if row.len() < 6 {
        return Err(TradingError::Exchange(
            "Bitget kline payload has fewer than 6 fields".to_owned(),
        ));
    }

    Ok(MarketEvent::Candle(trading_core::Candle {
        exchange: ExchangeId::Bitget,
        symbol: Symbol::new(envelope.arg.symbol),
        timeframe: envelope.arg.channel.trim_start_matches("candle").to_owned(),
        open_time: millis_to_utc(parse_i64(&row[0])?)?,
        open: parse_decimal(&row[1])?,
        high: parse_decimal(&row[2])?,
        low: parse_decimal(&row[3])?,
        close: parse_decimal(&row[4])?,
        volume: parse_decimal(&row[5])?,
    }))
}

fn parse_i64(value: &str) -> Result<i64> {
    value.parse::<i64>().map_err(|error| {
        TradingError::Exchange(format!("invalid Bitget integer value {value}: {error}"))
    })
}

fn parse_decimal(value: &str) -> Result<Decimal> {
    value.parse::<Decimal>().map_err(|error| {
        TradingError::Exchange(format!("invalid Bitget decimal value {value}: {error}"))
    })
}

fn millis_to_utc(value: i64) -> Result<chrono::DateTime<Utc>> {
    Utc.timestamp_millis_opt(value)
        .single()
        .ok_or_else(|| TradingError::Exchange(format!("invalid millisecond timestamp: {value}")))
}

fn deserialize_i64_from_string<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    value.parse().map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bitget_ticker_payload() {
        let payload = concat!(
            r#"{"action":"snapshot","arg":{"instType":"USDT-FUTURES","#,
            r#""channel":"ticker","instId":"BTCUSDT"},"data":[{"#,
            r#""instId":"BTCUSDT","bidPr":"68000.1","askPr":"68001.2","#,
            r#""bidSz":"1.1","askSz":"1.3","ts":"1710000001000"}]}"#
        );
        let event = parse_ticker(payload).unwrap();

        match event {
            MarketEvent::OrderBook(order_book) => {
                assert_eq!(order_book.exchange, ExchangeId::Bitget);
                assert_eq!(order_book.symbol.as_str(), "BTCUSDT");
            }
            MarketEvent::Candle(_) => panic!("expected order book"),
        }
    }

    #[test]
    fn ignores_bitget_subscription_ack() {
        let payload = concat!(
            r#"{"event":"subscribe","arg":{"instType":"USDT-FUTURES","#,
            r#""channel":"ticker","instId":"BTCUSDT"}}"#
        );
        assert!(!is_market_payload(payload));
    }

    #[test]
    fn parses_bitget_kline_payload() {
        let payload = concat!(
            r#"{"action":"snapshot","arg":{"instType":"USDT-FUTURES","#,
            r#""channel":"candle1m","instId":"BTCUSDT"},"data":[["#,
            r#""1710000000000","68000.1","68020.3","67990.4","#,
            r#""68010.2","12.5","850000","850000"]],"ts":1710000001000}"#
        );
        let event = parse_market_payload(payload).unwrap();

        match event {
            MarketEvent::Candle(candle) => {
                assert_eq!(candle.exchange, ExchangeId::Bitget);
                assert_eq!(candle.symbol.as_str(), "BTCUSDT");
                assert_eq!(candle.timeframe, "1m");
            }
            MarketEvent::OrderBook(_) => panic!("expected candle"),
        }
    }

    #[test]
    fn bitget_snapshot_batch_uses_newest_candle() {
        // On subscribe Bitget sends an `action:"snapshot"` batch of up to 500
        // historical candles ordered OLDEST-first (verified against the live v2
        // ws: first row ~499 min old, last row current). Taking the first row
        // makes open_time ~8h stale -> the latency gate (correctly) blocks it,
        // but it also poisons the candle buffer. The parser must emit the NEWEST
        // (last) row instead.
        let payload = concat!(
            r#"{"action":"snapshot","arg":{"instType":"USDT-FUTURES","#,
            r#""channel":"candle1m","instId":"BTCUSDT"},"data":[["#,
            r#""1710000000000","68000.1","68020.3","67990.4","68010.2","12.5","850000","850000"],["#,
            r#""1710000060000","68010.2","68030.0","68000.0","68025.0","11.0","840000","840000"],["#,
            r#""1710000120000","68025.0","68040.0","68015.0","68035.0","10.0","830000","830000"]],"#,
            r#""ts":1710000180000}"#
        );
        let event = parse_kline(payload).unwrap();

        match event {
            MarketEvent::Candle(candle) => {
                assert_eq!(
                    candle.open_time,
                    millis_to_utc(1_710_000_120_000).unwrap(),
                    "snapshot batch must yield the newest candle, not the oldest"
                );
                assert_eq!(candle.close, parse_decimal("68035.0").unwrap());
            }
            MarketEvent::OrderBook(_) => panic!("expected candle"),
        }
    }
}
