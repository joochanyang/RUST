use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use trading_core::{Candle, ExchangeId, PositionSide, Result, Signal, Symbol, TradingError};
use trading_risk::{AccountRiskState, BasicRiskGate, RiskGate};
use trading_strategy::{Strategy, VolatilityBreakoutStrategy};

const BACKTEST_HISTORY_LIMIT: usize = 64;

#[derive(Debug, Clone, Deserialize)]
pub struct BacktestConfig {
    pub exchange: Option<String>,
    pub symbols: Option<Vec<String>>,
    pub timeframe: Option<String>,
    pub period_start: Option<DateTime<Utc>>,
    pub period_end: Option<DateTime<Utc>>,
    pub initial_equity: Option<Decimal>,
    pub daily_loss_limit: Option<Decimal>,
    /// Volatility-breakout strategy parameters. When omitted, the strategy's
    /// own defaults (lookback 20, k 0.5) apply. Used for walk-forward sweeps.
    pub lookback: Option<usize>,
    pub k: Option<f64>,
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

    let default_strategy = VolatilityBreakoutStrategy::default();
    let strategy = match (config.lookback, config.k) {
        (None, None) => default_strategy,
        (lookback, k) => VolatilityBreakoutStrategy::new(
            lookback.unwrap_or_else(|| default_strategy.lookback()),
            k.unwrap_or_else(|| default_strategy.k()),
        ),
    };
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
            equity += position_pnl(position, last_candle.close);
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

fn close_positions(
    candle: &Candle,
    open_positions: &mut Vec<BacktestPosition>,
    equity: &mut Decimal,
    trades: &mut u64,
    wins: &mut u64,
    losses: &mut u64,
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
        let pnl = position_pnl(&open_positions[index], exit_price);
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
    "volatility_breakout_v1"
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
    fn max_drawdown_percentage_uses_peak_equity() {
        let peak = Decimal::new(10_000, 0);
        let drawdown = Decimal::new(250, 0);
        let pct = drawdown / peak * Decimal::new(100, 0);

        assert_eq!(pct.to_f64().unwrap(), 2.5);
    }
}
