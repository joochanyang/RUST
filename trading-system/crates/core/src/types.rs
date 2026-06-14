use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, TradingError>;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradingMode {
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

impl Default for TradingMode {
    fn default() -> Self {
        Self::Paper
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
            Self::Candle(candle) => candle.open_time,
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
}
