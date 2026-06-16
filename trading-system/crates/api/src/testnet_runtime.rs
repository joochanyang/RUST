use chrono::Utc;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use tokio::sync::mpsc;
use trading_ai::{
    AiDecisionProvider, AiEntryGate, AiGateConfig, AiGateDecision, MacroDecision, PatternDecision,
    StaticAiDecisionProvider,
};
use trading_core::{
    Candle, ExchangeId, MarketEvent, ObservedMarketEvent, Order, OrderFill, OrderStatus, OrderType,
    Position, ProtectedOrder, ProtectionPlan, TradingMode,
};
use trading_exchange::{
    binance::BinanceAdapter, ExchangeAdapter, MarketOrderRequest, OrderAck, ProtectionOrderRequest,
};
use trading_risk::{BasicRiskGate, RiskGate};
use trading_strategy::{Strategy, VolatilityBreakoutStrategy};

use crate::{
    dashboard_api::SharedRuntimeControl,
    execution_repository::{
        close_orphaned_positions_for_key, load_account_risk_state, load_open_position_keys,
        load_open_protected_orders_by_mode_and_position, mark_position_closed_without_exit,
        persist_protected_order, update_open_position_marks,
    },
    notify_format,
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
            notify_format::account_check_failed(&error.to_string()),
        )
        .await;
    } else {
        notify(
            &notifications,
            notify_format::frame_simple("🟢 *Testnet 봇 가동*", "거래소 계정 연결됨"),
        )
        .await;
    }

    let strategy = VolatilityBreakoutStrategy::default();
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
            notify(&notifications, notify_format::filters_loaded(filters.len())).await;
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
                notify_format::critical_lock(
                    "거래소 필터 로드 실패 — 런타임 LOCKED",
                    &notify_format::reason_row(&error.to_string()),
                ),
            )
            .await;
            std::collections::HashMap::new()
        }
    };

    let mut buffers = CandleBuffers::new(config.max_candles_per_key);
    let mut open_position_keys = match load_open_position_keys(&pool, TradingMode::Testnet).await {
        Ok(keys) => {
            if !keys.is_empty() {
                notify(
                    &notifications,
                    notify_format::positions_restored(keys.len()),
                )
                .await;
            }
            keys.into_iter().collect::<HashSet<_>>()
        }
        Err(error) => {
            tracing::error!(%error, "failed to restore Binance testnet open positions; locking runtime");
            {
                let mut control = control.write().await;
                control.mode = TradingMode::Locked;
                control.locked_reason =
                    Some("failed to restore Binance testnet open positions".to_owned());
            }
            notify(
                &notifications,
                notify_format::critical_lock(
                    "오픈 포지션 복원 실패 — 런타임 LOCKED",
                    &notify_format::reason_row(&error.to_string()),
                ),
            )
            .await;
            HashSet::new()
        }
    };

    // Position sweep: reconcile DB-restored keys against the exchange's actual open
    // positions. An SL/TP can trigger while the bot is offline (restart, OS sleep,
    // a frozen WS), closing the position on the exchange without the bot ever seeing
    // the fill. Without this, the DB keeps a stale open position forever and blocks
    // re-entry on a key that is already flat. Best-effort: a snapshot failure leaves
    // the keys as-is (the conservative side — never wrongly drop a real position).
    if !open_position_keys.is_empty() {
        match adapter.fetch_account_snapshot().await {
            Ok(snapshot) => {
                let exchange_open = exchange_open_position_keys(ExchangeId::Binance, &snapshot.raw);
                let orphans = orphaned_position_keys(&open_position_keys, &exchange_open);
                for key in orphans {
                    let Some((_, symbol_str)) = key.split_once(':') else {
                        continue;
                    };
                    let symbol = trading_core::Symbol::new(symbol_str);
                    match close_orphaned_positions_for_key(
                        &pool,
                        ExchangeId::Binance,
                        &symbol,
                        TradingMode::Testnet,
                    )
                    .await
                    {
                        Ok(closed) => {
                            open_position_keys.remove(&key);
                            notify(&notifications, notify_format::position_sweep(&key, closed))
                                .await;
                        }
                        Err(error) => {
                            tracing::error!(%error, %key, "position sweep failed to reconcile orphaned position");
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "position sweep skipped: account snapshot fetch failed");
            }
        }
    }

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
        if let Err(error) =
            update_open_position_marks(&pool, exchange, &candle.symbol, reference_price).await
        {
            tracing::error!(%error, "failed to mark testnet positions on candle close");
        }
        let candles = match buffers.push(candle) {
            Some(candles) => candles,
            None => continue,
        };
        let locked = control.read().await.mode != TradingMode::Testnet;
        let account = match load_account_risk_state(
            &pool,
            config.equity,
            config.daily_loss_limit,
            locked,
            observed.latency_ms,
        )
        .await
        {
            Ok(account) => account,
            Err(error) => {
                tracing::error!(%error, "failed to load testnet account risk state");
                continue;
            }
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
                    notify_format::ai_block(signal.symbol.as_str(), signal.side.as_str(), &reason),
                )
                .await;
                continue;
            }

            let risk_decision = match risk_gate.evaluate_entry(&signal, reference_price, &account) {
                Ok(decision) => decision,
                Err(error) => {
                    notify(
                        &notifications,
                        notify_format::signal_blocked(
                            signal.symbol.as_str(),
                            signal.side.as_str(),
                            &error.to_string(),
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
                    notify_format::entry_skipped_minimums(
                        signal.symbol.as_str(),
                        signal.side.as_str(),
                        quantity,
                        quantity * reference_price,
                    ),
                )
                .await;
                continue;
            }

            let client_order_id = client_order_id_for_signal(signal.id);
            let order = match adapter
                .place_market_order(MarketOrderRequest {
                    symbol: signal.symbol.clone(),
                    side: signal.side,
                    quantity,
                    reduce_only: false,
                    client_order_id: Some(client_order_id.clone()),
                })
                .await
            {
                Ok(order) => order,
                // A timeout means the order outcome is UNKNOWN — Binance may have
                // filled it, leaving a naked position. Reconcile by looking the
                // order up via its deterministic client order id: recover (protect)
                // a filled order, skip a non-existent one, or lock if the lookup
                // itself fails. Definitive (non-timeout) failures stay a plain skip.
                Err(error @ trading_core::TradingError::Timeout(_)) => {
                    let outcome = reconcile_entry_timeout(
                        &adapter,
                        &pool,
                        &control,
                        &notifications,
                        filters,
                        &signal.symbol,
                        signal.side,
                        position_side,
                        quantity,
                        reference_price,
                        stop_loss_price,
                        take_profit_price,
                        &client_order_id,
                        Some(signal.id),
                        &error,
                        RECONCILE_QUERY_ATTEMPTS,
                        RECONCILE_QUERY_DELAY,
                    )
                    .await;
                    if outcome == ReconcileOutcome::Registered {
                        open_position_keys.insert(position_key.clone());
                    }
                    continue;
                }
                Err(error) => {
                    notify(
                        &notifications,
                        notify_format::order_failed(
                            signal.symbol.as_str(),
                            signal.side.as_str(),
                            &error.to_string(),
                        ),
                    )
                    .await;
                    continue;
                }
            };

            let registered = finalize_entry_with_protection(
                &adapter,
                &pool,
                &control,
                &notifications,
                filters,
                &signal.symbol,
                signal.side,
                position_side,
                &client_order_id,
                Some(signal.id),
                reference_price,
                &order,
                quantity,
                stop_loss_price,
                take_profit_price,
            )
            .await;
            if registered {
                open_position_keys.insert(position_key.clone());
            }
        }
    }
}

/// Recovers from a failed protection-order placement after a market entry has
/// already filled, leaving a naked unprotected position. Locks the runtime,
/// flattens the position with a reduce-only opposite-side order, persists a
/// CRITICAL risk event, and alerts. The caller must NOT register the position
/// key afterwards. Extracted from the entry loop so the full safety sequence is
/// testable end-to-end.
#[allow(clippy::too_many_arguments)]
async fn handle_protection_failure(
    adapter: &dyn ExchangeAdapter,
    pool: &PgPool,
    control: &SharedRuntimeControl,
    notifications: &Option<NotificationSender>,
    filters: &trading_exchange::binance::SymbolFilters,
    symbol: &trading_core::Symbol,
    side: trading_core::Side,
    order: &OrderAck,
    quantity: Decimal,
    protection_error: &trading_core::TradingError,
) {
    {
        let mut control = control.write().await;
        control.mode = TradingMode::Locked;
        control.locked_reason =
            Some("Binance testnet protection order placement failed".to_owned());
    }

    // Round the flatten quantity to the lot step too, so the reduce-only
    // order itself is not rejected for precision.
    let flatten_quantity = filters.round_quantity(order.executed_quantity.max(quantity));
    let flatten = flatten_position(adapter, symbol, side, flatten_quantity).await;

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
        pool,
        severity,
        "binance_testnet",
        action,
        serde_json::json!({
            "symbol": symbol.as_str(),
            "side": side.as_str(),
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
        symbol = symbol.as_str(),
        %protection_error,
        flatten_ok = flatten.is_ok(),
        "testnet protection failed; runtime locked and flatten attempted"
    );
    notify(
        notifications,
        notify_format::critical_lock(
            "보호주문 실패 — 런타임 LOCKED",
            &notify_format::lock_body(
                symbol.as_str(),
                side.as_str(),
                quantity,
                &format!("보호 오류: `{protection_error}`\n{flatten_summary}"),
            ),
        ),
    )
    .await;
}

/// Handles an entry order whose placement TIMED OUT. The outcome is unknown:
/// the exchange may have filled the order, leaving a naked position the bot has
/// no record of. Locks the runtime, persists a CRITICAL risk event, and alerts,
/// so no further entry is attempted until an operator reconciles the exchange
/// state. Definitive (non-timeout) failures must NOT use this path.
#[allow(clippy::too_many_arguments)]
async fn handle_entry_timeout(
    pool: &PgPool,
    control: &SharedRuntimeControl,
    notifications: &Option<NotificationSender>,
    symbol: &trading_core::Symbol,
    side: trading_core::Side,
    quantity: Decimal,
    client_order_id: &str,
    error: &trading_core::TradingError,
) {
    {
        let mut control = control.write().await;
        control.mode = TradingMode::Locked;
        control.locked_reason =
            Some("Binance testnet entry order timed out (outcome unknown)".to_owned());
    }

    if let Err(persist_error) = persist_risk_event(
        pool,
        "critical",
        "binance_testnet",
        "entry_order_timeout_unknown_outcome",
        serde_json::json!({
            "symbol": symbol.as_str(),
            "side": side.as_str(),
            "quantity": quantity,
            "client_order_id": client_order_id,
            "error": error.to_string(),
        }),
    )
    .await
    {
        tracing::error!(%persist_error, "failed to persist entry-timeout risk event");
    }

    tracing::error!(
        symbol = symbol.as_str(),
        %error,
        "entry order timed out; outcome unknown, runtime locked"
    );
    notify(
        notifications,
        notify_format::critical_lock(
            "진입 주문 TIMEOUT (결과 불명) — 런타임 LOCKED",
            &notify_format::lock_body(
                symbol.as_str(),
                side.as_str(),
                quantity,
                "⚠️ 거래소에서 체결됐을 수 있음 — 수동 reconcile 필요",
            ),
        ),
    )
    .await;
}

/// Places protection (stop-loss / take-profit) orders for a filled entry and
/// reports the result. Returns `true` when the position is protected and the
/// caller should register the position key; `false` when protection failed (the
/// runtime is then locked and the position flattened via `handle_protection_failure`).
#[allow(clippy::too_many_arguments)]
async fn finalize_entry_with_protection(
    adapter: &dyn ExchangeAdapter,
    pool: &PgPool,
    control: &SharedRuntimeControl,
    notifications: &Option<NotificationSender>,
    filters: &trading_exchange::binance::SymbolFilters,
    symbol: &trading_core::Symbol,
    side: trading_core::Side,
    position_side: trading_core::PositionSide,
    entry_client_order_id: &str,
    signal_id: Option<uuid::Uuid>,
    reference_price: Decimal,
    order: &OrderAck,
    quantity: Decimal,
    stop_loss_price: Decimal,
    take_profit_price: Decimal,
) -> bool {
    let protection = adapter
        .place_protection_orders(ProtectionOrderRequest {
            symbol: symbol.clone(),
            position_side,
            quantity: order.executed_quantity.max(quantity),
            stop_loss_price,
            take_profit_price,
            stop_loss_client_algo_id: Some(protection_client_algo_id_for_entry(
                entry_client_order_id,
                "sl",
            )),
            take_profit_client_algo_id: Some(protection_client_algo_id_for_entry(
                entry_client_order_id,
                "tp",
            )),
        })
        .await;

    if let Err(protection_error) = &protection {
        // Protection placement failed: the entry left a naked, unprotected
        // position open. Lock the runtime, flatten it, and escalate. Never
        // report this as a successful entry and never register the position key.
        handle_protection_failure(
            adapter,
            pool,
            control,
            notifications,
            filters,
            symbol,
            side,
            order,
            quantity,
            protection_error,
        )
        .await;
        return false;
    }

    let protection = protection.expect("checked above");
    let protected_order = protected_order_from_testnet_ack(
        signal_id,
        symbol,
        side,
        position_side,
        order,
        quantity,
        reference_price,
        stop_loss_price,
        take_profit_price,
    );
    if let Err(error) = persist_protected_order(pool, &protected_order).await {
        {
            let mut control = control.write().await;
            control.mode = TradingMode::Locked;
            control.locked_reason =
                Some("failed to persist Binance testnet protected position".to_owned());
        }
        if let Err(persist_error) = persist_risk_event(
            pool,
            "critical",
            "binance_testnet",
            "protected_position_persistence_failed",
            serde_json::json!({
                "symbol": symbol.as_str(),
                "side": side.as_str(),
                "quantity": quantity,
                "entry_order_id": order.exchange_order_id,
                "entry_order_status": order.status,
                "protection_raw": protection.raw,
                "error": error.to_string(),
            }),
        )
        .await
        {
            tracing::error!(%persist_error, "failed to persist testnet persistence-failure risk event");
        }
        notify(
            notifications,
            notify_format::critical_lock(
                "보호 포지션 DB 저장 실패 — 런타임 LOCKED",
                &notify_format::lock_body(
                    symbol.as_str(),
                    side.as_str(),
                    quantity,
                    &format!("사유: `{error}`"),
                ),
            ),
        )
        .await;
        return true;
    }

    if let Err(error) = persist_risk_event(
        pool,
        "info",
        "binance_testnet",
        "order_submitted",
        serde_json::json!({
            "symbol": symbol.as_str(),
            "side": side.as_str(),
            "quantity": quantity,
            "order_id": order.exchange_order_id,
            "order_status": order.status,
            "stop_loss_order_id": protection.stop_loss_order_id,
            "take_profit_order_id": protection.take_profit_order_id,
            "protection_ok": true
        }),
    )
    .await
    {
        tracing::error!(%error, "failed to persist Binance testnet order event");
    }

    notify(
        notifications,
        notify_format::entry_submitted(
            symbol.as_str(),
            side.as_str(),
            quantity,
            &order.status.to_string(),
            stop_loss_price,
            take_profit_price,
        ),
    )
    .await;
    true
}

#[allow(clippy::too_many_arguments)]
fn protected_order_from_testnet_ack(
    signal_id: Option<uuid::Uuid>,
    symbol: &trading_core::Symbol,
    side: trading_core::Side,
    position_side: trading_core::PositionSide,
    order: &OrderAck,
    planned_quantity: Decimal,
    reference_price: Decimal,
    stop_loss_price: Decimal,
    take_profit_price: Decimal,
) -> ProtectedOrder {
    let now = Utc::now();
    let entry_order_id = uuid::Uuid::new_v4();
    let position_id = uuid::Uuid::new_v4();
    let filled_quantity = if order.executed_quantity > Decimal::ZERO {
        order.executed_quantity
    } else {
        planned_quantity
    };
    let entry_price = order.average_price.unwrap_or(reference_price);

    ProtectedOrder {
        entry_order: Order {
            id: entry_order_id,
            signal_id,
            exchange: ExchangeId::Binance,
            exchange_order_id: Some(order.exchange_order_id.clone()),
            mode: TradingMode::Testnet,
            symbol: symbol.clone(),
            side,
            order_type: OrderType::Market,
            status: order_status_from_ack(&order.status, filled_quantity),
            price: Some(entry_price),
            quantity: filled_quantity,
            created_at: now,
        },
        fill: OrderFill {
            order_id: entry_order_id,
            exchange: ExchangeId::Binance,
            symbol: symbol.clone(),
            side,
            price: entry_price,
            quantity: filled_quantity,
            filled_at: now,
        },
        position: Position {
            id: position_id,
            exchange: ExchangeId::Binance,
            symbol: symbol.clone(),
            side: position_side,
            entry_price,
            mark_price: entry_price,
            quantity: filled_quantity,
            leverage: Decimal::ONE,
            unrealized_pnl: Decimal::ZERO,
            opened_at: now,
        },
        protection: ProtectionPlan {
            stop_loss_price,
            take_profit_price,
        },
    }
}

fn order_status_from_ack(status: &str, filled_quantity: Decimal) -> OrderStatus {
    match status {
        "FILLED" => OrderStatus::Filled,
        "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
        "CANCELED" | "CANCELLED" => OrderStatus::Canceled,
        "REJECTED" | "EXPIRED" => OrderStatus::Rejected,
        "NEW" => OrderStatus::New,
        _ if filled_quantity > Decimal::ZERO => OrderStatus::Filled,
        _ => OrderStatus::New,
    }
}

/// How many times to look an order up before concluding it did not fill, and how
/// long to wait between attempts. Guards against a stale read right after a
/// placement timeout reporting a freshly-filled order as missing/unfilled.
const RECONCILE_QUERY_ATTEMPTS: usize = 3;
const RECONCILE_QUERY_DELAY: Duration = Duration::from_secs(2);

/// Looks an order up repeatedly until it is observed filled or the attempts are
/// exhausted. Returns as soon as a fill is seen; otherwise returns the last
/// not-filled/missing result. Any query error is surfaced immediately (the
/// outcome is unknown and the caller must lock rather than skip). This absorbs a
/// brief exchange-side read lag so a real fill is never mistaken for "no order".
async fn query_order_until_settled(
    adapter: &dyn ExchangeAdapter,
    symbol: &trading_core::Symbol,
    client_order_id: &str,
    attempts: usize,
    delay: Duration,
) -> trading_core::Result<Option<OrderAck>> {
    let mut last = Ok(None);
    for attempt in 0..attempts.max(1) {
        match adapter.query_order(symbol, client_order_id).await {
            Ok(Some(order)) if order_is_filled(&order) => return Ok(Some(order)),
            other @ Ok(_) => last = other,
            // Unknown outcome — do not let a transient query error look like "no
            // position". Surface it so the caller locks conservatively.
            error @ Err(_) => return error,
        }
        if attempt + 1 < attempts.max(1) {
            tokio::time::sleep(delay).await;
        }
    }
    last
}

/// Outcome of reconciling an entry order after its placement timed out.
#[derive(Debug, PartialEq, Eq)]
enum ReconcileOutcome {
    /// The order was found filled and is now protected — register the position.
    Registered,
    /// The order was found filled but protection failed — runtime locked, do not register.
    NotRegistered,
    /// The order does not exist or was not filled — no position, safe to skip.
    Skipped,
    /// The lookup itself failed — outcome unknown, runtime locked conservatively.
    Locked,
}

/// Reconciles an entry order whose placement timed out by looking it up on the
/// exchange via its (deterministic) client order id:
/// - filled       → place protection (full recovery) and register the position,
/// - not filled / missing → no position exists, skip safely,
/// - lookup failed → outcome unknown, lock the runtime conservatively.
#[allow(clippy::too_many_arguments)]
async fn reconcile_entry_timeout(
    adapter: &dyn ExchangeAdapter,
    pool: &PgPool,
    control: &SharedRuntimeControl,
    notifications: &Option<NotificationSender>,
    filters: &trading_exchange::binance::SymbolFilters,
    symbol: &trading_core::Symbol,
    side: trading_core::Side,
    position_side: trading_core::PositionSide,
    quantity: Decimal,
    reference_price: Decimal,
    stop_loss_price: Decimal,
    take_profit_price: Decimal,
    client_order_id: &str,
    signal_id: Option<uuid::Uuid>,
    timeout_error: &trading_core::TradingError,
    query_attempts: usize,
    query_delay: Duration,
) -> ReconcileOutcome {
    let settled = query_order_until_settled(
        adapter,
        symbol,
        client_order_id,
        query_attempts,
        query_delay,
    )
    .await;
    match settled {
        Ok(Some(order)) if order_is_filled(&order) => {
            notify(
                notifications,
                notify_format::entry_reconciled_filled(
                    symbol.as_str(),
                    side.as_str(),
                    order.executed_quantity,
                ),
            )
            .await;
            let registered = finalize_entry_with_protection(
                adapter,
                pool,
                control,
                notifications,
                filters,
                symbol,
                side,
                position_side,
                client_order_id,
                signal_id,
                reference_price,
                &order,
                quantity,
                stop_loss_price,
                take_profit_price,
            )
            .await;
            if registered {
                ReconcileOutcome::Registered
            } else {
                ReconcileOutcome::NotRegistered
            }
        }
        // Order exists but is not filled, or never reached the exchange: there is
        // no open position, so it is safe to skip without locking.
        Ok(_) => {
            notify(
                notifications,
                notify_format::entry_reconciled_not_filled(symbol.as_str(), side.as_str()),
            )
            .await;
            ReconcileOutcome::Skipped
        }
        // The lookup itself failed: the order's existence is unknown, so fall back
        // to the conservative lock — a possibly-naked position must not be ignored.
        Err(query_error) => {
            handle_entry_timeout(
                pool,
                control,
                notifications,
                symbol,
                side,
                quantity,
                client_order_id,
                timeout_error,
            )
            .await;
            tracing::error!(%query_error, "entry-timeout reconcile lookup failed; locked");
            ReconcileOutcome::Locked
        }
    }
}

/// An order counts as filled (a real position exists) when any quantity executed.
/// Covers both FILLED and PARTIALLY_FILLED — any non-zero fill is a live position
/// that must be protected.
fn order_is_filled(order: &OrderAck) -> bool {
    order.executed_quantity > Decimal::ZERO
}

/// Derives a deterministic Binance `newClientOrderId` from a signal id. The
/// canonical UUID string is exactly 36 chars (Binance's max) and uses only
/// hyphens and hex digits, all within the accepted charset, so it can be sent
/// verbatim. Determinism means a future retry of the same signal reuses the id
/// and the exchange rejects the duplicate instead of opening a second position.
fn client_order_id_for_signal(signal_id: uuid::Uuid) -> String {
    signal_id.to_string()
}

/// Derives deterministic Binance `clientAlgoId` values from the entry
/// `newClientOrderId`. Binance caps ids at 36 chars; compacting the UUID entry
/// id to 32 hex chars leaves room for `-sl` / `-tp` while staying unique per
/// signal and safe for retry/cancel compensation.
fn protection_client_algo_id_for_entry(entry_client_order_id: &str, leg: &str) -> String {
    let mut base = entry_client_order_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    if base.is_empty() {
        base = "entry".to_owned();
    }
    let suffix = format!("-{leg}");
    let max_base_len = 36_usize.saturating_sub(suffix.len());
    base.truncate(max_base_len);
    format!("{base}{suffix}")
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
            // Recovery flatten has no retry path; let the exchange assign an id.
            client_order_id: None,
        })
        .await
}

pub async fn close_open_binance_testnet_positions(
    pool: &PgPool,
    adapter: &dyn ExchangeAdapter,
    position_id: Option<uuid::Uuid>,
    trigger: trading_execution::ProtectionTrigger,
) -> trading_core::Result<u64> {
    let protected_orders =
        load_open_protected_orders_by_mode_and_position(pool, TradingMode::Testnet, position_id)
            .await?;
    let mut closed = 0;

    for protected_order in protected_orders {
        let cancel_failures = cancel_testnet_protection_orders(adapter, &protected_order).await;
        let position = &protected_order.position;
        let flatten = flatten_position(
            adapter,
            &position.symbol,
            protected_order.entry_order.side,
            position.quantity,
        )
        .await;

        let flatten_ack = match flatten {
            Ok(ack) => ack,
            Err(error) => {
                persist_risk_event(
                    pool,
                    "critical",
                    "binance_testnet",
                    "panic_close_flatten_failed",
                    serde_json::json!({
                        "position_id": position.id,
                        "symbol": position.symbol.as_str(),
                        "side": position.side.as_str(),
                        "quantity": position.quantity,
                        "trigger": trigger.as_str(),
                        "cancel_failures": cancel_failures,
                        "error": error.to_string(),
                    }),
                )
                .await?;
                return Err(error);
            }
        };

        let close_price = flatten_ack.average_price.unwrap_or(position.mark_price);
        if mark_position_closed_without_exit(
            pool,
            position.id,
            close_price,
            &format!("{}_exchange_closed", trigger.as_str()),
        )
        .await?
        {
            closed += 1;
        }

        persist_risk_event(
            pool,
            "critical",
            "binance_testnet",
            "panic_close_exchange_flattened",
            serde_json::json!({
                "position_id": position.id,
                "symbol": position.symbol.as_str(),
                "side": position.side.as_str(),
                "quantity": position.quantity,
                "trigger": trigger.as_str(),
                "flatten_order_id": flatten_ack.exchange_order_id,
                "cancel_failures": cancel_failures,
            }),
        )
        .await?;
    }

    Ok(closed)
}

async fn cancel_testnet_protection_orders(
    adapter: &dyn ExchangeAdapter,
    protected_order: &ProtectedOrder,
) -> Vec<String> {
    let Some(signal_id) = protected_order.entry_order.signal_id else {
        return vec![format!(
            "position {} has no signal_id; cannot derive Binance clientAlgoId",
            protected_order.position.id
        )];
    };
    let entry_client_order_id = client_order_id_for_signal(signal_id);
    let mut failures = Vec::new();
    for leg in ["sl", "tp"] {
        let client_algo_id = protection_client_algo_id_for_entry(&entry_client_order_id, leg);
        if let Err(error) = adapter.cancel_order(client_algo_id.clone()).await {
            failures.push(format!("{client_algo_id}: {error}"));
        }
    }
    failures
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

/// Parses the set of position keys (`exchange:SYMBOL`) that the exchange reports
/// as actually open (non-zero `positionAmt`) from a `/fapi/v3/account` snapshot.
/// Binance only lists non-zero positions, but we filter defensively anyway.
fn exchange_open_position_keys(
    exchange: ExchangeId,
    snapshot: &serde_json::Value,
) -> HashSet<String> {
    let mut keys = HashSet::new();
    let Some(positions) = snapshot.get("positions").and_then(|v| v.as_array()) else {
        return keys;
    };
    for position in positions {
        let amt = position
            .get("positionAmt")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        if amt == 0.0 {
            continue;
        }
        if let Some(symbol) = position.get("symbol").and_then(|v| v.as_str()) {
            keys.insert(format!("{}:{}", exchange.as_str(), symbol));
        }
    }
    keys
}

/// Returns the DB-restored position keys that the exchange does NOT report as
/// open — these are orphans (e.g. closed by an SL/TP trigger while the bot was
/// asleep or restarting) whose DB rows must be reconciled to closed.
fn orphaned_position_keys(
    db_keys: &HashSet<String>,
    exchange_open: &HashSet<String>,
) -> Vec<String> {
    db_keys
        .iter()
        .filter(|key| !exchange_open.contains(*key))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use trading_core::{ExchangeId, Side, Symbol, TradingError};
    use trading_exchange::{AccountSnapshot, CancelAck, MarketStream, ProtectionAck};

    // A /fapi/v3/account snapshot lists only non-zero positions, so a DB key whose
    // symbol is absent (or zero) means the exchange has no such open position — an
    // orphan left when an SL/TP triggered while the bot was asleep/restarting.
    #[test]
    fn orphaned_keys_are_db_keys_absent_from_exchange() {
        let snapshot = serde_json::json!({
            "positions": [
                { "symbol": "BTCUSDT", "positionAmt": "-0.0020" },
                { "symbol": "SOLUSDT", "positionAmt": "0" }
            ]
        });
        let exchange_open = exchange_open_position_keys(ExchangeId::Binance, &snapshot);
        // Only BTCUSDT is genuinely open (SOLUSDT is zero, so excluded).
        assert!(exchange_open.contains("binance:BTCUSDT"));
        assert!(!exchange_open.contains("binance:SOLUSDT"));

        let db_keys: HashSet<String> = ["binance:ETHUSDT", "binance:BTCUSDT"]
            .into_iter()
            .map(String::from)
            .collect();
        let orphans = orphaned_position_keys(&db_keys, &exchange_open);
        // ETHUSDT was closed on the exchange (not in snapshot) -> orphan.
        // BTCUSDT is still open on the exchange -> kept.
        assert_eq!(orphans, vec!["binance:ETHUSDT".to_owned()]);
    }

    // A missing/empty positions array means the exchange has nothing open, so every
    // DB key is an orphan (defensive: never silently keep a stale key).
    #[test]
    fn empty_snapshot_makes_all_db_keys_orphans() {
        let snapshot = serde_json::json!({});
        let exchange_open = exchange_open_position_keys(ExchangeId::Binance, &snapshot);
        assert!(exchange_open.is_empty());

        let db_keys: HashSet<String> = ["binance:ETHUSDT"].into_iter().map(String::from).collect();
        let orphans = orphaned_position_keys(&db_keys, &exchange_open);
        assert_eq!(orphans, vec!["binance:ETHUSDT".to_owned()]);
    }

    // The outcome a test wants `query_order` to return, expressed as a plain
    // enum so each test can pick a reconcile branch without juggling raw JSON.
    #[derive(Clone, Default)]
    enum QueryOrderBehavior {
        #[default]
        Unused, // returns Err (existing tests never call query_order)
        Filled(Decimal),
        ExistsNotFilled,
        NotFound,
        QueryFails,
        // Returns not-filled for the first N calls, then filled — models a stale
        // read that settles to filled on a later attempt.
        NotFilledForFirst(usize, Decimal),
    }

    #[derive(Default)]
    struct RecordingAdapter {
        last_market_order: Mutex<Option<MarketOrderRequest>>,
        fail_market_order: bool,
        query_behavior: QueryOrderBehavior,
        // When false (default), protection placement returns Err (matches the old
        // mock). When true, it records the request and succeeds.
        protection_succeeds: bool,
        last_protection_order: Mutex<Option<ProtectionOrderRequest>>,
        query_calls: Mutex<usize>,
        cancel_succeeds: bool,
        cancelled_orders: Mutex<Vec<String>>,
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
            request: ProtectionOrderRequest,
        ) -> trading_core::Result<ProtectionAck> {
            *self.last_protection_order.lock().unwrap() = Some(request.clone());
            if !self.protection_succeeds {
                return Err(TradingError::Exchange("not used".to_owned()));
            }
            Ok(ProtectionAck {
                stop_loss_order_id: Some("sl-1".to_owned()),
                take_profit_order_id: Some("tp-1".to_owned()),
                raw: Vec::new(),
            })
        }
        async fn cancel_order(&self, order_id: String) -> trading_core::Result<CancelAck> {
            self.cancelled_orders.lock().unwrap().push(order_id);
            if !self.cancel_succeeds {
                return Err(TradingError::Exchange("not used".to_owned()));
            }
            Ok(CancelAck {
                raw: serde_json::Value::Null,
            })
        }
        async fn query_order(
            &self,
            symbol: &Symbol,
            _client_order_id: &str,
        ) -> trading_core::Result<Option<OrderAck>> {
            let call_index = {
                let mut calls = self.query_calls.lock().unwrap();
                let index = *calls;
                *calls += 1;
                index
            };
            let filled = |qty: Decimal, status: &str| OrderAck {
                exchange_order_id: "queried-1".to_owned(),
                symbol: symbol.clone(),
                side: Side::Buy,
                status: status.to_owned(),
                average_price: None,
                executed_quantity: qty,
                raw: serde_json::Value::Null,
            };
            match &self.query_behavior {
                QueryOrderBehavior::Unused => Err(TradingError::Exchange("not used".to_owned())),
                QueryOrderBehavior::Filled(qty) => Ok(Some(filled(*qty, "FILLED"))),
                QueryOrderBehavior::ExistsNotFilled => Ok(Some(filled(Decimal::ZERO, "NEW"))),
                QueryOrderBehavior::NotFound => Ok(None),
                QueryOrderBehavior::QueryFails => {
                    Err(TradingError::Timeout("query timed out".to_owned()))
                }
                QueryOrderBehavior::NotFilledForFirst(n, qty) => {
                    if call_index < *n {
                        Ok(Some(filled(Decimal::ZERO, "NEW")))
                    } else {
                        Ok(Some(filled(*qty, "FILLED")))
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn query_until_settled_retries_until_fill_appears() {
        // First two reads are stale (not filled); the third shows the fill.
        let adapter = RecordingAdapter {
            query_behavior: QueryOrderBehavior::NotFilledForFirst(2, Decimal::new(3, 2)),
            ..RecordingAdapter::default()
        };
        let result = query_order_until_settled(
            &adapter,
            &Symbol::new("BTCUSDT"),
            "coid",
            5,
            std::time::Duration::ZERO,
        )
        .await
        .expect("query ok");
        let order = result.expect("a settled fill must be returned");
        assert!(
            order_is_filled(&order),
            "the eventually-visible fill must be detected, not skipped"
        );
        assert_eq!(
            *adapter.query_calls.lock().unwrap(),
            3,
            "must retry until the fill appears, then stop"
        );
    }

    #[tokio::test]
    async fn query_until_settled_returns_not_filled_after_exhausting_attempts() {
        let adapter = RecordingAdapter {
            query_behavior: QueryOrderBehavior::ExistsNotFilled,
            ..RecordingAdapter::default()
        };
        let result = query_order_until_settled(
            &adapter,
            &Symbol::new("BTCUSDT"),
            "coid",
            3,
            std::time::Duration::ZERO,
        )
        .await
        .expect("query ok");
        let order = result.expect("an existing-but-unfilled order is still Some");
        assert!(
            !order_is_filled(&order),
            "genuinely not filled after retries"
        );
        assert_eq!(
            *adapter.query_calls.lock().unwrap(),
            3,
            "must exhaust all attempts before concluding not filled"
        );
    }

    #[tokio::test]
    async fn query_until_settled_surfaces_query_error() {
        let adapter = RecordingAdapter {
            query_behavior: QueryOrderBehavior::QueryFails,
            ..RecordingAdapter::default()
        };
        let result = query_order_until_settled(
            &adapter,
            &Symbol::new("BTCUSDT"),
            "coid",
            3,
            std::time::Duration::ZERO,
        )
        .await;
        assert!(
            result.is_err(),
            "a query error means the outcome is unknown — must surface, not skip"
        );
    }

    #[test]
    fn client_order_id_for_signal_is_binance_safe() {
        let coid = client_order_id_for_signal(uuid::Uuid::nil());
        assert!(
            coid.len() <= 36,
            "Binance newClientOrderId max length is 36"
        );
        assert!(!coid.is_empty());
        assert!(
            coid.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '/' | '_' | '-')),
            "must only use Binance-accepted characters: {coid}"
        );
    }

    #[test]
    fn client_order_id_for_signal_is_deterministic() {
        let id = uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        assert_eq!(
            client_order_id_for_signal(id),
            client_order_id_for_signal(id),
            "same signal id must yield the same client order id for idempotency"
        );
    }

    #[test]
    fn protection_client_algo_ids_are_short_safe_and_leg_specific() {
        let entry = client_order_id_for_signal(uuid::Uuid::nil());
        let stop_loss = protection_client_algo_id_for_entry(&entry, "sl");
        let take_profit = protection_client_algo_id_for_entry(&entry, "tp");

        assert_ne!(
            stop_loss, take_profit,
            "stop-loss and take-profit legs must have distinct ids"
        );
        for id in [stop_loss, take_profit] {
            assert!(id.len() <= 36, "Binance clientAlgoId max length is 36");
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '/' | '_' | '-')),
                "must only use Binance-accepted characters: {id}"
            );
        }
    }

    #[test]
    fn testnet_ack_builds_persistable_protected_order() {
        let signal_id = uuid::Uuid::new_v4();
        let order = OrderAck {
            exchange_order_id: "binance-entry-1".to_owned(),
            symbol: Symbol::new("BTCUSDT"),
            side: Side::Buy,
            status: "FILLED".to_owned(),
            average_price: Some(Decimal::new(50_100, 0)),
            executed_quantity: Decimal::new(2, 2),
            raw: serde_json::Value::Null,
        };

        let protected = protected_order_from_testnet_ack(
            Some(signal_id),
            &Symbol::new("BTCUSDT"),
            Side::Buy,
            trading_core::PositionSide::Long,
            &order,
            Decimal::new(3, 2),
            Decimal::new(50_000, 0),
            Decimal::new(49_000, 0),
            Decimal::new(51_000, 0),
        );

        assert_eq!(protected.entry_order.signal_id, Some(signal_id));
        assert_eq!(protected.entry_order.mode, TradingMode::Testnet);
        assert_eq!(
            protected.entry_order.exchange_order_id.as_deref(),
            Some("binance-entry-1")
        );
        assert_eq!(
            protected.entry_order.status,
            trading_core::OrderStatus::Filled
        );
        assert_eq!(
            protected.fill.quantity,
            Decimal::new(2, 2),
            "persist the executed quantity, not the larger planned quantity"
        );
        assert_eq!(protected.position.entry_price, Decimal::new(50_100, 0));
        assert_eq!(
            protected.protection.stop_loss_price,
            Decimal::new(49_000, 0)
        );
    }

    #[tokio::test]
    async fn testnet_protection_cancel_uses_persisted_signal_id() {
        let signal_id = uuid::Uuid::new_v4();
        let entry_client_order_id = client_order_id_for_signal(signal_id);
        let order = order_ack("BTCUSDT", Side::Buy, Decimal::new(2, 2));
        let protected = protected_order_from_testnet_ack(
            Some(signal_id),
            &Symbol::new("BTCUSDT"),
            Side::Buy,
            trading_core::PositionSide::Long,
            &order,
            Decimal::new(2, 2),
            Decimal::new(50_000, 0),
            Decimal::new(49_000, 0),
            Decimal::new(51_000, 0),
        );
        let adapter = RecordingAdapter {
            cancel_succeeds: true,
            ..RecordingAdapter::default()
        };

        let failures = cancel_testnet_protection_orders(&adapter, &protected).await;

        assert!(failures.is_empty(), "cancel should succeed: {failures:?}");
        let cancelled = adapter.cancelled_orders.lock().unwrap().clone();
        assert_eq!(
            cancelled,
            vec![
                protection_client_algo_id_for_entry(&entry_client_order_id, "sl"),
                protection_client_algo_id_for_entry(&entry_client_order_id, "tp"),
            ],
            "panic close must cancel both deterministic protection legs"
        );
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

    fn order_ack(symbol: &str, side: Side, qty: Decimal) -> OrderAck {
        OrderAck {
            exchange_order_id: "entry-1".to_owned(),
            symbol: Symbol::new(symbol),
            side,
            status: "FILLED".to_owned(),
            average_price: None,
            executed_quantity: qty,
            raw: serde_json::Value::Null,
        }
    }

    // End-to-end check of the C3 protection-failure recovery sequence: when a
    // market entry fills but its protection orders fail, the runtime must (1) lock,
    // (2) flatten with a reduce-only opposite-side order, (3) persist a CRITICAL
    // risk event, (4) alert, and (5) NOT register the position key. This is the
    // money-safety path that turns a naked unprotected position into a closed one.
    #[tokio::test]
    async fn protection_failure_locks_flattens_alerts_and_persists_critical_event() {
        use sqlx::{postgres::PgPoolOptions, Row};

        let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("connect test database");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        let adapter = RecordingAdapter::default();
        let control: SharedRuntimeControl = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::dashboard_api::RuntimeControlState::new(TradingMode::Testnet),
        ));
        let (tx, mut rx) = mpsc::channel::<String>(4);
        // Zero increments leave the flatten quantity unchanged (round is identity).
        let filters = trading_exchange::binance::SymbolFilters {
            step_size: Decimal::ZERO,
            tick_size: Decimal::ZERO,
            min_qty: Decimal::ZERO,
            min_notional: Decimal::ZERO,
        };
        let symbol = Symbol::new("BTCUSDT");
        let quantity = Decimal::new(3, 2);
        let entry = order_ack("BTCUSDT", Side::Buy, quantity);
        // Marker to find exactly this test's risk event back out of the table.
        let marker_order_id = format!("e2e-{}", uuid::Uuid::new_v4());

        handle_protection_failure(
            &adapter,
            &pool,
            &control,
            &Some(tx),
            &filters,
            &symbol,
            Side::Buy,
            &OrderAck {
                exchange_order_id: marker_order_id.clone(),
                ..entry
            },
            quantity,
            &TradingError::Exchange("stop-loss rejected".to_owned()),
        )
        .await;

        // (1) runtime locked with a reason.
        {
            let control = control.read().await;
            assert_eq!(control.mode, TradingMode::Locked, "runtime must be locked");
            assert!(control.locked_reason.is_some(), "lock must record a reason");
        }

        // (2) flatten was a reduce-only order on the opposite (sell) side.
        let flatten = adapter.last_market_order.lock().unwrap().clone().unwrap();
        assert_eq!(flatten.side, Side::Sell, "must close the long with a sell");
        assert!(flatten.reduce_only, "flatten must be reduce-only");

        // (4) a CRITICAL alert was sent.
        let alert = rx.try_recv().expect("an alert must be sent");
        assert!(
            alert.contains("CRITICAL"),
            "alert must be flagged critical: {alert}"
        );

        // (3) a CRITICAL risk event was persisted for this entry.
        let row = sqlx::query(
            "SELECT severity, action FROM risk_events \
             WHERE details->>'entry_order_id' = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&marker_order_id)
        .fetch_one(&pool)
        .await
        .expect("a risk event must be persisted");
        let severity: String = row.get("severity");
        let action: String = row.get("action");
        assert_eq!(severity, "critical", "risk event must be critical");
        assert_eq!(
            action, "position_flattened_after_protection_failure",
            "successful flatten must record the flattened action"
        );
    }

    // When an entry order TIMES OUT, the outcome is unknown: Binance may have
    // filled it, leaving a naked position the bot can't see. The conservative
    // response is to lock the runtime, alert CRITICAL, and persist a risk event,
    // so no new entry is placed until an operator reconciles. (A definitive
    // non-timeout failure must NOT lock — that path stays a plain skip.)
    #[tokio::test]
    async fn entry_timeout_locks_runtime_and_persists_critical_event() {
        use sqlx::{postgres::PgPoolOptions, Row};

        let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("connect test database");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        let control: SharedRuntimeControl = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::dashboard_api::RuntimeControlState::new(TradingMode::Testnet),
        ));
        let (tx, mut rx) = mpsc::channel::<String>(4);
        let marker = format!("e2e-timeout-{}", uuid::Uuid::new_v4());

        handle_entry_timeout(
            &pool,
            &control,
            &Some(tx),
            &Symbol::new("BTCUSDT"),
            Side::Buy,
            Decimal::new(3, 2),
            &marker,
            &TradingError::Timeout("request timed out".to_owned()),
        )
        .await;

        // runtime locked.
        {
            let control = control.read().await;
            assert_eq!(
                control.mode,
                TradingMode::Locked,
                "an ambiguous timeout must lock the runtime"
            );
            assert!(control.locked_reason.is_some());
        }

        // CRITICAL alert sent.
        let alert = rx.try_recv().expect("an alert must be sent");
        assert!(
            alert.contains("CRITICAL"),
            "alert must be critical: {alert}"
        );

        // CRITICAL risk event persisted with the unknown-outcome action.
        let row = sqlx::query(
            "SELECT severity, action FROM risk_events \
             WHERE details->>'client_order_id' = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&marker)
        .fetch_one(&pool)
        .await
        .expect("a risk event must be persisted");
        let severity: String = row.get("severity");
        let action: String = row.get("action");
        assert_eq!(severity, "critical");
        assert_eq!(action, "entry_order_timeout_unknown_outcome");
    }

    async fn test_pool() -> Option<PgPool> {
        use sqlx::postgres::PgPoolOptions;
        let database_url = std::env::var("TEST_DATABASE_URL").ok()?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("connect test database");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        Some(pool)
    }

    fn fresh_control() -> SharedRuntimeControl {
        std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::dashboard_api::RuntimeControlState::new(TradingMode::Testnet),
        ))
    }

    fn zero_filters() -> trading_exchange::binance::SymbolFilters {
        trading_exchange::binance::SymbolFilters {
            step_size: Decimal::ZERO,
            tick_size: Decimal::ZERO,
            min_qty: Decimal::ZERO,
            min_notional: Decimal::ZERO,
        }
    }

    // Drives reconcile_entry_timeout with the given adapter behavior and returns
    // (outcome, was the runtime locked?).
    async fn run_reconcile(
        pool: &PgPool,
        adapter: &RecordingAdapter,
    ) -> (ReconcileOutcome, bool, SharedRuntimeControl) {
        let control = fresh_control();
        let (tx, _rx) = mpsc::channel::<String>(8);
        // The reconcile signal id below is referenced by orders.signal_id (a FK to
        // signals.id). In production the signal row is persisted before the entry
        // order, so seed it here too; otherwise persist_protected_order fails the
        // FK and the happy path locks spuriously.
        sqlx::query(
            r#"INSERT INTO signals (id, symbol, side, strategy, score, reason)
               VALUES ($1, 'BTCUSDT', 'buy', 'reconcile-test', 0, 'reconcile-test')
               ON CONFLICT (id) DO NOTHING"#,
        )
        .bind(uuid::Uuid::nil())
        .execute(pool)
        .await
        .expect("seed reconcile signal");
        let outcome = reconcile_entry_timeout(
            adapter,
            pool,
            &control,
            &Some(tx),
            &zero_filters(),
            &Symbol::new("BTCUSDT"),
            Side::Buy,
            trading_core::PositionSide::Long,
            Decimal::new(3, 2),
            Decimal::new(50_000, 0),
            Decimal::new(49000, 0),
            Decimal::new(51000, 0),
            "coid-reconcile-test",
            Some(uuid::Uuid::nil()),
            &TradingError::Timeout("entry timed out".to_owned()),
            3,
            Duration::ZERO,
        )
        .await;
        let locked = control.read().await.mode == TradingMode::Locked;
        (outcome, locked, control)
    }

    #[tokio::test]
    async fn reconcile_filled_order_places_protection_and_registers() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let adapter = RecordingAdapter {
            query_behavior: QueryOrderBehavior::Filled(Decimal::new(3, 2)),
            protection_succeeds: true,
            ..RecordingAdapter::default()
        };
        let (outcome, locked, _control) = run_reconcile(&pool, &adapter).await;
        assert_eq!(
            outcome,
            ReconcileOutcome::Registered,
            "a filled order with protection placed must register the position"
        );
        assert!(!locked, "a fully-recovered entry must not lock the runtime");
        assert!(
            adapter.last_protection_order.lock().unwrap().is_some(),
            "protection orders must be placed on the recovered position"
        );
    }

    #[tokio::test]
    async fn reconcile_filled_order_with_failed_protection_does_not_register() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let adapter = RecordingAdapter {
            query_behavior: QueryOrderBehavior::Filled(Decimal::new(3, 2)),
            protection_succeeds: false,
            ..RecordingAdapter::default()
        };
        let (outcome, locked, _control) = run_reconcile(&pool, &adapter).await;
        assert_eq!(
            outcome,
            ReconcileOutcome::NotRegistered,
            "a filled order whose protection fails must not be registered"
        );
        assert!(
            locked,
            "protection failure on a recovered position must lock (flatten path)"
        );
    }

    #[tokio::test]
    async fn reconcile_unfilled_order_skips_without_locking() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let adapter = RecordingAdapter {
            query_behavior: QueryOrderBehavior::ExistsNotFilled,
            ..RecordingAdapter::default()
        };
        let (outcome, locked, _control) = run_reconcile(&pool, &adapter).await;
        assert_eq!(
            outcome,
            ReconcileOutcome::Skipped,
            "an order that exists but is not filled means no position — skip"
        );
        assert!(!locked, "no naked position, so no lock");
    }

    #[tokio::test]
    async fn reconcile_missing_order_skips_without_locking() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let adapter = RecordingAdapter {
            query_behavior: QueryOrderBehavior::NotFound,
            ..RecordingAdapter::default()
        };
        let (outcome, locked, _control) = run_reconcile(&pool, &adapter).await;
        assert_eq!(
            outcome,
            ReconcileOutcome::Skipped,
            "an order that never reached the exchange means no position — skip"
        );
        assert!(!locked, "no position was opened, so no lock");
    }

    #[tokio::test]
    async fn reconcile_failed_query_locks_runtime() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let adapter = RecordingAdapter {
            query_behavior: QueryOrderBehavior::QueryFails,
            ..RecordingAdapter::default()
        };
        let (outcome, locked, _control) = run_reconcile(&pool, &adapter).await;
        assert_eq!(
            outcome,
            ReconcileOutcome::Locked,
            "if the lookup itself fails the outcome is unknown — lock conservatively"
        );
        assert!(locked, "unknown outcome must lock the runtime");
    }
}
