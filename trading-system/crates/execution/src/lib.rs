use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::collections::HashMap;
use trading_core::{
    Order, OrderBookTop, OrderFill, OrderRequest, OrderStatus, Position, PositionSide,
    ProtectedOrder, ProtectionPlan, Result, Signal, Symbol, TradingError, TradingMode,
};
use uuid::Uuid;

#[async_trait]
pub trait Broker: Send + Sync {
    async fn submit_signal(&self, signal: Signal) -> Result<Order>;
    async fn submit_order(&self, request: OrderRequest) -> Result<ProtectedOrder>;
}

#[derive(Debug, Clone)]
pub struct PaperBroker {
    stop_loss_fraction: Decimal,
    take_profit_fraction: Decimal,
    leverage: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionTrigger {
    StopLoss,
    TakeProfit,
    ManualClose,
    PanicClose,
}

impl ProtectionTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StopLoss => "stop_loss",
            Self::TakeProfit => "take_profit",
            Self::ManualClose => "manual_close",
            Self::PanicClose => "panic_close",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PaperExit {
    pub position_id: Uuid,
    pub entry_order_id: Uuid,
    pub exchange: trading_core::ExchangeId,
    pub symbol: Symbol,
    pub trigger: ProtectionTrigger,
    pub exit_price: Decimal,
    pub quantity: Decimal,
    pub realized_pnl: Decimal,
    pub triggered_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub struct PaperPositionTracker {
    open_positions: HashMap<Uuid, ProtectedOrder>,
}

impl PaperPositionTracker {
    pub fn insert(&mut self, protected_order: ProtectedOrder) {
        self.open_positions
            .insert(protected_order.position.id, protected_order);
    }

    pub fn update_mark(&mut self, order_book: &OrderBookTop) -> Vec<PaperExit> {
        let mut exits = Vec::new();
        let mut closed_positions = Vec::new();

        for (position_id, protected_order) in &self.open_positions {
            if protected_order.position.exchange != order_book.exchange {
                continue;
            }

            if protected_order.position.symbol != order_book.symbol {
                continue;
            }

            if let Some(exit) = evaluate_exit(protected_order, order_book) {
                exits.push(exit);
                closed_positions.push(*position_id);
            }
        }

        for position_id in closed_positions {
            self.open_positions.remove(&position_id);
        }

        exits
    }
}

impl Default for PaperBroker {
    fn default() -> Self {
        Self {
            stop_loss_fraction: Decimal::new(1, 2),
            take_profit_fraction: Decimal::new(2, 2),
            leverage: Decimal::new(3, 0),
        }
    }
}

impl PaperBroker {
    pub fn new(
        stop_loss_fraction: Decimal,
        take_profit_fraction: Decimal,
        leverage: Decimal,
    ) -> Self {
        Self {
            stop_loss_fraction,
            take_profit_fraction,
            leverage,
        }
    }

    pub fn simulate_fill(&self, request: OrderRequest) -> Result<ProtectedOrder> {
        if request.mode != TradingMode::Paper {
            return Err(TradingError::RiskBlocked(
                "paper broker refuses non-paper order request".to_owned(),
            ));
        }

        if request.reference_price <= Decimal::ZERO {
            return Err(TradingError::Exchange(
                "paper fill reference price must be positive".to_owned(),
            ));
        }

        if request.quantity <= Decimal::ZERO {
            return Err(TradingError::Exchange(
                "paper fill quantity must be positive".to_owned(),
            ));
        }

        let now = Utc::now();
        let order = Order {
            id: Uuid::new_v4(),
            signal_id: request.signal_id,
            exchange: request.exchange,
            exchange_order_id: None,
            mode: TradingMode::Paper,
            symbol: request.symbol.clone(),
            side: request.side,
            order_type: request.order_type,
            status: OrderStatus::Filled,
            price: Some(request.reference_price),
            quantity: request.quantity,
            created_at: now,
        };
        let fill = OrderFill {
            order_id: order.id,
            exchange: order.exchange,
            symbol: order.symbol.clone(),
            side: order.side,
            price: request.reference_price,
            quantity: request.quantity,
            filled_at: now,
        };
        let position_side = request.side.position_side();
        let position = Position {
            id: Uuid::new_v4(),
            exchange: order.exchange,
            symbol: order.symbol.clone(),
            side: position_side,
            entry_price: request.reference_price,
            mark_price: request.reference_price,
            quantity: request.quantity,
            leverage: self.leverage,
            unrealized_pnl: Decimal::ZERO,
            opened_at: now,
        };
        let protection = ProtectionPlan {
            stop_loss_price: protection_price(
                request.reference_price,
                position_side,
                self.stop_loss_fraction,
                true,
            ),
            take_profit_price: protection_price(
                request.reference_price,
                position_side,
                self.take_profit_fraction,
                false,
            ),
        };

        Ok(ProtectedOrder {
            entry_order: order,
            fill,
            position,
            protection,
        })
    }
}

#[async_trait]
impl Broker for PaperBroker {
    async fn submit_signal(&self, _signal: Signal) -> Result<Order> {
        Err(TradingError::Exchange(
            "submit_signal requires risk sizing before execution".to_owned(),
        ))
    }

    async fn submit_order(&self, request: OrderRequest) -> Result<ProtectedOrder> {
        self.simulate_fill(request)
    }
}

fn protection_price(
    entry_price: Decimal,
    side: trading_core::PositionSide,
    fraction: Decimal,
    stop_loss: bool,
) -> Decimal {
    match (side, stop_loss) {
        (trading_core::PositionSide::Long, true) | (trading_core::PositionSide::Short, false) => {
            entry_price * (Decimal::ONE - fraction)
        }
        (trading_core::PositionSide::Long, false) | (trading_core::PositionSide::Short, true) => {
            entry_price * (Decimal::ONE + fraction)
        }
    }
}

fn evaluate_exit(protected_order: &ProtectedOrder, order_book: &OrderBookTop) -> Option<PaperExit> {
    let position = &protected_order.position;
    let protection = &protected_order.protection;
    let (trigger, exit_price) = match position.side {
        PositionSide::Long if order_book.best_bid <= protection.stop_loss_price => {
            (ProtectionTrigger::StopLoss, protection.stop_loss_price)
        }
        PositionSide::Long if order_book.best_bid >= protection.take_profit_price => {
            (ProtectionTrigger::TakeProfit, protection.take_profit_price)
        }
        PositionSide::Short if order_book.best_ask >= protection.stop_loss_price => {
            (ProtectionTrigger::StopLoss, protection.stop_loss_price)
        }
        PositionSide::Short if order_book.best_ask <= protection.take_profit_price => {
            (ProtectionTrigger::TakeProfit, protection.take_profit_price)
        }
        _ => return None,
    };
    let realized_pnl = match position.side {
        PositionSide::Long => (exit_price - position.entry_price) * position.quantity,
        PositionSide::Short => (position.entry_price - exit_price) * position.quantity,
    };

    Some(PaperExit {
        position_id: position.id,
        entry_order_id: protected_order.entry_order.id,
        exchange: position.exchange,
        symbol: position.symbol.clone(),
        trigger,
        exit_price,
        quantity: position.quantity,
        realized_pnl,
        triggered_at: Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use trading_core::{ExchangeId, OrderType, Side, Symbol};

    #[test]
    fn paper_fill_creates_protected_long_position() {
        let request = OrderRequest {
            exchange: ExchangeId::Binance,
            mode: TradingMode::Paper,
            symbol: Symbol::new("BTCUSDT"),
            side: Side::Buy,
            order_type: OrderType::Market,
            quantity: Decimal::new(1, 2),
            reference_price: Decimal::new(50_000, 0),
            signal_id: None,
        };

        let protected = PaperBroker::default().simulate_fill(request).unwrap();

        assert_eq!(protected.entry_order.status, OrderStatus::Filled);
        assert_eq!(protected.position.entry_price, Decimal::new(50_000, 0));
        assert_eq!(
            protected.protection.stop_loss_price,
            Decimal::new(49_500, 0)
        );
        assert_eq!(
            protected.protection.take_profit_price,
            Decimal::new(51_000, 0)
        );
    }

    #[test]
    fn paper_broker_rejects_live_request() {
        let request = OrderRequest {
            exchange: ExchangeId::Binance,
            mode: TradingMode::Live,
            symbol: Symbol::new("BTCUSDT"),
            side: Side::Buy,
            order_type: OrderType::Market,
            quantity: Decimal::new(1, 2),
            reference_price: Decimal::new(50_000, 0),
            signal_id: None,
        };

        assert!(PaperBroker::default().simulate_fill(request).is_err());
    }

    #[test]
    fn tracker_closes_long_on_take_profit() {
        let request = OrderRequest {
            exchange: ExchangeId::Binance,
            mode: TradingMode::Paper,
            symbol: Symbol::new("BTCUSDT"),
            side: Side::Buy,
            order_type: OrderType::Market,
            quantity: Decimal::new(1, 0),
            reference_price: Decimal::new(100, 0),
            signal_id: None,
        };
        let protected = PaperBroker::default().simulate_fill(request).unwrap();
        let mut tracker = PaperPositionTracker::default();
        tracker.insert(protected);

        let exits = tracker.update_mark(&OrderBookTop {
            exchange: ExchangeId::Binance,
            symbol: Symbol::new("BTCUSDT"),
            event_time: Utc::now(),
            best_bid: Decimal::new(102, 0),
            best_ask: Decimal::new(103, 0),
            bid_size: Decimal::ONE,
            ask_size: Decimal::ONE,
        });

        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].trigger, ProtectionTrigger::TakeProfit);
        assert_eq!(exits[0].realized_pnl, Decimal::new(2, 0));
    }
}
