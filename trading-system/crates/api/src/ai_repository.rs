use rust_decimal::Decimal;
use serde_json::json;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use trading_ai::{AiEntryContext, AiGateDecision, MacroDecision, PatternDecision};
use trading_core::{Result, Signal, TradingError};
use uuid::Uuid;

pub async fn persist_ai_context(
    pool: &PgPool,
    signal: &Signal,
    context: &AiEntryContext,
    gate_decision: &AiGateDecision,
) -> Result<()> {
    if let Some(decision) = &context.macro_decision {
        persist_macro_decision(pool, signal, decision, gate_decision).await?;
    }

    if let Some(decision) = &context.pattern_decision {
        persist_pattern_decision(pool, signal, decision, gate_decision).await?;
    }

    Ok(())
}

async fn persist_macro_decision(
    pool: &PgPool,
    signal: &Signal,
    decision: &MacroDecision,
    gate_decision: &AiGateDecision,
) -> Result<()> {
    let details = json!({
        "signal_id": signal.id,
        "symbol": signal.symbol.as_str(),
        "risk_level": &decision.risk_level,
        "long_bias": decision.long_bias,
        "short_bias": decision.short_bias,
        "halt_reason": &decision.halt_reason,
        "gate_reason": gate_decision.reason(),
    });

    persist_ai_decision(
        pool,
        signal,
        "claude_macro",
        decision.macro_score,
        gate_decision.as_str(),
        &decision.model,
        &stable_hash(&decision.input_hash, &details.to_string()),
        details,
    )
    .await
}

async fn persist_pattern_decision(
    pool: &PgPool,
    signal: &Signal,
    decision: &PatternDecision,
    gate_decision: &AiGateDecision,
) -> Result<()> {
    let details = json!({
        "signal_id": signal.id,
        "symbol": signal.symbol.as_str(),
        "historical_win_rate": decision.historical_win_rate,
        "similar_cases": &decision.similar_cases,
        "gate_reason": gate_decision.reason(),
    });

    persist_ai_decision(
        pool,
        signal,
        "candle_pattern",
        decision.pattern_confidence,
        gate_decision.as_str(),
        &decision.model,
        &stable_hash(&decision.input_hash, &details.to_string()),
        details,
    )
    .await
}

async fn persist_ai_decision(
    pool: &PgPool,
    signal: &Signal,
    source: &str,
    score: Decimal,
    decision: &str,
    model: &str,
    input_hash: &str,
    details: Value,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO ai_decisions (
            id, signal_id, source, score, decision, model, input_hash, details, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(signal.id)
    .bind(source)
    .bind(score)
    .bind(decision)
    .bind(model)
    .bind(input_hash)
    .bind(details)
    .execute(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

fn stable_hash(seed: &str, payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher.update(b":");
    hasher.update(payload.as_bytes());
    format!("{:x}", hasher.finalize())
}
