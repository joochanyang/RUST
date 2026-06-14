use anyhow::{bail, Context};
use rust_decimal::Decimal;
use std::{env, net::SocketAddr};
use trading_core::TradingMode;

#[derive(Debug, Clone)]
pub struct Settings {
    pub api_host: String,
    pub api_port: u16,
    pub dashboard_control_token: Option<String>,
    pub database: DatabaseSettings,
    pub market_data: MarketDataSettings,
    pub ai_filter: AiFilterSettings,
    pub paper_trading: PaperTradingSettings,
    pub binance_testnet: BinanceTestnetSettings,
    pub telegram: TelegramSettings,
    pub trading: TradingSettings,
}

#[derive(Debug, Clone)]
pub struct DatabaseSettings {
    pub url: String,
    pub max_connections: u32,
    pub run_migrations: bool,
}

#[derive(Debug, Clone)]
pub struct TradingSettings {
    pub mode: TradingMode,
    pub live_trading_approved: bool,
}

#[derive(Debug, Clone)]
pub struct MarketDataSettings {
    pub enabled: bool,
    pub exchanges: Vec<String>,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PaperTradingSettings {
    pub enabled: bool,
    pub equity: Decimal,
    pub daily_loss_limit: Decimal,
    pub max_candles_per_key: usize,
}

#[derive(Debug, Clone)]
pub struct AiFilterSettings {
    pub enabled: bool,
    pub fail_closed: bool,
    pub macro_score: Decimal,
    pub long_bias: Decimal,
    pub short_bias: Decimal,
    pub pattern_confidence: Decimal,
    pub historical_win_rate: Decimal,
}

#[derive(Debug, Clone)]
pub struct BinanceTestnetSettings {
    pub enabled: bool,
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub max_order_notional: Decimal,
}

#[derive(Debug, Clone)]
pub struct TelegramSettings {
    pub enabled: bool,
    pub bot_token: Option<String>,
    pub notify_chat_id: Option<i64>,
    pub allowed_chat_id: Option<i64>,
}

impl MarketDataSettings {
    pub fn exchange_enabled(&self, exchange: &str) -> bool {
        self.exchanges.iter().any(|item| item == exchange)
    }
}

impl Settings {
    pub fn from_env() -> anyhow::Result<Self> {
        let trading = TradingSettings {
            mode: parse_mode(&env_value("TRADING_MODE", "paper"))?,
            live_trading_approved: parse_bool(&env_value("LIVE_TRADING_APPROVED", "false"))?,
        };

        if trading.mode == TradingMode::Live && !trading.live_trading_approved {
            bail!("live mode requires LIVE_TRADING_APPROVED=true");
        }

        let market_data = MarketDataSettings {
            enabled: parse_bool(&env_value("MARKET_DATA_ENABLED", "false"))?,
            exchanges: parse_csv_lower(&env_value("MARKET_DATA_EXCHANGES", "binance,bybit,bitget")),
            symbols: parse_csv(&env_value("MARKET_DATA_SYMBOLS", "BTCUSDT,ETHUSDT")),
        };

        if market_data.enabled && market_data.symbols.is_empty() {
            bail!("MARKET_DATA_SYMBOLS must not be empty when MARKET_DATA_ENABLED=true");
        }

        if market_data.enabled && market_data.exchanges.is_empty() {
            bail!("MARKET_DATA_EXCHANGES must not be empty when MARKET_DATA_ENABLED=true");
        }

        for exchange in &market_data.exchanges {
            if !matches!(exchange.as_str(), "binance" | "bybit" | "bitget") {
                bail!("unsupported MARKET_DATA_EXCHANGES value: {exchange}");
            }
        }

        let paper_trading = PaperTradingSettings {
            enabled: parse_bool(&env_value("PAPER_TRADING_ENABLED", "false"))?,
            equity: parse_decimal(&env_value("PAPER_EQUITY", "10000"))?,
            daily_loss_limit: parse_decimal(&env_value("PAPER_DAILY_LOSS_LIMIT", "500"))?,
            max_candles_per_key: env_value("PAPER_MAX_CANDLES_PER_KEY", "100")
                .parse()
                .context("PAPER_MAX_CANDLES_PER_KEY must be a valid usize")?,
        };

        if paper_trading.enabled && !market_data.enabled {
            bail!("PAPER_TRADING_ENABLED=true requires MARKET_DATA_ENABLED=true");
        }

        if trading.mode == TradingMode::Testnet && !market_data.enabled {
            bail!("TRADING_MODE=testnet requires MARKET_DATA_ENABLED=true");
        }

        let ai_filter = AiFilterSettings {
            enabled: parse_bool(&env_value("AI_FILTER_ENABLED", "false"))?,
            fail_closed: parse_bool(&env_value("AI_FILTER_FAIL_CLOSED", "true"))?,
            macro_score: parse_decimal(&env_value("AI_MACRO_SCORE", "0"))?,
            long_bias: parse_decimal(&env_value("AI_LONG_BIAS", "0"))?,
            short_bias: parse_decimal(&env_value("AI_SHORT_BIAS", "0"))?,
            pattern_confidence: parse_decimal(&env_value("AI_PATTERN_CONFIDENCE", "70"))?,
            historical_win_rate: parse_decimal(&env_value("AI_HISTORICAL_WIN_RATE", "70"))?,
        };

        let binance_testnet = BinanceTestnetSettings {
            enabled: parse_bool(&env_value("BINANCE_TESTNET_ENABLED", "false"))?,
            api_key: optional_env_value("BINANCE_TESTNET_API_KEY"),
            api_secret: optional_env_value("BINANCE_TESTNET_API_SECRET"),
            max_order_notional: parse_decimal(&env_value(
                "BINANCE_TESTNET_MAX_ORDER_NOTIONAL",
                "50",
            ))?,
        };

        if trading.mode == TradingMode::Testnet && !binance_testnet.enabled {
            bail!("TRADING_MODE=testnet requires BINANCE_TESTNET_ENABLED=true");
        }

        if binance_testnet.enabled
            && (binance_testnet.api_key.is_none() || binance_testnet.api_secret.is_none())
        {
            bail!("BINANCE_TESTNET_API_KEY and BINANCE_TESTNET_API_SECRET are required when BINANCE_TESTNET_ENABLED=true");
        }

        let telegram = TelegramSettings {
            enabled: parse_bool(&env_value("TELEGRAM_ENABLED", "false"))?,
            bot_token: optional_env_value("TELEGRAM_BOT_TOKEN"),
            notify_chat_id: optional_i64_env_value("TELEGRAM_NOTIFY_CHAT_ID")?,
            allowed_chat_id: optional_i64_env_value("TELEGRAM_ALLOWED_CHAT_ID")?,
        };

        if telegram.enabled && telegram.bot_token.is_none() {
            bail!("TELEGRAM_BOT_TOKEN is required when TELEGRAM_ENABLED=true");
        }

        if telegram.enabled && telegram.allowed_chat_id.is_none() {
            bail!("TELEGRAM_ALLOWED_CHAT_ID is required when TELEGRAM_ENABLED=true");
        }

        Ok(Self {
            api_host: env_value("API_HOST", "127.0.0.1"),
            api_port: env_value("API_PORT", "8080")
                .parse()
                .context("API_PORT must be a valid u16")?,
            dashboard_control_token: optional_env_value("DASHBOARD_CONTROL_TOKEN"),
            database: DatabaseSettings {
                url: env::var("DATABASE_URL").context("DATABASE_URL is required")?,
                max_connections: env_value("DATABASE_MAX_CONNECTIONS", "5")
                    .parse()
                    .context("DATABASE_MAX_CONNECTIONS must be a valid u32")?,
                run_migrations: parse_bool(&env_value("RUN_MIGRATIONS", "true"))?,
            },
            market_data,
            ai_filter,
            paper_trading,
            binance_testnet,
            telegram,
            trading,
        })
    }

    pub fn server_addr(&self) -> anyhow::Result<SocketAddr> {
        format!("{}:{}", self.api_host, self.api_port)
            .parse()
            .context("API_HOST and API_PORT must form a valid socket address")
    }
}

fn env_value(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn optional_env_value(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn optional_i64_env_value(key: &str) -> anyhow::Result<Option<i64>> {
    optional_env_value(key)
        .map(|value| {
            value
                .parse()
                .with_context(|| format!("{key} must be a valid i64"))
        })
        .transpose()
}

fn parse_bool(value: &str) -> anyhow::Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => bail!("invalid boolean value: {value}"),
    }
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.to_ascii_uppercase())
        .collect()
}

fn parse_csv_lower(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.to_ascii_lowercase())
        .collect()
}

fn parse_decimal(value: &str) -> anyhow::Result<Decimal> {
    value
        .parse::<Decimal>()
        .with_context(|| format!("invalid decimal value: {value}"))
}

fn parse_mode(value: &str) -> anyhow::Result<TradingMode> {
    match value.to_ascii_lowercase().as_str() {
        "paper" => Ok(TradingMode::Paper),
        "testnet" => Ok(TradingMode::Testnet),
        "live" => Ok(TradingMode::Live),
        "locked" => Ok(TradingMode::Locked),
        _ => bail!("TRADING_MODE must be paper, testnet, live, or locked"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_mode_requires_explicit_approval_flag() {
        assert_eq!(parse_mode("live").unwrap(), TradingMode::Live);
        assert!(parse_bool("false").is_ok());
    }

    #[test]
    fn invalid_mode_is_rejected() {
        assert!(parse_mode("demo").is_err());
    }

    #[test]
    fn boolean_parser_accepts_operator_friendly_values() {
        assert!(parse_bool("yes").unwrap());
        assert!(!parse_bool("off").unwrap());
    }

    #[test]
    fn csv_parser_normalizes_symbols() {
        assert_eq!(parse_csv("btcusdt, ethusdt"), vec!["BTCUSDT", "ETHUSDT"]);
    }

    #[test]
    fn csv_lower_parser_normalizes_exchanges() {
        assert_eq!(parse_csv_lower("Binance, BYBIT"), vec!["binance", "bybit"]);
    }

    #[test]
    fn decimal_parser_accepts_plain_numbers() {
        assert_eq!(parse_decimal("10000").unwrap(), Decimal::new(10_000, 0));
    }
}
