use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use futures_util::StreamExt;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use tokio::{sync::mpsc, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use trading_core::{
    Candle, ExchangeId, MarketEvent, OrderBookTop, PositionSide, Result, Side, Symbol, TradingError,
};

use crate::{
    AccountSnapshot, CancelAck, ExchangeAdapter, MarketOrderRequest, MarketStream, OrderAck,
    ProtectionAck, ProtectionOrderRequest,
};

pub struct BinanceAdapter {
    pub ws_base_url: String,
    pub rest_base_url: String,
    api_key: Option<String>,
    api_secret: Option<String>,
    recv_window_ms: u64,
    client: Client,
}

impl Default for BinanceAdapter {
    fn default() -> Self {
        Self {
            ws_base_url: "wss://stream.binancefuture.com".to_owned(),
            rest_base_url: "https://fapi.binance.com".to_owned(),
            api_key: None,
            api_secret: None,
            recv_window_ms: 5_000,
            client: Client::new(),
        }
    }
}

#[async_trait]
impl ExchangeAdapter for BinanceAdapter {
    fn exchange_id(&self) -> ExchangeId {
        ExchangeId::Binance
    }

    async fn subscribe_market_stream(&self, _symbols: &[Symbol]) -> Result<MarketStream> {
        let urls = self.market_stream_urls(_symbols);
        let (sender, receiver) = mpsc::channel(1024);

        for url in urls {
            let sender = sender.clone();
            tokio::spawn(async move {
                if let Err(error) = run_public_market_stream_with_reconnect(url, sender).await {
                    tracing::warn!(%error, "Binance market stream stopped");
                }
            });
        }

        if sender.is_closed() {
            return Err(TradingError::Exchange(
                "Binance market stream receiver closed during startup".to_owned(),
            ));
        }

        Ok(MarketStream::new(receiver))
    }

    async fn fetch_account_snapshot(&self) -> Result<AccountSnapshot> {
        let raw = self.get_signed("/fapi/v3/account", Vec::new()).await?;

        Ok(AccountSnapshot {
            exchange: ExchangeId::Binance,
            raw,
        })
    }

    async fn place_market_order(&self, request: MarketOrderRequest) -> Result<OrderAck> {
        let mut params = vec![
            ("symbol", request.symbol.as_str().to_owned()),
            ("side", binance_side(request.side).to_owned()),
            ("type", "MARKET".to_owned()),
            ("quantity", request.quantity.to_string()),
            ("newOrderRespType", "RESULT".to_owned()),
        ];
        if request.reduce_only {
            params.push(("reduceOnly", "true".to_owned()));
        }
        let raw = self.post_signed("/fapi/v1/order", params).await?;

        order_ack_from_value(raw)
    }

    async fn place_protection_orders(
        &self,
        request: ProtectionOrderRequest,
    ) -> Result<ProtectionAck> {
        let close_side = match request.position_side {
            PositionSide::Long => "SELL",
            PositionSide::Short => "BUY",
        };
        let stop_loss = self
            .post_signed(
                "/fapi/v1/order",
                vec![
                    ("symbol", request.symbol.as_str().to_owned()),
                    ("side", close_side.to_owned()),
                    ("type", "STOP_MARKET".to_owned()),
                    ("quantity", request.quantity.to_string()),
                    ("stopPrice", request.stop_loss_price.to_string()),
                    ("reduceOnly", "true".to_owned()),
                    ("workingType", "MARK_PRICE".to_owned()),
                    ("newOrderRespType", "RESULT".to_owned()),
                ],
            )
            .await?;
        let take_profit = self
            .post_signed(
                "/fapi/v1/order",
                vec![
                    ("symbol", request.symbol.as_str().to_owned()),
                    ("side", close_side.to_owned()),
                    ("type", "TAKE_PROFIT_MARKET".to_owned()),
                    ("quantity", request.quantity.to_string()),
                    ("stopPrice", request.take_profit_price.to_string()),
                    ("reduceOnly", "true".to_owned()),
                    ("workingType", "MARK_PRICE".to_owned()),
                    ("newOrderRespType", "RESULT".to_owned()),
                ],
            )
            .await?;

        Ok(ProtectionAck {
            stop_loss_order_id: order_id_from_value(&stop_loss),
            take_profit_order_id: order_id_from_value(&take_profit),
            raw: vec![stop_loss, take_profit],
        })
    }

    async fn cancel_order(&self, _order_id: String) -> Result<CancelAck> {
        Err(TradingError::Exchange(
            "Binance cancel order is not implemented yet".to_owned(),
        ))
    }
}

impl BinanceAdapter {
    pub fn testnet(api_key: String, api_secret: String) -> Self {
        Self {
            rest_base_url: "https://demo-fapi.binance.com".to_owned(),
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            ..Self::default()
        }
    }

    /// Fetches public `exchangeInfo` and returns lot-size/tick-size filters for
    /// the requested symbols. Unsigned (public) endpoint. Symbols whose filters
    /// are missing or malformed are simply absent from the returned map.
    pub async fn fetch_symbol_filters(
        &self,
        symbols: &[Symbol],
    ) -> Result<std::collections::HashMap<Symbol, SymbolFilters>> {
        let base = self.rest_base_url.trim_end_matches('/');
        let url = format!("{base}/fapi/v1/exchangeInfo");
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| TradingError::Exchange(error.to_string()))?;
        let raw = decode_binance_response(response).await?;

        Ok(parse_symbol_filters(&raw, symbols))
    }

    fn market_stream_urls(&self, symbols: &[Symbol]) -> Vec<String> {
        let kline_streams = symbols
            .iter()
            .map(|symbol| format!("{}@kline_1m", symbol.as_str().to_ascii_lowercase()))
            .collect::<Vec<_>>()
            .join("/");
        let book_ticker_streams = symbols
            .iter()
            .map(|symbol| format!("{}@bookTicker", symbol.as_str().to_ascii_lowercase()))
            .collect::<Vec<_>>()
            .join("/");

        let base = self.ws_base_url.trim_end_matches('/');
        let mut urls = Vec::with_capacity(2);

        if !kline_streams.is_empty() {
            urls.push(format!("{base}/stream?streams={kline_streams}"));
        }

        if !book_ticker_streams.is_empty() {
            urls.push(format!("{base}/stream?streams={book_ticker_streams}"));
        }

        urls
    }

    async fn get_signed(&self, path: &str, params: Vec<(&str, String)>) -> Result<Value> {
        let url = self.signed_url(path, params)?;
        let api_key = self.api_key()?;
        let response = self
            .client
            .get(url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(|error| TradingError::Exchange(error.to_string()))?;

        decode_binance_response(response).await
    }

    async fn post_signed(&self, path: &str, params: Vec<(&str, String)>) -> Result<Value> {
        let url = self.signed_url(path, params)?;
        let api_key = self.api_key()?;
        let response = self
            .client
            .post(url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(|error| TradingError::Exchange(error.to_string()))?;

        decode_binance_response(response).await
    }

    fn signed_url(&self, path: &str, mut params: Vec<(&str, String)>) -> Result<String> {
        let secret = self.api_secret()?;
        params.push(("recvWindow", self.recv_window_ms.to_string()));
        params.push(("timestamp", Utc::now().timestamp_millis().to_string()));
        let query = query_string(&params);
        let signature = hmac_sha256_hex(secret.as_bytes(), query.as_bytes());
        let base = self.rest_base_url.trim_end_matches('/');

        Ok(format!("{base}{path}?{query}&signature={signature}"))
    }

    fn api_key(&self) -> Result<&str> {
        self.api_key.as_deref().ok_or_else(|| {
            TradingError::Configuration("BINANCE_TESTNET_API_KEY is required".to_owned())
        })
    }

    fn api_secret(&self) -> Result<&str> {
        self.api_secret.as_deref().ok_or_else(|| {
            TradingError::Configuration("BINANCE_TESTNET_API_SECRET is required".to_owned())
        })
    }
}

async fn decode_binance_response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| TradingError::Exchange(error.to_string()))?;

    if !status.is_success() {
        return Err(TradingError::Exchange(format!(
            "Binance request failed with {status}: {body}"
        )));
    }

    serde_json::from_str(&body).map_err(|error| TradingError::Exchange(error.to_string()))
}

fn order_ack_from_value(raw: Value) -> Result<OrderAck> {
    let symbol = Symbol::new(required_string(&raw, "symbol")?);
    let side = match required_string(&raw, "side")? {
        "BUY" => Side::Buy,
        "SELL" => Side::Sell,
        other => {
            return Err(TradingError::Exchange(format!(
                "unsupported Binance order side: {other}"
            )))
        }
    };
    let average_price = decimal_field(&raw, "avgPrice")
        .or_else(|| decimal_field(&raw, "price"))
        .filter(|value| *value > Decimal::ZERO);

    Ok(OrderAck {
        exchange_order_id: order_id_from_value(&raw).unwrap_or_else(|| "unknown".to_owned()),
        symbol,
        side,
        status: required_string(&raw, "status")?.to_owned(),
        average_price,
        // Only report the actually-executed quantity. Never fall back to origQty:
        // a NEW/partial order would otherwise overstate fills and drive oversized
        // protection/flatten orders.
        executed_quantity: decimal_field(&raw, "executedQty").unwrap_or(Decimal::ZERO),
        raw,
    })
}

fn order_id_from_value(value: &Value) -> Option<String> {
    value
        .get("orderId")
        .and_then(|item| {
            item.as_i64()
                .map(|value| value.to_string())
                .or_else(|| item.as_str().map(str::to_owned))
        })
        .or_else(|| {
            value
                .get("algoId")
                .and_then(|item| item.as_i64().map(|value| value.to_string()))
        })
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| TradingError::Exchange(format!("missing Binance field: {key}")))
}

fn decimal_field(value: &Value, key: &str) -> Option<Decimal> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| Decimal::from_str(value).ok())
}

fn binance_side(side: Side) -> &'static str {
    match side {
        Side::Buy => "BUY",
        Side::Sell => "SELL",
    }
}

/// Per-symbol trading rules from Binance `exchangeInfo`, used to round order
/// quantity and price to the exchange's accepted increments before sending.
#[derive(Debug, Clone)]
pub struct SymbolFilters {
    pub step_size: Decimal,
    pub tick_size: Decimal,
    pub min_qty: Decimal,
    pub min_notional: Decimal,
}

impl SymbolFilters {
    /// Truncates `quantity` down to the nearest `step_size` multiple. Truncates
    /// (never rounds up) so a position is never larger than intended.
    pub fn round_quantity(&self, quantity: Decimal) -> Decimal {
        truncate_to_increment(quantity, self.step_size)
    }

    /// Rounds a protection (stop-loss / take-profit) price to a valid `tick_size`
    /// multiple in the conservative direction for the position side:
    /// - LONG  → round DOWN (floor): keeps the stop no tighter and the take no
    ///   further than the risk model intended.
    /// - SHORT → round UP (ceil): same conservative intent mirrored.
    ///
    /// For LONG both protection legs sit at-or-below the floor of their tick; for
    /// SHORT both sit at-or-above the ceil, so the stop never moves toward entry
    /// (weakening protection) and the take never moves away (becoming unreachable).
    pub fn round_protection_price(&self, price: Decimal, side: PositionSide) -> Decimal {
        match side {
            PositionSide::Long => truncate_to_increment(price, self.tick_size),
            PositionSide::Short => ceil_to_increment(price, self.tick_size),
        }
    }

    /// Returns true when `quantity` at `price` is a valid order: at or above the
    /// minimum quantity and minimum notional. Callers must round first.
    pub fn is_tradeable(&self, quantity: Decimal, price: Decimal) -> bool {
        quantity > Decimal::ZERO
            && quantity >= self.min_qty
            && quantity * price >= self.min_notional
    }
}

/// Truncates `value` down to the nearest multiple of `increment` (toward zero).
/// Returns `value` unchanged when `increment` is zero or negative.
fn truncate_to_increment(value: Decimal, increment: Decimal) -> Decimal {
    if increment <= Decimal::ZERO {
        return value;
    }
    (value / increment).floor() * increment
}

/// Rounds `value` up to the nearest multiple of `increment`.
/// Returns `value` unchanged when `increment` is zero or negative.
fn ceil_to_increment(value: Decimal, increment: Decimal) -> Decimal {
    if increment <= Decimal::ZERO {
        return value;
    }
    (value / increment).ceil() * increment
}

/// Parses `/fapi/v1/exchangeInfo` into per-symbol filters for the given symbols.
fn parse_symbol_filters(
    raw: &Value,
    wanted: &[Symbol],
) -> std::collections::HashMap<Symbol, SymbolFilters> {
    let mut filters = std::collections::HashMap::new();
    let Some(symbols) = raw.get("symbols").and_then(Value::as_array) else {
        return filters;
    };

    for entry in symbols {
        let Some(symbol_name) = entry.get("symbol").and_then(Value::as_str) else {
            continue;
        };
        let symbol = Symbol::new(symbol_name);
        // An empty `wanted` list means "all symbols" (used to cache the full
        // exchangeInfo at startup when the traded symbols are not known upfront).
        if !wanted.is_empty() && !wanted.iter().any(|s| s == &symbol) {
            continue;
        }

        let Some(filter_array) = entry.get("filters").and_then(Value::as_array) else {
            continue;
        };
        let mut step_size = None;
        let mut tick_size = None;
        let mut min_qty = None;
        let mut min_notional = None;

        for filter in filter_array {
            match filter.get("filterType").and_then(Value::as_str) {
                Some("LOT_SIZE") => {
                    step_size = decimal_field(filter, "stepSize");
                    min_qty = decimal_field(filter, "minQty");
                }
                Some("PRICE_FILTER") => {
                    tick_size = decimal_field(filter, "tickSize");
                }
                Some("MIN_NOTIONAL") => {
                    min_notional = decimal_field(filter, "notional")
                        .or_else(|| decimal_field(filter, "minNotional"));
                }
                _ => {}
            }
        }

        if let (Some(step_size), Some(tick_size)) = (step_size, tick_size) {
            filters.insert(
                symbol,
                SymbolFilters {
                    step_size,
                    tick_size,
                    min_qty: min_qty.unwrap_or(Decimal::ZERO),
                    min_notional: min_notional.unwrap_or(Decimal::ZERO),
                },
            );
        }
    }

    filters
}

fn query_string(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0_u8; BLOCK_SIZE];

    if key.len() > BLOCK_SIZE {
        let digest = Sha256::digest(key);
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut outer = [0x5c_u8; BLOCK_SIZE];
    let mut inner = [0x36_u8; BLOCK_SIZE];
    for index in 0..BLOCK_SIZE {
        outer[index] ^= key_block[index];
        inner[index] ^= key_block[index];
    }

    let mut inner_hash = Sha256::new();
    inner_hash.update(inner);
    inner_hash.update(message);
    let inner_result = inner_hash.finalize();

    let mut outer_hash = Sha256::new();
    outer_hash.update(outer);
    outer_hash.update(inner_result);
    hex_encode(&outer_hash.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn run_public_market_stream_with_reconnect(
    url: String,
    sender: mpsc::Sender<Result<trading_core::ObservedMarketEvent>>,
) -> Result<()> {
    let mut backoff = Duration::from_secs(1);

    loop {
        if sender.is_closed() {
            return Ok(());
        }

        match run_public_market_stream_once(&url, sender.clone()).await {
            Ok(()) => {
                backoff = Duration::from_secs(1);
            }
            Err(error) => {
                let _ = sender
                    .send(Err(TradingError::Exchange(format!(
                        "Binance market stream reconnecting after error: {error}"
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
    sender: mpsc::Sender<Result<trading_core::ObservedMarketEvent>>,
) -> Result<()> {
    let (stream, _) = connect_async(url).await.map_err(|error| {
        TradingError::Exchange(format!("Binance WebSocket connect failed: {error}"))
    })?;
    let (_, mut read) = stream.split();

    while let Some(message) = read.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let event = parse_market_payload(text.as_ref())?;
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
                        "Binance WebSocket read failed: {error}"
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

#[derive(Debug, Deserialize)]
struct BinanceKlineEnvelope {
    k: BinanceKline,
}

#[derive(Debug, Deserialize)]
struct BinanceKline {
    s: String,
    i: String,
    t: i64,
    o: Decimal,
    h: Decimal,
    l: Decimal,
    c: Decimal,
    v: Decimal,
}

#[derive(Debug, Deserialize)]
struct BinanceBookTicker {
    s: String,
    #[serde(rename = "E")]
    event_time: i64,
    b: Decimal,
    a: Decimal,
    #[serde(rename = "B")]
    bid_size: Decimal,
    #[serde(rename = "A")]
    ask_size: Decimal,
}

pub fn parse_market_payload(payload: &str) -> Result<MarketEvent> {
    let value: Value = serde_json::from_str(payload).map_err(|error| {
        TradingError::Exchange(format!("invalid Binance market payload JSON: {error}"))
    })?;
    let data = value.get("data").unwrap_or(&value);
    let event_type = data.get("e").and_then(Value::as_str).unwrap_or_default();
    let normalized = data.to_string();

    match event_type {
        "kline" => parse_kline(&normalized),
        "bookTicker" => parse_book_ticker(&normalized),
        other => Err(TradingError::Exchange(format!(
            "unsupported Binance market event type: {other}"
        ))),
    }
}

pub fn parse_kline(payload: &str) -> Result<MarketEvent> {
    let envelope: BinanceKlineEnvelope = serde_json::from_str(payload).map_err(|error| {
        TradingError::Exchange(format!("invalid Binance kline payload: {error}"))
    })?;
    let kline = envelope.k;

    Ok(MarketEvent::Candle(Candle {
        exchange: ExchangeId::Binance,
        symbol: Symbol::new(kline.s),
        timeframe: kline.i,
        open_time: millis_to_utc(kline.t)?,
        open: kline.o,
        high: kline.h,
        low: kline.l,
        close: kline.c,
        volume: kline.v,
    }))
}

pub fn parse_book_ticker(payload: &str) -> Result<MarketEvent> {
    let ticker: BinanceBookTicker = serde_json::from_str(payload).map_err(|error| {
        TradingError::Exchange(format!("invalid Binance book ticker payload: {error}"))
    })?;

    Ok(MarketEvent::OrderBook(OrderBookTop {
        exchange: ExchangeId::Binance,
        symbol: Symbol::new(ticker.s),
        event_time: millis_to_utc(ticker.event_time)?,
        best_bid: ticker.b,
        best_ask: ticker.a,
        bid_size: ticker.bid_size,
        ask_size: ticker.ask_size,
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
    fn parses_binance_kline_payload() {
        let payload = concat!(
            r#"{"e":"kline","E":1710000001000,"s":"BTCUSDT","k":{"#,
            r#""t":1710000000000,"s":"BTCUSDT","i":"1m","#,
            r#""o":"68000.1","c":"68010.2","h":"68020.3","#,
            r#""l":"67990.4","v":"12.5"}}"#
        );
        let event = parse_kline(payload).unwrap();

        match event {
            MarketEvent::Candle(candle) => {
                assert_eq!(candle.exchange, ExchangeId::Binance);
                assert_eq!(candle.symbol.as_str(), "BTCUSDT");
                assert_eq!(candle.timeframe, "1m");
            }
            MarketEvent::OrderBook(_) => panic!("expected candle"),
        }
    }

    #[test]
    fn parses_binance_combined_book_ticker_payload() {
        let payload = concat!(
            r#"{"stream":"btcusdt@bookTicker","data":{"#,
            r#""e":"bookTicker","u":1,"s":"BTCUSDT","#,
            r#""b":"68000.1","B":"2.1","a":"68001.2","A":"2.4","#,
            r#""E":1710000001000,"T":1710000001001}}"#
        );
        let event = parse_market_payload(payload).unwrap();

        match event {
            MarketEvent::OrderBook(order_book) => {
                assert_eq!(order_book.exchange, ExchangeId::Binance);
                assert_eq!(order_book.symbol.as_str(), "BTCUSDT");
            }
            MarketEvent::Candle(_) => panic!("expected order book"),
        }
    }

    #[test]
    fn builds_routed_stream_urls() {
        let adapter = BinanceAdapter::default();
        let urls = adapter.market_stream_urls(&[Symbol::new("BTCUSDT"), Symbol::new("ETHUSDT")]);

        assert_eq!(
            urls,
            vec![
                concat!(
                    "wss://stream.binancefuture.com/stream?streams=",
                    "btcusdt@kline_1m/ethusdt@kline_1m"
                ),
                concat!(
                    "wss://stream.binancefuture.com/stream?streams=",
                    "btcusdt@bookTicker/ethusdt@bookTicker"
                ),
            ]
        );
    }

    #[test]
    fn hmac_sha256_matches_known_test_vector() {
        assert_eq!(
            hmac_sha256_hex(b"key", b"The quick brown fox jumps over the lazy dog"),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn order_ack_uses_executed_quantity_when_present() {
        let raw = serde_json::json!({
            "symbol": "BTCUSDT",
            "side": "BUY",
            "status": "FILLED",
            "orderId": 123,
            "executedQty": "0.005",
            "origQty": "0.010",
            "avgPrice": "50000.0"
        });
        let ack = order_ack_from_value(raw).unwrap();
        assert_eq!(ack.executed_quantity, Decimal::from_str("0.005").unwrap());
    }

    #[test]
    fn order_ack_does_not_treat_ordered_quantity_as_filled() {
        // No executedQty: the order is not (yet) known to be filled. We must NOT
        // report origQty as executed, or partial/unfilled orders overstate fills
        // and drive oversized protection orders.
        let raw = serde_json::json!({
            "symbol": "BTCUSDT",
            "side": "BUY",
            "status": "NEW",
            "orderId": 123,
            "origQty": "0.010"
        });
        let ack = order_ack_from_value(raw).unwrap();
        assert_eq!(ack.executed_quantity, Decimal::ZERO);
    }

    fn btc_filters() -> SymbolFilters {
        SymbolFilters {
            step_size: Decimal::from_str("0.001").unwrap(),
            tick_size: Decimal::from_str("0.10").unwrap(),
            min_qty: Decimal::from_str("0.001").unwrap(),
            min_notional: Decimal::from_str("100").unwrap(),
        }
    }

    #[test]
    fn rounds_quantity_down_to_step_size() {
        let filters = btc_filters();
        // 0.0183745 truncates down to 0.018 (step 0.001), never up.
        assert_eq!(
            filters.round_quantity(Decimal::from_str("0.0183745").unwrap()),
            Decimal::from_str("0.018").unwrap()
        );
        // exact multiple is unchanged
        assert_eq!(
            filters.round_quantity(Decimal::from_str("0.020").unwrap()),
            Decimal::from_str("0.020").unwrap()
        );
        // below one step truncates to zero
        assert_eq!(
            filters.round_quantity(Decimal::from_str("0.0009").unwrap()),
            Decimal::ZERO
        );
    }

    #[test]
    fn long_protection_prices_round_down_to_tick() {
        let filters = btc_filters();
        // LONG: both stop (below entry) and take (above entry) round DOWN — stop
        // stays no tighter, take stays no further than intended.
        assert_eq!(
            filters
                .round_protection_price(Decimal::from_str("49500.17").unwrap(), PositionSide::Long),
            Decimal::from_str("49500.10").unwrap()
        );
        assert_eq!(
            filters
                .round_protection_price(Decimal::from_str("51000.17").unwrap(), PositionSide::Long),
            Decimal::from_str("51000.10").unwrap()
        );
    }

    #[test]
    fn short_protection_prices_round_up_to_tick() {
        let filters = btc_filters();
        // SHORT: both stop (above entry) and take (below entry) round UP — stop
        // does not move toward entry (no weaker protection); take stays reachable.
        assert_eq!(
            filters.round_protection_price(
                Decimal::from_str("50500.13").unwrap(),
                PositionSide::Short
            ),
            Decimal::from_str("50500.20").unwrap()
        );
        assert_eq!(
            filters.round_protection_price(
                Decimal::from_str("49000.13").unwrap(),
                PositionSide::Short
            ),
            Decimal::from_str("49000.20").unwrap()
        );
    }

    #[test]
    fn protection_price_on_exact_tick_is_unchanged() {
        let filters = btc_filters();
        assert_eq!(
            filters
                .round_protection_price(Decimal::from_str("50000.10").unwrap(), PositionSide::Long),
            Decimal::from_str("50000.10").unwrap()
        );
        assert_eq!(
            filters.round_protection_price(
                Decimal::from_str("50000.10").unwrap(),
                PositionSide::Short
            ),
            Decimal::from_str("50000.10").unwrap()
        );
    }

    #[test]
    fn truncate_with_zero_increment_is_identity() {
        assert_eq!(
            truncate_to_increment(Decimal::from_str("1.2345").unwrap(), Decimal::ZERO),
            Decimal::from_str("1.2345").unwrap()
        );
    }

    #[test]
    fn is_tradeable_enforces_min_qty_and_min_notional() {
        let filters = btc_filters();
        // 0.001 BTC @ 50000 = 50 notional < 100 min_notional -> not tradeable
        assert!(!filters.is_tradeable(
            Decimal::from_str("0.001").unwrap(),
            Decimal::from_str("50000").unwrap()
        ));
        // 0.003 BTC @ 50000 = 150 >= 100 and qty >= min_qty -> tradeable
        assert!(filters.is_tradeable(
            Decimal::from_str("0.003").unwrap(),
            Decimal::from_str("50000").unwrap()
        ));
        // zero qty is never tradeable
        assert!(!filters.is_tradeable(Decimal::ZERO, Decimal::from_str("50000").unwrap()));
    }

    #[test]
    fn parses_exchange_info_filters() {
        let raw = serde_json::json!({
            "symbols": [
                {
                    "symbol": "BTCUSDT",
                    "filters": [
                        { "filterType": "PRICE_FILTER", "tickSize": "0.10" },
                        { "filterType": "LOT_SIZE", "stepSize": "0.001", "minQty": "0.001" },
                        { "filterType": "MIN_NOTIONAL", "notional": "100" }
                    ]
                },
                {
                    "symbol": "ETHUSDT",
                    "filters": [
                        { "filterType": "PRICE_FILTER", "tickSize": "0.01" },
                        { "filterType": "LOT_SIZE", "stepSize": "0.01", "minQty": "0.01" },
                        { "filterType": "MIN_NOTIONAL", "notional": "20" }
                    ]
                }
            ]
        });
        let filters = parse_symbol_filters(&raw, &[Symbol::new("BTCUSDT")]);
        assert_eq!(filters.len(), 1, "only requested symbols are returned");
        let btc = filters.get(&Symbol::new("BTCUSDT")).unwrap();
        assert_eq!(btc.step_size, Decimal::from_str("0.001").unwrap());
        assert_eq!(btc.tick_size, Decimal::from_str("0.10").unwrap());
        assert_eq!(btc.min_qty, Decimal::from_str("0.001").unwrap());
        assert_eq!(btc.min_notional, Decimal::from_str("100").unwrap());

        // Empty `wanted` returns all symbols.
        let all = parse_symbol_filters(&raw, &[]);
        assert_eq!(all.len(), 2);
        assert!(all.contains_key(&Symbol::new("ETHUSDT")));
    }
}
