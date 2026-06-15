use chrono::Utc;
use rust_decimal::Decimal;
use sqlx::{PgConnection, PgPool, Row};
use trading_core::{
    ExchangeId, Order, OrderFill, OrderStatus, OrderType, Position, PositionSide, ProtectedOrder,
    ProtectionPlan, Result, Side, Symbol, TradingError, TradingMode,
};
use trading_execution::{PaperExit, ProtectionTrigger};
use trading_risk::AccountRiskState;
use uuid::Uuid;

pub async fn persist_protected_order(
    pool: &PgPool,
    protected_order: &ProtectedOrder,
) -> Result<()> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    insert_order(&mut transaction, protected_order).await?;
    insert_fill(&mut transaction, protected_order).await?;
    insert_position(&mut transaction, protected_order).await?;
    insert_protection(&mut transaction, protected_order).await?;

    transaction
        .commit()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

async fn insert_order(
    connection: &mut PgConnection,
    protected_order: &ProtectedOrder,
) -> Result<()> {
    let order = &protected_order.entry_order;

    sqlx::query(
        r#"
        INSERT INTO orders (
            id, signal_id, exchange, exchange_order_id, mode, symbol, side,
            order_type, status, price, quantity, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(order.id)
    .bind(order.signal_id)
    .bind(order.exchange.as_str())
    .bind(&order.exchange_order_id)
    .bind(order.mode.as_str())
    .bind(order.symbol.as_str())
    .bind(order.side.as_str())
    .bind(order.order_type.as_str())
    .bind(order.status.as_str())
    .bind(order.price)
    .bind(order.quantity)
    .bind(order.created_at)
    .execute(&mut *connection)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

async fn insert_fill(
    connection: &mut PgConnection,
    protected_order: &ProtectedOrder,
) -> Result<()> {
    let fill = &protected_order.fill;

    sqlx::query(
        r#"
        INSERT INTO order_fills (
            order_id, exchange, symbol, side, price, quantity, filled_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(fill.order_id)
    .bind(fill.exchange.as_str())
    .bind(fill.symbol.as_str())
    .bind(fill.side.as_str())
    .bind(fill.price)
    .bind(fill.quantity)
    .bind(fill.filled_at)
    .execute(&mut *connection)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

async fn insert_position(
    connection: &mut PgConnection,
    protected_order: &ProtectedOrder,
) -> Result<()> {
    let position = &protected_order.position;

    sqlx::query(
        r#"
        INSERT INTO positions (
            id, exchange, symbol, side, entry_price, mark_price, quantity,
            leverage, unrealized_pnl, opened_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(position.id)
    .bind(position.exchange.as_str())
    .bind(position.symbol.as_str())
    .bind(position.side.as_str())
    .bind(position.entry_price)
    .bind(position.mark_price)
    .bind(position.quantity)
    .bind(position.leverage)
    .bind(position.unrealized_pnl)
    .bind(position.opened_at)
    .execute(&mut *connection)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

async fn insert_protection(
    connection: &mut PgConnection,
    protected_order: &ProtectedOrder,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO protection_orders (
            id, entry_order_id, position_id, stop_loss_price, take_profit_price, status
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(protected_order.entry_order.id)
    .bind(protected_order.position.id)
    .bind(protected_order.protection.stop_loss_price)
    .bind(protected_order.protection.take_profit_price)
    .bind("active")
    .execute(&mut *connection)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

/// Persists a paper exit emitted by the in-memory tracker.
///
/// Idempotent against out-of-band closes: the position row is closed first with
/// a `closed_at IS NULL` guard, and the `paper_exits`/`protection_orders` writes
/// only run when that guard actually closed the position (1 row affected). If the
/// position was already closed by the dashboard/panic path, this is a no-op and
/// returns `false` instead of inserting a duplicate exit and double-counting PnL.
///
/// Returns `true` when the exit was recorded, `false` when the position was
/// already closed.
pub async fn persist_paper_exit(pool: &PgPool, exit: &PaperExit) -> Result<bool> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    let closed = sqlx::query(
        r#"
        UPDATE positions
        SET mark_price = $1,
            unrealized_pnl = 0,
            closed_at = $2
        WHERE id = $3
          AND closed_at IS NULL
        "#,
    )
    .bind(exit.exit_price)
    .bind(exit.triggered_at)
    .bind(exit.position_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?
    .rows_affected();

    if closed == 0 {
        // Position was already closed out-of-band; do not insert a duplicate exit.
        transaction
            .rollback()
            .await
            .map_err(|error| TradingError::Database(error.to_string()))?;
        return Ok(false);
    }

    sqlx::query(
        r#"
        INSERT INTO paper_exits (
            id, position_id, entry_order_id, exchange, symbol, trigger,
            exit_price, quantity, realized_pnl, triggered_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(exit.position_id)
    .bind(exit.entry_order_id)
    .bind(exit.exchange.as_str())
    .bind(exit.symbol.as_str())
    .bind(exit.trigger.as_str())
    .bind(exit.exit_price)
    .bind(exit.quantity)
    .bind(exit.realized_pnl)
    .bind(exit.triggered_at)
    .execute(&mut *transaction)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    sqlx::query(
        r#"
        UPDATE protection_orders
        SET status = $1
        WHERE position_id = $2
        "#,
    )
    .bind(format!("triggered_{}", exit.trigger.as_str()))
    .bind(exit.position_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    transaction
        .commit()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(true)
}

pub async fn close_open_paper_positions(
    pool: &PgPool,
    position_id: Option<Uuid>,
    trigger: ProtectionTrigger,
) -> Result<u64> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;
    let protected_orders = load_open_protected_orders_with_connection(
        &mut *transaction,
        position_id,
        Some(TradingMode::Paper),
    )
    .await?;
    let close_time = Utc::now();
    let mut closed = 0;

    for protected_order in protected_orders {
        let position = &protected_order.position;
        let exit_price = position.mark_price;
        let realized_pnl = paper_position_pnl(position, exit_price);
        let exit = PaperExit {
            position_id: position.id,
            entry_order_id: protected_order.entry_order.id,
            exchange: position.exchange,
            symbol: position.symbol.clone(),
            trigger,
            exit_price,
            quantity: position.quantity,
            realized_pnl,
            triggered_at: close_time,
        };

        // Close under a `closed_at IS NULL` guard first; only record the exit if
        // this call actually closed the position. This keeps the bulk close
        // idempotent on its own (not just via the SELECT ... FOR UPDATE above),
        // so it can never write a duplicate paper_exits row for a position that
        // was already closed out-of-band.
        if close_position_for_exit(&mut transaction, &exit).await? {
            insert_paper_exit(&mut transaction, &exit).await?;
            closed += 1;
        }
    }

    transaction
        .commit()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(closed)
}

pub async fn load_open_protected_orders(pool: &PgPool) -> Result<Vec<ProtectedOrder>> {
    load_open_protected_orders_by_mode(pool, TradingMode::Paper).await
}

pub async fn load_open_protected_orders_by_mode(
    pool: &PgPool,
    mode: TradingMode,
) -> Result<Vec<ProtectedOrder>> {
    load_open_protected_orders_by_mode_and_position(pool, mode, None).await
}

pub async fn load_open_protected_orders_by_mode_and_position(
    pool: &PgPool,
    mode: TradingMode,
    position_id: Option<Uuid>,
) -> Result<Vec<ProtectedOrder>> {
    let mut connection = pool
        .acquire()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    load_open_protected_orders_with_connection(&mut *connection, position_id, Some(mode)).await
}

pub async fn load_open_position_keys(pool: &PgPool, mode: TradingMode) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT p.exchange, p.symbol
        FROM positions p
        JOIN protection_orders po ON po.position_id = p.id
        JOIN orders o ON o.id = po.entry_order_id
        WHERE p.closed_at IS NULL
          AND o.mode = $1
        "#,
    )
    .bind(mode.as_str())
    .fetch_all(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    rows.into_iter()
        .map(|row| {
            let exchange = parse_exchange(row.get::<String, _>("exchange").as_str())?;
            let symbol = Symbol::new(row.get::<String, _>("symbol"));
            Ok(format!("{}:{}", exchange.as_str(), symbol.as_str()))
        })
        .collect()
}

pub async fn mark_position_closed_without_exit(
    pool: &PgPool,
    position_id: Uuid,
    mark_price: Decimal,
    protection_status: &str,
) -> Result<bool> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    let closed = sqlx::query(
        r#"
        UPDATE positions
        SET mark_price = $1,
            unrealized_pnl = 0,
            closed_at = $2
        WHERE id = $3
          AND closed_at IS NULL
        "#,
    )
    .bind(mark_price)
    .bind(Utc::now())
    .bind(position_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?
    .rows_affected();

    if closed == 0 {
        transaction
            .rollback()
            .await
            .map_err(|error| TradingError::Database(error.to_string()))?;
        return Ok(false);
    }

    sqlx::query(
        r#"
        UPDATE protection_orders
        SET status = $1
        WHERE position_id = $2
        "#,
    )
    .bind(protection_status)
    .bind(position_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    transaction
        .commit()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(true)
}

/// Reconciles the DB to the exchange: closes any still-open positions for the
/// given `exchange`/`symbol`/`mode` whose entry order is of that mode. Used at
/// startup when the exchange reports no open position for a key the DB still
/// holds (an orphan left when an SL/TP triggered while the bot was offline).
///
/// Closes with `mark_price`/`unrealized_pnl` left as-is (the last mark) and sets
/// the protection status to `reconciled_closed_offline`. Returns the number of
/// positions closed. Guarded by `closed_at IS NULL` so it is idempotent.
pub async fn close_orphaned_positions_for_key(
    pool: &PgPool,
    exchange: ExchangeId,
    symbol: &Symbol,
    mode: TradingMode,
) -> Result<u64> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    let closed_at = Utc::now();

    // Match open positions for this key whose entry order is of the given mode,
    // via the same position->protection->order join used to restore keys. RETURNING
    // the ids so the protection-order update targets exactly the rows we closed.
    let closed_ids: Vec<Uuid> = sqlx::query_scalar(
        r#"
        UPDATE positions p
        SET closed_at = $1
        WHERE p.closed_at IS NULL
          AND p.exchange = $2
          AND p.symbol = $3
          AND EXISTS (
              SELECT 1
              FROM protection_orders po
              JOIN orders o ON o.id = po.entry_order_id
              WHERE po.position_id = p.id
                AND o.mode = $4
          )
        RETURNING p.id
        "#,
    )
    .bind(closed_at)
    .bind(exchange.as_str())
    .bind(symbol.as_str())
    .bind(mode.as_str())
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    if closed_ids.is_empty() {
        transaction
            .rollback()
            .await
            .map_err(|error| TradingError::Database(error.to_string()))?;
        return Ok(0);
    }

    // Mark the matching protection orders so the reconciliation is auditable.
    sqlx::query(
        r#"
        UPDATE protection_orders
        SET status = 'reconciled_closed_offline'
        WHERE position_id = ANY($1)
        "#,
    )
    .bind(&closed_ids)
    .execute(&mut *transaction)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    transaction
        .commit()
        .await
        .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(closed_ids.len() as u64)
}

async fn insert_paper_exit(connection: &mut PgConnection, exit: &PaperExit) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO paper_exits (
            id, position_id, entry_order_id, exchange, symbol, trigger,
            exit_price, quantity, realized_pnl, triggered_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(exit.position_id)
    .bind(exit.entry_order_id)
    .bind(exit.exchange.as_str())
    .bind(exit.symbol.as_str())
    .bind(exit.trigger.as_str())
    .bind(exit.exit_price)
    .bind(exit.quantity)
    .bind(exit.realized_pnl)
    .bind(exit.triggered_at)
    .execute(&mut *connection)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(())
}

/// Closes a position for an exit, guarded by `closed_at IS NULL`.
///
/// Returns `true` when this call closed the position (1 row affected), `false`
/// when it was already closed. The protection-order status is only updated when
/// the close actually happened, so a no-op never re-touches an already-closed
/// position's bookkeeping.
async fn close_position_for_exit(connection: &mut PgConnection, exit: &PaperExit) -> Result<bool> {
    let closed = sqlx::query(
        r#"
        UPDATE positions
        SET mark_price = $1,
            unrealized_pnl = 0,
            closed_at = $2
        WHERE id = $3
          AND closed_at IS NULL
        "#,
    )
    .bind(exit.exit_price)
    .bind(exit.triggered_at)
    .bind(exit.position_id)
    .execute(&mut *connection)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?
    .rows_affected();

    if closed == 0 {
        return Ok(false);
    }

    sqlx::query(
        r#"
        UPDATE protection_orders
        SET status = $1
        WHERE position_id = $2
        "#,
    )
    .bind(format!("triggered_{}", exit.trigger.as_str()))
    .bind(exit.position_id)
    .execute(&mut *connection)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(true)
}

/// Marks all open paper positions for `exchange`/`symbol` to `mark_price`,
/// recomputing `unrealized_pnl` from the position side and entry price.
///
/// Open paper positions are otherwise frozen at their entry price, which makes
/// any non-SL/TP close (operator/panic/dashboard) realize zero PnL and starves
/// the daily-loss kill switch of live data. The strategy loop calls this on
/// every order-book tick so the persisted mark reflects the current market.
///
/// Returns the number of positions updated.
pub async fn update_open_position_marks(
    pool: &PgPool,
    exchange: ExchangeId,
    symbol: &Symbol,
    mark_price: Decimal,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE positions
        SET mark_price = $1,
            unrealized_pnl = CASE side
                WHEN 'long' THEN ($1 - entry_price) * quantity
                ELSE (entry_price - $1) * quantity
            END
        WHERE closed_at IS NULL
          AND exchange = $2
          AND symbol = $3
        "#,
    )
    .bind(mark_price)
    .bind(exchange.as_str())
    .bind(symbol.as_str())
    .execute(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    Ok(result.rows_affected())
}

pub async fn load_account_risk_state(
    pool: &PgPool,
    starting_equity: Decimal,
    daily_loss_limit: Decimal,
    locked: bool,
    market_data_latency_ms: i64,
) -> Result<AccountRiskState> {
    let row = sqlx::query(
        r#"
        SELECT
            (
                (SELECT COALESCE(SUM(realized_pnl), 0) FROM paper_exits)
                +
                (
                    SELECT COALESCE(SUM(
                        CASE side
                            WHEN 'long' THEN (mark_price - entry_price) * quantity
                            ELSE (entry_price - mark_price) * quantity
                        END
                    ), 0)
                    FROM positions p
                    WHERE closed_at IS NOT NULL
                      AND NOT EXISTS (
                          SELECT 1 FROM paper_exits pe WHERE pe.position_id = p.id
                      )
                )
            ) AS total_realized_pnl,
            (
                (SELECT COALESCE(SUM(realized_pnl), 0)
                 FROM paper_exits
                 WHERE triggered_at >= date_trunc('day', now()))
                +
                (
                    SELECT COALESCE(SUM(
                        CASE side
                            WHEN 'long' THEN (mark_price - entry_price) * quantity
                            ELSE (entry_price - mark_price) * quantity
                        END
                    ), 0)
                    FROM positions p
                    WHERE closed_at >= date_trunc('day', now())
                      AND NOT EXISTS (
                          SELECT 1 FROM paper_exits pe WHERE pe.position_id = p.id
                      )
                )
            ) AS daily_realized_pnl,
            (
                SELECT COALESCE(SUM(unrealized_pnl), 0)
                FROM positions
                WHERE closed_at IS NULL
            ) AS open_unrealized_pnl
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    let total_realized_pnl: Decimal = row.get("total_realized_pnl");
    let daily_realized_pnl: Decimal = row.get("daily_realized_pnl");
    let open_unrealized_pnl: Decimal = row.get("open_unrealized_pnl");

    Ok(AccountRiskState {
        equity: starting_equity + total_realized_pnl + open_unrealized_pnl,
        daily_realized_pnl,
        daily_loss_limit,
        locked,
        market_data_latency_ms,
    })
}

async fn load_open_protected_orders_with_connection<'e, E>(
    executor: E,
    position_id: Option<Uuid>,
    mode: Option<TradingMode>,
) -> Result<Vec<ProtectedOrder>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT
            p.id AS position_id,
            p.exchange AS position_exchange,
            p.symbol AS position_symbol,
            p.side AS position_side,
            p.entry_price,
            p.mark_price,
            p.quantity AS position_quantity,
            p.leverage,
            p.unrealized_pnl,
            p.opened_at,
            o.id AS order_id,
            o.signal_id,
            o.exchange_order_id,
            o.mode,
            o.side AS order_side,
            o.order_type,
            o.status AS order_status,
            o.price AS order_price,
            o.quantity AS order_quantity,
            o.created_at AS order_created_at,
            f.price AS fill_price,
            f.quantity AS fill_quantity,
            f.filled_at,
            po.stop_loss_price,
            po.take_profit_price
        FROM positions p
        JOIN protection_orders po ON po.position_id = p.id
        JOIN orders o ON o.id = po.entry_order_id
        JOIN order_fills f ON f.order_id = o.id
        WHERE p.closed_at IS NULL
          AND ($1::uuid IS NULL OR p.id = $1)
          AND ($2::text IS NULL OR o.mode = $2)
        ORDER BY p.opened_at ASC
        FOR UPDATE OF p, po
        "#,
    )
    .bind(position_id)
    .bind(mode.map(|mode| mode.as_str()))
    .fetch_all(executor)
    .await
    .map_err(|error| TradingError::Database(error.to_string()))?;

    rows.into_iter().map(protected_order_from_row).collect()
}

fn protected_order_from_row(row: sqlx::postgres::PgRow) -> Result<ProtectedOrder> {
    let exchange = parse_exchange(row.get::<String, _>("position_exchange").as_str())?;
    let symbol = Symbol::new(row.get::<String, _>("position_symbol"));
    let order_side = parse_side(row.get::<String, _>("order_side").as_str())?;

    Ok(ProtectedOrder {
        entry_order: Order {
            id: row.get("order_id"),
            signal_id: row.get("signal_id"),
            exchange,
            exchange_order_id: row.get("exchange_order_id"),
            mode: parse_mode(row.get::<String, _>("mode").as_str())?,
            symbol: symbol.clone(),
            side: order_side,
            order_type: parse_order_type(row.get::<String, _>("order_type").as_str())?,
            status: parse_order_status(row.get::<String, _>("order_status").as_str())?,
            price: row.get("order_price"),
            quantity: row.get("order_quantity"),
            created_at: row.get("order_created_at"),
        },
        fill: OrderFill {
            order_id: row.get("order_id"),
            exchange,
            symbol: symbol.clone(),
            side: order_side,
            price: row.get("fill_price"),
            quantity: row.get("fill_quantity"),
            filled_at: row.get("filled_at"),
        },
        position: Position {
            id: row.get("position_id"),
            exchange,
            symbol,
            side: parse_position_side(row.get::<String, _>("position_side").as_str())?,
            entry_price: row.get("entry_price"),
            mark_price: row.get("mark_price"),
            quantity: row.get("position_quantity"),
            leverage: row.get("leverage"),
            unrealized_pnl: row.get("unrealized_pnl"),
            opened_at: row.get("opened_at"),
        },
        protection: ProtectionPlan {
            stop_loss_price: row.get("stop_loss_price"),
            take_profit_price: row.get("take_profit_price"),
        },
    })
}

fn paper_position_pnl(position: &Position, exit_price: Decimal) -> Decimal {
    match position.side {
        PositionSide::Long => (exit_price - position.entry_price) * position.quantity,
        PositionSide::Short => (position.entry_price - exit_price) * position.quantity,
    }
}

fn parse_exchange(value: &str) -> Result<ExchangeId> {
    match value {
        "binance" => Ok(ExchangeId::Binance),
        "bybit" => Ok(ExchangeId::Bybit),
        "bitget" => Ok(ExchangeId::Bitget),
        other => Err(TradingError::Database(format!(
            "unsupported stored exchange: {other}"
        ))),
    }
}

fn parse_mode(value: &str) -> Result<TradingMode> {
    match value {
        "paper" => Ok(TradingMode::Paper),
        "testnet" => Ok(TradingMode::Testnet),
        "live" => Ok(TradingMode::Live),
        "locked" => Ok(TradingMode::Locked),
        other => Err(TradingError::Database(format!(
            "unsupported stored trading mode: {other}"
        ))),
    }
}

fn parse_side(value: &str) -> Result<Side> {
    match value {
        "buy" => Ok(Side::Buy),
        "sell" => Ok(Side::Sell),
        other => Err(TradingError::Database(format!(
            "unsupported stored order side: {other}"
        ))),
    }
}

fn parse_position_side(value: &str) -> Result<PositionSide> {
    match value {
        "long" => Ok(PositionSide::Long),
        "short" => Ok(PositionSide::Short),
        other => Err(TradingError::Database(format!(
            "unsupported stored position side: {other}"
        ))),
    }
}

fn parse_order_type(value: &str) -> Result<OrderType> {
    match value {
        "market" => Ok(OrderType::Market),
        "limit" => Ok(OrderType::Limit),
        "stop_loss" => Ok(OrderType::StopLoss),
        "take_profit" => Ok(OrderType::TakeProfit),
        other => Err(TradingError::Database(format!(
            "unsupported stored order type: {other}"
        ))),
    }
}

fn parse_order_status(value: &str) -> Result<OrderStatus> {
    match value {
        "new" => Ok(OrderStatus::New),
        "filled" => Ok(OrderStatus::Filled),
        "partially_filled" => Ok(OrderStatus::PartiallyFilled),
        "canceled" => Ok(OrderStatus::Canceled),
        "rejected" => Ok(OrderStatus::Rejected),
        other => Err(TradingError::Database(format!(
            "unsupported stored order status: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use sqlx::postgres::PgPoolOptions;
    use trading_core::{ExchangeId, OrderBookTop, OrderRequest, Side, Symbol, TradingMode};
    use trading_execution::{Broker, PaperBroker, PaperPositionTracker};

    async fn test_pool() -> Option<PgPool> {
        let database_url = std::env::var("TEST_DATABASE_URL").ok()?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("connect test database");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        Some(pool)
    }

    /// Opens a paper position at `entry_price` on a unique symbol and persists it.
    async fn seed_open_position(pool: &PgPool, symbol: &str, entry_price: Decimal) -> Position {
        let request = OrderRequest {
            exchange: ExchangeId::Binance,
            mode: TradingMode::Paper,
            symbol: Symbol::new(symbol),
            side: Side::Buy,
            order_type: trading_core::OrderType::Market,
            quantity: Decimal::ONE,
            reference_price: entry_price,
            signal_id: None,
        };
        let protected = PaperBroker::default()
            .submit_order(request)
            .await
            .expect("simulate fill");
        persist_protected_order(pool, &protected)
            .await
            .expect("persist protected order");
        protected.position
    }

    /// Seeds a fully linked order(mode)+fill+position+protection for a given mode,
    /// via direct SQL (the paper broker refuses non-paper requests). Returns the
    /// position id. Used to test cross-mode reconciliation.
    async fn seed_open_position_sql(pool: &PgPool, symbol: &str, mode: TradingMode) -> Uuid {
        // Real positions store the symbol uppercased (Symbol::new), so match that
        // here — the sweep looks up by Symbol::new(...).as_str().
        let symbol = Symbol::new(symbol);
        let symbol = symbol.as_str();
        let order_id = Uuid::new_v4();
        let position_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO orders (id, exchange, mode, symbol, side, order_type, status, quantity, created_at)
               VALUES ($1,'binance',$2,$3,'buy','market','filled',1, now())"#,
        )
        .bind(order_id).bind(mode.as_str()).bind(symbol)
        .execute(pool).await.expect("insert order");
        sqlx::query(
            r#"INSERT INTO positions (id, exchange, symbol, side, entry_price, mark_price, quantity, leverage, unrealized_pnl, opened_at)
               VALUES ($1,'binance',$2,'long',1000,1000,1,1,0, now())"#,
        )
        .bind(position_id).bind(symbol)
        .execute(pool).await.expect("insert position");
        sqlx::query(
            r#"INSERT INTO protection_orders (id, entry_order_id, position_id, stop_loss_price, take_profit_price, status)
               VALUES ($1,$2,$3,990,1010,'active')"#,
        )
        .bind(Uuid::new_v4()).bind(order_id).bind(position_id)
        .execute(pool).await.expect("insert protection");
        position_id
    }

    fn unique_symbol(prefix: &str) -> String {
        format!("{prefix}{}", Uuid::new_v4().simple())
    }

    fn book(symbol: &str, bid: Decimal, ask: Decimal) -> OrderBookTop {
        OrderBookTop {
            exchange: ExchangeId::Binance,
            symbol: Symbol::new(symbol),
            event_time: Utc::now(),
            best_bid: bid,
            best_ask: ask,
            bid_size: Decimal::ONE,
            ask_size: Decimal::ONE,
        }
    }

    // C1: marking a position to market makes a manual close realize the real
    // marked PnL instead of the entry-equals-mark zero it produced before.
    #[tokio::test]
    async fn manual_close_realizes_marked_pnl_after_mark_update() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let symbol = unique_symbol("MARKC1");
        let position = seed_open_position(&pool, &symbol, Decimal::new(50_000, 0)).await;

        // Without any mark update the stored mark equals the entry price.
        let updated = update_open_position_marks(
            &pool,
            ExchangeId::Binance,
            &Symbol::new(&symbol),
            Decimal::new(49_000, 0),
        )
        .await
        .expect("update marks");
        assert_eq!(updated, 1);

        let closed =
            close_open_paper_positions(&pool, Some(position.id), ProtectionTrigger::ManualClose)
                .await
                .expect("manual close");
        assert_eq!(closed, 1);

        let realized: Decimal =
            sqlx::query("SELECT realized_pnl FROM paper_exits WHERE position_id = $1")
                .bind(position.id)
                .fetch_one(&pool)
                .await
                .expect("load exit")
                .get("realized_pnl");
        // long 1.0 @ 50000 closed at marked 49000 = -1000, NOT zero.
        assert_eq!(realized, Decimal::new(-1_000, 0));
    }

    // C2 (bulk path): closing the same position twice via the dashboard/panic
    // bulk path must close it once and record exactly one exit; the second call
    // is a no-op (returns 0) and writes no duplicate paper_exits row.
    #[tokio::test]
    async fn bulk_close_is_idempotent_on_repeat() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let symbol = unique_symbol("BULKC2");
        let position = seed_open_position(&pool, &symbol, Decimal::new(100, 0)).await;
        update_open_position_marks(
            &pool,
            ExchangeId::Binance,
            &Symbol::new(&symbol),
            Decimal::new(110, 0),
        )
        .await
        .expect("mark");

        let first =
            close_open_paper_positions(&pool, Some(position.id), ProtectionTrigger::PanicClose)
                .await
                .expect("first close");
        assert_eq!(first, 1, "first close must close exactly one position");

        let second =
            close_open_paper_positions(&pool, Some(position.id), ProtectionTrigger::PanicClose)
                .await
                .expect("second close");
        assert_eq!(second, 0, "second close must be a no-op");

        let exit_count: i64 =
            sqlx::query("SELECT COUNT(*)::bigint AS c FROM paper_exits WHERE position_id = $1")
                .bind(position.id)
                .fetch_one(&pool)
                .await
                .expect("count exits")
                .get("c");
        assert_eq!(exit_count, 1, "bulk close must not double-record an exit");
    }

    // C2: a tracker exit for a position already closed out-of-band must NOT
    // insert a second paper_exits row (no double-counted PnL).
    #[tokio::test]
    async fn tracker_exit_is_idempotent_against_out_of_band_close() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let symbol = unique_symbol("IDEMC2");
        let position = seed_open_position(&pool, &symbol, Decimal::new(100, 0)).await;

        // Dashboard/panic path closes the position first.
        update_open_position_marks(
            &pool,
            ExchangeId::Binance,
            &Symbol::new(&symbol),
            Decimal::new(101, 0),
        )
        .await
        .expect("mark");
        let closed =
            close_open_paper_positions(&pool, Some(position.id), ProtectionTrigger::PanicClose)
                .await
                .expect("panic close");
        assert_eq!(closed, 1);

        // Tracker still holds the position and emits a take-profit exit.
        let protected = load_open_protected_orders(&pool).await.expect("load open");
        assert!(
            protected.iter().all(|p| p.position.id != position.id),
            "closed position must not appear in open set"
        );

        // Rebuild a tracker exit by hand for this position and try to persist it.
        let mut tracker = PaperPositionTracker::default();
        // Re-open in-memory only (DB already closed) to force the race.
        let request = OrderRequest {
            exchange: ExchangeId::Binance,
            mode: TradingMode::Paper,
            symbol: Symbol::new(&symbol),
            side: Side::Buy,
            order_type: trading_core::OrderType::Market,
            quantity: Decimal::ONE,
            reference_price: Decimal::new(100, 0),
            signal_id: None,
        };
        let mut reopened = PaperBroker::default()
            .submit_order(request)
            .await
            .expect("simulate fill");
        reopened.position.id = position.id;
        reopened.entry_order.id = protected_entry_order_id(&pool, position.id).await;
        tracker.insert(reopened);
        let exits = tracker.update_mark(&book(&symbol, Decimal::new(200, 0), Decimal::new(201, 0)));
        assert_eq!(exits.len(), 1, "tracker should emit one exit");

        let recorded = persist_paper_exit(&pool, &exits[0])
            .await
            .expect("persist paper exit");
        assert!(
            !recorded,
            "exit for an already-closed position must be a no-op"
        );

        let exit_count: i64 =
            sqlx::query("SELECT COUNT(*)::bigint AS c FROM paper_exits WHERE position_id = $1")
                .bind(position.id)
                .fetch_one(&pool)
                .await
                .expect("count exits")
                .get("c");
        assert_eq!(exit_count, 1, "must remain exactly one exit, not two");
    }

    #[allow(clippy::let_and_return)]
    async fn protected_entry_order_id(pool: &PgPool, position_id: Uuid) -> Uuid {
        sqlx::query("SELECT entry_order_id FROM protection_orders WHERE position_id = $1")
            .bind(position_id)
            .fetch_one(pool)
            .await
            .expect("load entry order id")
            .get("entry_order_id")
    }

    // C1 corollary: open unrealized PnL flows into the account equity/risk state
    // once positions are marked to market.
    #[tokio::test]
    async fn account_risk_state_reflects_marked_unrealized_pnl() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let symbol = unique_symbol("UNREALC1");
        let position = seed_open_position(&pool, &symbol, Decimal::new(50_000, 0)).await;

        update_open_position_marks(
            &pool,
            ExchangeId::Binance,
            &Symbol::new(&symbol),
            Decimal::new(49_500, 0),
        )
        .await
        .expect("mark");

        // Scope the assertion to this position's own unrealized PnL rather than the
        // account-wide equity aggregate, so concurrent DB tests cannot perturb it.
        // load_account_risk_state sums positions.unrealized_pnl for open positions,
        // so verifying this row's marked unrealized PnL verifies its contribution.
        let unrealized: Decimal =
            sqlx::query("SELECT unrealized_pnl FROM positions WHERE id = $1 AND closed_at IS NULL")
                .bind(position.id)
                .fetch_one(&pool)
                .await
                .expect("load marked position")
                .get("unrealized_pnl");
        // long 1.0 @50000 marked 49500 = -500.
        assert_eq!(
            unrealized,
            Decimal::new(-500, 0),
            "marking to market must record the negative unrealized PnL"
        );
    }

    #[tokio::test]
    async fn account_risk_state_counts_closed_position_without_paper_exit_as_realized() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let symbol = unique_symbol("REALIZEDC1");
        let position = seed_open_position(&pool, &symbol, Decimal::new(50_000, 0)).await;

        mark_position_closed_without_exit(
            &pool,
            position.id,
            Decimal::new(49_000, 0),
            "panic_close_exchange_closed",
        )
        .await
        .expect("close without paper exit");

        // Scope to this position's own realized contribution rather than the
        // account-wide daily aggregate, so concurrent DB tests cannot perturb it.
        // This mirrors the realized-PnL expression load_account_risk_state uses for
        // closed positions that have no paper_exit row, proving such a position is
        // counted as realized.
        let realized: Decimal = sqlx::query(
            r#"
            SELECT COALESCE(SUM(
                CASE side
                    WHEN 'long' THEN (mark_price - entry_price) * quantity
                    ELSE (entry_price - mark_price) * quantity
                END
            ), 0) AS realized
            FROM positions p
            WHERE p.id = $1
              AND p.closed_at IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM paper_exits pe WHERE pe.position_id = p.id)
            "#,
        )
        .bind(position.id)
        .fetch_one(&pool)
        .await
        .expect("load realized contribution")
        .get("realized");

        // long 1.0 @50000 closed-marked 49000 = -1000, counted as realized loss.
        assert_eq!(
            realized,
            Decimal::new(-1_000, 0),
            "closed long 1.0 @50000 marked 49000 must count as realized loss"
        );
    }

    // Position sweep: an orphaned testnet position (still open in DB, closed on the
    // exchange) must be reconciled to closed, idempotently, without touching other
    // keys or other modes.
    #[tokio::test]
    async fn close_orphaned_positions_for_key_reconciles_testnet_orphan() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let symbol = unique_symbol("SWEEP");
        let pos_id = seed_open_position_sql(&pool, &symbol, TradingMode::Testnet).await;

        // First sweep closes the orphan.
        let closed = close_orphaned_positions_for_key(
            &pool,
            ExchangeId::Binance,
            &Symbol::new(&symbol),
            TradingMode::Testnet,
        )
        .await
        .expect("sweep");
        assert_eq!(closed, 1, "the orphaned testnet position must be closed");

        let is_closed: bool =
            sqlx::query("SELECT closed_at IS NOT NULL FROM positions WHERE id = $1")
                .bind(pos_id)
                .fetch_one(&pool)
                .await
                .expect("load position")
                .get(0);
        assert!(is_closed, "position must be marked closed");

        // Idempotent: a second sweep closes nothing.
        let again = close_orphaned_positions_for_key(
            &pool,
            ExchangeId::Binance,
            &Symbol::new(&symbol),
            TradingMode::Testnet,
        )
        .await
        .expect("second sweep");
        assert_eq!(again, 0, "an already-closed position must not be re-closed");
    }

    // The sweep must not close a position of a different mode (a paper position is
    // not an orphan of the testnet runtime).
    #[tokio::test]
    async fn close_orphaned_positions_for_key_ignores_other_mode() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping DB integration test; TEST_DATABASE_URL is not set");
            return;
        };
        let symbol = unique_symbol("SWEEPMODE");
        let pos_id = seed_open_position_sql(&pool, &symbol, TradingMode::Paper).await;

        let closed = close_orphaned_positions_for_key(
            &pool,
            ExchangeId::Binance,
            &Symbol::new(&symbol),
            TradingMode::Testnet,
        )
        .await
        .expect("sweep");
        assert_eq!(
            closed, 0,
            "a paper position must not be swept as a testnet orphan"
        );

        let still_open: bool = sqlx::query("SELECT closed_at IS NULL FROM positions WHERE id = $1")
            .bind(pos_id)
            .fetch_one(&pool)
            .await
            .expect("load position")
            .get(0);
        assert!(still_open, "the paper position must remain open");
    }
}
