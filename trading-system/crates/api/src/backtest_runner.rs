use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use trading_core::{Candle, ExchangeId, PositionSide, Result, Signal, Symbol, TradingError};
use trading_risk::{AccountRiskState, BasicRiskGate, RiskGate};
use trading_strategy::{Strategy, TrendFilteredBreakoutStrategy};

const BACKTEST_HISTORY_LIMIT: usize = 64;

/// Per-side taker fee charged on notional (0.04%, Binance USDT-M futures taker).
/// Backtest-only: live/paper PnL (`execution_repository::paper_position_pnl`) is
/// untouched. See the design spec — exits assume idealized fills, so funding and
/// gap-through remain unmodeled; a passing OOS is necessary, not sufficient.
const BACKTEST_TAKER_FEE: Decimal = Decimal::from_parts(4, 0, 0, false, 4); // 0.0004
/// Per-side slippage as a fraction of notional (0.01%). Understates gap-through
/// at the exact moment a stop fires; treat OOS results as optimistic.
const BACKTEST_SLIPPAGE_PCT: Decimal = Decimal::from_parts(1, 0, 0, false, 4); // 0.0001

/// Round-trip cost (entry + exit) for one trade: both legs charged taker fee +
/// slippage on their respective notional, scaled by `multiplier` (1.0 in
/// production; the walk-forward harness sweeps 1×/2×/3× for cost sensitivity).
/// Returns a positive Decimal to subtract from the trade's PnL.
fn trade_cost(
    entry_price: Decimal,
    exit_price: Decimal,
    quantity: Decimal,
    multiplier: Decimal,
) -> Decimal {
    let entry_notional = entry_price * quantity;
    let exit_notional = exit_price * quantity;
    (entry_notional + exit_notional) * (BACKTEST_TAKER_FEE + BACKTEST_SLIPPAGE_PCT) * multiplier
}

#[derive(Debug, Clone, Deserialize)]
pub struct BacktestConfig {
    pub exchange: Option<String>,
    pub symbols: Option<Vec<String>>,
    pub timeframe: Option<String>,
    pub period_start: Option<DateTime<Utc>>,
    pub period_end: Option<DateTime<Utc>>,
    pub initial_equity: Option<Decimal>,
    pub daily_loss_limit: Option<Decimal>,
    /// Trend-filtered breakout parameters. When omitted, the strategy's own
    /// defaults (lookback 20, k 0.5, ma_period 50) apply. Used for walk-forward
    /// sweeps — the only config-injected strategy params (see spec §8).
    pub lookback: Option<usize>,
    pub k: Option<f64>,
    pub ma_period: Option<usize>,
    /// Multiplies the per-trade fee+slippage cost. Defaults to 1.0 (production).
    /// The walk-forward harness sweeps 2.0/3.0 to test verdict robustness.
    pub cost_multiplier: Option<f64>,
    /// Optional warm-up boundary. Candles loaded before this instant only warm
    /// the strategy's rolling buffer (so longer-horizon trend filters are
    /// computable at the true window start) but never open positions. `None`
    /// (production, HTTP API, and the 1m/5m/1h harnesses) = every loaded candle
    /// trades — unchanged. Used by the daily harness, where `period_start` is
    /// pulled back to pre-roll the SMA buffer without inflating trade counts.
    pub eval_start: Option<DateTime<Utc>>,
}

impl BacktestConfig {
    pub fn normalized(self) -> Self {
        let period_end = self.period_end.unwrap_or_else(Utc::now);
        Self {
            exchange: Some(self.exchange.unwrap_or_else(|| "binance".to_owned())),
            symbols: Some(
                self.symbols
                    .unwrap_or_else(|| vec!["BTCUSDT".to_owned(), "ETHUSDT".to_owned()]),
            ),
            timeframe: Some(self.timeframe.unwrap_or_else(|| "1m".to_owned())),
            period_start: Some(
                self.period_start
                    .unwrap_or_else(|| period_end - Duration::days(365 * 3)),
            ),
            period_end: Some(period_end),
            initial_equity: Some(self.initial_equity.unwrap_or(Decimal::new(10_000, 0))),
            daily_loss_limit: Some(self.daily_loss_limit.unwrap_or(Decimal::new(500, 0))),
            lookback: self.lookback,
            k: self.k,
            ma_period: self.ma_period,
            cost_multiplier: self.cost_multiplier,
            eval_start: self.eval_start,
        }
    }

    pub fn exchange_id(&self) -> Result<ExchangeId> {
        match self.exchange.as_deref().unwrap_or("binance") {
            "binance" => Ok(ExchangeId::Binance),
            "bybit" => Ok(ExchangeId::Bybit),
            "bitget" => Ok(ExchangeId::Bitget),
            other => Err(TradingError::Configuration(format!(
                "unsupported backtest exchange: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BacktestMetrics {
    pub exchange: String,
    pub symbols: Vec<String>,
    pub timeframe: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub initial_equity: Decimal,
    pub final_equity: Decimal,
    pub realized_pnl: Decimal,
    pub max_drawdown: Decimal,
    pub max_drawdown_pct: Decimal,
    pub trades: u64,
    pub wins: u64,
    pub losses: u64,
    pub candles_loaded: usize,
    pub signals_seen: u64,
}

#[derive(Debug, Clone)]
struct BacktestPosition {
    symbol: Symbol,
    side: PositionSide,
    entry_price: Decimal,
    quantity: Decimal,
    stop_loss_price: Decimal,
    take_profit_price: Decimal,
}

pub async fn run_backtest(pool: &PgPool, config: BacktestConfig) -> Result<BacktestMetrics> {
    let config = config.normalized();
    let exchange = config.exchange_id()?;
    let exchange_name = config
        .exchange
        .clone()
        .unwrap_or_else(|| "binance".to_owned());
    let symbols = config
        .symbols
        .clone()
        .unwrap_or_else(|| vec!["BTCUSDT".to_owned(), "ETHUSDT".to_owned()]);
    let timeframe = config.timeframe.clone().unwrap_or_else(|| "1m".to_owned());
    let period_start = config.period_start.unwrap();
    let period_end = config.period_end.unwrap();
    let initial_equity = config.initial_equity.unwrap();
    let daily_loss_limit = config.daily_loss_limit.unwrap();
    let eval_start = config.eval_start;

    if period_end <= period_start {
        return Err(TradingError::Configuration(
            "period_end must be after period_start".to_owned(),
        ));
    }

    let candles = load_candles(
        pool,
        &exchange_name,
        &symbols,
        &timeframe,
        period_start,
        period_end,
        exchange,
    )
    .await?;
    if candles.is_empty() {
        return Err(TradingError::Configuration(
            "backtest requires candles for the requested period".to_owned(),
        ));
    }

    let default_strategy = TrendFilteredBreakoutStrategy::default();
    let strategy = match (config.lookback, config.k, config.ma_period) {
        (None, None, None) => default_strategy,
        (lookback, k, ma_period) => TrendFilteredBreakoutStrategy::new(
            lookback.unwrap_or_else(|| default_strategy.lookback()),
            k.unwrap_or_else(|| default_strategy.k()),
            ma_period.unwrap_or_else(|| default_strategy.ma_period()),
        ),
    };
    let cost_multiplier =
        Decimal::from_f64_retain(config.cost_multiplier.unwrap_or(1.0)).unwrap_or(Decimal::ONE);
    let risk_gate = BasicRiskGate::default();
    let mut equity = initial_equity;
    let mut peak_equity = initial_equity;
    let mut max_drawdown = Decimal::ZERO;
    let mut trades = 0;
    let mut wins = 0;
    let mut losses = 0;
    let mut signals_seen = 0;
    let mut open_positions = Vec::<BacktestPosition>::new();
    let mut history_by_symbol = HashMap::<Symbol, Vec<Candle>>::new();

    for candle in &candles {
        close_positions(
            candle,
            &mut open_positions,
            &mut equity,
            &mut trades,
            &mut wins,
            &mut losses,
            cost_multiplier,
        );
        if equity > peak_equity {
            peak_equity = equity;
        }
        let drawdown = peak_equity - equity;
        if drawdown > max_drawdown {
            max_drawdown = drawdown;
        }

        let symbol_history = history_by_symbol.entry(candle.symbol.clone()).or_default();
        symbol_history.push(candle.clone());
        if symbol_history.len() > BACKTEST_HISTORY_LIMIT {
            symbol_history.remove(0);
        }

        // Pre-roll candles (before eval_start) only warm the buffer above; they
        // do not evaluate signals or enter, so trade/signal counts reflect only
        // the true window. With eval_start = None this never skips (unchanged).
        if !entry_allowed_at(candle.open_time, eval_start) {
            continue;
        }

        let signals = strategy.evaluate(symbol_history);
        signals_seen += signals.len() as u64;

        for signal in signals {
            if has_open_position(&open_positions, &signal) {
                continue;
            }
            if let Some(position) =
                build_position(&risk_gate, &signal, candle.close, equity, daily_loss_limit)?
            {
                open_positions.push(position);
            }
        }
    }

    for position in &open_positions {
        if let Some(last_candle) = candles
            .iter()
            .rev()
            .find(|candle| candle.symbol == position.symbol)
        {
            // Mark-to-close the leftover position, also net of round-trip cost
            // (win/loss counters intentionally untouched here, as before).
            equity += position_pnl(position, last_candle.close)
                - trade_cost(
                    position.entry_price,
                    last_candle.close,
                    position.quantity,
                    cost_multiplier,
                );
        }
    }

    let realized_pnl = equity - initial_equity;
    let max_drawdown_pct = if peak_equity > Decimal::ZERO {
        max_drawdown / peak_equity * Decimal::new(100, 0)
    } else {
        Decimal::ZERO
    };

    Ok(BacktestMetrics {
        exchange: exchange_name,
        symbols,
        timeframe,
        period_start,
        period_end,
        initial_equity,
        final_equity: equity,
        realized_pnl,
        max_drawdown,
        max_drawdown_pct,
        trades,
        wins,
        losses,
        candles_loaded: candles.len(),
        signals_seen,
    })
}

async fn load_candles(
    pool: &PgPool,
    exchange_name: &str,
    symbols: &[String],
    timeframe: &str,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
    exchange: ExchangeId,
) -> Result<Vec<Candle>> {
    let rows = sqlx::query(
        r#"
        SELECT symbol, timeframe, open_time, open, high, low, close, volume
        FROM candles
        WHERE exchange = $1
          AND symbol = ANY($2)
          AND timeframe = $3
          AND open_time >= $4
          AND open_time < $5
        ORDER BY open_time ASC, symbol ASC
        "#,
    )
    .bind(exchange_name)
    .bind(symbols)
    .bind(timeframe)
    .bind(period_start)
    .bind(period_end)
    .fetch_all(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| Candle {
            exchange,
            symbol: Symbol::new(row.get::<String, _>("symbol")),
            timeframe: row.get("timeframe"),
            open_time: row.get("open_time"),
            open: row.get("open"),
            high: row.get("high"),
            low: row.get("low"),
            close: row.get("close"),
            volume: row.get("volume"),
        })
        .collect())
}

fn build_position(
    risk_gate: &BasicRiskGate,
    signal: &Signal,
    reference_price: Decimal,
    equity: Decimal,
    daily_loss_limit: Decimal,
) -> Result<Option<BacktestPosition>> {
    let account = AccountRiskState {
        equity,
        daily_realized_pnl: Decimal::ZERO,
        daily_loss_limit,
        locked: false,
        market_data_latency_ms: 0,
    };
    let decision = risk_gate.evaluate_entry(signal, reference_price, &account)?;
    Ok(Some(BacktestPosition {
        symbol: signal.symbol.clone(),
        side: signal.side.position_side(),
        entry_price: reference_price,
        quantity: decision.sizing.quantity,
        stop_loss_price: decision.stop_loss_price,
        take_profit_price: decision.take_profit_price,
    }))
}

#[allow(clippy::too_many_arguments)]
fn close_positions(
    candle: &Candle,
    open_positions: &mut Vec<BacktestPosition>,
    equity: &mut Decimal,
    trades: &mut u64,
    wins: &mut u64,
    losses: &mut u64,
    cost_multiplier: Decimal,
) {
    let mut index = 0;
    while index < open_positions.len() {
        if open_positions[index].symbol != candle.symbol {
            index += 1;
            continue;
        }

        let Some(exit_price) = exit_price(&open_positions[index], candle) else {
            index += 1;
            continue;
        };
        let position = &open_positions[index];
        // Charge round-trip fee + slippage; win/loss is judged on after-cost PnL
        // so a trade that only profits before fees is correctly counted a loss.
        let pnl = position_pnl(position, exit_price)
            - trade_cost(
                position.entry_price,
                exit_price,
                position.quantity,
                cost_multiplier,
            );
        *equity += pnl;
        *trades += 1;
        if pnl >= Decimal::ZERO {
            *wins += 1;
        } else {
            *losses += 1;
        }
        open_positions.remove(index);
    }
}

fn exit_price(position: &BacktestPosition, candle: &Candle) -> Option<Decimal> {
    match position.side {
        PositionSide::Long if candle.low <= position.stop_loss_price => {
            Some(position.stop_loss_price)
        }
        PositionSide::Long if candle.high >= position.take_profit_price => {
            Some(position.take_profit_price)
        }
        PositionSide::Short if candle.high >= position.stop_loss_price => {
            Some(position.stop_loss_price)
        }
        PositionSide::Short if candle.low <= position.take_profit_price => {
            Some(position.take_profit_price)
        }
        _ => None,
    }
}

fn position_pnl(position: &BacktestPosition, exit_price: Decimal) -> Decimal {
    match position.side {
        PositionSide::Long => (exit_price - position.entry_price) * position.quantity,
        PositionSide::Short => (position.entry_price - exit_price) * position.quantity,
    }
}

fn has_open_position(open_positions: &[BacktestPosition], signal: &Signal) -> bool {
    open_positions
        .iter()
        .any(|position| position.symbol == signal.symbol)
}

/// Whether a candle at `candle_time` is allowed to open a new position.
///
/// `eval_start` is the optional warm-up boundary: when `Some(t)`, candles before
/// `t` only fill the strategy's rolling buffer (warming the trend-filter MA) but
/// never trade, so a window's trade/signal counts reflect only the true window
/// even though earlier candles were loaded as pre-roll. `None` (production and the
/// 1m/5m/1h harnesses) lets every loaded candle trade — unchanged behavior.
fn entry_allowed_at(candle_time: DateTime<Utc>, eval_start: Option<DateTime<Utc>>) -> bool {
    match eval_start {
        Some(start) => candle_time >= start,
        None => true,
    }
}

pub fn metrics_to_json(metrics: &BacktestMetrics) -> serde_json::Value {
    serde_json::to_value(metrics).unwrap_or_else(|_| serde_json::json!({}))
}

pub fn strategy_version() -> &'static str {
    "trend_filtered_breakout_v1"
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal::prelude::ToPrimitive;

    fn candle(index: i64, close: Decimal) -> Candle {
        Candle {
            exchange: ExchangeId::Binance,
            symbol: Symbol::new("BTCUSDT"),
            timeframe: "1m".to_owned(),
            open_time: Utc.timestamp_opt(1_710_000_000 + index * 60, 0).unwrap(),
            open: close,
            high: close,
            low: close,
            close,
            volume: Decimal::ONE,
        }
    }

    // ===== Mean-reversion replay harness (test-only) =========================
    //
    // The production `run_backtest` exits ONLY on BasicRiskGate's fixed −1%/+2%
    // bracket — a 2:1 trend-CONTINUATION exit. That is structurally hostile to
    // mean reversion (verified in the design review: a counter-trend entry is
    // forced to rally +2% to win while a −1% stop sits in the direction price is
    // already falling). So mean reversion gets its own exit here, leaving the
    // production path byte-for-byte unchanged. See
    // docs/superpowers/specs/2026-06-15-mean-reversion-validation-design.md.
    //
    // Exit, per bar, in priority order (conservative-first):
    //   1. hard stop  — entry ± MR_HARD_STOP_PCT (protective, not the thesis)
    //   2. mean revert — close back through the Bollinger middle band (the SMA)
    //   3. RSI re-cross 50 — momentum has reverted
    //   4. time stop  — held MR_TIME_STOP_BARS without 1–3 firing
    //
    // Entry reuses BasicRiskGate ONLY for realistic position sizing; its SL/TP
    // are discarded in favour of the rule above.

    use trading_strategy::TechnicalStrategy;

    /// Fraction of entry price for the protective hard stop (1%). Symmetric:
    /// LONG = entry·(1−x), SHORT = entry·(1+x). Fixed by design, not swept.
    const MR_HARD_STOP_PCT: f64 = 0.01;
    /// Max bars a mean-reversion position may be held before a forced flat
    /// (60 × 1m = 1h). Prevents a stuck position from starving re-entry for a
    /// whole OOS window (the no-trade → spurious-0.00 failure mode).
    const MR_TIME_STOP_BARS: usize = 60;

    #[derive(Clone)]
    struct MrPosition {
        side: PositionSide,
        entry_price: Decimal,
        quantity: Decimal,
        hard_stop_price: Decimal,
        bars_held: usize,
    }

    /// Simple moving average of the last `period` closes (latest included), or
    /// `None` if fewer than `period` values. Mirrors the strategy crate's own
    /// `simple_moving_average` (private there); recomputed here to avoid widening
    /// that crate's public surface for a test harness.
    fn mr_sma(closes: &[f64], period: usize) -> Option<f64> {
        if period == 0 || closes.len() < period {
            return None;
        }
        let window = &closes[closes.len() - period..];
        Some(window.iter().sum::<f64>() / period as f64)
    }

    /// RSI over the last `period` deltas. Mirrors the strategy crate's
    /// `calculate_rsi` exactly (same windowing, same no-loss → 100 convention).
    fn mr_rsi(closes: &[f64], period: usize) -> Option<f64> {
        if closes.len() <= period {
            return None;
        }
        let window = &closes[closes.len() - period - 1..];
        let mut gains = 0.0;
        let mut losses = 0.0;
        for pair in window.windows(2) {
            let delta = pair[1] - pair[0];
            if delta >= 0.0 {
                gains += delta;
            } else {
                losses += -delta;
            }
        }
        if losses == 0.0 {
            return Some(100.0);
        }
        let rs = gains / losses;
        Some(100.0 - (100.0 / (1.0 + rs)))
    }

    /// Mean-reversion exit price for a position given the current candle and the
    /// rolling close history (most recent last, including the current close).
    /// `None` = hold. Priority: hard stop → mean revert → RSI 50 → time stop.
    fn mr_exit_price(
        position: &MrPosition,
        candle: &Candle,
        closes: &[f64],
        rsi_period: usize,
        bollinger_period: usize,
    ) -> Option<Decimal> {
        // 1. Hard stop (conservative: evaluated first, fills at the stop price).
        match position.side {
            PositionSide::Long if candle.low <= position.hard_stop_price => {
                return Some(position.hard_stop_price);
            }
            PositionSide::Short if candle.high >= position.hard_stop_price => {
                return Some(position.hard_stop_price);
            }
            _ => {}
        }

        let close = candle.close.to_f64()?;

        // 2. Mean revert: close back through the Bollinger middle band (SMA).
        if let Some(mid) = mr_sma(closes, bollinger_period) {
            let reverted = match position.side {
                PositionSide::Long => close >= mid,
                PositionSide::Short => close <= mid,
            };
            if reverted {
                return Some(candle.close);
            }
        }

        // 3. RSI re-crosses 50 (momentum reverted toward neutral).
        if let Some(rsi) = mr_rsi(closes, rsi_period) {
            let reverted = match position.side {
                PositionSide::Long => rsi >= 50.0,
                PositionSide::Short => rsi <= 50.0,
            };
            if reverted {
                return Some(candle.close);
            }
        }

        // 4. Time stop.
        if position.bars_held >= MR_TIME_STOP_BARS {
            return Some(candle.close);
        }

        None
    }

    /// Replays the candle DB for a mean-reversion `TechnicalStrategy`, entering on
    /// its signals (sized via BasicRiskGate) and exiting via `mr_exit_price`.
    /// Returns full metrics so the walk-forward harness can report trade counts
    /// and after-fee per-trade expectancy, not just summed PnL.
    async fn run_mean_reversion_backtest(
        pool: &PgPool,
        config: BacktestConfig,
        strategy: &TechnicalStrategy,
    ) -> Result<BacktestMetrics> {
        let config = config.normalized();
        let exchange = config.exchange_id()?;
        let exchange_name = config.exchange.clone().unwrap();
        let symbols = config.symbols.clone().unwrap();
        let timeframe = config.timeframe.clone().unwrap();
        let period_start = config.period_start.unwrap();
        let period_end = config.period_end.unwrap();
        let initial_equity = config.initial_equity.unwrap();
        let daily_loss_limit = config.daily_loss_limit.unwrap();
        let cost_multiplier =
            Decimal::from_f64_retain(config.cost_multiplier.unwrap_or(1.0)).unwrap_or(Decimal::ONE);

        let candles = load_candles(
            pool,
            &exchange_name,
            &symbols,
            &timeframe,
            period_start,
            period_end,
            exchange,
        )
        .await?;
        if candles.is_empty() {
            return Err(TradingError::Configuration(
                "backtest requires candles for the requested period".to_owned(),
            ));
        }

        let rsi_period = strategy.rsi_period();
        let bollinger_period = strategy.bollinger_period();
        let risk_gate = BasicRiskGate::default();
        let mut equity = initial_equity;
        let mut peak_equity = initial_equity;
        let mut max_drawdown = Decimal::ZERO;
        let mut trades = 0u64;
        let mut wins = 0u64;
        let mut losses = 0u64;
        let mut signals_seen = 0u64;
        let mut open: HashMap<Symbol, MrPosition> = HashMap::new();
        let mut close_f64_by_symbol: HashMap<Symbol, Vec<f64>> = HashMap::new();
        let mut history_by_symbol: HashMap<Symbol, Vec<Candle>> = HashMap::new();

        for candle in &candles {
            let closes = close_f64_by_symbol
                .entry(candle.symbol.clone())
                .or_default();
            if let Some(value) = candle.close.to_f64() {
                closes.push(value);
            }
            if closes.len() > BACKTEST_HISTORY_LIMIT {
                closes.remove(0);
            }
            let closes_snapshot = closes.clone();

            // --- exit check on the open position for this symbol -------------
            if let Some(position) = open.get_mut(&candle.symbol) {
                position.bars_held += 1;
                if let Some(exit) = mr_exit_price(
                    position,
                    candle,
                    &closes_snapshot,
                    rsi_period,
                    bollinger_period,
                ) {
                    let pnl = mr_position_pnl(position, exit)
                        - trade_cost(
                            position.entry_price,
                            exit,
                            position.quantity,
                            cost_multiplier,
                        );
                    equity += pnl;
                    trades += 1;
                    if pnl >= Decimal::ZERO {
                        wins += 1;
                    } else {
                        losses += 1;
                    }
                    open.remove(&candle.symbol);
                }
            }

            if equity > peak_equity {
                peak_equity = equity;
            }
            let drawdown = peak_equity - equity;
            if drawdown > max_drawdown {
                max_drawdown = drawdown;
            }

            // --- entry --------------------------------------------------------
            let history = history_by_symbol.entry(candle.symbol.clone()).or_default();
            history.push(candle.clone());
            if history.len() > BACKTEST_HISTORY_LIMIT {
                history.remove(0);
            }

            let signals = strategy.evaluate(history);
            signals_seen += signals.len() as u64;
            for signal in signals {
                if open.contains_key(&signal.symbol) {
                    continue;
                }
                if let Some(sized) =
                    build_position(&risk_gate, &signal, candle.close, equity, daily_loss_limit)?
                {
                    let entry = candle.close;
                    let stop = Decimal::from_f64_retain(MR_HARD_STOP_PCT).unwrap_or(Decimal::ZERO);
                    let hard_stop_price = match sized.side {
                        PositionSide::Long => entry * (Decimal::ONE - stop),
                        PositionSide::Short => entry * (Decimal::ONE + stop),
                    };
                    open.insert(
                        signal.symbol.clone(),
                        MrPosition {
                            side: sized.side,
                            entry_price: entry,
                            quantity: sized.quantity,
                            hard_stop_price,
                            bars_held: 0,
                        },
                    );
                }
            }
        }

        // Mark-to-close leftovers at each symbol's last candle (cost-netted).
        for (symbol, position) in &open {
            if let Some(last) = candles.iter().rev().find(|c| &c.symbol == symbol) {
                equity += mr_position_pnl(position, last.close)
                    - trade_cost(
                        position.entry_price,
                        last.close,
                        position.quantity,
                        cost_multiplier,
                    );
            }
        }

        let realized_pnl = equity - initial_equity;
        let max_drawdown_pct = if peak_equity > Decimal::ZERO {
            max_drawdown / peak_equity * Decimal::new(100, 0)
        } else {
            Decimal::ZERO
        };

        Ok(BacktestMetrics {
            exchange: exchange_name,
            symbols,
            timeframe,
            period_start,
            period_end,
            initial_equity,
            final_equity: equity,
            realized_pnl,
            max_drawdown,
            max_drawdown_pct,
            trades,
            wins,
            losses,
            candles_loaded: candles.len(),
            signals_seen,
        })
    }

    fn mr_position_pnl(position: &MrPosition, exit_price: Decimal) -> Decimal {
        match position.side {
            PositionSide::Long => (exit_price - position.entry_price) * position.quantity,
            PositionSide::Short => (position.entry_price - exit_price) * position.quantity,
        }
    }

    #[test]
    fn entry_allowed_at_gates_only_pre_eval_start_candles() {
        let boundary = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let before = Utc.with_ymd_and_hms(2024, 12, 31, 0, 0, 0).unwrap();
        let after = Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap();

        // No eval_start (production / 1m–1h paths): every candle may trade.
        assert!(entry_allowed_at(before, None));
        assert!(entry_allowed_at(boundary, None));
        assert!(entry_allowed_at(after, None));

        // With eval_start, only candles AT OR AFTER the boundary may enter; the
        // earlier pre-roll candles warm the buffer but never trade.
        assert!(!entry_allowed_at(before, Some(boundary)));
        assert!(entry_allowed_at(boundary, Some(boundary)));
        assert!(entry_allowed_at(after, Some(boundary)));
    }

    #[test]
    fn long_exit_prefers_stop_loss_when_same_candle_touches_both_sides() {
        let position = BacktestPosition {
            symbol: Symbol::new("BTCUSDT"),
            side: PositionSide::Long,
            entry_price: Decimal::new(100, 0),
            quantity: Decimal::ONE,
            stop_loss_price: Decimal::new(99, 0),
            take_profit_price: Decimal::new(102, 0),
        };
        let mut range = candle(1, Decimal::new(100, 0));
        range.low = Decimal::new(98, 0);
        range.high = Decimal::new(103, 0);

        assert_eq!(exit_price(&position, &range), Some(Decimal::new(99, 0)));
    }

    #[test]
    fn trade_cost_charges_both_legs_fee_and_slippage() {
        // entry 100, exit 110, qty 2.
        // entry_notional = 200, exit_notional = 220, sum = 420.
        // rate = taker 0.0004 + slippage 0.0001 = 0.0005.
        // cost = 420 * 0.0005 = 0.21.
        let cost = trade_cost(
            Decimal::new(100, 0),
            Decimal::new(110, 0),
            Decimal::new(2, 0),
            Decimal::ONE,
        );
        assert_eq!(cost, Decimal::new(21, 2)); // 0.21
    }

    #[test]
    fn trade_cost_scales_with_multiplier() {
        // Same trade at 3x cost → 0.21 * 3 = 0.63.
        let cost = trade_cost(
            Decimal::new(100, 0),
            Decimal::new(110, 0),
            Decimal::new(2, 0),
            Decimal::new(3, 0),
        );
        assert_eq!(cost, Decimal::new(63, 2)); // 0.63
    }

    #[test]
    fn trade_cost_is_zero_for_zero_quantity() {
        let cost = trade_cost(
            Decimal::new(100, 0),
            Decimal::new(110, 0),
            Decimal::ZERO,
            Decimal::ONE,
        );
        assert_eq!(cost, Decimal::ZERO);
    }

    #[test]
    fn max_drawdown_percentage_uses_peak_equity() {
        let peak = Decimal::new(10_000, 0);
        let drawdown = Decimal::new(250, 0);
        let pct = drawdown / peak * Decimal::new(100, 0);

        assert_eq!(pct.to_f64().unwrap(), 2.5);
    }

    // ----- Mean-reversion exit unit tests ------------------------------------

    fn mr_long(entry: i64, hard_stop: Decimal) -> MrPosition {
        MrPosition {
            side: PositionSide::Long,
            entry_price: Decimal::new(entry, 0),
            quantity: Decimal::ONE,
            hard_stop_price: hard_stop,
            bars_held: 0,
        }
    }

    #[test]
    fn mr_exit_hard_stop_fires_first_even_when_other_rules_would() {
        // A long whose low pierces the hard stop exits AT the stop price, taking
        // priority over any mean-revert/RSI signal in the same bar.
        let position = mr_long(100, Decimal::new(99, 0));
        let mut c = candle(1, Decimal::new(101, 0)); // close above → would mean-revert
        c.low = Decimal::new(98, 0); // but low pierced the stop
                                     // closes that would otherwise trigger mean-revert (close >= SMA).
        let closes = vec![100.0, 100.0, 101.0];
        assert_eq!(
            mr_exit_price(&position, &c, &closes, 14, 2),
            Some(Decimal::new(99, 0))
        );
    }

    #[test]
    fn mr_exit_mean_revert_closes_long_at_close_when_back_above_band() {
        // No stop breach; close (102) is at/above the 2-period SMA of [100,102]
        // = 101 → revert exit at the close price.
        let position = mr_long(100, Decimal::new(90, 0));
        let c = candle(1, Decimal::new(102, 0));
        let closes = vec![100.0, 102.0];
        assert_eq!(
            mr_exit_price(&position, &c, &closes, 14, 2),
            Some(Decimal::new(102, 0))
        );
    }

    #[test]
    fn mr_exit_holds_when_below_band_and_rsi_low_and_within_time() {
        // Long still underwater: close below the SMA, RSI low, bars within limit,
        // no stop breach → hold (None).
        let position = mr_long(100, Decimal::new(90, 0));
        let c = candle(1, Decimal::new(95, 0));
        // SMA of [100, 95] = 97.5 > close 95 → not reverted. RSI over a steadily
        // falling series is < 50.
        let closes = vec![110.0, 108.0, 106.0, 104.0, 102.0, 100.0, 95.0];
        assert_eq!(mr_exit_price(&position, &c, &closes, 3, 2), None);
    }

    #[test]
    fn mr_exit_time_stop_forces_flat_after_limit() {
        // Underwater long, but bars_held has hit the limit → forced exit at close.
        let mut position = mr_long(100, Decimal::new(90, 0));
        position.bars_held = MR_TIME_STOP_BARS;
        let c = candle(1, Decimal::new(95, 0));
        let closes = vec![110.0, 108.0, 106.0, 104.0, 102.0, 100.0, 95.0];
        assert_eq!(
            mr_exit_price(&position, &c, &closes, 3, 2),
            Some(Decimal::new(95, 0))
        );
    }

    // ----- Walk-forward falsification harness --------------------------------
    //
    // This is NOT a pass/fail unit test. It is a permanent, #[ignore]-d probe
    // that replays the real candle DB to ask, honestly: does the trend-filtered
    // breakout have an out-of-sample edge AFTER fees? The prior pure-breakout
    // hypothesis failed this exact test at OOS +0.00%; the expected result here
    // is also ~0%. A near-zero result is a SUCCESSFUL falsification (no edge ->
    // STOP), not a prompt to add a 4th knob. The grid is fixed by the design
    // spec; do NOT widen it after seeing OOS numbers (that is re-overfitting).
    //
    // Run: `cargo test -p trading-api --bin trading-api \
    //        backtest_runner::tests::walk_forward_trend_filter -- --ignored --nocapture`
    // Reads the production candle DB via DATABASE_URL (3yr binance 1m loaded).

    use sqlx::postgres::PgPoolOptions;

    #[derive(Clone, Copy)]
    struct Combo {
        lookback: usize,
        k: f64,
        ma_period: usize,
    }

    /// Fixed grid (spec §5): lookback × k × ma_period, keeping only ma_period >
    /// lookback so the trend filter is actually longer-horizon than the breakout
    /// window (combos where it is not just rediscover the failed pure breakout).
    fn walk_forward_grid() -> Vec<Combo> {
        let mut combos = Vec::new();
        for &lookback in &[10usize, 20] {
            for &k in &[0.3f64, 0.5, 0.7] {
                for &ma_period in &[30usize, 50] {
                    if ma_period > lookback {
                        combos.push(Combo {
                            lookback,
                            k,
                            ma_period,
                        });
                    }
                }
            }
        }
        combos
    }

    async fn run_combo(
        pool: &PgPool,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        combo: Combo,
        cost_multiplier: f64,
    ) -> Decimal {
        let config = BacktestConfig {
            exchange: Some("binance".to_owned()),
            symbols: Some(vec!["BTCUSDT".to_owned(), "ETHUSDT".to_owned()]),
            timeframe: Some("1m".to_owned()),
            period_start: Some(start),
            period_end: Some(end),
            initial_equity: Some(Decimal::new(10_000, 0)),
            daily_loss_limit: Some(Decimal::new(500, 0)),
            lookback: Some(combo.lookback),
            k: Some(combo.k),
            ma_period: Some(combo.ma_period),
            cost_multiplier: Some(cost_multiplier),
            eval_start: None,
        };
        match run_backtest(pool, config).await {
            Ok(metrics) => metrics.realized_pnl,
            Err(error) => {
                eprintln!("  combo run failed ({error}); treating as 0");
                Decimal::ZERO
            }
        }
    }

    fn median(mut values: Vec<f64>) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mid = values.len() / 2;
        if values.len() % 2 == 0 {
            (values[mid - 1] + values[mid]) / 2.0
        } else {
            values[mid]
        }
    }

    #[tokio::test]
    #[ignore = "walk-forward replay over the production candle DB; run explicitly"]
    async fn walk_forward_trend_filter() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping walk-forward; DATABASE_URL is not set");
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url)
            .await
            .expect("connect candle database");

        let grid = walk_forward_grid();
        eprintln!(
            "=== Walk-forward falsification: trend-filtered breakout ===\n\
             grid = {} combos (ma_period > lookback), 4 windows, IS 4mo -> OOS 2mo, BTC+ETH, fee-aware",
            grid.len()
        );

        // 4 rolling windows ending 2026-06. Each: IS = 4 months, OOS = next 2 months.
        let windows: [(i32, u32, i32, u32, i32, u32); 4] = [
            (2024, 6, 2024, 10, 2024, 12), // IS 2024-06..10, OOS 2024-10..12
            (2025, 1, 2025, 5, 2025, 7),   // IS 2025-01..05, OOS 2025-05..07
            (2025, 8, 2025, 12, 2026, 2),  // IS 2025-08..12, OOS 2025-12..2026-02
            (2026, 1, 2026, 5, 2026, 7),   // IS 2026-01..05, OOS 2026-05..07 (clamped to now)
        ];

        let mut oos_results: Vec<f64> = Vec::new();
        let now = Utc::now();

        for (idx, w) in windows.iter().enumerate() {
            let is_start = Utc.with_ymd_and_hms(w.0, w.1, 1, 0, 0, 0).unwrap();
            let oos_start = Utc.with_ymd_and_hms(w.2, w.3, 1, 0, 0, 0).unwrap();
            let mut oos_end = Utc.with_ymd_and_hms(w.4, w.5, 1, 0, 0, 0).unwrap();
            if oos_end > now {
                oos_end = now;
            }

            // IS grid: pick the combo with the highest in-sample PnL (fee-aware).
            let mut is_pnls: Vec<(Combo, f64)> = Vec::new();
            for &combo in &grid {
                let pnl = run_combo(&pool, is_start, oos_start, combo, 1.0).await;
                is_pnls.push((combo, pnl.to_f64().unwrap_or(0.0)));
            }
            let (best, best_is_pnl) = is_pnls
                .iter()
                .copied()
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .expect("non-empty grid");

            // OOS with the IS-best combo, at 1x / 2x / 3x cost (sensitivity).
            let oos_1x = run_combo(&pool, oos_start, oos_end, best, 1.0).await;
            let oos_2x = run_combo(&pool, oos_start, oos_end, best, 2.0).await;
            let oos_3x = run_combo(&pool, oos_start, oos_end, best, 3.0).await;
            let oos_1x_f = oos_1x.to_f64().unwrap_or(0.0);
            oos_results.push(oos_1x_f);

            eprintln!(
                "\nWindow {}: IS {}..{} -> OOS {}..{}",
                idx + 1,
                is_start.date_naive(),
                oos_start.date_naive(),
                oos_start.date_naive(),
                oos_end.date_naive()
            );
            eprintln!(
                "  IS-best: lookback={} k={} ma_period={}  IS PnL={:.2}",
                best.lookback, best.k, best.ma_period, best_is_pnl
            );
            eprintln!(
                "  OOS PnL: 1x={:.2}  2x={:.2}  3x={:.2}  (cost sensitivity)",
                oos_1x_f,
                oos_2x.to_f64().unwrap_or(0.0),
                oos_3x.to_f64().unwrap_or(0.0)
            );
            // Full IS grid for plateau-vs-spike inspection.
            eprintln!("  IS grid (combo -> PnL):");
            for (combo, pnl) in &is_pnls {
                eprintln!(
                    "    lb={:>2} k={:.1} ma={:>2} -> {:.2}",
                    combo.lookback, combo.k, combo.ma_period, pnl
                );
            }
        }

        let n = oos_results.len() as f64;
        let mean = oos_results.iter().sum::<f64>() / n;
        let med = median(oos_results.clone());
        let worst = oos_results.iter().copied().fold(f64::INFINITY, f64::min);
        let positive = oos_results.iter().filter(|&&p| p > 0.0).count();

        eprintln!("\n=== OOS SUMMARY (1x cost, the honest numbers) ===");
        eprintln!("  per-window: {oos_results:?}");
        eprintln!("  mean={mean:.2}  median={med:.2}  worst={worst:.2}  positive={positive}/4");
        eprintln!(
            "  NOTE: '>=3/4 positive' happens 31.2% of the time under pure noise. \
             Pass requires ALL 4 positive AND mean > 2x round-trip cost AND survives 2x cost. \
             Near-zero = successful falsification (no edge) -> STOP, do not widen the grid."
        );
    }

    // ----- Mean-reversion walk-forward harness -------------------------------
    //
    // Honestly tests whether RSI+Bollinger mean reversion has an OOS edge AFTER
    // fees, using a THESIS-APPROPRIATE exit (mid-band revert / RSI-50 / hard stop
    // / time stop — see run_mean_reversion_backtest), unlike the breakout harness
    // which exits on a fixed 2:1 bracket. Design + pre-registered verdict:
    // docs/superpowers/specs/2026-06-15-mean-reversion-validation-design.md.
    //
    // This is the 3rd strategy family tested on the SAME candle DB / BTC+ETH /
    // 4 contiguous 2024–2026 windows. A marginal pass is NOT adoption — it
    // demands a fresh, never-used holdout window first (spec §7).
    //
    // Run: `cargo test -p trading-api --bin trading-api \
    //        backtest_runner::tests::walk_forward_mean_reversion -- --ignored --nocapture`

    #[derive(Clone, Copy)]
    struct MrCombo {
        rsi_period: usize,
        bollinger_period: usize,
        oversold: f64,
        overbought: f64,
    }

    /// Fixed grid (spec §3): 3 × 3 × 3 = 27 combos. Bollinger std-dev multiplier
    /// stays 2.0 and the exit params are fixed — only the ENTRY signal is swept.
    /// Do NOT widen after seeing numbers.
    fn mr_grid() -> Vec<MrCombo> {
        let mut combos = Vec::new();
        for &rsi_period in &[7usize, 14, 21] {
            for &bollinger_period in &[14usize, 20, 30] {
                for &(oversold, overbought) in &[(30.0, 70.0), (20.0, 80.0), (25.0, 75.0)] {
                    combos.push(MrCombo {
                        rsi_period,
                        bollinger_period,
                        oversold,
                        overbought,
                    });
                }
            }
        }
        combos
    }

    /// Minimum closed trades for a window's verdict to count. Below it the
    /// window is INCONCLUSIVE (no-data), never "no edge" (spec §1).
    const MR_MIN_TRADES: u64 = 20;

    async fn run_mr_combo(
        pool: &PgPool,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        combo: MrCombo,
        cost_multiplier: f64,
    ) -> BacktestMetrics {
        let config = BacktestConfig {
            exchange: Some("binance".to_owned()),
            symbols: Some(vec!["BTCUSDT".to_owned(), "ETHUSDT".to_owned()]),
            timeframe: Some("1m".to_owned()),
            period_start: Some(start),
            period_end: Some(end),
            initial_equity: Some(Decimal::new(10_000, 0)),
            daily_loss_limit: Some(Decimal::new(500, 0)),
            lookback: None,
            k: None,
            ma_period: None,
            cost_multiplier: Some(cost_multiplier),
            eval_start: None,
        };
        let strategy = TechnicalStrategy::new(
            combo.rsi_period,
            combo.bollinger_period,
            combo.oversold,
            combo.overbought,
        );
        run_mean_reversion_backtest(pool, config, &strategy)
            .await
            .unwrap_or_else(|error| {
                eprintln!("  mr combo run failed ({error}); treating as empty");
                BacktestMetrics {
                    exchange: "binance".to_owned(),
                    symbols: vec![],
                    timeframe: "1m".to_owned(),
                    period_start: start,
                    period_end: end,
                    initial_equity: Decimal::new(10_000, 0),
                    final_equity: Decimal::new(10_000, 0),
                    realized_pnl: Decimal::ZERO,
                    max_drawdown: Decimal::ZERO,
                    max_drawdown_pct: Decimal::ZERO,
                    trades: 0,
                    wins: 0,
                    losses: 0,
                    candles_loaded: 0,
                    signals_seen: 0,
                }
            })
    }

    fn pnl_f64(m: &BacktestMetrics) -> f64 {
        m.realized_pnl.to_f64().unwrap_or(0.0)
    }

    fn expectancy(m: &BacktestMetrics) -> f64 {
        if m.trades == 0 {
            0.0
        } else {
            pnl_f64(m) / m.trades as f64
        }
    }

    #[tokio::test]
    #[ignore = "walk-forward replay over the production candle DB; run explicitly"]
    async fn walk_forward_mean_reversion() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping walk-forward; DATABASE_URL is not set");
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url)
            .await
            .expect("connect candle database");

        let grid = mr_grid();
        eprintln!(
            "=== Walk-forward: mean reversion (RSI+Bollinger, mid-band/RSI50 exit) ===\n\
             grid = {} combos, 4 windows, IS 4mo -> OOS 2mo, BTC+ETH, fee-aware.\n\
             exit: hard stop {:.0}% / mean-revert to SMA / RSI-50 / time stop {} bars.\n\
             min trades/window for a verdict = {}.",
            grid.len(),
            MR_HARD_STOP_PCT * 100.0,
            MR_TIME_STOP_BARS,
            MR_MIN_TRADES,
        );

        let windows: [(i32, u32, i32, u32, i32, u32); 4] = [
            (2024, 6, 2024, 10, 2024, 12),
            (2025, 1, 2025, 5, 2025, 7),
            (2025, 8, 2025, 12, 2026, 2),
            (2026, 1, 2026, 5, 2026, 7),
        ];

        let mut oos_results: Vec<f64> = Vec::new();
        let mut oos_expectancies: Vec<f64> = Vec::new();
        let mut window_trade_counts: Vec<u64> = Vec::new();
        let mut control_results: Vec<f64> = Vec::new();
        let now = Utc::now();

        for (idx, w) in windows.iter().enumerate() {
            let is_start = Utc.with_ymd_and_hms(w.0, w.1, 1, 0, 0, 0).unwrap();
            let oos_start = Utc.with_ymd_and_hms(w.2, w.3, 1, 0, 0, 0).unwrap();
            let mut oos_end = Utc.with_ymd_and_hms(w.4, w.5, 1, 0, 0, 0).unwrap();
            if oos_end > now {
                oos_end = now;
            }

            // IS grid: among combos clearing the IS min-trades floor, pick the
            // highest in-sample PnL (spec §5 — guards against crowning a combo
            // that simply never traded).
            let mut is_metrics: Vec<(MrCombo, BacktestMetrics)> = Vec::new();
            for &combo in &grid {
                let m = run_mr_combo(&pool, is_start, oos_start, combo, 1.0).await;
                is_metrics.push((combo, m));
            }
            let eligible: Vec<&(MrCombo, BacktestMetrics)> = is_metrics
                .iter()
                .filter(|(_, m)| m.trades >= MR_MIN_TRADES)
                .collect();
            let pick = eligible
                .iter()
                .copied()
                .max_by(|a, b| pnl_f64(&a.1).partial_cmp(&pnl_f64(&b.1)).unwrap());
            let Some((best, best_is)) = pick else {
                eprintln!(
                    "\nWindow {}: IS {}..{} -> OOS {}..{}\n  NO IS combo cleared {} trades — INCONCLUSIVE window.",
                    idx + 1,
                    is_start.date_naive(),
                    oos_start.date_naive(),
                    oos_start.date_naive(),
                    oos_end.date_naive(),
                    MR_MIN_TRADES,
                );
                oos_results.push(0.0);
                oos_expectancies.push(0.0);
                window_trade_counts.push(0);
                control_results.push(0.0);
                continue;
            };

            let oos_1x = run_mr_combo(&pool, oos_start, oos_end, *best, 1.0).await;
            let oos_2x = run_mr_combo(&pool, oos_start, oos_end, *best, 2.0).await;
            let oos_3x = run_mr_combo(&pool, oos_start, oos_end, *best, 3.0).await;
            let oos_1x_pnl = pnl_f64(&oos_1x);
            oos_results.push(oos_1x_pnl);
            oos_expectancies.push(expectancy(&oos_1x));
            window_trade_counts.push(oos_1x.trades);

            // Control: same IS-best entry under a symmetric 1:1 bracket via the
            // breakout runner (TechnicalStrategy + BasicRiskGate's 1%/2% would not
            // be 1:1; instead reuse run_backtest's fixed bracket is breakout's job).
            // Here the control is the OOS PnL at 1x with the time/mean exit but we
            // report expectancy to separate edge from asymmetry — a negative
            // expectancy here means the ENTRY, not an exit asymmetry, is the issue.
            control_results.push(expectancy(&oos_1x));

            eprintln!(
                "\nWindow {}: IS {}..{} -> OOS {}..{}",
                idx + 1,
                is_start.date_naive(),
                oos_start.date_naive(),
                oos_start.date_naive(),
                oos_end.date_naive()
            );
            eprintln!(
                "  IS-best: rsi={} bb={} oversold={:.0} overbought={:.0}  IS PnL={:.2} IS trades={}",
                best.rsi_period,
                best.bollinger_period,
                best.oversold,
                best.overbought,
                pnl_f64(best_is),
                best_is.trades,
            );
            eprintln!(
                "  OOS PnL: 1x={:.2}  2x={:.2}  3x={:.2}  | OOS trades={} wins={} losses={} expectancy={:.4}",
                oos_1x_pnl,
                pnl_f64(&oos_2x),
                pnl_f64(&oos_3x),
                oos_1x.trades,
                oos_1x.wins,
                oos_1x.losses,
                expectancy(&oos_1x),
            );
            eprintln!("  IS grid (combo -> PnL, trades):");
            for (combo, m) in &is_metrics {
                eprintln!(
                    "    rsi={:>2} bb={:>2} os={:.0} ob={:.0} -> {:.2} (trades={})",
                    combo.rsi_period,
                    combo.bollinger_period,
                    combo.oversold,
                    combo.overbought,
                    pnl_f64(m),
                    m.trades,
                );
            }
        }

        let n = oos_results.len() as f64;
        let mean = oos_results.iter().sum::<f64>() / n;
        let med = median(oos_results.clone());
        let worst = oos_results.iter().copied().fold(f64::INFINITY, f64::min);
        let positive = oos_results.iter().filter(|&&p| p > 0.0).count();
        let below_floor = window_trade_counts
            .iter()
            .filter(|&&t| t < MR_MIN_TRADES)
            .count();
        let mean_expectancy = oos_expectancies.iter().sum::<f64>() / n;
        let positive_expectancy = oos_expectancies.iter().filter(|&&e| e > 0.0).count();

        eprintln!("\n=== OOS SUMMARY (1x cost, the honest numbers) ===");
        eprintln!("  per-window PnL:        {oos_results:?}");
        eprintln!("  per-window expectancy: {oos_expectancies:?}");
        eprintln!("  per-window trades:     {window_trade_counts:?}");
        eprintln!(
            "  mean={mean:.2} median={med:.2} worst={worst:.2} positive={positive}/4 \
             | mean_expectancy={mean_expectancy:.4} positive_expectancy={positive_expectancy}/4 \
             | windows_below_{MR_MIN_TRADES}_trades={below_floor}"
        );
        eprintln!("  control (expectancy = edge net of exit asymmetry): {control_results:?}");

        // Three-way verdict (spec §1), pre-registered before any numbers.
        let verdict = if below_floor > 0 {
            "INCONCLUSIVE — at least one window below the trade floor; the harness \
             could not fairly test mean reversion. NOT a falsification."
        } else if positive == 4 && positive_expectancy == 4 && mean > 0.0 {
            "POSSIBLE EDGE — all 4 windows positive with positive expectancy. Per spec §7 \
             this is the 3rd family on the same data; do NOT adopt. Require a fresh holdout \
             window + 2x-cost survival before any testnet move."
        } else {
            "FAIL (no edge) — sufficient trades but not consistently positive after fees. \
             Clean falsification -> STOP. Do NOT widen the grid or retune."
        };
        eprintln!("\n=== VERDICT ===\n  {verdict}");
    }

    // ----- Trend-filtered breakout on higher timeframes (5m / 1h) ------------
    //
    // The 1m families (pure breakout, trend-filtered breakout, mean reversion)
    // all died with no OOS edge, the mean-reversion run showing loss scaling with
    // trade count = fee drag. Higher timeframes trade far less often, directly
    // cutting that drag — so this re-runs the EXISTING trend-filtered breakout
    // grid on 5m and 1h candles (rolled up from 1m, materialized in the DB).
    //
    // Same pre-registered grid (walk_forward_grid), windows, IS-best selector,
    // cost sensitivity, and three-way verdict as the MR harness. Higher timeframes
    // mean fewer trades, so the MR_MIN_TRADES floor matters MORE here: a window
    // below it is INCONCLUSIVE (no-data), never "no edge". Do NOT widen the grid.
    //
    // Run: `cargo test -p trading-api --bin trading-api \
    //        backtest_runner::tests::walk_forward_breakout_higher_timeframes -- --ignored --nocapture`

    #[allow(clippy::too_many_arguments)]
    async fn run_combo_tf(
        pool: &PgPool,
        timeframe: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        combo: Combo,
        cost_multiplier: f64,
        pre_roll_days: Option<i64>,
    ) -> BacktestMetrics {
        // With pre-roll (daily harness), load `pre_roll_days` of extra history
        // before `start` to warm the SMA buffer, but only trade from `start` on
        // (eval_start = start). Without it (5m/1h), behavior is unchanged: the
        // loaded window is exactly [start, end) and every candle may trade.
        let (period_start, eval_start) = match pre_roll_days {
            Some(days) => (start - Duration::days(days), Some(start)),
            None => (start, None),
        };
        let config = BacktestConfig {
            exchange: Some("binance".to_owned()),
            symbols: Some(vec!["BTCUSDT".to_owned(), "ETHUSDT".to_owned()]),
            timeframe: Some(timeframe.to_owned()),
            period_start: Some(period_start),
            period_end: Some(end),
            initial_equity: Some(Decimal::new(10_000, 0)),
            daily_loss_limit: Some(Decimal::new(500, 0)),
            lookback: Some(combo.lookback),
            k: Some(combo.k),
            ma_period: Some(combo.ma_period),
            cost_multiplier: Some(cost_multiplier),
            eval_start,
        };
        run_backtest(pool, config).await.unwrap_or_else(|error| {
            eprintln!("  combo run failed ({error}); treating as empty");
            BacktestMetrics {
                exchange: "binance".to_owned(),
                symbols: vec![],
                timeframe: timeframe.to_owned(),
                period_start: start,
                period_end: end,
                initial_equity: Decimal::new(10_000, 0),
                final_equity: Decimal::new(10_000, 0),
                realized_pnl: Decimal::ZERO,
                max_drawdown: Decimal::ZERO,
                max_drawdown_pct: Decimal::ZERO,
                trades: 0,
                wins: 0,
                losses: 0,
                candles_loaded: 0,
                signals_seen: 0,
            }
        })
    }

    /// One full 4-window walk-forward for the trend-filtered breakout on a single
    /// timeframe. Returns nothing; prints diagnostics + a three-way verdict.
    async fn walk_forward_breakout_for_timeframe(pool: &PgPool, timeframe: &str) {
        let grid = walk_forward_grid();
        // OOS widened to 3 months (vs the 1m harness's 2). This is NOT a results-
        // driven rule change: the prior 1h run was INCONCLUSIVE because window 2
        // had only 16 trades (< the 20 floor); a 3mo OOS lifts every window above
        // the floor (W2 ~24 trades) WITHOUT touching the entry grid. IS length,
        // window count, and the grid are unchanged. W4 still clamps to "now".
        let windows: [(i32, u32, i32, u32, i32, u32); 4] = [
            (2024, 6, 2024, 10, 2025, 1),
            (2025, 1, 2025, 5, 2025, 8),
            (2025, 8, 2025, 12, 2026, 3),
            (2026, 1, 2026, 5, 2026, 8),
        ];
        let now = Utc::now();

        eprintln!(
            "\n############ timeframe = {timeframe} ############\n\
             grid = {} combos, 4 windows IS 4mo -> OOS 3mo, BTC+ETH, fee-aware, \
             min trades/window = {}.",
            grid.len(),
            MR_MIN_TRADES,
        );

        let mut oos_results: Vec<f64> = Vec::new();
        let mut oos_expectancies: Vec<f64> = Vec::new();
        let mut window_trade_counts: Vec<u64> = Vec::new();

        for (idx, w) in windows.iter().enumerate() {
            let is_start = Utc.with_ymd_and_hms(w.0, w.1, 1, 0, 0, 0).unwrap();
            let oos_start = Utc.with_ymd_and_hms(w.2, w.3, 1, 0, 0, 0).unwrap();
            let mut oos_end = Utc.with_ymd_and_hms(w.4, w.5, 1, 0, 0, 0).unwrap();
            if oos_end > now {
                oos_end = now;
            }

            // IS-best among combos clearing the trade floor (same rule as MR).
            let mut is_metrics: Vec<(Combo, BacktestMetrics)> = Vec::new();
            for &combo in &grid {
                let m = run_combo_tf(pool, timeframe, is_start, oos_start, combo, 1.0, None).await;
                is_metrics.push((combo, m));
            }
            let pick = is_metrics
                .iter()
                .filter(|(_, m)| m.trades >= MR_MIN_TRADES)
                .max_by(|a, b| pnl_f64(&a.1).partial_cmp(&pnl_f64(&b.1)).unwrap());
            let Some((best, best_is)) = pick else {
                eprintln!(
                    "\nWindow {}: IS {}..{} -> OOS {}..{}\n  NO IS combo cleared {} trades — INCONCLUSIVE window.",
                    idx + 1,
                    is_start.date_naive(),
                    oos_start.date_naive(),
                    oos_start.date_naive(),
                    oos_end.date_naive(),
                    MR_MIN_TRADES,
                );
                oos_results.push(0.0);
                oos_expectancies.push(0.0);
                window_trade_counts.push(0);
                continue;
            };

            let oos_1x = run_combo_tf(pool, timeframe, oos_start, oos_end, *best, 1.0, None).await;
            let oos_2x = run_combo_tf(pool, timeframe, oos_start, oos_end, *best, 2.0, None).await;
            let oos_3x = run_combo_tf(pool, timeframe, oos_start, oos_end, *best, 3.0, None).await;
            oos_results.push(pnl_f64(&oos_1x));
            oos_expectancies.push(expectancy(&oos_1x));
            window_trade_counts.push(oos_1x.trades);

            eprintln!(
                "\nWindow {}: IS {}..{} -> OOS {}..{}",
                idx + 1,
                is_start.date_naive(),
                oos_start.date_naive(),
                oos_start.date_naive(),
                oos_end.date_naive()
            );
            eprintln!(
                "  IS-best: lookback={} k={} ma_period={}  IS PnL={:.2} IS trades={}",
                best.lookback,
                best.k,
                best.ma_period,
                pnl_f64(best_is),
                best_is.trades,
            );
            eprintln!(
                "  OOS PnL: 1x={:.2}  2x={:.2}  3x={:.2}  | OOS trades={} wins={} losses={} expectancy={:.4}",
                pnl_f64(&oos_1x),
                pnl_f64(&oos_2x),
                pnl_f64(&oos_3x),
                oos_1x.trades,
                oos_1x.wins,
                oos_1x.losses,
                expectancy(&oos_1x),
            );
        }

        let n = oos_results.len() as f64;
        let mean = oos_results.iter().sum::<f64>() / n;
        let med = median(oos_results.clone());
        let worst = oos_results.iter().copied().fold(f64::INFINITY, f64::min);
        let positive = oos_results.iter().filter(|&&p| p > 0.0).count();
        let below_floor = window_trade_counts
            .iter()
            .filter(|&&t| t < MR_MIN_TRADES)
            .count();
        let positive_expectancy = oos_expectancies.iter().filter(|&&e| e > 0.0).count();

        eprintln!("\n=== {timeframe} OOS SUMMARY (1x cost) ===");
        eprintln!("  per-window PnL:        {oos_results:?}");
        eprintln!("  per-window expectancy: {oos_expectancies:?}");
        eprintln!("  per-window trades:     {window_trade_counts:?}");
        eprintln!(
            "  mean={mean:.2} median={med:.2} worst={worst:.2} positive={positive}/4 \
             positive_expectancy={positive_expectancy}/4 windows_below_{MR_MIN_TRADES}_trades={below_floor}"
        );

        let verdict = if below_floor > 0 {
            "INCONCLUSIVE — at least one window below the trade floor; too few trades \
             to test fairly on this timeframe. NOT a falsification."
        } else if positive == 4 && positive_expectancy == 4 && mean > 0.0 {
            "POSSIBLE EDGE — all 4 windows positive with positive expectancy. Do NOT adopt; \
             require a fresh holdout window + 2x-cost survival first."
        } else {
            "FAIL (no edge) — sufficient trades but not consistently positive after fees. \
             Clean falsification -> STOP."
        };
        eprintln!("=== {timeframe} VERDICT ===\n  {verdict}");
    }

    #[tokio::test]
    #[ignore = "walk-forward replay over the production candle DB; run explicitly"]
    async fn walk_forward_breakout_higher_timeframes() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping walk-forward; DATABASE_URL is not set");
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url)
            .await
            .expect("connect candle database");

        eprintln!(
            "=== Walk-forward: trend-filtered breakout on higher timeframes (5m, 1h) ===\n\
             Re-running the 1m grid on rolled-up candles to test whether lower trade \
             frequency (less fee drag) reveals an edge the 1m runs lacked."
        );
        for timeframe in ["5m", "1h"] {
            walk_forward_breakout_for_timeframe(&pool, timeframe).await;
        }
    }

    // ----- Daily (1d) walk-forward harness -----------------------------------
    //
    // The user-selected next bet after pure/trend-filtered breakout and mean
    // reversion all failed on 1m/5m/1h: a CLASSIC DAILY breakout. Design fixed
    // by a 7-agent propose/attack/synthesize review (the verdict logic, grid,
    // selector, fees, and floor are reused unchanged; only the window cadence is
    // fit to daily data volume — a rule-permitted structural change, NOT entry
    // tuning).
    //
    // Why this is a SEPARATE function (not a call to walk_forward_breakout_for_
    // timeframe): that shared fn hard-codes a 4-window array and a `now`-clamp,
    // and its `positive == 4` verdict is a banked 5m/1h falsification we must not
    // mutate. Daily needs 3 windows, no clamp, n=3 verdict, and a warm-up
    // pre-roll. So the loop+verdict are COPIED with n=3, reusing run_combo_tf /
    // walk_forward_grid / pnl_f64 / expectancy / median / MR_MIN_TRADES verbatim.
    //
    // Three decisive daily facts (verified against the candle DB + code):
    //  1. Warm-up burn: load_candles is [start,end) with NO pre-roll, so a 50-day
    //     SMA eats the first ~50 candles/symbol of each window. On 6mo OOS that
    //     would understate trade counts and force a false INCONCLUSIVE. Fixed via
    //     pre_roll_days=Some(60): 60 extra days warm the buffer, eval_start=start
    //     keeps those candles non-tradeable (see entry_allowed_at).
    //  2. No now-clamp: the last OOS ends 2026-01-01 (pinned), so an INCONCLUSIVE
    //     would be genuine thin volume, not a 6-week calendar sliver.
    //  3. Degenerate selector: only lb10/k0.3 reliably clears the 20-trade floor
    //     on daily, so the IS-best pick has little real choice — the printout
    //     surfaces the per-window pick + signals_seen so a human can confirm.
    //
    // 2026-01 -> now is deliberately RESERVED as the fresh holdout any POSSIBLE
    // EDGE must clear before a testnet move (family-wise overfitting guard).
    //
    // Run: `DATABASE_URL=… cargo test -p trading-api --bin trading-api \
    //        backtest_runner::tests::walk_forward_breakout_daily_test -- --ignored --nocapture`

    /// Largest ma_period in walk_forward_grid; the rolling buffer (and pre-roll)
    /// must exceed it or daily signals are silently swallowed.
    const DAILY_MAX_MA_PERIOD: usize = 50;
    /// Calendar days of buffer pre-roll before each window's true start. 60 > the
    /// 50-day SMA warm-up with slack for any missing daily candles.
    const DAILY_PRE_ROLL_DAYS: i64 = 60;

    // Compile-time buffer headroom: the rolling history must hold the longest MA
    // plus its breakout candle. If the grid ever widens ma_period past the buffer
    // (or the buffer shrinks), the build fails here instead of daily silently
    // returning empty signals.
    const _: () = assert!(BACKTEST_HISTORY_LIMIT > DAILY_MAX_MA_PERIOD);

    async fn walk_forward_breakout_daily(pool: &PgPool) {
        let grid = walk_forward_grid();
        // 3 contiguous-IS windows, IS 12mo -> OOS 6mo, none clamped to now. The
        // 2026-01..now tail is reserved as the fresh holdout (not tested here).
        let windows: [(i32, u32, i32, u32, i32, u32); 3] = [
            (2023, 7, 2024, 7, 2025, 1), // IS 2023-07..2024-07, OOS 2024-07..2025-01
            (2024, 7, 2025, 1, 2025, 7), // IS 2024-07..2025-01, OOS 2025-01..2025-07
            (2025, 1, 2025, 7, 2026, 1), // IS 2025-01..2025-07, OOS 2025-07..2026-01
        ];

        eprintln!(
            "\n############ timeframe = 1d (daily breakout) ############\n\
             grid = {} combos, 3 windows IS 12mo -> OOS 6mo, BTC+ETH, fee-aware, \
             {}d pre-roll warm-up, min trades/window = {}. 2026-01..now reserved as holdout.",
            grid.len(),
            DAILY_PRE_ROLL_DAYS,
            MR_MIN_TRADES,
        );

        let mut oos_results: Vec<f64> = Vec::new();
        let mut oos_expectancies: Vec<f64> = Vec::new();
        let mut window_trade_counts: Vec<u64> = Vec::new();

        for (idx, w) in windows.iter().enumerate() {
            let is_start = Utc.with_ymd_and_hms(w.0, w.1, 1, 0, 0, 0).unwrap();
            let oos_start = Utc.with_ymd_and_hms(w.2, w.3, 1, 0, 0, 0).unwrap();
            let oos_end = Utc.with_ymd_and_hms(w.4, w.5, 1, 0, 0, 0).unwrap();

            // IS-best among combos clearing the trade floor (same rule as 5m/1h),
            // each IS run pre-rolled so the SMA is warm at is_start.
            let mut is_metrics: Vec<(Combo, BacktestMetrics)> = Vec::new();
            for &combo in &grid {
                let m = run_combo_tf(
                    pool,
                    "1d",
                    is_start,
                    oos_start,
                    combo,
                    1.0,
                    Some(DAILY_PRE_ROLL_DAYS),
                )
                .await;
                is_metrics.push((combo, m));
            }
            let pick = is_metrics
                .iter()
                .filter(|(_, m)| m.trades >= MR_MIN_TRADES)
                .max_by(|a, b| pnl_f64(&a.1).partial_cmp(&pnl_f64(&b.1)).unwrap());
            let Some((best, best_is)) = pick else {
                eprintln!(
                    "\nWindow {}: IS {}..{} -> OOS {}..{}\n  NO IS combo cleared {} trades — INCONCLUSIVE window.",
                    idx + 1,
                    is_start.date_naive(),
                    oos_start.date_naive(),
                    oos_start.date_naive(),
                    oos_end.date_naive(),
                    MR_MIN_TRADES,
                );
                oos_results.push(0.0);
                oos_expectancies.push(0.0);
                window_trade_counts.push(0);
                continue;
            };

            let oos_1x = run_combo_tf(
                pool,
                "1d",
                oos_start,
                oos_end,
                *best,
                1.0,
                Some(DAILY_PRE_ROLL_DAYS),
            )
            .await;
            let oos_2x = run_combo_tf(
                pool,
                "1d",
                oos_start,
                oos_end,
                *best,
                2.0,
                Some(DAILY_PRE_ROLL_DAYS),
            )
            .await;
            let oos_3x = run_combo_tf(
                pool,
                "1d",
                oos_start,
                oos_end,
                *best,
                3.0,
                Some(DAILY_PRE_ROLL_DAYS),
            )
            .await;
            oos_results.push(pnl_f64(&oos_1x));
            oos_expectancies.push(expectancy(&oos_1x));
            window_trade_counts.push(oos_1x.trades);

            eprintln!(
                "\nWindow {}: IS {}..{} -> OOS {}..{}",
                idx + 1,
                is_start.date_naive(),
                oos_start.date_naive(),
                oos_start.date_naive(),
                oos_end.date_naive()
            );
            eprintln!(
                "  IS-best: lookback={} k={} ma_period={}  IS PnL={:.2} IS trades={} IS signals={}",
                best.lookback,
                best.k,
                best.ma_period,
                pnl_f64(best_is),
                best_is.trades,
                best_is.signals_seen,
            );
            eprintln!(
                "  OOS PnL: 1x={:.2}  2x={:.2}  3x={:.2}  | OOS trades={} wins={} losses={} signals={} expectancy={:.4}",
                pnl_f64(&oos_1x),
                pnl_f64(&oos_2x),
                pnl_f64(&oos_3x),
                oos_1x.trades,
                oos_1x.wins,
                oos_1x.losses,
                oos_1x.signals_seen,
                expectancy(&oos_1x),
            );
        }

        let n = oos_results.len();
        let mean = oos_results.iter().sum::<f64>() / n as f64;
        let med = median(oos_results.clone());
        let worst = oos_results.iter().copied().fold(f64::INFINITY, f64::min);
        let positive = oos_results.iter().filter(|&&p| p > 0.0).count();
        let below_floor = window_trade_counts
            .iter()
            .filter(|&&t| t < MR_MIN_TRADES)
            .count();
        let positive_expectancy = oos_expectancies.iter().filter(|&&e| e > 0.0).count();

        eprintln!("\n=== 1d OOS SUMMARY (1x cost) ===");
        eprintln!("  per-window PnL:        {oos_results:?}");
        eprintln!("  per-window expectancy: {oos_expectancies:?}");
        eprintln!("  per-window trades:     {window_trade_counts:?}");
        eprintln!(
            "  mean={mean:.2} median={med:.2} worst={worst:.2} positive={positive}/{n} \
             positive_expectancy={positive_expectancy}/{n} windows_below_{MR_MIN_TRADES}_trades={below_floor}"
        );

        // Same three-way verdict as 5m/1h, hard-coded for n=3. A POSSIBLE EDGE is
        // PROVISIONAL: it must survive 2x cost AND the reserved 2026-01..now
        // holdout before any testnet step (family-wise guard — 4th family on the
        // same BTC/ETH regime). With only 3 windows "all positive" is a weaker
        // bar than 4/4, so the provisional framing matters more here.
        let verdict = if below_floor > 0 {
            "INCONCLUSIVE — at least one window below the trade floor; too few daily \
             trades to test fairly even with pre-roll. NOT a falsification."
        } else if positive == n && positive_expectancy == n && mean > 0.0 {
            "POSSIBLE EDGE (PROVISIONAL) — all 3 windows positive with positive \
             expectancy. Do NOT adopt: 3/3 is a ~12.5%-under-noise bar; require 2x-cost \
             survival AND the reserved 2026-01..now holdout before any testnet move."
        } else {
            "FAIL (no edge) — sufficient trades but not consistently positive after fees. \
             Clean falsification -> STOP."
        };
        eprintln!("=== 1d VERDICT ===\n  {verdict}");
    }

    #[tokio::test]
    #[ignore = "daily walk-forward replay over the production candle DB; run explicitly"]
    async fn walk_forward_breakout_daily_test() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping daily walk-forward; DATABASE_URL is not set");
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url)
            .await
            .expect("connect candle database");

        eprintln!(
            "=== Walk-forward: CLASSIC DAILY (1d) breakout ===\n\
             4th strategy family on the same BTC/ETH/2023-26 regime; engineered to \
             FALSIFY (a FAIL kills the daily bet; a pass is provisional only)."
        );
        walk_forward_breakout_daily(&pool).await;
    }
}
