use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use trading_core::TradingError;
use uuid::Uuid;

use crate::backtest_runner::{
    metrics_to_json, run_backtest as execute_backtest, strategy_version, BacktestConfig,
    BacktestMetrics,
};
use crate::dashboard_api::{require_control_token, DashboardState};

#[derive(Debug, Deserialize)]
pub struct BacktestRunRequest {
    pub strategy_version: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub metrics: Value,
}

#[derive(Debug, Serialize)]
pub struct BacktestRunResponse {
    pub id: Uuid,
    pub metrics: BacktestMetrics,
}

pub async fn record_backtest_run(
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Json(request): Json<BacktestRunRequest>,
) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    if request.period_end <= request.period_start {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "period_end must be after period_start" })),
        )
            .into_response();
    }

    match insert_backtest_run(
        &state,
        &request.strategy_version,
        request.period_start,
        request.period_end,
        request.metrics,
    )
    .await
    {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(error) => api_error(error),
    }
}

pub async fn run_backtest(
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Json(request): Json<BacktestConfig>,
) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    let metrics = match execute_backtest(&state.pool, request).await {
        Ok(metrics) => metrics,
        Err(error) => return api_error(error),
    };
    match insert_backtest_run(
        &state,
        strategy_version(),
        metrics.period_start,
        metrics.period_end,
        metrics_to_json(&metrics),
    )
    .await
    {
        Ok(id) => (
            StatusCode::CREATED,
            Json(BacktestRunResponse { id, metrics }),
        )
            .into_response(),
        Err(error) => api_error(error),
    }
}

async fn insert_backtest_run(
    state: &DashboardState,
    strategy_version: &str,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
    metrics: Value,
) -> Result<Uuid, TradingError> {
    let id = Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO backtest_runs (
            id, strategy_version, period_start, period_end, metrics, created_at
        )
        VALUES ($1, $2, $3, $4, $5, now())
        "#,
    )
    .bind(id)
    .bind(strategy_version)
    .bind(period_start)
    .bind(period_end)
    .bind(metrics)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => Ok(id),
        Err(error) => Err(TradingError::Database(error.to_string())),
    }
}

fn api_error(error: TradingError) -> axum::response::Response {
    tracing::error!(%error, "backtest api request failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": error.to_string() })),
    )
        .into_response()
}
