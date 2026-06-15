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

/// Wall-clock cap on a single REST request. Bounds how long a stuck order or
/// account call can block the strategy loop before surfacing an error.
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum gap between market-data frames before the WebSocket is treated as
/// stalled and reconnected. Binance pushes bookTicker/kline updates for active
/// symbols far more often than this and sends a ping at least every few minutes,
/// so a silent gap this long means the connection has half-opened. Without this
/// bound a silent stall hangs `read.next()` forever and the strategy freezes on
/// stale data (the latency gate then blocks all entries).
const MARKET_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Builds the REST client used for all signed/unsigned Binance calls with a
/// bounded request timeout. A request must never hang the strategy loop
/// indefinitely; falling back to `Client::new()` (no timeout) is unsafe here.
fn http_client() -> Client {
    Client::builder()
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| Client::new())
}

impl Default for BinanceAdapter {
    fn default() -> Self {
        Self {
            ws_base_url: "wss://fstream.binance.com".to_owned(),
            rest_base_url: "https://fapi.binance.com".to_owned(),
            api_key: None,
            api_secret: None,
            recv_window_ms: 5_000,
            client: http_client(),
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
            // Only the bookTicker socket carries order-book frames; the kline
            // socket carries none, so it must NOT arm the order-book staleness
            // clock or it would reconnect every cycle on a healthy feed.
            let expects_orderbook = url.contains("bookTicker");
            tokio::spawn(async move {
                if let Err(error) =
                    run_public_market_stream_with_reconnect(url, sender, expects_orderbook).await
                {
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
        let params = market_order_params(&request);
        let raw = self.post_signed("/fapi/v1/order", params).await?;

        order_ack_from_value(raw)
    }

    async fn place_protection_orders(
        &self,
        request: ProtectionOrderRequest,
    ) -> Result<ProtectionAck> {
        let stop_loss = match self
            .post_signed(ALGO_ORDER_PATH, stop_loss_algo_params(&request))
            .await
        {
            Ok(stop_loss) => stop_loss,
            Err(error) => {
                let cancel_summary = if matches!(error, TradingError::Timeout(_)) {
                    Some(
                        self.cancel_algo_candidates(
                            "stop-loss",
                            vec![request.stop_loss_client_algo_id.clone()],
                        )
                        .await,
                    )
                } else {
                    None
                };
                return Err(protection_failure_error("stop-loss", error, cancel_summary));
            }
        };
        let take_profit = match self
            .post_signed(ALGO_ORDER_PATH, take_profit_algo_params(&request))
            .await
        {
            Ok(take_profit) => take_profit,
            Err(error) => {
                let mut cancel_summaries = vec![
                    self.cancel_algo_candidates(
                        "stop-loss",
                        protection_cancel_candidates(&request.stop_loss_client_algo_id, &stop_loss),
                    )
                    .await,
                ];
                if matches!(error, TradingError::Timeout(_)) {
                    cancel_summaries.push(
                        self.cancel_algo_candidates(
                            "take-profit",
                            vec![request.take_profit_client_algo_id.clone()],
                        )
                        .await,
                    );
                }

                return Err(protection_failure_error(
                    "take-profit",
                    error,
                    Some(cancel_summaries.join("; ")),
                ));
            }
        };

        Ok(ProtectionAck {
            stop_loss_order_id: order_id_from_value(&stop_loss),
            take_profit_order_id: order_id_from_value(&take_profit),
            raw: vec![stop_loss, take_profit],
        })
    }

    async fn cancel_order(&self, order_id: String) -> Result<CancelAck> {
        let raw = self
            .delete_signed(ALGO_ORDER_PATH, cancel_algo_order_params(&order_id))
            .await?;
        Ok(CancelAck { raw })
    }

    async fn query_order(
        &self,
        symbol: &Symbol,
        client_order_id: &str,
    ) -> Result<Option<OrderAck>> {
        let response = self
            .get_signed(
                "/fapi/v1/order",
                vec![
                    ("symbol", symbol.as_str().to_owned()),
                    ("origClientOrderId", client_order_id.to_owned()),
                ],
            )
            .await;
        query_order_result(response)
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
            .map_err(|error| TradingError::Exchange(reqwest_error_message(error)))?;
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
            urls.push(format!("{base}/market/stream?streams={kline_streams}"));
        }

        if !book_ticker_streams.is_empty() {
            urls.push(format!(
                "{base}/public/stream?streams={book_ticker_streams}"
            ));
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
            .map_err(map_request_error)?;

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
            .map_err(map_request_error)?;

        decode_binance_response(response).await
    }

    async fn delete_signed(&self, path: &str, params: Vec<(&str, String)>) -> Result<Value> {
        let url = self.signed_url(path, params)?;
        let api_key = self.api_key()?;
        let response = self
            .client
            .delete(url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(map_request_error)?;

        decode_binance_response(response).await
    }

    async fn cancel_algo_candidates(&self, label: &str, candidates: Vec<Option<String>>) -> String {
        let mut ids = Vec::new();
        for candidate in candidates.into_iter().flatten() {
            if !candidate.is_empty() && !ids.contains(&candidate) {
                ids.push(candidate);
            }
        }

        if ids.is_empty() {
            return format!("{label} cancel skipped: no algo id");
        }

        let mut errors = Vec::new();
        for id in ids {
            match self.cancel_order(id.clone()).await {
                Ok(_) => return format!("{label} cancel ok via {id}"),
                Err(error) => errors.push(format!("{id}: {error}")),
            }
        }

        format!("{label} cancel failed ({})", errors.join(", "))
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
        .map_err(|error| TradingError::Exchange(reqwest_error_message(error)))?;

    if !status.is_success() {
        return Err(TradingError::Exchange(format!(
            "Binance request failed with {status}: {body}"
        )));
    }

    serde_json::from_str(&body).map_err(|error| TradingError::Exchange(error.to_string()))
}

/// Binance error code for a query against an order that does not exist.
const ORDER_NOT_FOUND_CODE: i64 = -2013;

/// Classifies the result of a `GET /fapi/v1/order` lookup into:
/// - `Ok(Some(ack))` — the order exists (caller inspects `ack.status`),
/// - `Ok(None)` — the order does not exist on the exchange (code -2013),
/// - `Err(_)` — the query itself failed, so the order's existence is UNKNOWN.
///
/// Separating "does not exist" from "query failed" is the whole point: only the
/// former is safe to treat as "no position".
fn query_order_result(response: Result<Value>) -> Result<Option<OrderAck>> {
    match response {
        Ok(raw) => order_ack_from_value(raw).map(Some),
        Err(error) if is_order_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Detects Binance's "Order does not exist" (-2013) response embedded in a failed
/// request's error message body.
fn is_order_not_found(error: &TradingError) -> bool {
    let TradingError::Exchange(message) = error else {
        return false;
    };
    // The error message contains the raw JSON body, e.g.
    // `{"code":-2013,"msg":"Order does not exist."}`. Find and parse it.
    let Some(start) = message.find('{') else {
        return false;
    };
    serde_json::from_str::<Value>(&message[start..])
        .ok()
        .and_then(|body| body.get("code").and_then(Value::as_i64))
        == Some(ORDER_NOT_FOUND_CODE)
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

fn client_algo_id_from_value(value: &Value) -> Option<String> {
    value
        .get("clientAlgoId")
        .and_then(Value::as_str)
        .map(str::to_owned)
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

/// Maps a transport-level request error to a `TradingError`, distinguishing a
/// timeout (remote outcome unknown — the order may have executed) from a generic
/// exchange/transport failure. Callers branch on `Timeout` to avoid treating a
/// possibly-filled order as a definitive failure.
fn map_request_error(error: reqwest::Error) -> TradingError {
    if error.is_timeout() {
        TradingError::Timeout(reqwest_error_message(error))
    } else {
        TradingError::Exchange(reqwest_error_message(error))
    }
}

fn reqwest_error_message(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

/// Builds the signed-request parameters for a market order. Extracted so the
/// parameter wiring (reduce-only flag, optional idempotency key) is unit-testable
/// without a network round-trip.
/// The endpoint for conditional (stop-loss / take-profit) orders. Binance moved
/// these off `/fapi/v1/order` to the Algo service on 2025-12-09; the old path
/// now returns -4120 for STOP_MARKET / TAKE_PROFIT_MARKET.
const ALGO_ORDER_PATH: &str = "/fapi/v1/algoOrder";

fn protection_close_side(position_side: PositionSide) -> &'static str {
    match position_side {
        PositionSide::Long => "SELL",
        PositionSide::Short => "BUY",
    }
}

fn stop_loss_algo_params(request: &ProtectionOrderRequest) -> Vec<(&'static str, String)> {
    conditional_algo_params(
        request,
        "STOP_MARKET",
        request.stop_loss_price,
        request.stop_loss_client_algo_id.as_deref(),
    )
}

fn take_profit_algo_params(request: &ProtectionOrderRequest) -> Vec<(&'static str, String)> {
    conditional_algo_params(
        request,
        "TAKE_PROFIT_MARKET",
        request.take_profit_price,
        request.take_profit_client_algo_id.as_deref(),
    )
}

/// Builds the algo-order params shared by both protection legs. The algo
/// endpoint requires `algoType=CONDITIONAL` and uses `triggerPrice` in place of
/// the legacy `stopPrice`.
fn conditional_algo_params(
    request: &ProtectionOrderRequest,
    order_type: &'static str,
    trigger_price: Decimal,
    client_algo_id: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut params = vec![
        ("algoType", "CONDITIONAL".to_owned()),
        ("symbol", request.symbol.as_str().to_owned()),
        (
            "side",
            protection_close_side(request.position_side).to_owned(),
        ),
        ("type", order_type.to_owned()),
        ("quantity", request.quantity.to_string()),
        ("triggerPrice", trigger_price.to_string()),
        ("reduceOnly", "true".to_owned()),
        ("workingType", "MARK_PRICE".to_owned()),
        ("newOrderRespType", "RESULT".to_owned()),
    ];
    if let Some(client_algo_id) = client_algo_id {
        params.push(("clientAlgoId", client_algo_id.to_owned()));
    }
    params
}

fn cancel_algo_order_params(order_id: &str) -> Vec<(&'static str, String)> {
    if order_id.chars().all(|character| character.is_ascii_digit()) {
        vec![("algoId", order_id.to_owned())]
    } else {
        vec![("clientAlgoId", order_id.to_owned())]
    }
}

fn protection_cancel_candidates(
    client_algo_id: &Option<String>,
    raw: &Value,
) -> Vec<Option<String>> {
    vec![
        client_algo_id.clone(),
        client_algo_id_from_value(raw),
        order_id_from_value(raw),
    ]
}

fn protection_failure_error(
    failed_leg: &str,
    error: TradingError,
    cancel_summary: Option<String>,
) -> TradingError {
    let mut message = format!("Binance {failed_leg} protection order failed: {error}");
    if let Some(cancel_summary) = cancel_summary {
        message.push_str("; compensation: ");
        message.push_str(&cancel_summary);
    }
    TradingError::Exchange(message)
}

fn market_order_params(request: &MarketOrderRequest) -> Vec<(&'static str, String)> {
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
    if let Some(client_order_id) = &request.client_order_id {
        params.push(("newClientOrderId", client_order_id.clone()));
    }
    params
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
    expects_orderbook: bool,
) -> Result<()> {
    let mut backoff = Duration::from_secs(1);

    loop {
        if sender.is_closed() {
            return Ok(());
        }

        match run_public_market_stream_once(
            &url,
            sender.clone(),
            MARKET_STREAM_IDLE_TIMEOUT,
            expects_orderbook,
        )
        .await
        {
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
    idle_timeout: Duration,
    expects_orderbook: bool,
) -> Result<()> {
    let (stream, _) = connect_async(url).await.map_err(|error| {
        TradingError::Exchange(format!("Binance WebSocket connect failed: {error}"))
    })?;
    let (_, mut read) = stream.split();

    // Receive-time of the most recent order-book frame on THIS connection. The
    // whole-socket idle timeout below resets on any frame (ping/kline included),
    // so it cannot see a bookTicker-only stall; tracking the order-book gap
    // separately is what catches that production failure mode.
    //
    // Anchored to the connect instant on order-book-bearing sockets so a feed
    // that NEVER (re)starts order-book frames — while kline/pings keep the socket
    // alive past the idle timeout — is still caught (the partial-stall bug would
    // otherwise silently reappear one reconnect later). The kline-only socket
    // passes `false` and stays `None`, leaving its check inert (the whole-socket
    // idle timeout guards it) so a healthy kline feed never false-reconnects.
    let mut last_orderbook_at: Option<chrono::DateTime<Utc>> = expects_orderbook.then(Utc::now);
    let staleness = chrono::Duration::seconds(trading_core::ORDERBOOK_STREAM_STALENESS_SECS);

    loop {
        // A silent connection (no data, no ping, no close) must not hang the loop
        // forever: time the read out so the reconnect loop can re-establish it.
        let message = match tokio::time::timeout(idle_timeout, read.next()).await {
            Ok(Some(message)) => message,
            // Stream ended cleanly.
            Ok(None) => return Ok(()),
            // No frame within the idle window: treat as a stalled connection and
            // surface an error so the caller reconnects.
            Err(_elapsed) => {
                return Err(TradingError::Exchange(format!(
                    "Binance WebSocket stalled: no frame within {idle_timeout:?}"
                )));
            }
        };

        match message {
            Ok(Message::Text(text)) => {
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
                    "Binance WebSocket read failed: {error}"
                )));
            }
        }

        // Partial stall: order-book frames stopped while other frames keep the
        // socket alive. Surface an error so the caller reconnects rather than
        // streaming stale data the latency gate would silently block on.
        if trading_core::orderbook_stream_is_stale(last_orderbook_at, Utc::now(), staleness) {
            return Err(TradingError::Exchange(format!(
                "Binance order-book stream stalled: no order-book frame within {staleness}"
            )));
        }
    }
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
                    "wss://fstream.binance.com/market/stream?streams=",
                    "btcusdt@kline_1m/ethusdt@kline_1m"
                ),
                concat!(
                    "wss://fstream.binance.com/public/stream?streams=",
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

    fn market_request(reduce_only: bool, client_order_id: Option<&str>) -> MarketOrderRequest {
        MarketOrderRequest {
            symbol: Symbol::new("BTCUSDT"),
            side: Side::Buy,
            quantity: Decimal::new(3, 2),
            reduce_only,
            client_order_id: client_order_id.map(str::to_owned),
        }
    }

    fn param<'a>(params: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        params
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn query_order_result_parses_existing_order() {
        let raw = serde_json::json!({
            "symbol": "BTCUSDT",
            "side": "BUY",
            "status": "FILLED",
            "orderId": 42,
            "avgPrice": "50000.0",
            "executedQty": "0.03",
        });
        let result = query_order_result(Ok(raw)).expect("ok");
        let ack = result.expect("an existing order must parse to Some");
        assert_eq!(ack.status, "FILLED");
        assert_eq!(ack.executed_quantity, Decimal::new(3, 2));
    }

    #[test]
    fn query_order_result_maps_order_not_found_to_none() {
        // Binance returns HTTP 400 with code -2013 when the order does not exist.
        let not_found = TradingError::Exchange(
            "Binance request failed with 400 Bad Request: {\"code\":-2013,\"msg\":\"Order does not exist.\"}"
                .to_owned(),
        );
        let result = query_order_result(Err(not_found)).expect("not-found must be Ok(None)");
        assert!(
            result.is_none(),
            "a non-existent order must map to Ok(None), not an error"
        );
    }

    #[test]
    fn query_order_result_propagates_other_errors() {
        let other = TradingError::Timeout("request timed out".to_owned());
        let result = query_order_result(Err(other));
        assert!(
            result.is_err(),
            "a query that itself failed must surface an error (unknown outcome)"
        );
    }

    #[test]
    fn market_order_params_include_client_order_id_when_present() {
        let params = market_order_params(&market_request(false, Some("sig-abc")));
        assert_eq!(
            param(&params, "newClientOrderId"),
            Some("sig-abc"),
            "a deterministic client order id must be sent for idempotency"
        );
    }

    #[test]
    fn market_order_params_omit_client_order_id_when_absent() {
        let params = market_order_params(&market_request(false, None));
        assert_eq!(
            param(&params, "newClientOrderId"),
            None,
            "no client order id field should be sent when none is provided"
        );
    }

    #[test]
    fn market_order_params_include_reduce_only_only_when_set() {
        let with = market_order_params(&market_request(true, None));
        assert_eq!(param(&with, "reduceOnly"), Some("true"));
        let without = market_order_params(&market_request(false, None));
        assert_eq!(param(&without, "reduceOnly"), None);
    }

    fn protection_request() -> ProtectionOrderRequest {
        ProtectionOrderRequest {
            symbol: Symbol::new("ETHUSDT"),
            position_side: PositionSide::Long,
            quantity: Decimal::new(1, 2),
            stop_loss_price: Decimal::new(1000, 0),
            take_profit_price: Decimal::new(9000, 0),
            stop_loss_client_algo_id: Some("entryabc-sl".to_owned()),
            take_profit_client_algo_id: Some("entryabc-tp".to_owned()),
        }
    }

    // Binance migrated conditional orders (STOP_MARKET / TAKE_PROFIT_MARKET) to
    // the Algo Order service on 2025-12-09; the old /fapi/v1/order endpoint now
    // rejects them with -4120. The params must therefore carry algoType and use
    // triggerPrice (not stopPrice), which is what /fapi/v1/algoOrder expects.
    #[test]
    fn stop_loss_algo_params_use_trigger_price_and_conditional_type() {
        let params = stop_loss_algo_params(&protection_request());
        assert_eq!(param(&params, "algoType"), Some("CONDITIONAL"));
        assert_eq!(param(&params, "type"), Some("STOP_MARKET"));
        assert_eq!(param(&params, "side"), Some("SELL"));
        assert_eq!(param(&params, "triggerPrice"), Some("1000"));
        assert_eq!(param(&params, "reduceOnly"), Some("true"));
        assert_eq!(param(&params, "clientAlgoId"), Some("entryabc-sl"));
        assert_eq!(
            param(&params, "stopPrice"),
            None,
            "the algo endpoint uses triggerPrice, not stopPrice"
        );
    }

    #[test]
    fn take_profit_algo_params_use_trigger_price_and_conditional_type() {
        let params = take_profit_algo_params(&protection_request());
        assert_eq!(param(&params, "algoType"), Some("CONDITIONAL"));
        assert_eq!(param(&params, "type"), Some("TAKE_PROFIT_MARKET"));
        assert_eq!(param(&params, "side"), Some("SELL"));
        assert_eq!(param(&params, "triggerPrice"), Some("9000"));
        assert_eq!(param(&params, "reduceOnly"), Some("true"));
        assert_eq!(param(&params, "clientAlgoId"), Some("entryabc-tp"));
        assert_eq!(param(&params, "stopPrice"), None);
    }

    #[test]
    fn cancel_algo_params_route_numeric_id_as_algo_id() {
        let params = cancel_algo_order_params("2146760");
        assert_eq!(param(&params, "algoId"), Some("2146760"));
        assert_eq!(param(&params, "clientAlgoId"), None);
    }

    #[test]
    fn cancel_algo_params_route_string_id_as_client_algo_id() {
        let params = cancel_algo_order_params("entryabc-sl");
        assert_eq!(param(&params, "clientAlgoId"), Some("entryabc-sl"));
        assert_eq!(param(&params, "algoId"), None);
    }

    // A request through the production HTTP client must not hang forever when the
    // server accepts the connection but never responds. Without a configured
    // request timeout, a single stuck order would block the strategy loop
    // indefinitely. We point the client at a local listener that accepts and then
    // goes silent, and require the request to fail (time out) within a bound well
    // under any default.
    #[tokio::test]
    async fn http_client_times_out_on_silent_server() {
        use std::time::Instant;
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept the connection and hold it open without ever replying.
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                tokio::time::sleep(Duration::from_secs(120)).await;
            }
        });

        let client = http_client();
        let start = Instant::now();
        let result = client.get(format!("http://{addr}/")).send().await;
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "a silent server must produce an error, not hang"
        );
        let error = result.unwrap_err();
        assert!(error.is_timeout(), "the error must be a timeout");
        assert!(
            elapsed < HTTP_REQUEST_TIMEOUT + Duration::from_secs(2),
            "request should time out near the configured bound, took {elapsed:?}"
        );
        // A timed-out request must be classified as Timeout, not a generic
        // Exchange error: the caller treats the order outcome as UNKNOWN (the
        // exchange may have filled it) rather than as a definitive failure.
        assert!(
            matches!(map_request_error(error), TradingError::Timeout(_)),
            "a timeout must map to TradingError::Timeout"
        );
    }

    // A market-data WebSocket that connects but then goes silent (no data, no
    // ping, no close frame) must not hang the read loop forever. Without an idle
    // read timeout, run_public_market_stream_once blocks on read.next() and the
    // reconnect loop never fires, so the strategy freezes on stale data. We point
    // the loop at a local WS server that accepts and then stays silent, and
    // require the single connection attempt to return an error within the bound.
    #[tokio::test]
    async fn market_stream_times_out_on_silent_websocket() {
        use std::time::Instant;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept the WS handshake, then hold the connection open without sending.
        tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                if let Ok(ws) = tokio_tungstenite::accept_async(socket).await {
                    // Keep the stream alive but never send a frame.
                    let _ws = ws;
                    tokio::time::sleep(Duration::from_secs(120)).await;
                }
            }
        });

        let (sender, _receiver) = mpsc::channel(8);
        let idle_timeout = Duration::from_millis(500);
        let start = Instant::now();
        let result =
            run_public_market_stream_once(&format!("ws://{addr}/"), sender, idle_timeout, true)
                .await;
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "a silent websocket must produce an error, not hang"
        );
        assert!(
            elapsed < idle_timeout + Duration::from_secs(2),
            "the read loop should time out near the idle bound, took {elapsed:?}"
        );
    }
}
