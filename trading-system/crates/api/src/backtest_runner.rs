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
}
