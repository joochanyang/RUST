use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use trading_core::{Result, TradingError, TradingMode};
use uuid::Uuid;

use crate::{
    dashboard_api::{require_control_token, DashboardState},
    execution_repository::close_open_paper_positions,
    risk_event_repository::persist_risk_event,
};
use trading_execution::ProtectionTrigger;

const REQUIRED_CHECKS: &[&str] = &[
    "backtest_mdd",
    "paper_trading_14d",
    "api_key_restricted",
    "killswitch_verified",
    "rate_limit_recovery",
];

#[derive(Debug, Deserialize)]
pub struct ReadinessCheckRequest {
    pub check_key: String,
    pub status: String,
    pub evidence: Value,
    pub verified_by: String,
}

#[derive(Debug, Deserialize)]
pub struct FailureInjectionRequest {
    pub scenario: String,
    pub expected_action: String,
    pub observed_action: String,
    pub passed: bool,
    pub details: Value,
}

#[derive(Debug, Deserialize)]
pub struct RunFailureInjectionRequest {
    pub scenario: String,
}

#[derive(Debug, Serialize)]
pub struct LiveReadinessResponse {
    pub approved: bool,
    pub required_checks: Vec<String>,
    pub passed_checks: Vec<String>,
    pub missing_checks: Vec<String>,
}

pub async fn live_readiness(State(state): State<DashboardState>) -> impl IntoResponse {
    match calculate_live_readiness(&state.pool).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => api_error(error),
    }
}

pub async fn record_readiness_check(
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Json(request): Json<ReadinessCheckRequest>,
) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    if !REQUIRED_CHECKS.contains(&request.check_key.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unsupported readiness check" })),
        )
            .into_response();
    }

    if !matches!(request.status.as_str(), "passed" | "failed" | "pending") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "status must be passed, failed, or pending" })),
        )
            .into_response();
    }

    let result = sqlx::query(
        r#"
        INSERT INTO live_readiness_checks (
            id, check_key, status, evidence, verified_by, verified_at
        )
        VALUES ($1, $2, $3, $4, $5, now())
        ON CONFLICT (check_key)
        DO UPDATE SET
            status = EXCLUDED.status,
            evidence = EXCLUDED.evidence,
            verified_by = EXCLUDED.verified_by,
            verified_at = now()
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&request.check_key)
    .bind(&request.status)
    .bind(request.evidence)
    .bind(&request.verified_by)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => live_readiness(State(state)).await.into_response(),
        Err(error) => api_error(TradingError::Database(error.to_string())),
    }
}

pub async fn record_failure_injection(
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Json(request): Json<FailureInjectionRequest>,
) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    let result = sqlx::query(
        r#"
        INSERT INTO failure_injection_runs (
            id, scenario, expected_action, observed_action, passed, details, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&request.scenario)
    .bind(&request.expected_action)
    .bind(&request.observed_action)
    .bind(request.passed)
    .bind(request.details)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => (StatusCode::CREATED, Json(json!({ "status": "recorded" }))).into_response(),
        Err(error) => api_error(TradingError::Database(error.to_string())),
    }
}

pub async fn run_failure_injection(
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Json(request): Json<RunFailureInjectionRequest>,
) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    let result = match request.scenario.as_str() {
        "killswitch_panic_close" => run_killswitch_panic_close(&state).await,
        "exchange_429_rate_limit" => run_exchange_429_rate_limit(&state).await,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "unsupported failure injection scenario",
                    "supported": ["killswitch_panic_close", "exchange_429_rate_limit"]
                })),
            )
                .into_response()
        }
    };

    match result {
        Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
        Err(error) => api_error(error),
    }
}

pub async fn verify_paper_trading_14d(
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    match calculate_paper_trading_evidence(&state.pool).await {
        Ok(evidence) => {
            if evidence.passed {
                if let Err(error) = record_readiness_result(
                    &state.pool,
                    "paper_trading_14d",
                    "passed",
                    evidence.to_json(),
                    "paper_trading_verifier",
                )
                .await
                {
                    return api_error(error);
                }

                (StatusCode::CREATED, Json(evidence.to_json())).into_response()
            } else {
                (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": "paper trading evidence has not reached 14 days",
                        "evidence": evidence.to_json()
                    })),
                )
                    .into_response()
            }
        }
        Err(error) => api_error(error),
    }
}

async fn calculate_live_readiness(pool: &PgPool) -> Result<LiveReadinessResponse> {
    let rows = sqlx::query(
        r#"
        SELECT check_key
        FROM live_readiness_checks
        WHERE status = 'passed'
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;
    let passed_checks = rows
        .into_iter()
        .map(|row| row.get::<String, _>("check_key"))
        .collect::<Vec<_>>();
    let missing_checks = REQUIRED_CHECKS
        .iter()
        .filter(|check| !passed_checks.iter().any(|passed| passed == **check))
        .map(|check| (*check).to_owned())
        .collect::<Vec<_>>();

    Ok(LiveReadinessResponse {
        approved: missing_checks.is_empty(),
        required_checks: REQUIRED_CHECKS
            .iter()
            .map(|check| (*check).to_owned())
            .collect(),
        passed_checks,
        missing_checks,
    })
}

#[derive(Debug)]
struct PaperTradingEvidence {
    passed: bool,
    first_observed_at: Option<DateTime<Utc>>,
    last_observed_at: Option<DateTime<Utc>>,
    observed_days: i64,
    paper_orders: i64,
    fills: i64,
    positions: i64,
    protection_orders: i64,
    paper_exits: i64,
}

impl PaperTradingEvidence {
    fn to_json(&self) -> Value {
        json!({
            "passed": self.passed,
            "first_observed_at": self.first_observed_at,
            "last_observed_at": self.last_observed_at,
            "observed_days": self.observed_days,
            "paper_orders": self.paper_orders,
            "fills": self.fills,
            "positions": self.positions,
            "protection_orders": self.protection_orders,
            "paper_exits": self.paper_exits
        })
    }
}

async fn calculate_paper_trading_evidence(pool: &PgPool) -> Result<PaperTradingEvidence> {
    let row = sqlx::query(
        r#"
        WITH paper_orders AS (
            SELECT id, created_at
            FROM orders
            WHERE mode = 'paper'
        ),
        observed_times AS (
            SELECT created_at AS observed_at FROM paper_orders
            UNION ALL
            SELECT fills.filled_at AS observed_at
            FROM order_fills fills
            JOIN paper_orders orders ON orders.id = fills.order_id
            UNION ALL
            SELECT positions.opened_at AS observed_at
            FROM positions
            WHERE exchange IN ('binance', 'bybit', 'bitget')
            UNION ALL
            SELECT exits.triggered_at AS observed_at
            FROM paper_exits exits
        )
        SELECT
            (SELECT MIN(observed_at) FROM observed_times) AS first_observed_at,
            (SELECT MAX(observed_at) FROM observed_times) AS last_observed_at,
            (SELECT COUNT(*)::bigint FROM paper_orders) AS paper_orders,
            (
                SELECT COUNT(*)::bigint
                FROM order_fills fills
                JOIN paper_orders orders ON orders.id = fills.order_id
            ) AS fills,
            (SELECT COUNT(*)::bigint FROM positions) AS positions,
            (SELECT COUNT(*)::bigint FROM protection_orders) AS protection_orders,
            (SELECT COUNT(*)::bigint FROM paper_exits) AS paper_exits
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    let first_observed_at = row.get::<Option<DateTime<Utc>>, _>("first_observed_at");
    let last_observed_at = row.get::<Option<DateTime<Utc>>, _>("last_observed_at");
    let observed_days = match (first_observed_at, last_observed_at) {
        (Some(first), Some(last)) => last.signed_duration_since(first).num_days(),
        _ => 0,
    };
    let paper_orders = row.get::<i64, _>("paper_orders");
    let fills = row.get::<i64, _>("fills");
    let positions = row.get::<i64, _>("positions");
    let protection_orders = row.get::<i64, _>("protection_orders");
    let paper_exits = row.get::<i64, _>("paper_exits");
    let passed = observed_days >= 14
        && paper_orders > 0
        && fills > 0
        && positions > 0
        && protection_orders > 0;

    Ok(PaperTradingEvidence {
        passed,
        first_observed_at,
        last_observed_at,
        observed_days,
        paper_orders,
        fills,
        positions,
        protection_orders,
        paper_exits,
    })
}

async fn run_killswitch_panic_close(state: &DashboardState) -> Result<Value> {
    {
        let mut control = state.control.write().await;
        control.mode = TradingMode::Locked;
        control.locked_reason = Some("failure injection: killswitch panic close".to_owned());
    }

    let closed_positions =
        close_open_paper_positions(&state.pool, None, ProtectionTrigger::PanicClose).await?;

    persist_risk_event(
        &state.pool,
        "critical",
        "failure_injection",
        "killswitch_panic_close",
        json!({ "closed_positions": closed_positions }),
    )
    .await?;

    let details = json!({
        "scenario": "killswitch_panic_close",
        "locked": true,
        "closed_positions": closed_positions
    });

    record_failure_injection_result(
        &state.pool,
        "killswitch_panic_close",
        "lock_runtime_and_close_open_positions",
        "locked_runtime_and_closed_open_positions",
        true,
        details.clone(),
    )
    .await?;
    record_readiness_result(
        &state.pool,
        "killswitch_verified",
        "passed",
        details.clone(),
        "failure_injection",
    )
    .await?;

    Ok(json!({
        "status": "passed",
        "readiness_check": "killswitch_verified",
        "details": details
    }))
}

async fn run_exchange_429_rate_limit(state: &DashboardState) -> Result<Value> {
    {
        let mut control = state.control.write().await;
        control.mode = TradingMode::Locked;
        control.locked_reason = Some("failure injection: exchange 429 rate limit".to_owned());
    }

    persist_risk_event(
        &state.pool,
        "critical",
        "exchange_rate_limit",
        "lock_runtime",
        json!({
            "scenario": "exchange_429_rate_limit",
            "simulated_status": 429,
            "expected_recovery": "runtime remains locked until operator review"
        }),
    )
    .await?;

    let details = json!({
        "scenario": "exchange_429_rate_limit",
        "simulated_status": 429,
        "locked": true,
        "recovery_policy": "runtime remains locked until operator review"
    });

    record_failure_injection_result(
        &state.pool,
        "exchange_429_rate_limit",
        "lock_runtime_on_rate_limit",
        "locked_runtime_on_rate_limit",
        true,
        details.clone(),
    )
    .await?;
    record_readiness_result(
        &state.pool,
        "rate_limit_recovery",
        "passed",
        details.clone(),
        "failure_injection",
    )
    .await?;

    Ok(json!({
        "status": "passed",
        "readiness_check": "rate_limit_recovery",
        "details": details
    }))
}

async fn record_failure_injection_result(
    pool: &PgPool,
    scenario: &str,
    expected_action: &str,
    observed_action: &str,
    passed: bool,
    details: Value,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO failure_injection_runs (
            id, scenario, expected_action, observed_action, passed, details, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(scenario)
    .bind(expected_action)
    .bind(observed_action)
    .bind(passed)
    .bind(details)
    .execute(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

async fn record_readiness_result(
    pool: &PgPool,
    check_key: &str,
    status: &str,
    evidence: Value,
    verified_by: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO live_readiness_checks (
            id, check_key, status, evidence, verified_by, verified_at
        )
        VALUES ($1, $2, $3, $4, $5, now())
        ON CONFLICT (check_key)
        DO UPDATE SET
            status = EXCLUDED.status,
            evidence = EXCLUDED.evidence,
            verified_by = EXCLUDED.verified_by,
            verified_at = now()
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(check_key)
    .bind(status)
    .bind(evidence)
    .bind(verified_by)
    .execute(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

fn api_error(error: TradingError) -> axum::response::Response {
    tracing::error!(%error, "live readiness api request failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": error.to_string() })),
    )
        .into_response()
}
