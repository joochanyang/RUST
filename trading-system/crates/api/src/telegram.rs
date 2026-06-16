use axum::http::StatusCode;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use tokio::{sync::mpsc, time::Duration};
use trading_core::{Result, TradingError, TradingMode};
use trading_execution::ProtectionTrigger;

use crate::{
    dashboard_api::{close_open_positions_for_control, unlock_target_mode, DashboardState},
    live_readiness::LiveReadinessResponse,
    risk_event_repository::persist_risk_event,
};

pub type NotificationSender = mpsc::Sender<String>;

#[derive(Clone)]
pub struct TelegramClient {
    base_url: String,
    client: reqwest::Client,
}

impl TelegramClient {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            base_url: format!("https://api.telegram.org/bot{}", bot_token.into()),
            client: reqwest::Client::new(),
        }
    }

    pub async fn send_message(&self, chat_id: i64, text: impl Into<String>) -> Result<()> {
        let response = self
            .client
            .post(format!("{}/sendMessage", self.base_url))
            .json(&json!({
                "chat_id": chat_id,
                "text": text.into(),
                "parse_mode": "Markdown",
                "disable_web_page_preview": true
            }))
            .send()
            .await
            .map_err(|error| TradingError::Exchange(reqwest_error_message(error)))?;

        decode_telegram_response(response).await.map(|_| ())
    }

    async fn get_updates(&self, offset: Option<i64>) -> Result<Vec<TelegramUpdate>> {
        let mut body = json!({
            "timeout": 30,
            "allowed_updates": ["message"]
        });
        if let Some(offset) = offset {
            body["offset"] = json!(offset);
        }

        let response = self
            .client
            .post(format!("{}/getUpdates", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|error| TradingError::Exchange(reqwest_error_message(error)))?;
        let value = decode_telegram_response(response).await?;

        serde_json::from_value(value["result"].clone())
            .map_err(|error| TradingError::Exchange(error.to_string()))
    }
}

pub async fn run_telegram_notification_sink(
    client: TelegramClient,
    chat_id: i64,
    mut receiver: mpsc::Receiver<String>,
) {
    while let Some(message) = receiver.recv().await {
        if let Err(error) = client.send_message(chat_id, message).await {
            tracing::warn!(%error, "failed to send Telegram notification");
        }
    }
}

pub async fn run_telegram_command_loop(
    state: DashboardState,
    client: TelegramClient,
    allowed_chat_id: i64,
) {
    let mut offset = None;

    loop {
        match client.get_updates(offset).await {
            Ok(updates) => {
                for update in updates {
                    offset = Some(update.update_id + 1);
                    if let Some(message) = update.message {
                        if message.chat.id != allowed_chat_id {
                            tracing::warn!(
                                chat_id = message.chat.id,
                                "ignored unauthorized Telegram command"
                            );
                            continue;
                        }

                        if let Some(text) = message.text {
                            let response = handle_command(&state, &text).await;
                            if let Err(error) = client.send_message(allowed_chat_id, response).await
                            {
                                tracing::warn!(%error, "failed to send Telegram command response");
                            }
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "Telegram getUpdates failed");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn handle_command(state: &DashboardState, text: &str) -> String {
    let command = text.split_whitespace().next().unwrap_or_default();

    match command {
        "/start" | "/help" => help_text(),
        "/status" => match status_text(state).await {
            Ok(text) => text,
            Err(error) => format!("status error: {error}"),
        },
        "/readiness" => match readiness_text(&state.pool).await {
            Ok(text) => text,
            Err(error) => format!("readiness error: {error}"),
        },
        "/lock" => match lock_runtime(state, "telegram lock").await {
            Ok(()) => "locked".to_owned(),
            Err(error) => format!("lock error: {error}"),
        },
        "/unlock" => match unlock_runtime(state).await {
            Ok(mode) => format!("unlocked to {} mode", mode.as_str()),
            Err(error) => format!("unlock error: {error}"),
        },
        "/panic_close" => match panic_close(state).await {
            Ok(closed) => format!("panic close recorded for {closed} open positions"),
            Err(error) => format!("panic close error: {error}"),
        },
        _ => "unknown command. Use /help".to_owned(),
    }
}

fn help_text() -> String {
    [
        "Commands:",
        "/status - runtime, positions, PnL summary",
        "/readiness - live readiness gates",
        "/lock - block new entries",
        "/unlock - return runtime to paper mode",
        "/panic_close - close open paper positions and lock",
    ]
    .join("\n")
}

async fn status_text(state: &DashboardState) -> Result<String> {
    let control = state.control.read().await.clone();
    let row = sqlx::query(
        r#"
        SELECT
            (SELECT COUNT(*)::bigint FROM positions WHERE closed_at IS NULL) AS open_positions,
            (
                SELECT COALESCE(SUM(realized_pnl), 0)
                FROM paper_exits
                WHERE triggered_at >= date_trunc('day', now())
            ) AS daily_realized_pnl,
            (SELECT COUNT(*)::bigint FROM risk_events WHERE acknowledged_at IS NULL) AS open_risk_events
        "#,
    )
    .fetch_one(&state.pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(format!(
        "mode: {}\nlocked_reason: {}\nopen_positions: {}\ndaily_realized_pnl: {}\nopen_risk_events: {}",
        control.mode.as_str(),
        control.locked_reason.unwrap_or_else(|| "-".to_owned()),
        row.get::<i64, _>("open_positions"),
        row.get::<Decimal, _>("daily_realized_pnl"),
        row.get::<i64, _>("open_risk_events"),
    ))
}

async fn readiness_text(pool: &PgPool) -> Result<String> {
    let rows = sqlx::query(
        r#"
        SELECT check_key
        FROM live_readiness_checks
        WHERE status = 'passed'
        ORDER BY check_key
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;
    let passed = rows
        .into_iter()
        .map(|row| row.get::<String, _>("check_key"))
        .collect::<Vec<_>>();
    let required = [
        "backtest_mdd",
        "paper_trading_14d",
        "api_key_restricted",
        "killswitch_verified",
        "rate_limit_recovery",
    ];
    let missing = required
        .iter()
        .filter(|check| !passed.iter().any(|passed| passed == **check))
        .copied()
        .collect::<Vec<_>>();
    let response = LiveReadinessResponse {
        approved: missing.is_empty(),
        required_checks: required.iter().map(|check| (*check).to_owned()).collect(),
        passed_checks: passed,
        missing_checks: missing.iter().map(|check| (*check).to_owned()).collect(),
    };

    Ok(format!(
        "approved: {}\npassed: {}\nmissing: {}",
        response.approved,
        response.passed_checks.join(", "),
        response.missing_checks.join(", ")
    ))
}

async fn lock_runtime(state: &DashboardState, reason: &str) -> Result<()> {
    {
        let mut control = state.control.write().await;
        control.mode = TradingMode::Locked;
        control.locked_reason = Some(reason.to_owned());
    }

    persist_risk_event(
        &state.pool,
        "warning",
        "telegram_control",
        "lock",
        json!({ "reason": reason }),
    )
    .await
}

async fn unlock_runtime(state: &DashboardState) -> Result<TradingMode> {
    let target_mode = unlock_target_mode(state);
    {
        let mut control = state.control.write().await;
        control.mode = target_mode;
        control.locked_reason = None;
    }

    persist_risk_event(
        &state.pool,
        "info",
        "telegram_control",
        "unlock",
        json!({ "mode": target_mode.as_str() }),
    )
    .await?;

    Ok(target_mode)
}

async fn panic_close(state: &DashboardState) -> Result<u64> {
    {
        let mut control = state.control.write().await;
        control.mode = TradingMode::Locked;
        control.locked_reason = Some("telegram panic close".to_owned());
    }
    let closed =
        close_open_positions_for_control(state, None, ProtectionTrigger::PanicClose).await?;

    persist_risk_event(
        &state.pool,
        "critical",
        "telegram_control",
        "panic_close",
        json!({ "closed_positions": closed }),
    )
    .await?;

    Ok(closed)
}

async fn decode_telegram_response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| TradingError::Exchange(reqwest_error_message(error)))?;

    if status != StatusCode::OK {
        return Err(TradingError::Exchange(format!(
            "Telegram request failed with {status}: {body}"
        )));
    }

    serde_json::from_str(&body).map_err(|error| TradingError::Exchange(error.to_string()))
}

fn reqwest_error_message(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    chat: TelegramChat,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
}
