use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet, VecDeque};
use tokio::sync::mpsc;
use trading_ai::{
    AiDecisionProvider, AiEntryGate, AiGateConfig, AiGateDecision, MacroDecision, PatternDecision,
    StaticAiDecisionProvider,
};
use trading_core::{Candle, MarketEvent, ObservedMarketEvent, TradingMode};
use trading_exchange::{
    binance::BinanceAdapter, ExchangeAdapter, MarketOrderRequest, OrderAck, ProtectionOrderRequest,
};
use trading_risk::{AccountRiskState, BasicRiskGate, RiskGate};
use trading_strategy::{Strategy, TechnicalStrategy};

use crate::{
    dashboard_api::SharedRuntimeControl,
    risk_event_repository::{persist_ai_block_risk_event, persist_risk_event},
    signal_repository::persist_signal,
    telegram::NotificationSender,
};

#[derive(Debug, Clone)]
pub struct TestnetStrategyRuntimeConfig {
    pub equity: Decimal,
    pub daily_loss_limit: Decimal,
    pub max_order_notional: Decimal,
    pub max_candles_per_key: usize,
    pub ai_filter_enabled: bool,
    pub ai_fail_closed: bool,
    pub ai_macro_score: Decimal,
    pub ai_long_bias: Decimal,
    pub ai_short_bias: Decimal,
    pub ai_pattern_confidence: Decimal,
    pub ai_historical_win_rate: Decimal,
}

pub async fn run_binance_testnet_strategy_loop(
    mut receiver: mpsc::Receiver<ObservedMarketEvent>,
    pool: PgPool,
    control: SharedRuntimeControl,
    notifications: Option<NotificationSender>,
    adapter: BinanceAdapter,
    config: TestnetStrategyRuntimeConfig,
) {
    if let Err(error) = adapter.fetch_account_snapshot().await {
        tracing::warn!(%error, "failed to fetch Binance testnet account snapshot at startup");
        notify(
            &notifications,
            format!("Binance testnet account check failed: {error}"),
        )
        .await;
    } else {
        notify(
            &notifications,
            "Binance testnet account connected".to_owned(),
        )
        .await;
    }

    let strategy = TechnicalStrategy::default();
    let risk_gate = BasicRiskGate::default();
    let ai_gate = AiEntryGate::new(AiGateConfig {
        enabled: config.ai_filter_enabled,
        fail_closed: config.ai_fail_closed,
        ..AiGateConfig::default()
    });
    let ai_provider = StaticAiDecisionProvider::new(
        Some(MacroDecision {
            macro_score: config.ai_macro_score,
            long_bias: config.ai_long_bias,
            short_bias: config.ai_short_bias,
            ..MacroDecision::default()
        }),
        Some(PatternDecision {
            pattern_confidence: config.ai_pattern_confidence,
            historical_win_rate: config.ai_historical_win_rate,
            ..PatternDecision::default()
        }),
    );
    // Cache exchange lot-size/tick-size filters once so orders are rounded to
    // the exchange's accepted increments. Without this, quantity = notional/price
    // almost always violates stepSize and the exchange rejects the order (-1111).
    let symbol_filters = match adapter.fetch_symbol_filters(&[]).await {
        Ok(filters) => {
            notify(
                &notifications,
                format!(
                    "Binance testnet exchange filters loaded ({} symbols)",
                    filters.len()
                ),
            )
            .await;
            filters
        }
        Err(error) => {
            tracing::error!(%error, "failed to fetch Binance exchange filters; locking runtime");
            {
                let mut control = control.write().await;
                control.mode = TradingMode::Locked;
                control.locked_reason =
                    Some("Binance exchangeInfo fetch failed at startup".to_owned());
            }
            notify(
                &notifications,
                format!("\u{1f6a8} CRITICAL: cannot load exchange filters; runtime LOCKED\nreason: {error}"),
            )
            .await;
            std::collections::HashMap::new()
        }
    };

    let mut buffers = CandleBuffers::new(config.max_candles_per_key);
    let mut open_position_keys = HashSet::<String>::new();

    while let Some(observed) = receiver.recv().await {
        let MarketEvent::Candle(candle) = observed.event else {
            continue;
        };
        let exchange = candle.exchange;
        if exchange != trading_core::ExchangeId::Binance {
            continue;
        }
        let reference_price = candle.close;
        let position_key = format!("{}:{}", exchange.as_str(), candle.symbol.as_str());
        let candles = match buffers.push(candle) {
            Some(candles) => candles,
            None => continue,
        };
        let locked = control.read().await.mode != TradingMode::Testnet;
        let account = AccountRiskState {
            equity: config.equity,
            daily_realized_pnl: Decimal::ZERO,
            daily_loss_limit: config.daily_loss_limit,
            locked,
            market_data_latency_ms: observed.latency_ms,
        };

        for signal in strategy.evaluate(candles) {
            if let Err(error) = persist_signal(&pool, &signal).await {
                tracing::error!(%error, "failed to persist testnet strategy signal");
                continue;
            }

            if open_position_keys.contains(&position_key) {
                continue;
            }

            let ai_context = match ai_provider.decisions_for_signal(&signal) {
                Ok(context) => context,
                Err(error) => {
                    let reason = format!("AI decision provider error: {error}");
                    if let Err(error) = persist_ai_block_risk_event(&pool, &signal, &reason).await {
                        tracing::error!(%error, "failed to persist AI risk event");
                    }
                    continue;
                }
            };
            let ai_decision = ai_gate.evaluate_or_block(&signal, &ai_context);

            if let AiGateDecision::Block { reason } = ai_decision {
                if let Err(error) = persist_ai_block_risk_event(&pool, &signal, &reason).await {
                    tracing::error!(%error, "failed to persist AI risk event");
                }
                notify(
                    &notifications,
                    format!(
                        "testnet AI block\nsymbol: {}\nside: {}\nreason: {}",
                        signal.symbol.as_str(),
                        signal.side.as_str(),
                        reason
                    ),
                )
                .await;
                continue;
            }

            let risk_decision = match risk_gate.evaluate_entry(&signal, reference_price, &account) {
                Ok(decision) => decision,
                Err(error) => {
                    notify(
                        &notifications,
                        format!(
                            "testnet signal blocked\nsymbol: {}\nside: {}\nreason: {}",
                            signal.symbol.as_str(),
                            signal.side.as_str(),
                            error
                        ),
                    )
                    .await;
                    continue;
                }
            };
            let notional = risk_decision.sizing.notional.min(config.max_order_notional);
            let raw_quantity = notional / reference_price;

            // Round to the exchange's lot/tick increments before sending, or the
            // order is rejected (-1111 precision). Skip the signal if no filters
            // are known for the symbol or the rounded order is below the minimums.
            let Some(filters) = symbol_filters.get(&signal.symbol) else {
                tracing::warn!(
                    symbol = signal.symbol.as_str(),
                    "no exchange filters for symbol; skipping testnet entry"
                );
                continue;
            };
            let position_side = signal.side.position_side();
            let quantity = filters.round_quantity(raw_quantity);
            let stop_loss_price =
                filters.round_protection_price(risk_decision.stop_loss_price, position_side);
            let take_profit_price =
                filters.round_protection_price(risk_decision.take_profit_price, position_side);

            if !filters.is_tradeable(quantity, reference_price) {
                notify(
                    &notifications,
                    format!(
                        "testnet entry skipped (below exchange minimums)\nsymbol: {}\nside: {}\nqty: {}\nnotional: {}",
                        signal.symbol.as_str(),
                        signal.side.as_str(),
                        quantity,
                        quantity * reference_price,
                    ),
                )
                .await;
                continue;
            }

            let order = match adapter
                .place_market_order(MarketOrderRequest {
                    symbol: signal.symbol.clone(),
                    side: signal.side,
                    quantity,
                    reduce_only: false,
                })
                .await
            {
                Ok(order) => order,
                Err(error) => {
                    notify(
                        &notifications,
                        format!(
                            "Binance testnet order failed\nsymbol: {}\nside: {}\nreason: {}",
                            signal.symbol.as_str(),
                            signal.side.as_str(),
                            error
                        ),
                    )
                    .await;
                    continue;
                }
            };

            let protection = adapter
                .place_protection_orders(ProtectionOrderRequest {
                    symbol: signal.symbol.clone(),
                    position_side,
                    quantity: order.executed_quantity.max(quantity),
                    stop_loss_price,
                    take_profit_price,
                })
                .await;

            if let Err(protection_error) = &protection {
                // Protection placement failed: the market entry left a naked,
                // unprotected position open. Lock the runtime and immediately try
                // to flatten the position with a reduce-only market order on the
                // opposite side, then escalate. Never report this as a successful
                // entry and never register the position key.
                {
                    let mut control = control.write().await;
                    control.mode = TradingMode::Locked;
                    control.locked_reason =
                        Some("Binance testnet protection order placement failed".to_owned());
                }

                // Round the flatten quantity to the lot step too, so the reduce-only
                // order itself is not rejected for precision.
                let flatten_quantity =
                    filters.round_quantity(order.executed_quantity.max(quantity));
                let flatten =
                    flatten_position(&adapter, &signal.symbol, signal.side, flatten_quantity).await;

                let (severity, action, flatten_summary) = match &flatten {
                    Ok(ack) => (
                        "critical",
                        "position_flattened_after_protection_failure",
                        format!("flattened (order {})", ack.exchange_order_id),
                    ),
                    Err(flatten_error) => (
                        "critical",
                        "naked_position_flatten_failed",
                        format!("FLATTEN FAILED: {flatten_error}"),
                    ),
                };

                if let Err(error) = persist_risk_event(
                    &pool,
                    severity,
                    "binance_testnet",
                    action,
                    serde_json::json!({
                        "symbol": signal.symbol.as_str(),
                        "side": signal.side.as_str(),
                        "quantity": quantity,
                        "entry_order_id": order.exchange_order_id,
                        "entry_order_status": order.status,
                        "protection_error": protection_error.to_string(),
                        "flatten_ok": flatten.is_ok(),
                    }),
                )
                .await
                {
                    tracing::error!(%error, "failed to persist naked-position risk event");
                }

                tracing::error!(
                    symbol = signal.symbol.as_str(),
                    %protection_error,
                    flatten_ok = flatten.is_ok(),
                    "testnet protection failed; runtime locked and flatten attempted"
                );
                notify(
                    &notifications,
                    format!(
                        "\u{1f6a8} CRITICAL: testnet protection FAILED\nsymbol: {}\nside: {}\nqty: {}\nprotection error: {protection_error}\n{flatten_summary}\nruntime LOCKED",
                        signal.symbol.as_str(),
                        signal.side.as_str(),
                        quantity,
                    ),
                )
                .await;

                // Do not register open_position_keys; the position is (intended to be)
                // closed, and the runtime is locked pending operator review.
                continue;
            }

            if let Err(error) = persist_risk_event(
                &pool,
                "info",
                "binance_testnet",
                "order_submitted",
                serde_json::json!({
                    "symbol": signal.symbol.as_str(),
                    "side": signal.side.as_str(),
                    "quantity": quantity,
                    "order_id": order.exchange_order_id,
                    "order_status": order.status,
                    "protection_ok": true
                }),
            )
            .await
            {
                tracing::error!(%error, "failed to persist Binance testnet order event");
            }

            notify(
                &notifications,
                format!(
                    "Binance testnet entry submitted\nsymbol: {}\nside: {}\nqty: {}\nstatus: {}\nstop: {}\ntake: {}",
                    signal.symbol.as_str(),
                    signal.side.as_str(),
                    quantity,
                    order.status,
                    stop_loss_price,
                    take_profit_price
                ),
            )
            .await;
            open_position_keys.insert(position_key.clone());
        }
    }
}

/// Closes an open position with a reduce-only market order on the opposite side.
///
/// `reduce_only` guarantees the order can only shrink/close the position and can
/// never flip it into a reverse position even if `quantity` is overstated.
async fn flatten_position(
    adapter: &dyn ExchangeAdapter,
    symbol: &trading_core::Symbol,
    entry_side: trading_core::Side,
    quantity: Decimal,
) -> trading_core::Result<OrderAck> {
    let close_side = match entry_side {
        trading_core::Side::Buy => trading_core::Side::Sell,
        trading_core::Side::Sell => trading_core::Side::Buy,
    };
    adapter
        .place_market_order(MarketOrderRequest {
            symbol: symbol.clone(),
            side: close_side,
            quantity,
            reduce_only: true,
        })
        .await
}

async fn notify(sender: &Option<NotificationSender>, message: String) {
    if let Some(sender) = sender {
        if sender.send(message).await.is_err() {
            tracing::warn!("Telegram notification channel is closed");
        }
    }
}

struct CandleBuffers {
    max_len: usize,
    buffers: HashMap<String, VecDeque<Candle>>,
}

impl CandleBuffers {
    fn new(max_len: usize) -> Self {
        Self {
            max_len: max_len.max(1),
            buffers: HashMap::new(),
        }
    }

    fn push(&mut self, candle: Candle) -> Option<&[Candle]> {
        let key = format!(
            "{}:{}:{}",
            candle.exchange.as_str(),
            candle.symbol.as_str(),
            candle.timeframe
        );
        let buffer = self.buffers.entry(key).or_default();

        if let Some(last) = buffer.back_mut() {
            if last.open_time == candle.open_time {
                *last = candle;
                return None;
            }
        }

        buffer.push_back(candle);

        while buffer.len() > self.max_len {
            buffer.pop_front();
        }

        Some(buffer.make_contiguous())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use trading_core::{ExchangeId, Side, Symbol, TradingError};
    use trading_exchange::{AccountSnapshot, CancelAck, MarketStream, ProtectionAck};

    #[derive(Default)]
    struct RecordingAdapter {
        last_market_order: Mutex<Option<MarketOrderRequest>>,
        fail_market_order: bool,
    }

    #[async_trait]
    impl ExchangeAdapter for RecordingAdapter {
        fn exchange_id(&self) -> ExchangeId {
            ExchangeId::Binance
        }
        async fn subscribe_market_stream(
            &self,
            _symbols: &[Symbol],
        ) -> trading_core::Result<MarketStream> {
            Err(TradingError::Exchange("not used".to_owned()))
        }
        async fn fetch_account_snapshot(&self) -> trading_core::Result<AccountSnapshot> {
            Err(TradingError::Exchange("not used".to_owned()))
        }
        async fn place_market_order(
            &self,
            request: MarketOrderRequest,
        ) -> trading_core::Result<OrderAck> {
            *self.last_market_order.lock().unwrap() = Some(request.clone());
            if self.fail_market_order {
                return Err(TradingError::Exchange("flatten rejected".to_owned()));
            }
            Ok(OrderAck {
                exchange_order_id: "flatten-1".to_owned(),
                symbol: request.symbol,
                side: request.side,
                status: "FILLED".to_owned(),
                average_price: None,
                executed_quantity: request.quantity,
                raw: serde_json::Value::Null,
            })
        }
        async fn place_protection_orders(
            &self,
            _request: ProtectionOrderRequest,
        ) -> trading_core::Result<ProtectionAck> {
            Err(TradingError::Exchange("not used".to_owned()))
        }
        async fn cancel_order(&self, _order_id: String) -> trading_core::Result<CancelAck> {
            Err(TradingError::Exchange("not used".to_owned()))
        }
    }

    #[tokio::test]
    async fn flatten_sends_reduce_only_opposite_side_order() {
        let adapter = RecordingAdapter::default();
        let ack = flatten_position(
            &adapter,
            &Symbol::new("BTCUSDT"),
            Side::Buy,
            Decimal::new(3, 2),
        )
        .await
        .expect("flatten succeeds");

        let recorded = adapter.last_market_order.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.side, Side::Sell, "must close a long with a sell");
        assert!(recorded.reduce_only, "flatten must be reduce-only");
        assert_eq!(recorded.quantity, Decimal::new(3, 2));
        assert_eq!(ack.exchange_order_id, "flatten-1");
    }

    #[tokio::test]
    async fn flatten_closes_short_with_buy() {
        let adapter = RecordingAdapter::default();
        flatten_position(&adapter, &Symbol::new("ETHUSDT"), Side::Sell, Decimal::ONE)
            .await
            .expect("flatten succeeds");
        let recorded = adapter.last_market_order.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.side, Side::Buy, "must close a short with a buy");
        assert!(recorded.reduce_only);
    }

    #[tokio::test]
    async fn flatten_surfaces_failure() {
        let adapter = RecordingAdapter {
            fail_market_order: true,
            ..RecordingAdapter::default()
        };
        let result =
            flatten_position(&adapter, &Symbol::new("BTCUSDT"), Side::Buy, Decimal::ONE).await;
        assert!(result.is_err(), "a rejected flatten must surface an error");
    }
}
