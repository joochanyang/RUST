use anyhow::{bail, Context};
use chrono::{DateTime, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, QueryBuilder};
use std::{env, time::Duration as StdDuration};

const DEFAULT_BASE_URL: &str = "https://fapi.binance.com";
const DEFAULT_SYMBOLS: &str = "BTCUSDT,ETHUSDT";
const DEFAULT_TIMEFRAME: &str = "1m";
const DEFAULT_PAGE_LIMIT: u16 = 1500;
const DEFAULT_DELAY_MS: u64 = 200;

#[derive(Debug, Clone)]
struct ImportConfig {
    database_url: String,
    base_url: String,
    symbols: Vec<String>,
    timeframe: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    page_limit: u16,
    request_delay: StdDuration,
    run_migrations: bool,
}

#[derive(Debug, Clone)]
struct HistoricalCandle {
    symbol: String,
    timeframe: String,
    open_time: DateTime<Utc>,
    open: Decimal,
    high: Decimal,
    low: Decimal,
    close: Decimal,
    volume: Decimal,
}

#[derive(Debug, Default)]
struct ImportSummary {
    symbols: usize,
    pages: u64,
    candles: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = ImportConfig::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await
        .context("failed to connect to PostgreSQL")?;

    if config.run_migrations {
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .context("failed to run database migrations")?;
    }

    let summary = import_binance_history(&pool, &Client::new(), &config).await?;
    println!(
        "imported historical candles: symbols={} pages={} candles={} range={}..{} timeframe={}",
        summary.symbols,
        summary.pages,
        summary.candles,
        config.start.to_rfc3339(),
        config.end.to_rfc3339(),
        config.timeframe
    );

    Ok(())
}

impl ImportConfig {
    fn from_env() -> anyhow::Result<Self> {
        let end = match optional_env("HISTORICAL_END") {
            Some(value) => parse_datetime(&value).context("invalid HISTORICAL_END")?,
            None => Utc::now(),
        };
        let start = match optional_env("HISTORICAL_START") {
            Some(value) => parse_datetime(&value).context("invalid HISTORICAL_START")?,
            None => end - Duration::days(365 * 3),
        };
        if end <= start {
            bail!("HISTORICAL_END must be after HISTORICAL_START");
        }

        let exchange = env_value("HISTORICAL_EXCHANGE", "binance").to_ascii_lowercase();
        if exchange != "binance" {
            bail!("only HISTORICAL_EXCHANGE=binance is currently supported");
        }

        let symbols = parse_symbols(&env_value("HISTORICAL_SYMBOLS", DEFAULT_SYMBOLS));
        if symbols.is_empty() {
            bail!("HISTORICAL_SYMBOLS must contain at least one symbol");
        }

        let page_limit = env_value("HISTORICAL_PAGE_LIMIT", &DEFAULT_PAGE_LIMIT.to_string())
            .parse::<u16>()
            .context("HISTORICAL_PAGE_LIMIT must be an integer")?;
        if !(1..=1500).contains(&page_limit) {
            bail!("HISTORICAL_PAGE_LIMIT must be between 1 and 1500");
        }

        let timeframe = env_value("HISTORICAL_TIMEFRAME", DEFAULT_TIMEFRAME);
        interval_duration(&timeframe)
            .with_context(|| format!("unsupported HISTORICAL_TIMEFRAME: {timeframe}"))?;

        Ok(Self {
            database_url: env::var("DATABASE_URL").context("DATABASE_URL is required")?,
            base_url: env_value("HISTORICAL_BINANCE_BASE_URL", DEFAULT_BASE_URL),
            symbols,
            timeframe,
            start,
            end,
            page_limit,
            request_delay: StdDuration::from_millis(
                env_value("HISTORICAL_REQUEST_DELAY_MS", &DEFAULT_DELAY_MS.to_string())
                    .parse()
                    .context("HISTORICAL_REQUEST_DELAY_MS must be an integer")?,
            ),
            run_migrations: parse_bool(&env_value("RUN_MIGRATIONS", "true"))?,
        })
    }
}

async fn import_binance_history(
    pool: &PgPool,
    client: &Client,
    config: &ImportConfig,
) -> anyhow::Result<ImportSummary> {
    let mut summary = ImportSummary {
        symbols: config.symbols.len(),
        ..ImportSummary::default()
    };
    let interval = interval_duration(&config.timeframe).expect("timeframe validated");

    for symbol in &config.symbols {
        let mut cursor = config.start;
        while cursor < config.end {
            let candles = fetch_binance_klines(client, config, symbol, cursor, config.end).await?;
            if candles.is_empty() {
                break;
            }

            let last_open_time = candles
                .last()
                .map(|candle| candle.open_time)
                .expect("checked non-empty");
            let count = candles.len() as u64;
            upsert_candles(pool, &candles).await?;
            summary.pages += 1;
            summary.candles += count;

            let next_cursor = last_open_time + interval;
            if next_cursor <= cursor {
                bail!("historical import cursor did not advance for {symbol}");
            }
            cursor = next_cursor;

            if cursor < config.end && !config.request_delay.is_zero() {
                tokio::time::sleep(config.request_delay).await;
            }
        }
    }

    Ok(summary)
}

async fn fetch_binance_klines(
    client: &Client,
    config: &ImportConfig,
    symbol: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> anyhow::Result<Vec<HistoricalCandle>> {
    let url = format!("{}/fapi/v1/klines", config.base_url.trim_end_matches('/'));
    let response = client
        .get(url)
        .query(&[
            ("symbol", symbol),
            ("interval", config.timeframe.as_str()),
            ("startTime", &start.timestamp_millis().to_string()),
            ("endTime", &end.timestamp_millis().to_string()),
            ("limit", &config.page_limit.to_string()),
        ])
        .send()
        .await
        .with_context(|| format!("failed to request Binance klines for {symbol}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read Binance kline response body")?;
    if !status.is_success() {
        bail!("Binance kline request failed with {status}: {body}");
    }

    parse_binance_klines(symbol, &config.timeframe, &body)
}

fn parse_binance_klines(
    symbol: &str,
    timeframe: &str,
    payload: &str,
) -> anyhow::Result<Vec<HistoricalCandle>> {
    let rows: Vec<Vec<Value>> =
        serde_json::from_str(payload).context("invalid Binance kline JSON")?;

    rows.into_iter()
        .map(|row| parse_binance_kline_row(symbol, timeframe, row))
        .collect()
}

fn parse_binance_kline_row(
    symbol: &str,
    timeframe: &str,
    row: Vec<Value>,
) -> anyhow::Result<HistoricalCandle> {
    if row.len() < 6 {
        bail!("Binance kline row has fewer than 6 fields");
    }

    Ok(HistoricalCandle {
        symbol: symbol.to_ascii_uppercase(),
        timeframe: timeframe.to_owned(),
        open_time: millis_to_utc(value_i64(&row[0], "open_time")?)?,
        open: value_decimal(&row[1], "open")?,
        high: value_decimal(&row[2], "high")?,
        low: value_decimal(&row[3], "low")?,
        close: value_decimal(&row[4], "close")?,
        volume: value_decimal(&row[5], "volume")?,
    })
}

async fn upsert_candles(pool: &PgPool, candles: &[HistoricalCandle]) -> anyhow::Result<()> {
    if candles.is_empty() {
        return Ok(());
    }

    let mut builder: QueryBuilder<Postgres> = QueryBuilder::new(
        r#"
        INSERT INTO candles (
            exchange, symbol, timeframe, open_time, open, high, low, close, volume
        )
        "#,
    );

    builder.push_values(candles, |mut row, candle| {
        row.push_bind("binance")
            .push_bind(&candle.symbol)
            .push_bind(&candle.timeframe)
            .push_bind(candle.open_time)
            .push_bind(candle.open)
            .push_bind(candle.high)
            .push_bind(candle.low)
            .push_bind(candle.close)
            .push_bind(candle.volume);
    });

    builder.push(
        r#"
        ON CONFLICT (exchange, symbol, timeframe, open_time)
        DO UPDATE SET
            open = EXCLUDED.open,
            high = EXCLUDED.high,
            low = EXCLUDED.low,
            close = EXCLUDED.close,
            volume = EXCLUDED.volume
        "#,
    );

    builder
        .build()
        .execute(pool)
        .await
        .context("failed to upsert historical candles")?;

    Ok(())
}

fn value_i64(value: &Value, field: &str) -> anyhow::Result<i64> {
    value
        .as_i64()
        .with_context(|| format!("Binance kline field {field} must be an integer"))
}

fn value_decimal(value: &Value, field: &str) -> anyhow::Result<Decimal> {
    value
        .as_str()
        .with_context(|| format!("Binance kline field {field} must be a string decimal"))?
        .parse::<Decimal>()
        .with_context(|| format!("Binance kline field {field} is not a decimal"))
}

fn millis_to_utc(value: i64) -> anyhow::Result<DateTime<Utc>> {
    match Utc.timestamp_millis_opt(value) {
        LocalResult::Single(time) => Ok(time),
        _ => bail!("invalid millisecond timestamp: {value}"),
    }
}

fn parse_datetime(value: &str) -> anyhow::Result<DateTime<Utc>> {
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        return Ok(time.with_timezone(&Utc));
    }

    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("expected RFC3339 timestamp or YYYY-MM-DD date: {value}"))?;
    Ok(date.and_hms_opt(0, 0, 0).context("invalid date")?.and_utc())
}

fn interval_duration(timeframe: &str) -> Option<Duration> {
    match timeframe {
        "1m" => Some(Duration::minutes(1)),
        "3m" => Some(Duration::minutes(3)),
        "5m" => Some(Duration::minutes(5)),
        "15m" => Some(Duration::minutes(15)),
        "30m" => Some(Duration::minutes(30)),
        "1h" => Some(Duration::hours(1)),
        "2h" => Some(Duration::hours(2)),
        "4h" => Some(Duration::hours(4)),
        "6h" => Some(Duration::hours(6)),
        "8h" => Some(Duration::hours(8)),
        "12h" => Some(Duration::hours(12)),
        "1d" => Some(Duration::days(1)),
        _ => None,
    }
}

fn parse_symbols(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_ascii_uppercase)
        .collect()
}

fn parse_bool(value: &str) -> anyhow::Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => bail!("invalid boolean value: {value}"),
    }
}

fn env_value(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_binance_kline_rows() {
        let payload = r#"[
          [
            1499040000000,
            "0.01634790",
            "0.80000000",
            "0.01575800",
            "0.01577100",
            "148976.11427815",
            1499644799999,
            "2434.19055334",
            308,
            "1756.87402397",
            "28.46694368",
            "17928899.62484339"
          ]
        ]"#;

        let candles = parse_binance_klines("btcusdt", "1m", payload).unwrap();

        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].symbol, "BTCUSDT");
        assert_eq!(candles[0].timeframe, "1m");
        assert_eq!(
            candles[0].open_time,
            Utc.timestamp_millis_opt(1499040000000).unwrap()
        );
        assert_eq!(candles[0].close, Decimal::new(1577100, 8));
    }

    #[test]
    fn parses_dates_and_rfc3339_timestamps() {
        assert_eq!(
            parse_datetime("2024-01-02").unwrap(),
            Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap()
        );
        assert_eq!(
            parse_datetime("2024-01-02T03:04:05Z").unwrap(),
            Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap()
        );
    }

    #[test]
    fn rejects_unsupported_intervals() {
        assert!(interval_duration("7m").is_none());
    }
}
