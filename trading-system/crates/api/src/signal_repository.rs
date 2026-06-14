use sqlx::PgPool;
use trading_core::{Result, Signal, TradingError};

pub async fn persist_signal(pool: &PgPool, signal: &Signal) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO signals (
            id, symbol, side, strategy, score, reason, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(signal.id)
    .bind(signal.symbol.as_str())
    .bind(signal.side.as_str())
    .bind(&signal.strategy)
    .bind(signal.score)
    .bind(&signal.reason)
    .bind(signal.created_at)
    .execute(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}
