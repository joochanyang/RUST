use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::{sync::mpsc, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use trading_core::{ExchangeId, MarketEvent, OrderBookTop, Result, Symbol, TradingError};

use crate::{
    AccountSnapshot, CancelAck, ExchangeAdapter, MarketOrderRequest, MarketStream, OrderAck,
    ProtectionAck, ProtectionOrderRequest,
};

pub struct BybitAdapter {
    pub ws_base_url: String,
    pub rest_base_url: String,
}

impl Default for BybitAdapter {
    fn default() -> Self {
        Self {
            ws_base_url: "wss://stream.bybit.com/v5/public/linear".to_owned(),
            rest_base_url: "https://api.bybit.com".to_owned(),
        }
    }
}

#[async_trait]
impl ExchangeAdapter for BybitAdapter {
    fn exchange_id(&self) -> ExchangeId {
        ExchangeId::Bybit
    }

    async fn subscribe_market_stream(&self, symbols: &[Symbol]) -> Result<MarketStream> {
        let topics = symbols
            .iter()
            .flat_map(|symbol| {
                [
                    format!("tickers.{}", symbol.as_str()),
                    format!("kline.1.{}", symbol.as_str()),
                ]
            })
            .collect::<Vec<_>>();
        let (sender, receiver) = mpsc::channel(1024);
        let url = self.ws_base_url.clone();

        tokio::spawn(async move {
            if let Err(error) = run_public_market_stream_with_reconnect(url, topics, sender).await {
                tracing::warn!(%error, "Bybit market stream stopped");
            }
        });

        Ok(MarketStream::new(receiver))
    }

    async fn fetch_account_snapshot(&self) -> Result<AccountSnapshot> {
        Err(TradingError::Exchange(
            "Bybit account snapshot is not implemented yet".to_owned(),
        ))
    }

    async fn place_market_order(&self, _request: MarketOrderRequest) -> Result<OrderAck> {
        Err(TradingError::Exchange(
            "Bybit live order routing is not implemented yet".to_owned(),
        ))
    }

    async fn place_protection_orders(
        &self,
        _request: ProtectionOrderRequest,
    ) -> Result<ProtectionAck> {
        Err(TradingError::Exchange(
            "Bybit protection orders are not implemented yet".to_owned(),
        ))
    }

    async fn cancel_order(&self, _order_id: String) -> Result<CancelAck> {
        Err(TradingError::Exchange(
            "Bybit cancel order is not implemented yet".to_owned(),
        ))
    }
}

async fn run_public_market_stream_with_reconnect(
    url: String,
    topics: Vec<String>,
    sender: mpsc::Sender<Result<trading_core::ObservedMarketEvent>>,
) -> Result<()> {
    let mut backoff = Duration::from_secs(1);

    loop {
        if sender.is_closed() {
            return Ok(());
        }

        match run_public_market_stream_once(&url, &topics, sender.clone()).await {
            Ok(()) => {
                backoff = Duration::from_secs(1);
            }
            Err(error) => {
                let _ = sender
                    .send(Err(TradingError::Exchange(format!(
                        "Bybit market stream reconnecting after error: {error}"
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
    topics: &[String],
    sender: mpsc::Sender<Result<trading_core::ObservedMarketEvent>>,
) -> Result<()> {
    let (stream, _) = connect_async(url).await.map_err(|error| {
        TradingError::Exchange(format!("Bybit WebSocket connect failed: {error}"))
    })?;
    let (mut write, mut read) = stream.split();
    let subscribe = json!({ "op": "subscribe", "args": topics });
    write
        .send(Message::Text(subscribe.to_string().into()))
        .await
        .map_err(|error| TradingError::Exchange(format!("Bybit subscribe failed: {error}")))?;
    let mut ticker_state = HashMap::new();

    while let Some(message) = read.next().await {
        match message {
            Ok(Message::Text(text)) => {
                if !is_market_payload(text.as_ref()) {
                    continue;
                }
                let event = parse_market_payload_with_state(text.as_ref(), &mut ticker_state)?;
                let observed = trading_core::ObservedMarketEvent::new(event, Utc::now());
                if sender.send(Ok(observed)).await.is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => {
                let send_result = sender
                    .send(Err(TradingError::Exchange(format!(
                        "Bybit WebSocket read failed: {error}"
                    ))))
                    .await;
                if send_result.is_err() {
                    break;
                }
            }
        }
    }

    Ok(())
}

fn is_market_payload(payload: &str) -> bool {
    serde_json::from_str::<Value>(payload)
        .ok()
        .is_some_and(|value| {
            value.get("topic").and_then(Value::as_str).is_some() && value.get("data").is_some()
        })
}

pub fn parse_market_payload(payload: &str) -> Result<MarketEvent> {
    parse_market_payload_with_state(payload, &mut HashMap::new())
}

fn parse_market_payload_with_state(
    payload: &str,
    ticker_state: &mut HashMap<String, OrderBookTop>,
) -> Result<MarketEvent> {
    let value: Value = serde_json::from_str(payload)
        .map_err(|error| TradingError::Exchange(format!("invalid Bybit payload JSON: {error}")))?;
    let topic = value
        .get("topic")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if topic.starts_with("tickers.") {
        return parse_ticker_with_state(payload, ticker_state);
    }

    if topic.starts_with("kline.") {
        return parse_kline(payload);
    }

    Err(TradingError::Exchange(format!(
        "unsupported Bybit market topic: {topic}"
    )))
}

#[derive(Debug, Deserialize)]
struct BybitTickerEnvelope {
    ts: i64,
    data: BybitTicker,
}

#[derive(Debug, Deserialize)]
struct BybitTicker {
    symbol: String,
    #[serde(rename = "bid1Price")]
    bid_price: Option<Decimal>,
    #[serde(rename = "ask1Price")]
    ask_price: Option<Decimal>,
    #[serde(rename = "bid1Size")]
    bid_size: Option<Decimal>,
    #[serde(rename = "ask1Size")]
    ask_size: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BybitKlineEnvelope {
    topic: String,
    data: Vec<BybitKline>,
}

#[derive(Debug, Deserialize)]
struct BybitKline {
    start: i64,
    interval: String,
    open: Decimal,
    close: Decimal,
    high: Decimal,
    low: Decimal,
    volume: Decimal,
}

pub fn parse_ticker(payload: &str) -> Result<MarketEvent> {
    parse_ticker_with_state(payload, &mut HashMap::new())
}

fn parse_ticker_with_state(
    payload: &str,
    ticker_state: &mut HashMap<String, OrderBookTop>,
) -> Result<MarketEvent> {
    let envelope: BybitTickerEnvelope = serde_json::from_str(payload).map_err(|error| {
        TradingError::Exchange(format!("invalid Bybit ticker payload: {error}"))
    })?;
    let previous = ticker_state.get(&envelope.data.symbol);
    let symbol = Symbol::new(envelope.data.symbol.clone());
    let best_bid = envelope
        .data
        .bid_price
        .or_else(|| previous.map(|item| item.best_bid))
        .ok_or_else(|| TradingError::Exchange("Bybit ticker missing bid price".to_owned()))?;
    let best_ask = envelope
        .data
        .ask_price
        .or_else(|| previous.map(|item| item.best_ask))
        .ok_or_else(|| TradingError::Exchange("Bybit ticker missing ask price".to_owned()))?;
    let bid_size = envelope
        .data
        .bid_size
        .or_else(|| previous.map(|item| item.bid_size))
        .ok_or_else(|| TradingError::Exchange("Bybit ticker missing bid size".to_owned()))?;
    let ask_size = envelope
        .data
        .ask_size
        .or_else(|| previous.map(|item| item.ask_size))
        .ok_or_else(|| TradingError::Exchange("Bybit ticker missing ask size".to_owned()))?;

    let order_book = OrderBookTop {
        exchange: ExchangeId::Bybit,
        symbol,
        event_time: millis_to_utc(envelope.ts)?,
        best_bid,
        best_ask,
        bid_size,
        ask_size,
    };
    ticker_state.insert(order_book.symbol.to_string(), order_book.clone());

    Ok(MarketEvent::OrderBook(order_book))
}

pub fn parse_kline(payload: &str) -> Result<MarketEvent> {
    let envelope: BybitKlineEnvelope = serde_json::from_str(payload)
        .map_err(|error| TradingError::Exchange(format!("invalid Bybit kline payload: {error}")))?;
    let kline = envelope
        .data
        .into_iter()
        .next()
        .ok_or_else(|| TradingError::Exchange("Bybit kline payload has no data".to_owned()))?;
    let symbol = envelope
        .topic
        .rsplit('.')
        .next()
        .ok_or_else(|| TradingError::Exchange("Bybit kline topic has no symbol".to_owned()))?;

    Ok(MarketEvent::Candle(trading_core::Candle {
        exchange: ExchangeId::Bybit,
        symbol: Symbol::new(symbol),
        timeframe: kline.interval,
        open_time: millis_to_utc(kline.start)?,
        open: kline.open,
        high: kline.high,
        low: kline.low,
        close: kline.close,
        volume: kline.volume,
    }))
}

fn millis_to_utc(value: i64) -> Result<chrono::DateTime<Utc>> {
    Utc.timestamp_millis_opt(value)
        .single()
        .ok_or_else(|| TradingError::Exchange(format!("invalid millisecond timestamp: {value}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bybit_ticker_payload() {
        let payload = concat!(
            r#"{"topic":"tickers.BTCUSDT","ts":1710000001000,"#,
            r#""type":"snapshot","data":{"symbol":"BTCUSDT","#,
            r#""bid1Price":"68000.1","ask1Price":"68001.2","#,
            r#""bid1Size":"2.1","ask1Size":"2.4"}}"#
        );
        let event = parse_ticker(payload).unwrap();

        match event {
            MarketEvent::OrderBook(order_book) => {
                assert_eq!(order_book.exchange, ExchangeId::Bybit);
                assert_eq!(order_book.symbol.as_str(), "BTCUSDT");
            }
            MarketEvent::Candle(_) => panic!("expected order book"),
        }
    }

    #[test]
    fn ignores_bybit_subscription_ack() {
        let payload = r#"{"success":true,"ret_msg":"","conn_id":"abc","op":"subscribe"}"#;
        assert!(!is_market_payload(payload));
    }

    #[test]
    fn parses_bybit_kline_payload() {
        let payload = concat!(
            r#"{"topic":"kline.1.BTCUSDT","type":"snapshot","ts":1710000001000,"data":[{"#,
            r#""start":1710000000000,"end":1710000059999,"interval":"1","#,
            r#""open":"68000.1","close":"68010.2","high":"68020.3","#,
            r#""low":"67990.4","volume":"12.5","turnover":"850000","#,
            r#""confirm":false,"timestamp":1710000001000}]}"#
        );
        let event = parse_market_payload(payload).unwrap();

        match event {
            MarketEvent::Candle(candle) => {
                assert_eq!(candle.exchange, ExchangeId::Bybit);
                assert_eq!(candle.symbol.as_str(), "BTCUSDT");
                assert_eq!(candle.timeframe, "1");
            }
            MarketEvent::OrderBook(_) => panic!("expected candle"),
        }
    }

    #[test]
    fn merges_bybit_ticker_delta_with_previous_snapshot() {
        let snapshot = concat!(
            r#"{"topic":"tickers.BTCUSDT","ts":1710000001000,"#,
            r#""type":"snapshot","data":{"symbol":"BTCUSDT","#,
            r#""bid1Price":"68000.1","ask1Price":"68001.2","#,
            r#""bid1Size":"2.1","ask1Size":"2.4"}}"#
        );
        let delta = concat!(
            r#"{"topic":"tickers.BTCUSDT","ts":1710000002000,"#,
            r#""type":"delta","data":{"symbol":"BTCUSDT","bid1Price":"68005.5"}}"#
        );
        let mut state = HashMap::new();
        parse_market_payload_with_state(snapshot, &mut state).unwrap();
        let event = parse_market_payload_with_state(delta, &mut state).unwrap();

        match event {
            MarketEvent::OrderBook(order_book) => {
                assert_eq!(order_book.symbol.as_str(), "BTCUSDT");
                assert_eq!(order_book.best_bid.to_string(), "68005.5");
                assert_eq!(order_book.best_ask.to_string(), "68001.2");
            }
            MarketEvent::Candle(_) => panic!("expected order book"),
        }
    }
}
