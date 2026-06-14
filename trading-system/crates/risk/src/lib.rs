use rust_decimal::Decimal;
use trading_core::{
    ExchangeId, OrderRequest, PositionSide, Result, Signal, TradingError, TradingMode,
};

#[derive(Debug, Clone)]
pub struct RiskLimits {
    pub default_leverage: Decimal,
    pub max_leverage: Decimal,
    pub max_entry_fraction: Decimal,
    pub stop_loss_fraction: Decimal,
    pub take_profit_fraction: Decimal,
    pub min_reward_risk_ratio: Decimal,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            default_leverage: Decimal::new(3, 0),
            max_leverage: Decimal::new(5, 0),
            max_entry_fraction: Decimal::new(5, 2),
            stop_loss_fraction: Decimal::new(1, 2),
            take_profit_fraction: Decimal::new(2, 2),
            min_reward_risk_ratio: Decimal::new(2, 0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountRiskState {
    pub equity: Decimal,
    pub daily_realized_pnl: Decimal,
    pub daily_loss_limit: Decimal,
    pub locked: bool,
    pub market_data_latency_ms: i64,
}

#[derive(Debug, Clone)]
pub struct PositionSizing {
    pub notional: Decimal,
    pub quantity: Decimal,
    pub leverage: Decimal,
}

#[derive(Debug, Clone)]
pub struct EntryRiskDecision {
    pub sizing: PositionSizing,
    pub stop_loss_price: Decimal,
    pub take_profit_price: Decimal,
}

pub trait RiskGate: Send + Sync {
    fn evaluate_entry(
        &self,
        signal: &Signal,
        reference_price: Decimal,
        account: &AccountRiskState,
    ) -> Result<EntryRiskDecision>;
}

#[derive(Debug, Clone, Default)]
pub struct BasicRiskGate {
    limits: RiskLimits,
}

impl BasicRiskGate {
    pub fn new(limits: RiskLimits) -> Self {
        Self { limits }
    }

    pub fn build_order_request(
        &self,
        signal: &Signal,
        exchange: ExchangeId,
        reference_price: Decimal,
        account: &AccountRiskState,
    ) -> Result<OrderRequest> {
        let decision = self.evaluate_entry(signal, reference_price, account)?;

        Ok(OrderRequest {
            exchange,
            mode: TradingMode::Paper,
            symbol: signal.symbol.clone(),
            side: signal.side,
            order_type: trading_core::OrderType::Market,
            quantity: decision.sizing.quantity,
            reference_price,
            signal_id: Some(signal.id),
        })
    }

    fn validate_static_limits(&self) -> Result<()> {
        if self.limits.default_leverage > self.limits.max_leverage {
            return Err(TradingError::RiskBlocked(
                "default leverage exceeds max leverage".to_owned(),
            ));
        }

        let reward_risk_ratio = self.limits.take_profit_fraction / self.limits.stop_loss_fraction;
        if reward_risk_ratio < self.limits.min_reward_risk_ratio {
            return Err(TradingError::RiskBlocked(
                "reward/risk ratio is below configured minimum".to_owned(),
            ));
        }

        Ok(())
    }
}

impl RiskGate for BasicRiskGate {
    fn evaluate_entry(
        &self,
        signal: &Signal,
        reference_price: Decimal,
        account: &AccountRiskState,
    ) -> Result<EntryRiskDecision> {
        self.validate_static_limits()?;

        if account.locked {
            return Err(TradingError::RiskBlocked("account is locked".to_owned()));
        }

        if account.market_data_latency_ms > trading_core::MARKET_DATA_LATENCY_THRESHOLD_MS {
            return Err(TradingError::RiskBlocked(
                "market data latency exceeds 2 seconds".to_owned(),
            ));
        }

        if account.daily_realized_pnl <= -account.daily_loss_limit {
            return Err(TradingError::RiskBlocked(
                "daily loss limit has been reached".to_owned(),
            ));
        }

        if reference_price <= Decimal::ZERO {
            return Err(TradingError::RiskBlocked(
                "reference price must be positive".to_owned(),
            ));
        }

        let notional = account.equity * self.limits.max_entry_fraction;
        let quantity = notional / reference_price;
        let position_side = signal.side.position_side();
        let stop_loss_price = protection_price(
            reference_price,
            position_side,
            self.limits.stop_loss_fraction,
            true,
        );
        let take_profit_price = protection_price(
            reference_price,
            position_side,
            self.limits.take_profit_fraction,
            false,
        );

        Ok(EntryRiskDecision {
            sizing: PositionSizing {
                notional,
                quantity,
                leverage: self.limits.default_leverage,
            },
            stop_loss_price,
            take_profit_price,
        })
    }
}

fn protection_price(
    entry_price: Decimal,
    side: PositionSide,
    fraction: Decimal,
    stop_loss: bool,
) -> Decimal {
    match (side, stop_loss) {
        (PositionSide::Long, true) | (PositionSide::Short, false) => {
            entry_price * (Decimal::ONE - fraction)
        }
        (PositionSide::Long, false) | (PositionSide::Short, true) => {
            entry_price * (Decimal::ONE + fraction)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use trading_core::{Side, Symbol};
    use uuid::Uuid;

    fn signal(side: Side) -> Signal {
        Signal {
            id: Uuid::new_v4(),
            symbol: Symbol::new("BTCUSDT"),
            side,
            strategy: "test".to_owned(),
            score: Decimal::new(80, 0),
            reason: "test".to_owned(),
            created_at: Utc::now(),
        }
    }

    fn account() -> AccountRiskState {
        AccountRiskState {
            equity: Decimal::new(10_000, 0),
            daily_realized_pnl: Decimal::ZERO,
            daily_loss_limit: Decimal::new(500, 0),
            locked: false,
            market_data_latency_ms: 100,
        }
    }

    #[test]
    fn sizes_position_at_five_percent_equity() {
        let decision = BasicRiskGate::default()
            .evaluate_entry(&signal(Side::Buy), Decimal::new(50_000, 0), &account())
            .unwrap();

        assert_eq!(decision.sizing.notional, Decimal::new(500, 0));
        assert_eq!(decision.sizing.quantity, Decimal::new(1, 2));
        assert_eq!(decision.stop_loss_price, Decimal::new(49_500, 0));
        assert_eq!(decision.take_profit_price, Decimal::new(51_000, 0));
    }

    #[test]
    fn blocks_when_locked() {
        let mut account = account();
        account.locked = true;

        assert!(BasicRiskGate::default()
            .evaluate_entry(&signal(Side::Buy), Decimal::new(50_000, 0), &account)
            .is_err());
    }

    #[test]
    fn blocks_when_latency_exceeds_threshold() {
        let mut account = account();
        account.market_data_latency_ms = trading_core::MARKET_DATA_LATENCY_THRESHOLD_MS + 1;

        assert!(BasicRiskGate::default()
            .evaluate_entry(&signal(Side::Buy), Decimal::new(50_000, 0), &account)
            .is_err());
    }

    #[test]
    fn allows_at_threshold() {
        // The gate uses a strict `>`, so latency exactly at the threshold passes.
        let mut account = account();
        account.market_data_latency_ms = trading_core::MARKET_DATA_LATENCY_THRESHOLD_MS;

        assert!(BasicRiskGate::default()
            .evaluate_entry(&signal(Side::Buy), Decimal::new(50_000, 0), &account)
            .is_ok());
    }
}
