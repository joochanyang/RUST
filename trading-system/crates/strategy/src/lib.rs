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

/// Rolling-window volatility breakout (bidirectional Larry Williams variant).
///
/// Each evaluation looks at the just-closed candle (`latest`) and the `lookback`
/// candles before it. The reference range is `max(high) - min(low)` over that
/// prior window; the breakout target is `latest.open ± range * k`. A close at or
/// beyond the upper target is a Buy, at or below the lower target a Sell.
///
/// The window excludes `latest` so the breakout candle never inflates its own
/// reference range. The live buffer caps at 100 candles with no startup backfill,
/// so a short rolling window (not a full prior-day range) is used by design —
/// see the design spec for rationale.
#[derive(Debug, Clone)]
pub struct VolatilityBreakoutStrategy {
    lookback: usize,
    k: f64,
}

impl Default for VolatilityBreakoutStrategy {
    fn default() -> Self {
        Self {
            lookback: 20,
            k: 0.5,
        }
    }
}

impl Strategy for VolatilityBreakoutStrategy {
    fn name(&self) -> &'static str {
        "volatility_breakout"
    }

    fn evaluate(&self, candles: &[Candle]) -> Vec<Signal> {
        if candles.len() < self.lookback + 1 {
            return Vec::new();
        }

        let latest = match candles.last() {
            Some(candle) => candle,
            None => return Vec::new(),
        };

        // Window is the `lookback` candles immediately before `latest` (latest
        // excluded so it cannot inflate its own reference range).
        let window = &candles[candles.len() - 1 - self.lookback..candles.len() - 1];
        let (highest, lowest) = match window_high_low(window) {
            Some(values) => values,
            None => return Vec::new(),
        };
        let range = highest - lowest;
        if range <= 0.0 {
            return Vec::new();
        }

        let open = match latest.open.to_f64() {
            Some(value) => value,
            None => return Vec::new(),
        };
        let close = match latest.close.to_f64() {
            Some(value) => value,
            None => return Vec::new(),
        };

        let offset = range * self.k;
        let long_target = open + offset;
        let short_target = open - offset;

        if close >= long_target {
            return vec![build_signal(
                latest,
                Side::Buy,
                self.name(),
                score_from_distance(close - long_target),
                format!(
                    "Close {close:.2} broke above volatility target {long_target:.2} \
                     (open {open:.2} + range {range:.2} * {k})",
                    k = self.k
                ),
            )];
        }

        if close <= short_target {
            return vec![build_signal(
                latest,
                Side::Sell,
                self.name(),
                score_from_distance(short_target - close),
                format!(
                    "Close {close:.2} broke below volatility target {short_target:.2} \
                     (open {open:.2} - range {range:.2} * {k})",
                    k = self.k
                ),
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

/// Highest high and lowest low over a candle window as f64. Returns `None` for
/// an empty window or if any Decimal→f64 conversion fails.
fn window_high_low(candles: &[Candle]) -> Option<(f64, f64)> {
    let mut highest = f64::NEG_INFINITY;
    let mut lowest = f64::INFINITY;
    for candle in candles {
        let high = candle.high.to_f64()?;
        let low = candle.low.to_f64()?;
        if high > highest {
            highest = high;
        }
        if low < lowest {
            lowest = low;
        }
    }
    if highest.is_finite() && lowest.is_finite() {
        Some((highest, lowest))
    } else {
        None
    }
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

    fn candle_hlc(index: i64, open: i64, high: i64, low: i64, close: i64) -> Candle {
        Candle {
            exchange: ExchangeId::Binance,
            symbol: Symbol::new("BTCUSDT"),
            timeframe: "1m".to_owned(),
            open_time: Utc.timestamp_opt(1_710_000_000 + index * 60, 0).unwrap(),
            open: Decimal::new(open, 0),
            high: Decimal::new(high, 0),
            low: Decimal::new(low, 0),
            close: Decimal::new(close, 0),
            volume: Decimal::new(10, 0),
        }
    }

    /// Builds `lookback` flat reference candles, each with a fixed `range`
    /// (high − low) centered on `mid`, so the breakout window range is
    /// deterministic. The breakout candle is appended separately by each test.
    fn breakout_window(lookback: usize, mid: i64, range: i64) -> Vec<Candle> {
        (0..lookback as i64)
            .map(|index| candle_hlc(index, mid, mid + range / 2, mid - range / 2, mid))
            .collect()
    }

    #[test]
    fn breakout_does_not_signal_with_insufficient_history() {
        // lookback 20 needs 21 candles; 20 is one short.
        let candles = breakout_window(20, 50_000, 100);
        assert!(VolatilityBreakoutStrategy::default()
            .evaluate(&candles)
            .is_empty());
    }

    #[test]
    fn signals_buy_on_upside_breakout() {
        // window range = 100, k = 0.5 → offset = 50. Breakout open 50_000,
        // close 50_060 (> 50_050 target) → Buy.
        let mut candles = breakout_window(20, 50_000, 100);
        candles.push(candle_hlc(20, 50_000, 50_060, 49_990, 50_060));

        let signals = VolatilityBreakoutStrategy::default().evaluate(&candles);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
    }

    #[test]
    fn signals_sell_on_downside_breakout() {
        // Symmetric downside: close 49_940 (< 49_950 short target) → Sell.
        let mut candles = breakout_window(20, 50_000, 100);
        candles.push(candle_hlc(20, 50_000, 50_010, 49_940, 49_940));

        let signals = VolatilityBreakoutStrategy::default().evaluate(&candles);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Sell);
    }

    #[test]
    fn no_signal_inside_band() {
        // close 50_030 sits between short target 49_950 and long target 50_050.
        let mut candles = breakout_window(20, 50_000, 100);
        candles.push(candle_hlc(20, 50_000, 50_040, 49_970, 50_030));

        assert!(VolatilityBreakoutStrategy::default()
            .evaluate(&candles)
            .is_empty());
    }

    #[test]
    fn no_signal_on_zero_range() {
        // All reference candles identical → range 0 → no meaningful breakout.
        let mut candles = breakout_window(20, 50_000, 0);
        candles.push(candle_hlc(20, 50_000, 60_000, 40_000, 55_000));

        assert!(VolatilityBreakoutStrategy::default()
            .evaluate(&candles)
            .is_empty());
    }

    #[test]
    fn score_grows_with_breakout_distance() {
        let small = {
            let mut candles = breakout_window(20, 50_000, 100);
            candles.push(candle_hlc(20, 50_000, 50_055, 49_990, 50_055));
            VolatilityBreakoutStrategy::default().evaluate(&candles)
        };
        let large = {
            let mut candles = breakout_window(20, 50_000, 100);
            candles.push(candle_hlc(20, 50_000, 50_500, 49_990, 50_500));
            VolatilityBreakoutStrategy::default().evaluate(&candles)
        };

        assert_eq!(small.len(), 1);
        assert_eq!(large.len(), 1);
        assert!(large[0].score > small[0].score);
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
