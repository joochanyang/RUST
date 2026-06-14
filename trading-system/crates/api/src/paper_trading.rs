use rust_decimal::Decimal;
use sqlx::PgPool;
use trading_core::{ExchangeId, ProtectedOrder, Result, Signal};
use trading_execution::{Broker, PaperBroker};
use trading_risk::{AccountRiskState, BasicRiskGate};

use crate::{execution_repository::persist_protected_order, signal_repository::persist_signal};

pub async fn evaluate_and_execute_paper_signal(
    pool: &PgPool,
    risk_gate: &BasicRiskGate,
    broker: &PaperBroker,
    signal: &Signal,
    exchange: ExchangeId,
    reference_price: Decimal,
    account: &AccountRiskState,
) -> Result<ProtectedOrder> {
    persist_signal(pool, signal).await?;

    let request = risk_gate.build_order_request(signal, exchange, reference_price, account)?;
    let protected_order = broker.submit_order(request).await?;
    persist_protected_order(pool, &protected_order).await?;

    Ok(protected_order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use sqlx::{postgres::PgPoolOptions, Row};
    use trading_ai::{AiEntryContext, AiGateDecision, MacroDecision, PatternDecision};
    use trading_core::{OrderBookTop, Side, Symbol};
    use trading_execution::PaperPositionTracker;
    use uuid::Uuid;

    use crate::{
        ai_repository::persist_ai_context,
        execution_repository::{close_open_paper_positions, load_open_protected_orders},
        signal_repository::persist_signal,
    };
    use trading_execution::ProtectionTrigger;

    #[tokio::test]
    async fn persists_signal_ai_and_protected_paper_order_when_database_is_configured() {
        let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("connect test database");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        let signal = Signal {
            id: Uuid::new_v4(),
            symbol: Symbol::new("BTCUSDT"),
            side: Side::Buy,
            strategy: "db_integration".to_owned(),
            score: Decimal::new(90, 0),
            reason: "deterministic integration signal".to_owned(),
            created_at: Utc::now(),
        };
        let ai_context = AiEntryContext {
            macro_decision: Some(MacroDecision {
                macro_score: Decimal::new(80, 0),
                long_bias: Decimal::new(80, 0),
                input_hash: "db-integration-macro".to_owned(),
                ..MacroDecision::default()
            }),
            pattern_decision: Some(PatternDecision {
                pattern_confidence: Decimal::new(90, 0),
                historical_win_rate: Decimal::new(80, 0),
                input_hash: "db-integration-pattern".to_owned(),
                ..PatternDecision::default()
            }),
        };

        persist_signal(&pool, &signal)
            .await
            .expect("persist signal");
        persist_ai_context(&pool, &signal, &ai_context, &AiGateDecision::Allow)
            .await
            .expect("persist AI context");
        let protected_order = evaluate_and_execute_paper_signal(
            &pool,
            &BasicRiskGate::default(),
            &PaperBroker::default(),
            &signal,
            ExchangeId::Binance,
            Decimal::new(50_000, 0),
            &AccountRiskState {
                equity: Decimal::new(10_000, 0),
                daily_realized_pnl: Decimal::ZERO,
                daily_loss_limit: Decimal::new(500, 0),
                locked: false,
                market_data_latency_ms: 100,
            },
        )
        .await
        .expect("execute protected paper order");

        let row = sqlx::query(
            r#"
            SELECT
                (SELECT COUNT(*) FROM signals WHERE id = $1) AS signals,
                (SELECT COUNT(*) FROM ai_decisions WHERE signal_id = $1) AS ai_decisions,
                (SELECT COUNT(*) FROM orders WHERE signal_id = $1) AS orders,
                (SELECT COUNT(*) FROM order_fills WHERE order_id = $2) AS order_fills,
                (SELECT COUNT(*) FROM positions WHERE id = $3) AS positions,
                (SELECT COUNT(*) FROM protection_orders WHERE position_id = $3) AS protection_orders
            "#,
        )
        .bind(signal.id)
        .bind(protected_order.entry_order.id)
        .bind(protected_order.position.id)
        .fetch_one(&pool)
        .await
        .expect("load persisted paper order rows");

        assert_eq!(row.get::<i64, _>("signals"), 1);
        assert_eq!(row.get::<i64, _>("ai_decisions"), 2);
        assert_eq!(row.get::<i64, _>("orders"), 1);
        assert_eq!(row.get::<i64, _>("order_fills"), 1);
        assert_eq!(row.get::<i64, _>("positions"), 1);
        assert_eq!(row.get::<i64, _>("protection_orders"), 1);

        let mut tracker = PaperPositionTracker::default();
        tracker.insert(protected_order.clone());
        let exits = tracker.update_mark(&OrderBookTop {
            exchange: ExchangeId::Binance,
            symbol: Symbol::new("BTCUSDT"),
            event_time: Utc::now(),
            best_bid: protected_order.protection.take_profit_price,
            best_ask: protected_order.protection.take_profit_price + Decimal::ONE,
            bid_size: Decimal::ONE,
            ask_size: Decimal::ONE,
        });
        assert_eq!(exits.len(), 1);
        let recorded = crate::execution_repository::persist_paper_exit(&pool, &exits[0])
            .await
            .expect("persist paper exit");
        assert!(
            recorded,
            "first exit for an open position should be recorded"
        );

        let exit_row = sqlx::query(
            r#"
            SELECT
                (SELECT COUNT(*) FROM paper_exits WHERE position_id = $1) AS paper_exits,
                (SELECT COUNT(*) FROM positions WHERE id = $1 AND closed_at IS NOT NULL) AS closed_positions,
                (SELECT COUNT(*) FROM protection_orders WHERE position_id = $1 AND status = 'triggered_take_profit') AS triggered_protection
            "#,
        )
        .bind(protected_order.position.id)
        .fetch_one(&pool)
        .await
        .expect("load persisted paper exit rows");

        assert_eq!(exit_row.get::<i64, _>("paper_exits"), 1);
        assert_eq!(exit_row.get::<i64, _>("closed_positions"), 1);
        assert_eq!(exit_row.get::<i64, _>("triggered_protection"), 1);

        let manual_signal = Signal {
            id: Uuid::new_v4(),
            symbol: Symbol::new("BTCUSDT"),
            side: Side::Buy,
            strategy: "db_integration_manual_close".to_owned(),
            score: Decimal::new(90, 0),
            reason: "deterministic integration signal".to_owned(),
            created_at: Utc::now(),
        };
        let manual_order = evaluate_and_execute_paper_signal(
            &pool,
            &BasicRiskGate::default(),
            &PaperBroker::default(),
            &manual_signal,
            ExchangeId::Binance,
            Decimal::new(50_000, 0),
            &AccountRiskState {
                equity: Decimal::new(10_000, 0),
                daily_realized_pnl: Decimal::ZERO,
                daily_loss_limit: Decimal::new(500, 0),
                locked: false,
                market_data_latency_ms: 100,
            },
        )
        .await
        .expect("execute second protected paper order");

        let restored = load_open_protected_orders(&pool)
            .await
            .expect("restore open protected orders");
        assert!(restored
            .iter()
            .any(|order| order.position.id == manual_order.position.id));

        sqlx::query("UPDATE positions SET mark_price = $1 WHERE id = $2")
            .bind(Decimal::new(49_000, 0))
            .bind(manual_order.position.id)
            .execute(&pool)
            .await
            .expect("update mark price for manual close");

        let closed = close_open_paper_positions(
            &pool,
            Some(manual_order.position.id),
            ProtectionTrigger::ManualClose,
        )
        .await
        .expect("manual close records paper exit");
        assert_eq!(closed, 1);

        let manual_exit_row = sqlx::query(
            r#"
            SELECT
                trigger,
                realized_pnl,
                (SELECT COUNT(*) FROM positions WHERE id = $1 AND closed_at IS NOT NULL) AS closed_positions,
                (SELECT COUNT(*) FROM protection_orders WHERE position_id = $1 AND status = 'triggered_manual_close') AS triggered_protection
            FROM paper_exits
            WHERE position_id = $1
            "#,
        )
        .bind(manual_order.position.id)
        .fetch_one(&pool)
        .await
        .expect("load manual close exit row");

        assert_eq!(manual_exit_row.get::<String, _>("trigger"), "manual_close");
        assert_eq!(
            manual_exit_row.get::<Decimal, _>("realized_pnl"),
            Decimal::new(-10, 0)
        );
        assert_eq!(manual_exit_row.get::<i64, _>("closed_positions"), 1);
        assert_eq!(manual_exit_row.get::<i64, _>("triggered_protection"), 1);
    }
}
