use sqlx::PgPool;
use trading_core::{MarketEvent, ObservedMarketEvent, Result, TradingError};

pub async fn persist_observed_market_event(
    pool: &PgPool,
    observed: &ObservedMarketEvent,
) -> Result<()> {
    match &observed.event {
        MarketEvent::Candle(candle) => {
            sqlx::query(
                r#"
                INSERT INTO candles (
                    exchange, symbol, timeframe, open_time, open, high, low, close, volume
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (exchange, symbol, timeframe, open_time)
                DO UPDATE SET
                    open = EXCLUDED.open,
                    high = EXCLUDED.high,
                    low = EXCLUDED.low,
                    close = EXCLUDED.close,
                    volume = EXCLUDED.volume
                "#,
            )
            .bind(candle.exchange.as_str())
            .bind(candle.symbol.as_str())
            .bind(&candle.timeframe)
            .bind(candle.open_time)
            .bind(candle.open)
            .bind(candle.high)
            .bind(candle.low)
            .bind(candle.close)
            .bind(candle.volume)
            .execute(pool)
            .await
            .map_err(|error| TradingError::Database(error.to_string()))?;
        }
        MarketEvent::OrderBook(order_book) => {
            sqlx::query(
                r#"
                INSERT INTO order_books (
                    exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                "#,
            )
            .bind(order_book.exchange.as_str())
            .bind(order_book.symbol.as_str())
            .bind(order_book.event_time)
            .bind(order_book.best_bid)
            .bind(order_book.best_ask)
            .bind(order_book.bid_size)
            .bind(order_book.ask_size)
            .execute(pool)
            .await
            .map_err(|error| TradingError::Database(error.to_string()))?;
        }
    }

    Ok(())
}
