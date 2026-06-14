use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet, VecDeque};
use tokio::sync::mpsc;
use trading_ai::{
    AiDecisionProvider, AiEntryGate, AiGateConfig, AiGateDecision, MacroDecision, PatternDecision,
    StaticAiDecisionProvider,
};
use trading_core::{Candle, MarketEvent, ObservedMarketEvent};
use trading_execution::{PaperBroker, PaperPositionTracker};
use trading_risk::BasicRiskGate;
use trading_strategy::{Strategy, TechnicalStrategy};

use crate::{
    ai_repository::persist_ai_context,
    dashboard_api::SharedRuntimeControl,
    execution_repository::{
        load_account_risk_state, load_open_protected_orders, persist_paper_exit,
        update_open_position_marks,
    },
    paper_trading::evaluate_and_execute_paper_signal,
    risk_event_repository::persist_ai_block_risk_event,
    signal_repository::persist_signal,
    telegram::NotificationSender,
};

#[derive(Debug, Clone)]
pub struct PaperStrategyRuntimeConfig {
    pub equity: Decimal,
    pub daily_loss_limit: Decimal,
    pub max_candles_per_key: usize,
    pub ai_filter_enabled: bool,
    pub ai_fail_closed: bool,
    pub ai_macro_score: Decimal,
    pub ai_long_bias: Decimal,
    pub ai_short_bias: Decimal,
    pub ai_pattern_confidence: Decimal,
    pub ai_historical_win_rate: Decimal,
}

impl Default for PaperStrategyRuntimeConfig {
    fn default() -> Self {
        Self {
            equity: Decimal::new(10_000, 0),
            daily_loss_limit: Decimal::new(500, 0),
            max_candles_per_key: 100,
            ai_filter_enabled: false,
            ai_fail_closed: true,
            ai_macro_score: Decimal::ZERO,
            ai_long_bias: Decimal::ZERO,
            ai_short_bias: Decimal::ZERO,
            ai_pattern_confidence: Decimal::new(70, 0),
            ai_historical_win_rate: Decimal::new(70, 0),
        }
    }
}

pub async fn run_paper_strategy_loop(
    mut receiver: mpsc::Receiver<ObservedMarketEvent>,
    pool: PgPool,
    control: SharedRuntimeControl,
    notifications: Option<NotificationSender>,
    config: PaperStrategyRuntimeConfig,
) {
    let strategy = TechnicalStrategy::default();
    let risk_gate = BasicRiskGate::default();
    let broker = PaperBroker::default();
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
    let mut tracker = PaperPositionTracker::default();
    let mut buffers = CandleBuffers::new(config.max_candles_per_key);
    let mut open_position_keys = match load_open_protected_orders(&pool).await {
        Ok(protected_orders) => {
            let mut keys = HashSet::new();
            for protected_order in protected_orders {
                keys.insert(position_key(
                    protected_order.position.exchange,
                    &protected_order.position.symbol,
                ));
                tracker.insert(protected_order);
            }
            keys
        }
        Err(error) => {
            tracing::error!(%error, "failed to restore open paper positions; starting with empty tracker");
            HashSet::new()
        }
    };

    while let Some(observed) = receiver.recv().await {
        match observed.event {
            MarketEvent::Candle(candle) => {
                let exchange = candle.exchange;
                let reference_price = candle.close;
                let position_key = position_key(exchange, &candle.symbol);

                // Mark open positions to the candle close as well, so positions
                // stay marked to market even if the order-book stream stalls or is
                // absent (e.g. only kline data is flowing). Order-book ticks below
                // refine this with the live mid price.
                if let Err(error) =
                    update_open_position_marks(&pool, exchange, &candle.symbol, reference_price)
                        .await
                {
                    tracing::error!(%error, "failed to mark positions on candle close");
                }

                let candles = match buffers.push(candle) {
                    Some(candles) => candles,
                    None => continue,
                };
                let account = match load_account_risk_state(
                    &pool,
                    config.equity,
                    config.daily_loss_limit,
                    control.read().await.mode != trading_core::TradingMode::Paper,
                    observed.latency_ms,
                )
                .await
                {
                    Ok(account) => account,
                    Err(error) => {
                        tracing::error!(%error, "failed to load account risk state");
                        continue;
                    }
                };

                for signal in strategy.evaluate(candles) {
                    if let Err(error) = persist_signal(&pool, &signal).await {
                        tracing::error!(%error, "failed to persist strategy signal");
                        continue;
                    }

                    if open_position_keys.contains(&position_key) {
                        tracing::debug!(
                            position_key = %position_key,
                            "skipping paper signal because a position is already open"
                        );
                        continue;
                    }

                    let ai_context = match ai_provider.decisions_for_signal(&signal) {
                        Ok(context) => context,
                        Err(error) => {
                            let reason = format!("AI decision provider error: {error}");
                            if let Err(error) =
                                persist_ai_block_risk_event(&pool, &signal, &reason).await
                            {
                                tracing::error!(%error, "failed to persist AI risk event");
                            }
                            continue;
                        }
                    };
                    let ai_decision = ai_gate.evaluate_or_block(&signal, &ai_context);

                    if let Err(error) =
                        persist_ai_context(&pool, &signal, &ai_context, &ai_decision).await
                    {
                        tracing::error!(%error, "failed to persist AI decision context");
                        continue;
                    }

                    if let AiGateDecision::Block { reason } = ai_decision {
                        if let Err(error) =
                            persist_ai_block_risk_event(&pool, &signal, &reason).await
                        {
                            tracing::error!(%error, "failed to persist AI risk event");
                        }
                        notify(
                            &notifications,
                            format!(
                                "AI gate blocked signal\nsymbol: {}\nside: {}\nreason: {}",
                                signal.symbol.as_str(),
                                signal.side.as_str(),
                                reason
                            ),
                        )
                        .await;
                        tracing::warn!(%reason, "AI gate blocked paper signal");
                        continue;
                    }

                    match evaluate_and_execute_paper_signal(
                        &pool,
                        &risk_gate,
                        &broker,
                        &signal,
                        exchange,
                        reference_price,
                        &account,
                    )
                    .await
                    {
                        Ok(protected_order) => {
                            open_position_keys.insert(position_key.clone());
                            notify(
                                &notifications,
                                format!(
                                    "paper entry filled\nsymbol: {}\nside: {}\nentry: {}\nqty: {}\nstop: {}\ntake: {}",
                                    protected_order.position.symbol.as_str(),
                                    protected_order.entry_order.side.as_str(),
                                    protected_order.position.entry_price,
                                    protected_order.position.quantity,
                                    protected_order.protection.stop_loss_price,
                                    protected_order.protection.take_profit_price
                                ),
                            )
                            .await;
                            tracker.insert(protected_order);
                        }
                        Err(error) => {
                            notify(
                                &notifications,
                                format!(
                                    "paper signal blocked\nsymbol: {}\nside: {}\nreason: {}",
                                    signal.symbol.as_str(),
                                    signal.side.as_str(),
                                    error
                                ),
                            )
                            .await;
                            tracing::warn!(%error, "paper signal execution blocked");
                        }
                    }
                }
            }
            MarketEvent::OrderBook(order_book) => {
                // Mark open positions to the latest top-of-book mid price so the
                // persisted mark_price/unrealized_pnl reflect the live market.
                // Without this, any non-SL/TP close realizes zero PnL and the
                // daily-loss kill switch never sees unrealized losses.
                let mark_price = (order_book.best_bid + order_book.best_ask) / Decimal::from(2);
                if let Err(error) = update_open_position_marks(
                    &pool,
                    order_book.exchange,
                    &order_book.symbol,
                    mark_price,
                )
                .await
                {
                    tracing::error!(%error, "failed to update open position marks");
                }

                for exit in tracker.update_mark(&order_book) {
                    match persist_paper_exit(&pool, &exit).await {
                        Ok(true) => {
                            notify(
                                &notifications,
                                format!(
                                    "paper exit\nsymbol: {}\ntrigger: {}\nexit_price: {}\nrealized_pnl: {}",
                                    exit.symbol.as_str(),
                                    exit.trigger.as_str(),
                                    exit.exit_price,
                                    exit.realized_pnl
                                ),
                            )
                            .await;
                            open_position_keys.remove(&position_key(exit.exchange, &exit.symbol));
                        }
                        Ok(false) => {
                            // Position was already closed out-of-band (dashboard/panic);
                            // drop it from the local set without double-recording.
                            tracing::debug!(
                                position_id = %exit.position_id,
                                "tracker exit skipped; position already closed"
                            );
                            open_position_keys.remove(&position_key(exit.exchange, &exit.symbol));
                        }
                        Err(error) => {
                            tracing::error!(%error, "failed to persist paper exit");
                        }
                    }
                }
            }
        }
    }

    tracing::warn!("paper strategy loop ended");
}

async fn notify(sender: &Option<NotificationSender>, message: String) {
    if let Some(sender) = sender {
        if sender.send(message).await.is_err() {
            tracing::warn!("Telegram notification channel is closed");
        }
    }
}

fn position_key(exchange: trading_core::ExchangeId, symbol: &trading_core::Symbol) -> String {
    format!("{}:{}", exchange.as_str(), symbol.as_str())
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
    use chrono::{TimeZone, Utc};
    use trading_core::{ExchangeId, Symbol};

    fn candle(index: i64) -> Candle {
        Candle {
            exchange: ExchangeId::Binance,
            symbol: Symbol::new("BTCUSDT"),
            timeframe: "1m".to_owned(),
            open_time: Utc.timestamp_opt(1_710_000_000 + index * 60, 0).unwrap(),
            open: Decimal::new(50_000, 0),
            high: Decimal::new(50_000, 0),
            low: Decimal::new(50_000, 0),
            close: Decimal::new(50_000 + index, 0),
            volume: Decimal::ONE,
        }
    }

    #[test]
    fn candle_buffer_keeps_recent_items() {
        let mut buffers = CandleBuffers::new(2);

        buffers.push(candle(0)).unwrap();
        buffers.push(candle(1)).unwrap();
        let current = buffers.push(candle(2)).unwrap();

        assert_eq!(current.len(), 2);
        assert_eq!(current[0].close, Decimal::new(50_001, 0));
        assert_eq!(current[1].close, Decimal::new(50_002, 0));
    }

    #[test]
    fn candle_buffer_updates_current_candle_without_new_signal() {
        let mut buffers = CandleBuffers::new(2);

        assert!(buffers.push(candle(0)).is_some());
        assert!(buffers.push(candle(0)).is_none());
    }
}
