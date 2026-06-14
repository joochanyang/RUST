use serde_json::{json, Value};
use sqlx::PgPool;
use trading_core::{ObservedMarketEvent, Result, Signal, TradingError};
use uuid::Uuid;

// Re-exported from trading-core so the ingestion warn/persist path here and the
// entry-block site in trading-risk share one source of truth and cannot drift.
pub use trading_core::MARKET_DATA_LATENCY_THRESHOLD_MS;

pub async fn persist_ai_block_risk_event(
    pool: &PgPool,
    signal: &Signal,
    reason: &str,
) -> Result<()> {
    let details = json!({
        "signal_id": signal.id,
        "symbol": signal.symbol.as_str(),
        "side": signal.side.as_str(),
        "strategy": &signal.strategy,
        "reason": reason,
    });

    persist_risk_event(pool, "warning", "ai_entry_gate", "block_entry", details).await
}

pub async fn persist_market_latency_risk_event(
    pool: &PgPool,
    observed: &ObservedMarketEvent,
) -> Result<()> {
    persist_risk_event(
        pool,
        "warning",
        "market_data_latency",
        "block_entry",
        market_latency_risk_event_details(observed),
    )
    .await
}

fn market_latency_risk_event_details(observed: &ObservedMarketEvent) -> Value {
    json!({
        "exchange": observed.event.exchange().as_str(),
        "symbol": observed.event.symbol().as_str(),
        "event_time": observed.event.event_time(),
        "received_at": observed.received_at,
        "latency_ms": observed.latency_ms,
        "threshold_ms": MARKET_DATA_LATENCY_THRESHOLD_MS,
    })
}

pub async fn persist_risk_event(
    pool: &PgPool,
    severity: &str,
    rule: &str,
    action: &str,
    details: Value,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO risk_events (
            id, severity, rule, action, details, created_at
        )
        VALUES ($1, $2, $3, $4, $5, now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(severity)
    .bind(rule)
    .bind(action)
    .bind(details)
    .execute(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use rust_decimal::Decimal;
    use trading_core::{Candle, ExchangeId, MarketEvent, Symbol};

    #[test]
    fn market_latency_details_include_gate_context() {
        let event_time = Utc.timestamp_opt(1_710_000_000, 0).unwrap();
        // Candle freshness is now measured against close_time (open_time + 1m),
        // so to exercise a 2_501ms latency the receive time must sit 2_501ms past
        // the candle's close, i.e. one interval + 2_501ms past open_time.
        let received_at = event_time + Duration::milliseconds(60_000 + 2_501);
        let observed = ObservedMarketEvent::new(
            MarketEvent::Candle(Candle {
                exchange: ExchangeId::Binance,
                symbol: Symbol::new("btcusdt"),
                timeframe: "1m".to_owned(),
                open_time: event_time,
                open: Decimal::new(50_000, 0),
                high: Decimal::new(50_100, 0),
                low: Decimal::new(49_900, 0),
                close: Decimal::new(50_050, 0),
                volume: Decimal::ONE,
            }),
            received_at,
        );

        let details = market_latency_risk_event_details(&observed);

        assert_eq!(details["exchange"], "binance");
        assert_eq!(details["symbol"], "BTCUSDT");
        assert_eq!(details["latency_ms"], 2_501);
        assert_eq!(details["threshold_ms"], MARKET_DATA_LATENCY_THRESHOLD_MS);
    }
}
