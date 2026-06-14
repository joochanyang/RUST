pub mod binance;
pub mod bitget;
pub mod bybit;

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::mpsc;
use trading_core::{ExchangeId, ObservedMarketEvent, PositionSide, Result, Side, Symbol};

pub type MarketStreamReceiver = mpsc::Receiver<Result<ObservedMarketEvent>>;

pub struct MarketStream {
    receiver: MarketStreamReceiver,
}

impl MarketStream {
    pub fn new(receiver: MarketStreamReceiver) -> Self {
        Self { receiver }
    }

    pub async fn recv(&mut self) -> Option<Result<ObservedMarketEvent>> {
        self.receiver.recv().await
    }
}

#[derive(Debug, Clone)]
pub struct AccountSnapshot {
    pub exchange: ExchangeId,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct MarketOrderRequest {
    pub symbol: Symbol,
    pub side: Side,
    pub quantity: Decimal,
    /// When `true`, the order may only reduce/close an existing position and can
    /// never open or flip into a new one. Used to flatten a position safely after
    /// a protection-order failure.
    pub reduce_only: bool,
    /// Optional deterministic client order id (Binance `newClientOrderId`).
    /// When set, the exchange rejects a second order with the same id, so a
    /// future retry that reuses it cannot create a duplicate position. `None`
    /// lets the exchange assign one. Must be Binance-safe (<=36 chars, charset
    /// `[.A-Za-z0-9:/_-]`); the caller is responsible for producing a valid id.
    pub client_order_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OrderAck {
    pub exchange_order_id: String,
    pub symbol: Symbol,
    pub side: Side,
    pub status: String,
    pub average_price: Option<Decimal>,
    pub executed_quantity: Decimal,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct ProtectionOrderRequest {
    pub symbol: Symbol,
    pub position_side: PositionSide,
    pub quantity: Decimal,
    pub stop_loss_price: Decimal,
    pub take_profit_price: Decimal,
}

#[derive(Debug, Clone)]
pub struct ProtectionAck {
    pub stop_loss_order_id: Option<String>,
    pub take_profit_order_id: Option<String>,
    pub raw: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct CancelAck {
    pub raw: Value,
}

#[async_trait]
pub trait ExchangeAdapter: Send + Sync {
    fn exchange_id(&self) -> ExchangeId;
    async fn subscribe_market_stream(&self, symbols: &[Symbol]) -> Result<MarketStream>;
    async fn fetch_account_snapshot(&self) -> Result<AccountSnapshot>;
    async fn place_market_order(&self, request: MarketOrderRequest) -> Result<OrderAck>;
    async fn place_protection_orders(
        &self,
        request: ProtectionOrderRequest,
    ) -> Result<ProtectionAck>;
    async fn cancel_order(&self, order_id: String) -> Result<CancelAck>;
}
