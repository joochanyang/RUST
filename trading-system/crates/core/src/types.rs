use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, TradingError>;

/// Entry-blocking market-data staleness threshold. Single source of truth for
/// both the risk gate (`trading-risk`) and the ingestion warn/persist path
/// (`trading-api`), so the two can never drift.
pub const MARKET_DATA_LATENCY_THRESHOLD_MS: i64 = 2_000;

#[derive(Debug, Error)]
pub enum TradingError {
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("exchange error: {0}")]
    Exchange(String),
    /// A network request did not complete within its deadline. The remote
    /// outcome is UNKNOWN — for an order this means it may have executed on the
    /// exchange even though we never saw the response, so callers must not treat
    /// it as a definitive failure.
    #[error("request timed out: {0}")]
    Timeout(String),
    #[error("risk rule blocked action: {0}")]
    RiskBlocked(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeId {
    Binance,
    Bybit,
    Bitget,
}

impl ExchangeId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Binance => "binance",
            Self::Bybit => "bybit",
            Self::Bitget => "bitget",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol(String);

impl Symbol {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into().to_ascii_uppercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TradingMode {
    #[default]
    Paper,
    Testnet,
    Live,
    Locked,
}

impl TradingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Paper => "paper",
            Self::Testnet => "testnet",
            Self::Live => "live",
            Self::Locked => "locked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn position_side(self) -> PositionSide {
        match self {
            Self::Buy => PositionSide::Long,
            Self::Sell => PositionSide::Short,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionSide {
    Long,
    Short,
}

impl PositionSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    Market,
    Limit,
    StopLoss,
    TakeProfit,
}

impl OrderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit => "limit",
            Self::StopLoss => "stop_loss",
            Self::TakeProfit => "take_profit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    New,
    Filled,
    PartiallyFilled,
    Canceled,
    Rejected,
}

impl OrderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Filled => "filled",
            Self::PartiallyFilled => "partially_filled",
            Self::Canceled => "canceled",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MarketEvent {
    Candle(Candle),
    OrderBook(OrderBookTop),
}

impl MarketEvent {
    pub fn exchange(&self) -> ExchangeId {
        match self {
            Self::Candle(candle) => candle.exchange,
            Self::OrderBook(order_book) => order_book.exchange,
        }
    }

    pub fn symbol(&self) -> &Symbol {
        match self {
            Self::Candle(candle) => &candle.symbol,
            Self::OrderBook(order_book) => &order_book.symbol,
        }
    }

    pub fn event_time(&self) -> DateTime<Utc> {
        match self {
            // Freshness of a candle is measured against its expected CLOSE, not
            // its open: a 1m kline streams partial ticks for the whole minute,
            // all stamped with the minute's open_time, so measuring against
            // open_time makes every tick read seconds-to-a-minute stale and the
            // latency gate blocks every entry. Against close_time a healthy
            // in-progress candle clamps to ~0 while a genuinely late one still
            // trips the gate.
            Self::Candle(candle) => candle.close_time(),
            Self::OrderBook(order_book) => order_book.event_time,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedMarketEvent {
    pub event: MarketEvent,
    pub received_at: DateTime<Utc>,
    pub latency_ms: i64,
}

impl ObservedMarketEvent {
    pub fn new(event: MarketEvent, received_at: DateTime<Utc>) -> Self {
        let latency_ms = received_at
            .signed_duration_since(event.event_time())
            .num_milliseconds()
            .max(0);

        Self {
            event,
            received_at,
            latency_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub timeframe: String,
    pub open_time: DateTime<Utc>,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
}

impl Candle {
    /// The instant this candle is expected to close (`open_time + interval`),
    /// derived from the `timeframe` string. This is the correct reference for
    /// market-data freshness — see `MarketEvent::event_time`. An unrecognized
    /// timeframe falls back to `open_time`, which is fail-safe toward MORE
    /// blocking (today's behavior) rather than silently passing a stale candle.
    pub fn close_time(&self) -> DateTime<Utc> {
        match timeframe_duration(&self.timeframe) {
            Some(duration) => self.open_time + duration,
            None => self.open_time,
        }
    }
}

/// Parses an exchange candle interval into a `Duration`. Handles the suffixed
/// form used by Binance and Bitget ("1m", "1h", "1d") and the bare form Bybit
/// stores ("1", "60", "D"). Matching is case-insensitive because Bitget echoes
/// its channel with capital units ("candle1H", "candle1D", "candle1W"), so a
/// non-1m subscription must not silently fall through to the open_time bug.
fn timeframe_duration(timeframe: &str) -> Option<chrono::Duration> {
    let minutes = match timeframe.to_ascii_lowercase().as_str() {
        "1m" | "1" => 1,
        "3m" | "3" => 3,
        "5m" | "5" => 5,
        "15m" | "15" => 15,
        "30m" | "30" => 30,
        "1h" | "60" => 60,
        "2h" | "120" => 120,
        "4h" | "240" => 240,
        "6h" | "360" => 360,
        "8h" | "480" => 480,
        "12h" | "720" => 720,
        "1d" | "d" => 1_440,
        "1w" | "w" => 10_080,
        _ => return None,
    };
    Some(chrono::Duration::minutes(minutes))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookTop {
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub event_time: DateTime<Utc>,
    pub best_bid: Decimal,
    pub best_ask: Decimal,
    pub bid_size: Decimal,
    pub ask_size: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: Uuid,
    pub symbol: Symbol,
    pub side: Side,
    pub strategy: String,
    pub score: Decimal,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub exchange: ExchangeId,
    pub mode: TradingMode,
    pub symbol: Symbol,
    pub side: Side,
    pub order_type: OrderType,
    pub quantity: Decimal,
    pub reference_price: Decimal,
    pub signal_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: Uuid,
    pub signal_id: Option<Uuid>,
    pub exchange: ExchangeId,
    pub exchange_order_id: Option<String>,
    pub mode: TradingMode,
    pub symbol: Symbol,
    pub side: Side,
    pub order_type: OrderType,
    pub status: OrderStatus,
    pub price: Option<Decimal>,
    pub quantity: Decimal,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderFill {
    pub order_id: Uuid,
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub side: Side,
    pub price: Decimal,
    pub quantity: Decimal,
    pub filled_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectionPlan {
    pub stop_loss_price: Decimal,
    pub take_profit_price: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectedOrder {
    pub entry_order: Order,
    pub fill: OrderFill,
    pub position: Position,
    pub protection: ProtectionPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub id: Uuid,
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub side: PositionSide,
    pub entry_price: Decimal,
    pub mark_price: Decimal,
    pub quantity: Decimal,
    pub leverage: Decimal,
    pub unrealized_pnl: Decimal,
    pub opened_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn symbol_normalizes_to_uppercase() {
        assert_eq!(Symbol::new("btcusdt").as_str(), "BTCUSDT");
    }

    #[test]
    fn trading_mode_defaults_to_paper() {
        assert_eq!(TradingMode::default(), TradingMode::Paper);
    }

    #[test]
    fn exchange_id_has_stable_storage_value() {
        assert_eq!(ExchangeId::Binance.as_str(), "binance");
        assert_eq!(ExchangeId::Bybit.as_str(), "bybit");
        assert_eq!(ExchangeId::Bitget.as_str(), "bitget");
    }

    #[test]
    fn side_maps_to_position_side() {
        assert_eq!(Side::Buy.position_side(), PositionSide::Long);
        assert_eq!(Side::Sell.position_side(), PositionSide::Short);
    }

    #[test]
    fn enum_storage_values_are_stable() {
        assert_eq!(TradingMode::Paper.as_str(), "paper");
        assert_eq!(TradingMode::Testnet.as_str(), "testnet");
        assert_eq!(Side::Buy.as_str(), "buy");
        assert_eq!(PositionSide::Long.as_str(), "long");
        assert_eq!(OrderType::StopLoss.as_str(), "stop_loss");
        assert_eq!(OrderStatus::PartiallyFilled.as_str(), "partially_filled");
    }

    fn test_candle(timeframe: &str, open_time: DateTime<Utc>) -> Candle {
        Candle {
            exchange: ExchangeId::Binance,
            symbol: Symbol::new("BTCUSDT"),
            timeframe: timeframe.to_owned(),
            open_time,
            open: Decimal::new(50_000, 0),
            high: Decimal::new(50_000, 0),
            low: Decimal::new(50_000, 0),
            close: Decimal::new(50_000, 0),
            volume: Decimal::ONE,
        }
    }

    // A 1m kline tick that arrives a few seconds into the minute is FRESH, not
    // stale: it is measured against the candle's expected CLOSE (open_time + 1m),
    // not its open. Against open_time the same tick reads ~3s and trips the gate;
    // against close_time it clamps to 0. This is the core bug repro.
    fn open_time_at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn fresh_candle_tick_has_near_zero_latency() {
        let open_time = open_time_at(1_710_000_000);
        let received_at = open_time + chrono::Duration::seconds(3);
        let observed = ObservedMarketEvent::new(
            MarketEvent::Candle(test_candle("1m", open_time)),
            received_at,
        );
        assert!(
            observed.latency_ms <= MARKET_DATA_LATENCY_THRESHOLD_MS,
            "fresh in-minute candle tick should not trip the latency gate, got {}ms",
            observed.latency_ms
        );
    }

    #[test]
    fn stalled_candle_feed_still_trips_gate() {
        // A candle whose close is already 5s in the past (next candle never
        // arrived in time) is genuinely stale and must still trip the gate.
        let open_time = open_time_at(1_710_000_000);
        let received_at = open_time + chrono::Duration::seconds(65);
        let observed = ObservedMarketEvent::new(
            MarketEvent::Candle(test_candle("1m", open_time)),
            received_at,
        );
        assert!(
            observed.latency_ms > MARKET_DATA_LATENCY_THRESHOLD_MS,
            "a candle arriving 5s after its close must still be flagged stale, got {}ms",
            observed.latency_ms
        );
    }

    #[test]
    fn bybit_bare_interval_resolves_duration() {
        // Bybit stores the bare interval ("1") rather than "1m"; the close-time
        // derivation must understand that form too, or Bybit silently regresses.
        let open_time = open_time_at(1_710_000_000);
        let received_at = open_time + chrono::Duration::seconds(3);
        let observed = ObservedMarketEvent::new(
            MarketEvent::Candle(test_candle("1", open_time)),
            received_at,
        );
        assert!(
            observed.latency_ms <= MARKET_DATA_LATENCY_THRESHOLD_MS,
            "bybit bare interval '1' should resolve to 1 minute, got {}ms",
            observed.latency_ms
        );
    }

    #[test]
    fn bitget_capital_interval_resolves_case_insensitively() {
        // Bitget echoes its channel with capital units ("candle1H" -> "1H"); the
        // duration map matches case-insensitively so a non-1m subscription cannot
        // silently fall back to open_time and re-introduce the latency bug.
        let open_time = open_time_at(1_710_000_000);
        let received_at = open_time + chrono::Duration::seconds(3);
        let observed = ObservedMarketEvent::new(
            MarketEvent::Candle(test_candle("1H", open_time)),
            received_at,
        );
        assert!(
            observed.latency_ms <= MARKET_DATA_LATENCY_THRESHOLD_MS,
            "bitget capital interval '1H' should resolve to 1 hour, got {}ms",
            observed.latency_ms
        );
    }

    #[test]
    fn orderbook_latency_unchanged() {
        // Regression lock: the OrderBook arm of event_time() is untouched, so
        // order-book staleness protection is preserved byte-for-byte.
        let event_time = open_time_at(1_710_000_000);
        let received_at = event_time + chrono::Duration::milliseconds(2_500);
        let order_book = OrderBookTop {
            exchange: ExchangeId::Binance,
            symbol: Symbol::new("BTCUSDT"),
            event_time,
            best_bid: Decimal::new(50_000, 0),
            best_ask: Decimal::new(50_001, 0),
            bid_size: Decimal::ONE,
            ask_size: Decimal::ONE,
        };
        let observed = ObservedMarketEvent::new(MarketEvent::OrderBook(order_book), received_at);
        assert_eq!(observed.latency_ms, 2_500);
    }
}
