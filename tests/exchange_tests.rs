//! Tests for exchange factory and approval components.

mod support;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use edgelord::adapter::outbound::polymarket::client::PolymarketClient;
use edgelord::adapter::outbound::polymarket::settings::{PolymarketConfig, PolymarketHttpConfig};
use edgelord::domain::id::TokenId;
use edgelord::error::{ConfigError, Error};
use edgelord::infrastructure::config::settings::{Config, ExchangeSpecificConfig};
use edgelord::infrastructure::exchange::factory::ExchangeFactory;
use edgelord::infrastructure::exchange::reconnecting::ReconnectingDataStream;
use edgelord::port::{outbound::exchange::MarketDataStream, outbound::exchange::MarketEvent};
use edgelord::testkit;
use edgelord::testkit::stream::ScriptedStream;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

#[test]
fn factory_returns_error_when_exchange_config_missing() {
    let config = Config {
        exchange_config: ExchangeSpecificConfig::Polymarket(PolymarketConfig {
            ws_url: String::new(),
            api_url: String::new(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = ExchangeFactory::create_scorer(&config);
    assert!(
        matches!(
            result,
            Err(Error::Config(ConfigError::MissingField { field: "ws_url" }))
        ),
        "Expected MissingField error when ws_url is empty"
    );

    let result = ExchangeFactory::create_filter(&config);
    assert!(
        matches!(
            result,
            Err(Error::Config(ConfigError::MissingField { field: "ws_url" }))
        ),
        "Expected MissingField error when ws_url is empty"
    );

    let result = ExchangeFactory::create_deduplicator(&config);
    assert!(
        matches!(
            result,
            Err(Error::Config(ConfigError::MissingField { field: "ws_url" }))
        ),
        "Expected MissingField error when ws_url is empty"
    );
}

#[tokio::test]
async fn reconnect_retries_on_subscribe_failure() {
    let mock = ScriptedStream::new()
        .with_connect_results(vec![Ok(()), Ok(()), Ok(())])
        .with_subscribe_results(vec![
            Ok(()),
            Err(Error::Connection("subscribe failed".into())),
            Ok(()),
        ])
        .with_events(vec![
            Some(testkit::domain::disconnect_event("test disconnect")),
            Some(testkit::domain::snapshot_event("token-1")),
        ]);
    let (connect_count, subscribe_count) = mock.counts();

    let mut stream = ReconnectingDataStream::new(mock, testkit::config::reconnection());
    stream.connect().await.unwrap();
    stream
        .subscribe(&[TokenId::from("token-1".to_string())])
        .await
        .unwrap();

    assert!(matches!(
        stream.next_event().await,
        Some(MarketEvent::Disconnected { .. })
    ));
    let event = stream.next_event().await;
    assert!(matches!(event, Some(MarketEvent::BookSnapshot { .. })));
    assert!(
        connect_count.load(Ordering::SeqCst) >= 3,
        "Expected reconnect to retry after subscribe failure"
    );
    assert!(
        subscribe_count.load(Ordering::SeqCst) >= 3,
        "Expected resubscribe retry after failure"
    );
}

#[tokio::test]
async fn client_times_out_on_slow_response() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicU32::new(0));
    let accepted_clone = Arc::clone(&accepted);

    let server = tokio::spawn(async move {
        while accepted_clone.load(Ordering::SeqCst) < 2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            accepted_clone.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            });
        }
    });

    let base_url = format!("http://{}", addr);
    let config = PolymarketConfig {
        api_url: base_url,
        http: PolymarketHttpConfig {
            timeout_ms: 10,
            connect_timeout_ms: 10,
            retry_max_attempts: 2,
            retry_backoff_ms: 0,
        },
        ..Default::default()
    };

    let client = PolymarketClient::from_config(&config);
    let result = client.get_active_markets(1).await;

    assert!(
        matches!(result, Err(Error::Http(err)) if err.is_timeout()),
        "Expected timeout error for slow response"
    );
    assert!(
        accepted.load(Ordering::SeqCst) >= 2,
        "Expected retry attempts for slow response"
    );

    let _ = server.await;
}
