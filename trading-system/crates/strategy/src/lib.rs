use chrono::Utc;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use trading_core::{Candle, Side, Signal};
use uuid::Uuid;

pub trait Strategy: Send + Sync {
    fn name(&self) -> &'static str;
    fn evaluate(&self, candles: &[Candle]) -> Vec<Signal>;
}

#[derive(Debug, Clone)]
pub struct TechnicalStrategy {
    rsi_period: usize,
    bollinger_period: usize,
    oversold_threshold: f64,
    overbought_threshold: f64,
}

impl Default for TechnicalStrategy {
    fn default() -> Self {
        Self {
            rsi_period: 14,
            bollinger_period: 20,
            oversold_threshold: 30.0,
            overbought_threshold: 70.0,
        }
    }
}

impl Strategy for TechnicalStrategy {
    fn name(&self) -> &'static str {
        "technical_rsi_bollinger"
    }

    fn evaluate(&self, candles: &[Candle]) -> Vec<Signal> {
        if candles.len() < self.rsi_period.max(self.bollinger_period) + 1 {
            return Vec::new();
        }

        let latest = match candles.last() {
            Some(candle) => candle,
            None => return Vec::new(),
        };
        let closes = match closes_as_f64(candles) {
            Some(values) => values,
            None => return Vec::new(),
        };
        let rsi = match calculate_rsi(&closes, self.rsi_period) {
            Some(value) => value,
            None => return Vec::new(),
        };
        let bands = match calculate_bollinger_bands(&closes, self.bollinger_period, 2.0) {
            Some(value) => value,
            None => return Vec::new(),
        };
        let close = match closes.last().copied() {
            Some(value) => value,
            None => return Vec::new(),
        };

        if rsi <= self.oversold_threshold && close <= bands.lower {
            return vec![build_signal(
                latest,
                Side::Buy,
                self.name(),
                score_from_distance(self.oversold_threshold - rsi),
                format!("RSI {rsi:.2} oversold and close below lower Bollinger band"),
            )];
        }

        if rsi >= self.overbought_threshold && close >= bands.upper {
            return vec![build_signal(
                latest,
                Side::Sell,
                self.name(),
                score_from_distance(rsi - self.overbought_threshold),
                format!("RSI {rsi:.2} overbought and close above upper Bollinger band"),
            )];
        }

        Vec::new()
    }
}

#[derive(Debug, Clone, Copy)]
struct BollingerBands {
    upper: f64,
    lower: f64,
}

fn build_signal(
    candle: &Candle,
    side: Side,
    strategy: &str,
    score: Decimal,
    reason: String,
) -> Signal {
    Signal {
        id: Uuid::new_v4(),
        symbol: candle.symbol.clone(),
        side,
        strategy: strategy.to_owned(),
        score,
        reason,
        created_at: Utc::now(),
    }
}

fn closes_as_f64(candles: &[Candle]) -> Option<Vec<f64>> {
    candles.iter().map(|candle| candle.close.to_f64()).collect()
}

fn calculate_rsi(closes: &[f64], period: usize) -> Option<f64> {
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

    let relative_strength = gains / losses;
    Some(100.0 - (100.0 / (1.0 + relative_strength)))
}

fn calculate_bollinger_bands(
    closes: &[f64],
    period: usize,
    standard_deviation_multiplier: f64,
) -> Option<BollingerBands> {
    if closes.len() < period {
        return None;
    }

    let window = &closes[closes.len() - period..];
    let mean = window.iter().sum::<f64>() / period as f64;
    let variance = window
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / period as f64;
    let standard_deviation = variance.sqrt();

    Some(BollingerBands {
        upper: mean + standard_deviation * standard_deviation_multiplier,
        lower: mean - standard_deviation * standard_deviation_multiplier,
    })
}

fn score_from_distance(distance: f64) -> Decimal {
    let clamped = (70.0 + distance).clamp(0.0, 100.0);
    Decimal::new(clamped.round() as i64, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use trading_core::{ExchangeId, Symbol};

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
            volume: Decimal::new(10, 0),
        }
    }

    #[test]
    fn does_not_signal_with_insufficient_history() {
        let candles = vec![candle(0, Decimal::new(50_000, 0))];
        assert!(TechnicalStrategy::default().evaluate(&candles).is_empty());
    }

    #[test]
    fn produces_signal_for_extreme_drop() {
        let mut candles = (0..20)
            .map(|index| candle(index, Decimal::new(50_000 + index * 10, 0)))
            .collect::<Vec<_>>();
        candles.push(candle(21, Decimal::new(45_000, 0)));
        candles.push(candle(22, Decimal::new(44_000, 0)));

        let signals = TechnicalStrategy::default().evaluate(&candles);

        assert!(signals.iter().any(|signal| signal.side == Side::Buy));
    }

    #[test]
    fn rsi_returns_hundred_when_there_are_no_losses() {
        let closes = (1..20).map(|value| value as f64).collect::<Vec<_>>();
        assert_eq!(calculate_rsi(&closes, 14), Some(100.0));
    }
}
