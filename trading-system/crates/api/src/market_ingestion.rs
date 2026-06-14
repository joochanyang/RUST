use sqlx::PgPool;
use tokio::sync::mpsc;
use trading_core::ObservedMarketEvent;
use trading_exchange::MarketStream;

use crate::market_repository::persist_observed_market_event;
use crate::risk_event_repository::{
    persist_market_latency_risk_event, MARKET_DATA_LATENCY_THRESHOLD_MS,
};
use crate::telegram::NotificationSender;

pub async fn run_market_ingestion_with_forwarder(
    mut stream: MarketStream,
    pool: PgPool,
    event_sender: Option<mpsc::Sender<ObservedMarketEvent>>,
    notifications: Option<NotificationSender>,
) {
    while let Some(message) = stream.recv().await {
        match message {
            Ok(observed) => {
                let exchange = observed.event.exchange();
                let symbol = observed.event.symbol().to_string();
                let latency_ms = observed.latency_ms;

                if latency_ms > MARKET_DATA_LATENCY_THRESHOLD_MS {
                    tracing::warn!(
                        ?exchange,
                        %symbol,
                        latency_ms,
                        "market data latency exceeded entry gate threshold"
                    );
                    if let Err(error) = persist_market_latency_risk_event(&pool, &observed).await {
                        tracing::error!(
                            %error,
                            ?exchange,
                            %symbol,
                            latency_ms,
                            "failed to persist market latency risk event"
                        );
                    }
                    notify(
                        &notifications,
                        format!(
                            "market latency warning\nexchange: {}\nsymbol: {}\nlatency_ms: {}",
                            exchange.as_str(),
                            symbol,
                            latency_ms
                        ),
                    )
                    .await;
                }

                if let Err(error) = persist_observed_market_event(&pool, &observed).await {
                    tracing::error!(
                        %error,
                        ?exchange,
                        %symbol,
                        latency_ms,
                        "failed to persist market event"
                    );
                    continue;
                }

                tracing::debug!(
                    ?exchange,
                    %symbol,
                    latency_ms,
                    "persisted market event"
                );

                if let Some(sender) = &event_sender {
                    if sender.send(observed).await.is_err() {
                        tracing::warn!("market event strategy forwarder is closed");
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "market stream emitted an error");
            }
        }
    }

    tracing::warn!("market stream ended");
}

async fn notify(sender: &Option<NotificationSender>, message: String) {
    if let Some(sender) = sender {
        if sender.send(message).await.is_err() {
            tracing::warn!("Telegram notification channel is closed");
        }
    }
}
