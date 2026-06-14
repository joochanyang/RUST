use axum::{
    extract::ws::Message,
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    response::Response,
    Json,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use tokio::{sync::RwLock, time::Duration};
use trading_core::{Result, TradingError, TradingMode};

use crate::{
    execution_repository::close_open_paper_positions, risk_event_repository::persist_risk_event,
    settings::Settings,
};
use trading_execution::ProtectionTrigger;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RuntimeControlState {
    pub mode: TradingMode,
    pub locked_reason: Option<String>,
}

impl RuntimeControlState {
    pub fn new(mode: TradingMode) -> Self {
        Self {
            mode,
            locked_reason: None,
        }
    }
}

pub type SharedRuntimeControl = Arc<RwLock<RuntimeControlState>>;

#[derive(Clone)]
pub struct DashboardState {
    pub pool: PgPool,
    pub settings: Settings,
    pub control: SharedRuntimeControl,
}

#[derive(Debug, Deserialize)]
pub struct LimitQuery {
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct AccountResponse {
    pub mode: TradingMode,
    pub locked_reason: Option<String>,
    pub configured_equity: Decimal,
    pub open_positions: i64,
    pub daily_realized_pnl: Decimal,
    pub ai_filter_enabled: bool,
    pub market_data_enabled: bool,
    pub paper_trading_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct PositionResponse {
    pub id: String,
    pub exchange: String,
    pub symbol: String,
    pub side: String,
    pub entry_price: Decimal,
    pub mark_price: Decimal,
    pub quantity: Decimal,
    pub leverage: Decimal,
    pub unrealized_pnl: Decimal,
    pub opened_at: DateTime<Utc>,
    pub stop_loss_price: Option<Decimal>,
    pub take_profit_price: Option<Decimal>,
    pub protection_status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SignalResponse {
    pub id: String,
    pub symbol: String,
    pub side: String,
    pub strategy: String,
    pub score: Decimal,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub ai_decision: Option<String>,
    pub ai_score: Option<Decimal>,
    pub order_status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RiskEventResponse {
    pub id: String,
    pub severity: String,
    pub rule: String,
    pub action: String,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub acknowledged_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct PerformanceResponse {
    pub total_entries: i64,
    pub open_positions: i64,
    pub closed_trades: i64,
    pub winning_trades: i64,
    pub losing_trades: i64,
    pub take_profit_count: i64,
    pub stop_loss_count: i64,
    pub manual_close_count: i64,
    pub panic_close_count: i64,
    pub realized_pnl: Decimal,
    pub daily_realized_pnl: Decimal,
    pub unrealized_pnl: Decimal,
    pub net_pnl: Decimal,
    pub win_rate_pct: Decimal,
    pub average_pnl: Decimal,
    pub best_trade_pnl: Decimal,
    pub worst_trade_pnl: Decimal,
}

#[derive(Debug, Serialize)]
pub struct TradeHistoryResponse {
    pub position_id: String,
    pub exchange: String,
    pub symbol: String,
    pub side: String,
    pub strategy: Option<String>,
    pub entry_price: Decimal,
    pub mark_price: Decimal,
    pub quantity: Decimal,
    pub opened_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub exit_price: Option<Decimal>,
    pub realized_pnl: Option<Decimal>,
    pub exit_trigger: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshotResponse {
    pub account: AccountResponse,
    pub positions: Vec<PositionResponse>,
    pub performance: PerformanceResponse,
    pub trade_history: Vec<TradeHistoryResponse>,
    pub risk_events: Vec<RiskEventResponse>,
}

#[derive(Debug, Serialize)]
pub struct ControlResponse {
    pub mode: TradingMode,
    pub message: String,
}

pub async fn account(State(state): State<DashboardState>) -> impl IntoResponse {
    match account_response(&state).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => api_error(error),
    }
}

pub async fn positions(State(state): State<DashboardState>) -> impl IntoResponse {
    match position_rows(&state.pool).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => api_error(error),
    }
}

pub async fn signals(
    State(state): State<DashboardState>,
    Query(query): Query<LimitQuery>,
) -> impl IntoResponse {
    match signal_rows(&state.pool, query.limit.unwrap_or(100).clamp(1, 500)).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => api_error(error),
    }
}

pub async fn risk_events(
    State(state): State<DashboardState>,
    Query(query): Query<LimitQuery>,
) -> impl IntoResponse {
    match risk_event_rows(&state.pool, query.limit.unwrap_or(100).clamp(1, 500)).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => api_error(error),
    }
}

pub async fn performance(State(state): State<DashboardState>) -> impl IntoResponse {
    match performance_response(&state.pool).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => api_error(error),
    }
}

pub async fn trade_history(
    State(state): State<DashboardState>,
    Query(query): Query<LimitQuery>,
) -> impl IntoResponse {
    match trade_history_rows(&state.pool, query.limit.unwrap_or(100).clamp(1, 500)).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => api_error(error),
    }
}

pub async fn lock(State(state): State<DashboardState>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    {
        let mut control = state.control.write().await;
        control.mode = TradingMode::Locked;
        control.locked_reason = Some("operator lock".to_owned());
    }

    if let Err(error) = persist_risk_event(
        &state.pool,
        "warning",
        "operator_control",
        "lock",
        json!({ "reason": "operator lock" }),
    )
    .await
    {
        return api_error(error);
    }

    (
        StatusCode::OK,
        Json(ControlResponse {
            mode: TradingMode::Locked,
            message: "locked".to_owned(),
        }),
    )
        .into_response()
}

pub async fn unlock(State(state): State<DashboardState>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    let target_mode = unlock_target_mode(&state);
    {
        let mut control = state.control.write().await;
        control.mode = target_mode;
        control.locked_reason = None;
    }

    if let Err(error) = persist_risk_event(
        &state.pool,
        "info",
        "operator_control",
        "unlock",
        json!({ "mode": target_mode.as_str() }),
    )
    .await
    {
        return api_error(error);
    }

    (
        StatusCode::OK,
        Json(ControlResponse {
            mode: target_mode,
            message: format!("unlocked to {} mode", target_mode.as_str()),
        }),
    )
        .into_response()
}

pub async fn panic_close(
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    {
        let mut control = state.control.write().await;
        control.mode = TradingMode::Locked;
        control.locked_reason = Some("panic close requested".to_owned());
    }

    let affected =
        match close_open_paper_positions(&state.pool, None, ProtectionTrigger::PanicClose).await {
            Ok(affected) => affected,
            Err(error) => return api_error(error),
        };

    if let Err(error) = persist_risk_event(
        &state.pool,
        "critical",
        "operator_control",
        "panic_close",
        json!({ "closed_positions": affected }),
    )
    .await
    {
        return api_error(error);
    }

    (
        StatusCode::OK,
        Json(ControlResponse {
            mode: TradingMode::Locked,
            message: format!("panic close requested for {affected} open positions"),
        }),
    )
        .into_response()
}

pub async fn ack_risk_event(
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    let result = sqlx::query(
        r#"
        UPDATE risk_events
        SET acknowledged_at = now()
        WHERE id = $1::uuid
        "#,
    )
    .bind(&id)
    .execute(&state.pool)
    .await;

    let affected = match result {
        Ok(result) => result.rows_affected(),
        Err(error) => return api_error(TradingError::Database(error.to_string())),
    };

    if let Err(error) = persist_risk_event(
        &state.pool,
        "info",
        "operator_control",
        "ack_risk_event",
        json!({ "risk_event_id": id, "updated": affected }),
    )
    .await
    {
        return api_error(error);
    }

    (
        StatusCode::OK,
        Json(ControlResponse {
            mode: state.control.read().await.mode,
            message: format!("acknowledged {affected} risk event"),
        }),
    )
        .into_response()
}

pub async fn close_position(
    State(state): State<DashboardState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(response) = require_control_token(&state, &headers) {
        return response;
    }

    let position_id = match Uuid::parse_str(&id) {
        Ok(position_id) => position_id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "position id must be a uuid" })),
            )
                .into_response()
        }
    };
    let affected = match close_open_paper_positions(
        &state.pool,
        Some(position_id),
        ProtectionTrigger::ManualClose,
    )
    .await
    {
        Ok(affected) => affected,
        Err(error) => return api_error(error),
    };

    if let Err(error) = persist_risk_event(
        &state.pool,
        "warning",
        "operator_control",
        "close_position",
        json!({ "position_id": id, "updated": affected }),
    )
    .await
    {
        return api_error(error);
    }

    (
        StatusCode::OK,
        Json(ControlResponse {
            mode: state.control.read().await.mode,
            message: format!("manual close updated {affected} position"),
        }),
    )
        .into_response()
}

pub async fn dashboard_ws(State(state): State<DashboardState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        loop {
            let payload = match dashboard_snapshot_response(&state).await {
                Ok(snapshot) => serde_json::json!({ "snapshot": snapshot }),
                Err(error) => serde_json::json!({ "error": error.to_string() }),
            };

            if socket
                .send(Message::Text(payload.to_string().into()))
                .await
                .is_err()
            {
                break;
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}

async fn dashboard_snapshot_response(state: &DashboardState) -> Result<DashboardSnapshotResponse> {
    Ok(DashboardSnapshotResponse {
        account: account_response(state).await?,
        positions: position_rows(&state.pool).await?,
        performance: performance_response(&state.pool).await?,
        trade_history: trade_history_rows(&state.pool, 100).await?,
        risk_events: risk_event_rows(&state.pool, 20).await?,
    })
}

async fn account_response(state: &DashboardState) -> Result<AccountResponse> {
    let control = state.control.read().await.clone();
    let open_positions =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM positions WHERE closed_at IS NULL")
            .fetch_one(&state.pool)
            .await
            .map_err(|error| TradingError::Database(error.to_string()))?;
    let daily_realized_pnl = sqlx::query_scalar::<_, Option<Decimal>>(
        r#"
        SELECT SUM(realized_pnl)
        FROM paper_exits
        WHERE triggered_at >= date_trunc('day', now())
        "#,
    )
    .fetch_one(&state.pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?
    .unwrap_or(Decimal::ZERO);

    Ok(AccountResponse {
        mode: control.mode,
        locked_reason: control.locked_reason,
        configured_equity: state.settings.paper_trading.equity,
        open_positions,
        daily_realized_pnl,
        ai_filter_enabled: state.settings.ai_filter.enabled,
        market_data_enabled: state.settings.market_data.enabled,
        paper_trading_enabled: state.settings.paper_trading.enabled,
    })
}

async fn position_rows(pool: &PgPool) -> Result<Vec<PositionResponse>> {
    let rows = sqlx::query(
        r#"
        SELECT
            p.id::text AS id, p.exchange, p.symbol, p.side, p.entry_price,
            p.mark_price, p.quantity, p.leverage, p.unrealized_pnl, p.opened_at,
            po.stop_loss_price, po.take_profit_price, po.status AS protection_status
        FROM positions p
        LEFT JOIN protection_orders po ON po.position_id = p.id
        WHERE p.closed_at IS NULL
        ORDER BY p.opened_at DESC
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| PositionResponse {
            id: row.get("id"),
            exchange: row.get("exchange"),
            symbol: row.get("symbol"),
            side: row.get("side"),
            entry_price: row.get("entry_price"),
            mark_price: row.get("mark_price"),
            quantity: row.get("quantity"),
            leverage: row.get("leverage"),
            unrealized_pnl: row.get("unrealized_pnl"),
            opened_at: row.get("opened_at"),
            stop_loss_price: row.get("stop_loss_price"),
            take_profit_price: row.get("take_profit_price"),
            protection_status: row.get("protection_status"),
        })
        .collect())
}

async fn signal_rows(pool: &PgPool, limit: i64) -> Result<Vec<SignalResponse>> {
    let rows = sqlx::query(
        r#"
        SELECT
            s.id::text AS id, s.symbol, s.side, s.strategy, s.score, s.reason,
            s.created_at, a.decision AS ai_decision, a.score AS ai_score,
            o.status AS order_status
        FROM signals s
        LEFT JOIN LATERAL (
            SELECT decision, score
            FROM ai_decisions
            WHERE signal_id = s.id
            ORDER BY created_at DESC
            LIMIT 1
        ) a ON true
        LEFT JOIN orders o ON o.signal_id = s.id
        ORDER BY s.created_at DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| SignalResponse {
            id: row.get("id"),
            symbol: row.get("symbol"),
            side: row.get("side"),
            strategy: row.get("strategy"),
            score: row.get("score"),
            reason: row.get("reason"),
            created_at: row.get("created_at"),
            ai_decision: row.get("ai_decision"),
            ai_score: row.get("ai_score"),
            order_status: row.get("order_status"),
        })
        .collect())
}

async fn risk_event_rows(pool: &PgPool, limit: i64) -> Result<Vec<RiskEventResponse>> {
    let rows = sqlx::query(
        r#"
        SELECT
            id::text AS id, severity, rule, action, details, created_at, acknowledged_at
        FROM risk_events
        ORDER BY created_at DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| RiskEventResponse {
            id: row.get("id"),
            severity: row.get("severity"),
            rule: row.get("rule"),
            action: row.get("action"),
            details: row.get("details"),
            created_at: row.get("created_at"),
            acknowledged_at: row.get("acknowledged_at"),
        })
        .collect())
}

async fn performance_response(pool: &PgPool) -> Result<PerformanceResponse> {
    let row = sqlx::query(
        r#"
        SELECT
            (SELECT COUNT(*)::bigint FROM orders WHERE mode IN ('paper', 'testnet')) AS total_entries,
            (SELECT COUNT(*)::bigint FROM positions WHERE closed_at IS NULL) AS open_positions,
            (SELECT COUNT(*)::bigint FROM paper_exits) AS closed_trades,
            (SELECT COUNT(*)::bigint FROM paper_exits WHERE realized_pnl > 0) AS winning_trades,
            (SELECT COUNT(*)::bigint FROM paper_exits WHERE realized_pnl < 0) AS losing_trades,
            (SELECT COUNT(*)::bigint FROM paper_exits WHERE trigger = 'take_profit') AS take_profit_count,
            (SELECT COUNT(*)::bigint FROM paper_exits WHERE trigger = 'stop_loss') AS stop_loss_count,
            (SELECT COUNT(*)::bigint FROM paper_exits WHERE trigger = 'manual_close') AS manual_close_count,
            (SELECT COUNT(*)::bigint FROM paper_exits WHERE trigger = 'panic_close') AS panic_close_count,
            (SELECT COALESCE(SUM(realized_pnl), 0) FROM paper_exits) AS realized_pnl,
            (
                SELECT COALESCE(SUM(realized_pnl), 0)
                FROM paper_exits
                WHERE triggered_at >= date_trunc('day', now())
            ) AS daily_realized_pnl,
            (
                SELECT COALESCE(SUM(unrealized_pnl), 0)
                FROM positions
                WHERE closed_at IS NULL
            ) AS unrealized_pnl,
            (SELECT COALESCE(AVG(realized_pnl), 0) FROM paper_exits) AS average_pnl,
            (SELECT COALESCE(MAX(realized_pnl), 0) FROM paper_exits) AS best_trade_pnl,
            (SELECT COALESCE(MIN(realized_pnl), 0) FROM paper_exits) AS worst_trade_pnl
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    let closed_trades = row.get::<i64, _>("closed_trades");
    let winning_trades = row.get::<i64, _>("winning_trades");
    let realized_pnl = row.get::<Decimal, _>("realized_pnl");
    let unrealized_pnl = row.get::<Decimal, _>("unrealized_pnl");
    let win_rate_pct = if closed_trades > 0 {
        Decimal::from(winning_trades) / Decimal::from(closed_trades) * Decimal::new(100, 0)
    } else {
        Decimal::ZERO
    };

    Ok(PerformanceResponse {
        total_entries: row.get("total_entries"),
        open_positions: row.get("open_positions"),
        closed_trades,
        winning_trades,
        losing_trades: row.get("losing_trades"),
        take_profit_count: row.get("take_profit_count"),
        stop_loss_count: row.get("stop_loss_count"),
        manual_close_count: row.get("manual_close_count"),
        panic_close_count: row.get("panic_close_count"),
        realized_pnl,
        daily_realized_pnl: row.get("daily_realized_pnl"),
        unrealized_pnl,
        net_pnl: realized_pnl + unrealized_pnl,
        win_rate_pct,
        average_pnl: row.get("average_pnl"),
        best_trade_pnl: row.get("best_trade_pnl"),
        worst_trade_pnl: row.get("worst_trade_pnl"),
    })
}

async fn trade_history_rows(pool: &PgPool, limit: i64) -> Result<Vec<TradeHistoryResponse>> {
    let rows = sqlx::query(
        r#"
        SELECT
            p.id::text AS position_id,
            p.exchange,
            p.symbol,
            p.side,
            p.entry_price,
            p.mark_price,
            p.quantity,
            p.opened_at,
            p.closed_at,
            s.strategy,
            exits.exit_price,
            exits.realized_pnl,
            exits.trigger AS exit_trigger,
            exits.triggered_at
        FROM positions p
        LEFT JOIN protection_orders po ON po.position_id = p.id
        LEFT JOIN orders o ON o.id = po.entry_order_id
        LEFT JOIN signals s ON s.id = o.signal_id
        LEFT JOIN LATERAL (
            SELECT exit_price, realized_pnl, trigger, triggered_at
            FROM paper_exits
            WHERE position_id = p.id
            ORDER BY triggered_at DESC
            LIMIT 1
        ) exits ON true
        ORDER BY COALESCE(exits.triggered_at, p.opened_at) DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let closed_at = row.get::<Option<DateTime<Utc>>, _>("closed_at");
            TradeHistoryResponse {
                position_id: row.get("position_id"),
                exchange: row.get("exchange"),
                symbol: row.get("symbol"),
                side: row.get("side"),
                strategy: row.get("strategy"),
                entry_price: row.get("entry_price"),
                mark_price: row.get("mark_price"),
                quantity: row.get("quantity"),
                opened_at: row.get("opened_at"),
                closed_at,
                exit_price: row.get("exit_price"),
                realized_pnl: row.get("realized_pnl"),
                exit_trigger: row.get("exit_trigger"),
                status: if closed_at.is_some() {
                    "closed".to_owned()
                } else {
                    "open".to_owned()
                },
            }
        })
        .collect())
}

fn api_error(error: TradingError) -> axum::response::Response {
    tracing::error!(%error, "dashboard api request failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": error.to_string() })),
    )
        .into_response()
}

pub fn require_control_token(state: &DashboardState, headers: &HeaderMap) -> Option<Response> {
    let expected = state.settings.dashboard_control_token.as_ref()?;
    let provided = headers
        .get("x-dashboard-control-token")
        .and_then(|value| value.to_str().ok());

    if provided == Some(expected.as_str()) {
        None
    } else {
        Some(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "dashboard control token is required" })),
            )
                .into_response(),
        )
    }
}

pub fn unlock_target_mode(state: &DashboardState) -> TradingMode {
    if state.settings.trading.mode == TradingMode::Testnet {
        TradingMode::Testnet
    } else {
        TradingMode::Paper
    }
}
