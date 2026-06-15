mod ai_repository;
mod backtest_api;
mod backtest_runner;
mod dashboard_api;
mod execution_repository;
mod live_readiness;
mod market_ingestion;
mod market_repository;
mod paper_trading;
mod risk_event_repository;
mod settings;
mod signal_repository;
mod strategy_runtime;
mod telegram;
mod testnet_runtime;

use anyhow::Context;
use axum::{
    extract::State,
    http::{header, HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use settings::Settings;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::Arc;
use std::{net::SocketAddr, time::Duration};
use tokio::{net::TcpListener, sync::mpsc, sync::RwLock};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use trading_core::{Symbol, TradingMode};
use trading_exchange::{
    binance::BinanceAdapter, bitget::BitgetAdapter, bybit::BybitAdapter, ExchangeAdapter,
};

use crate::{
    dashboard_api::{DashboardState, RuntimeControlState},
    market_ingestion::run_market_ingestion_with_forwarder,
    strategy_runtime::{run_paper_strategy_loop, PaperStrategyRuntimeConfig},
    telegram::{run_telegram_command_loop, run_telegram_notification_sink, TelegramClient},
    testnet_runtime::{run_binance_testnet_strategy_loop, TestnetStrategyRuntimeConfig},
};

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    database: &'static str,
    mode: TradingMode,
    service: &'static str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let settings = Settings::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(settings.database.max_connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&settings.database.url)
        .await
        .context("failed to connect to PostgreSQL")?;

    if settings.database.run_migrations {
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .context("failed to run database migrations")?;
    }

    if settings.trading.mode == TradingMode::Live {
        ensure_live_readiness(&pool).await?;
        // Live readiness can pass, but no live order-execution runtime exists yet
        // (only paper and Binance testnet loops are implemented). Refuse to boot
        // rather than start a process that looks "approved live" but never places
        // a real order — or worse, silently runs the paper loop.
        anyhow::bail!(
            "live trading runtime is not implemented; refusing to start in live mode \
             (readiness checks passed, but no live execution path exists)"
        );
    }

    let control = Arc::new(RwLock::new(RuntimeControlState::new(settings.trading.mode)));
    let state = DashboardState {
        pool: pool.clone(),
        settings: settings.clone(),
        control: control.clone(),
    };
    let notification_sender = if settings.telegram.enabled {
        let client = TelegramClient::new(
            settings
                .telegram
                .bot_token
                .clone()
                .context("TELEGRAM_BOT_TOKEN is required")?,
        );
        let chat_id = settings
            .telegram
            .notify_chat_id
            .or(settings.telegram.allowed_chat_id)
            .context("TELEGRAM_NOTIFY_CHAT_ID or TELEGRAM_ALLOWED_CHAT_ID is required")?;
        let allowed_chat_id = settings
            .telegram
            .allowed_chat_id
            .context("TELEGRAM_ALLOWED_CHAT_ID is required")?;
        let (sender, receiver) = mpsc::channel(256);

        tokio::spawn(run_telegram_notification_sink(
            client.clone(),
            chat_id,
            receiver,
        ));
        tokio::spawn(run_telegram_command_loop(
            state.clone(),
            client,
            allowed_chat_id,
        ));

        Some(sender)
    } else {
        None
    };

    if settings.market_data.enabled {
        let symbols = settings
            .market_data
            .symbols
            .iter()
            .map(|symbol| Symbol::new(symbol.as_str()))
            .collect::<Vec<_>>();
        let mut runtime_senders = Vec::new();

        if settings.paper_trading.enabled {
            let (sender, receiver) = mpsc::channel(1024);
            let config = PaperStrategyRuntimeConfig {
                equity: settings.paper_trading.equity,
                daily_loss_limit: settings.paper_trading.daily_loss_limit,
                max_candles_per_key: settings.paper_trading.max_candles_per_key,
                ai_filter_enabled: settings.ai_filter.enabled,
                ai_fail_closed: settings.ai_filter.fail_closed,
                ai_macro_score: settings.ai_filter.macro_score,
                ai_long_bias: settings.ai_filter.long_bias,
                ai_short_bias: settings.ai_filter.short_bias,
                ai_pattern_confidence: settings.ai_filter.pattern_confidence,
                ai_historical_win_rate: settings.ai_filter.historical_win_rate,
            };
            tokio::spawn(run_paper_strategy_loop(
                receiver,
                pool.clone(),
                control.clone(),
                notification_sender.clone(),
                config,
            ));
            runtime_senders.push(sender);
        }

        if settings.trading.mode == TradingMode::Testnet {
            let (sender, receiver) = mpsc::channel(1024);
            let config = TestnetStrategyRuntimeConfig {
                equity: settings.paper_trading.equity,
                daily_loss_limit: settings.paper_trading.daily_loss_limit,
                max_order_notional: settings.binance_testnet.max_order_notional,
                max_candles_per_key: settings.paper_trading.max_candles_per_key,
                ai_filter_enabled: settings.ai_filter.enabled,
                ai_fail_closed: settings.ai_filter.fail_closed,
                ai_macro_score: settings.ai_filter.macro_score,
                ai_long_bias: settings.ai_filter.long_bias,
                ai_short_bias: settings.ai_filter.short_bias,
                ai_pattern_confidence: settings.ai_filter.pattern_confidence,
                ai_historical_win_rate: settings.ai_filter.historical_win_rate,
            };
            let adapter = BinanceAdapter::testnet(
                settings
                    .binance_testnet
                    .api_key
                    .clone()
                    .context("BINANCE_TESTNET_API_KEY is required")?,
                settings
                    .binance_testnet
                    .api_secret
                    .clone()
                    .context("BINANCE_TESTNET_API_SECRET is required")?,
            );
            tokio::spawn(run_binance_testnet_strategy_loop(
                receiver,
                pool.clone(),
                control.clone(),
                notification_sender.clone(),
                adapter,
                config,
            ));
            runtime_senders.push(sender);
        }

        let strategy_sender = if runtime_senders.is_empty() {
            None
        } else {
            let (sender, mut receiver) = mpsc::channel::<trading_core::ObservedMarketEvent>(1024);
            tokio::spawn(async move {
                while let Some(event) = receiver.recv().await {
                    for runtime_sender in &runtime_senders {
                        if runtime_sender.send(event.clone()).await.is_err() {
                            tracing::warn!("market event runtime forwarder is closed");
                        }
                    }
                }
            });
            Some(sender)
        };

        if settings.market_data.exchange_enabled("binance") {
            let stream = BinanceAdapter::default()
                .subscribe_market_stream(&symbols)
                .await
                .context("failed to subscribe Binance market stream")?;
            tokio::spawn(run_market_ingestion_with_forwarder(
                stream,
                pool.clone(),
                strategy_sender.clone(),
                notification_sender.clone(),
                settings.market_data.orderbook_sample_secs,
            ));
        }

        if settings.market_data.exchange_enabled("bybit") {
            let stream = BybitAdapter::default()
                .subscribe_market_stream(&symbols)
                .await
                .context("failed to subscribe Bybit market stream")?;
            tokio::spawn(run_market_ingestion_with_forwarder(
                stream,
                pool.clone(),
                strategy_sender.clone(),
                notification_sender.clone(),
                settings.market_data.orderbook_sample_secs,
            ));
        }

        if settings.market_data.exchange_enabled("bitget") {
            let stream = BitgetAdapter::default()
                .subscribe_market_stream(&symbols)
                .await
                .context("failed to subscribe Bitget market stream")?;
            tokio::spawn(run_market_ingestion_with_forwarder(
                stream,
                pool.clone(),
                strategy_sender.clone(),
                notification_sender.clone(),
                settings.market_data.orderbook_sample_secs,
            ));
        }

        info!(
            symbols = ?settings.market_data.symbols,
            exchanges = ?settings.market_data.exchanges,
            paper_trading = settings.paper_trading.enabled,
            ai_filter = settings.ai_filter.enabled,
            "market ingestion started"
        );
    }

    let app = router(state);
    let server_addr = settings.server_addr()?;
    let listener = TcpListener::bind(server_addr).await?;
    info!(addr = %server_addr, "trading api listening");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

fn router(state: DashboardState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/health", get(health))
        .route("/api/account", get(dashboard_api::account))
        .route("/api/positions", get(dashboard_api::positions))
        .route("/api/performance", get(dashboard_api::performance))
        .route("/api/trade-history", get(dashboard_api::trade_history))
        .route("/api/signals", get(dashboard_api::signals))
        .route("/api/risk-events", get(dashboard_api::risk_events))
        .route(
            "/api/risk-events/:id/ack",
            post(dashboard_api::ack_risk_event),
        )
        .route(
            "/api/positions/:id/close",
            post(dashboard_api::close_position),
        )
        .route("/api/live-readiness", get(live_readiness::live_readiness))
        .route(
            "/api/live-readiness/checks",
            post(live_readiness::record_readiness_check),
        )
        .route(
            "/api/live-readiness/paper-trading-14d/run",
            post(live_readiness::verify_paper_trading_14d),
        )
        .route(
            "/api/failure-injection-runs",
            post(live_readiness::record_failure_injection),
        )
        .route(
            "/api/failure-injection-runs/run",
            post(live_readiness::run_failure_injection),
        )
        .route(
            "/api/backtest-runs",
            post(backtest_api::record_backtest_run),
        )
        .route("/api/backtest-runs/run", post(backtest_api::run_backtest))
        .route("/api/control/lock", post(dashboard_api::lock))
        .route("/api/control/unlock", post(dashboard_api::unlock))
        .route("/api/control/panic-close", post(dashboard_api::panic_close))
        .route("/ws/dashboard", get(dashboard_api::dashboard_ws))
        .layer(local_dashboard_cors())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn local_dashboard_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("http://127.0.0.1:3000"),
            HeaderValue::from_static("http://localhost:3000"),
        ])
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([
            header::CONTENT_TYPE,
            header::HeaderName::from_static("x-dashboard-control-token"),
        ])
}

async fn ensure_live_readiness(pool: &PgPool) -> anyhow::Result<()> {
    let missing_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM (
            VALUES
                ('backtest_mdd'),
                ('paper_trading_14d'),
                ('api_key_restricted'),
                ('killswitch_verified'),
                ('rate_limit_recovery')
        ) AS required(check_key)
        LEFT JOIN live_readiness_checks checks
            ON checks.check_key = required.check_key
           AND checks.status = 'passed'
        WHERE checks.check_key IS NULL
        "#,
    )
    .fetch_one(pool)
    .await
    .context("failed to verify live readiness checks")?;

    if missing_count > 0 {
        anyhow::bail!("live mode requires all live readiness checks to pass");
    }

    Ok(())
}

async fn health(State(state): State<DashboardState>) -> impl IntoResponse {
    let database_ok = sqlx::query_scalar::<_, i64>("SELECT 1::bigint")
        .fetch_one(&state.pool)
        .await
        .map(|value| value == 1)
        .unwrap_or(false);

    let status = if database_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(HealthResponse {
            status: if database_ok { "ok" } else { "degraded" },
            database: if database_ok { "ok" } else { "unavailable" },
            mode: state.control.read().await.mode,
            service: "trading-api",
        }),
    )
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().json())
        .init();
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        warn!(%error, "failed to install shutdown signal handler");
    }
}
