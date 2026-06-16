use std::collections::HashMap;

use sqlx::PgPool;
use tokio::sync::mpsc;
use trading_core::{MarketEvent, ObservedMarketEvent};
use trading_exchange::MarketStream;

use crate::market_repository::persist_observed_market_event;
use crate::notify_format;
use crate::risk_event_repository::{
    persist_market_latency_risk_event, MARKET_DATA_LATENCY_THRESHOLD_MS,
};
use crate::telegram::NotificationSender;

/// Throttles how often ORDER-BOOK events are written to `order_books`, keeping
/// at most one row per (exchange, symbol) per `sample_secs`-wide time bucket.
///
/// Order-book top-of-book updates arrive many times per second; persisting every
/// one fills disk in ~2 weeks on the capture host. Downsampling to 1s keeps the
/// resolution the order-book-imbalance hypothesis needs (it predicts moves over
/// minutes) while cutting volume ~50-100x so a multi-month capture fits.
///
/// `sample_secs == 0` disables throttling (persist every event) — the default,
/// so the trading paths are unaffected. CANDLE events are never throttled; only
/// order-book persistence is sampled. Bucketing is by event_time (the venue's
/// stamp), so it is independent of local processing jitter.
#[derive(Default)]
pub struct OrderbookSampler {
    sample_secs: i64,
    last_bucket: HashMap<String, i64>,
}

impl OrderbookSampler {
    pub fn new(sample_secs: i64) -> Self {
        Self {
            sample_secs: sample_secs.max(0),
            last_bucket: HashMap::new(),
        }
    }

    /// Returns true if this event should be persisted to `order_books`. Always
    /// true when throttling is off, for candle events, or for the first event in
    /// a new bucket; false for a later order-book event in an already-persisted
    /// bucket. Mutates internal state to record the persisted bucket.
    pub fn should_persist(&mut self, observed: &ObservedMarketEvent) -> bool {
        if self.sample_secs == 0 {
            return true;
        }
        // Only order-book rows are throttled; candles flow through untouched.
        if !matches!(observed.event, MarketEvent::OrderBook(_)) {
            return true;
        }
        let bucket = observed.event.event_time().timestamp() / self.sample_secs;
        let key = format!(
            "{}:{}",
            observed.event.exchange().as_str(),
            observed.event.symbol()
        );
        match self.last_bucket.get(&key) {
            Some(&seen) if seen == bucket => false,
            _ => {
                self.last_bucket.insert(key, bucket);
                true
            }
        }
    }
}

pub async fn run_market_ingestion_with_forwarder(
    mut stream: MarketStream,
    pool: PgPool,
    event_sender: Option<mpsc::Sender<ObservedMarketEvent>>,
    notifications: Option<NotificationSender>,
    orderbook_sample_secs: i64,
) {
    let mut sampler = OrderbookSampler::new(orderbook_sample_secs);

    while let Some(message) = stream.recv().await {
        match message {
            Ok(observed) => {
                let exchange = observed.event.exchange();
                let symbol = observed.event.symbol().to_string();
                let latency_ms = observed.latency_ms;

                if latency_ms > MARKET_DATA_LATENCY_THRESHOLD_MS {
                    tracing::warn!(
                        ?exchange,
                        %symbol,
                        latency_ms,
                        "market data latency exceeded entry gate threshold"
                    );
                    if let Err(error) = persist_market_latency_risk_event(&pool, &observed).await {
                        tracing::error!(
                            %error,
                            ?exchange,
                            %symbol,
                            latency_ms,
                            "failed to persist market latency risk event"
                        );
                    }
                    notify(
                        &notifications,
                        notify_format::market_latency_warning(
                            exchange.as_str(),
                            &symbol.to_string(),
                            latency_ms,
                        ),
                    )
                    .await;
                }

                // Order-book persistence is downsampled (see OrderbookSampler);
                // candles and the latency path above are never throttled, and the
                // strategy forwarder below always sees every event regardless.
                if sampler.should_persist(&observed) {
                    if let Err(error) = persist_observed_market_event(&pool, &observed).await {
                        tracing::error!(
                            %error,
                            ?exchange,
                            %symbol,
                            latency_ms,
                            "failed to persist market event"
                        );
                        continue;
                    }

                    tracing::debug!(
                        ?exchange,
                        %symbol,
                        latency_ms,
                        "persisted market event"
                    );
                }

                if let Some(sender) = &event_sender {
                    if sender.send(observed).await.is_err() {
                        tracing::warn!("market event strategy forwarder is closed");
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "market stream emitted an error");
            }
        }
    }

    tracing::warn!("market stream ended");
}

async fn notify(sender: &Option<NotificationSender>, message: String) {
    if let Some(sender) = sender {
        if sender.send(message).await.is_err() {
            tracing::warn!("Telegram notification channel is closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;
    use trading_core::{Candle, ExchangeId, OrderBookTop, Symbol};

    fn book_at(secs: i64) -> ObservedMarketEvent {
        let event_time = Utc.timestamp_opt(secs, 0).unwrap();
        ObservedMarketEvent::new(
            MarketEvent::OrderBook(OrderBookTop {
                exchange: ExchangeId::Binance,
                symbol: Symbol::new("BTCUSDT"),
                event_time,
                best_bid: Decimal::new(50_000, 0),
                best_ask: Decimal::new(50_001, 0),
                bid_size: Decimal::ONE,
                ask_size: Decimal::ONE,
            }),
            event_time,
        )
    }

    fn candle_at(secs: i64) -> ObservedMarketEvent {
        let open_time = Utc.timestamp_opt(secs, 0).unwrap();
        ObservedMarketEvent::new(
            MarketEvent::Candle(Candle {
                exchange: ExchangeId::Binance,
                symbol: Symbol::new("BTCUSDT"),
                timeframe: "1m".to_owned(),
                open_time,
                open: Decimal::new(50_000, 0),
                high: Decimal::new(50_000, 0),
                low: Decimal::new(50_000, 0),
                close: Decimal::new(50_000, 0),
                volume: Decimal::ONE,
            }),
            open_time,
        )
    }

    #[test]
    fn samples_one_orderbook_per_second_bucket() {
        let mut sampler = OrderbookSampler::new(1);
        // First event in the second is persisted; later events in the SAME second
        // are dropped; the next second persists again.
        assert!(
            sampler.should_persist(&book_at(100)),
            "first in bucket persists"
        );
        assert!(
            !sampler.should_persist(&book_at(100)),
            "second event in the same 1s bucket must be dropped"
        );
        assert!(
            sampler.should_persist(&book_at(101)),
            "first event in the next second persists"
        );
    }

    #[test]
    fn disabled_sampling_persists_every_event() {
        let mut sampler = OrderbookSampler::new(0);
        assert!(sampler.should_persist(&book_at(100)));
        assert!(
            sampler.should_persist(&book_at(100)),
            "sample_secs=0 must persist every event (trading path unchanged)"
        );
    }

    #[test]
    fn candles_are_never_throttled() {
        let mut sampler = OrderbookSampler::new(1);
        assert!(sampler.should_persist(&candle_at(100)));
        assert!(
            sampler.should_persist(&candle_at(100)),
            "candles must never be sampled, even in the same bucket"
        );
    }

    #[test]
    fn buckets_are_per_exchange_symbol() {
        let mut sampler = OrderbookSampler::new(1);
        let eth = {
            let event_time = Utc.timestamp_opt(100, 0).unwrap();
            ObservedMarketEvent::new(
                MarketEvent::OrderBook(OrderBookTop {
                    exchange: ExchangeId::Binance,
                    symbol: Symbol::new("ETHUSDT"),
                    event_time,
                    best_bid: Decimal::new(3_000, 0),
                    best_ask: Decimal::new(3_001, 0),
                    bid_size: Decimal::ONE,
                    ask_size: Decimal::ONE,
                }),
                event_time,
            )
        };
        assert!(sampler.should_persist(&book_at(100)), "BTC bucket 100");
        assert!(
            sampler.should_persist(&eth),
            "ETH in the same second is a different key and must persist"
        );
    }
}
