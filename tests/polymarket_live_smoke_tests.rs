use std::env;
use std::time::Duration;

use edgelord::adapter::outbound::polymarket::client::PolymarketClient;
use tokio::time::timeout;

fn smoke_enabled() -> bool {
    matches!(env::var("EDGELORD_SMOKE").ok().as_deref(), Some("1"))
}

#[tokio::test]
#[ignore = "requires EDGELORD_SMOKE=1 and network access"]
async fn smoke_polymarket_rest_markets_readonly() {
    if !smoke_enabled() {
        eprintln!("Skipping smoke test (set EDGELORD_SMOKE=1 to enable)");
        return;
    }

    let base_url = env::var("POLYMARKET_API_URL")
        .unwrap_or_else(|_| "https://clob.polymarket.com".to_string());
    let client = PolymarketClient::new(base_url.clone());

    let markets = timeout(Duration::from_secs(20), client.get_active_markets(5))
        .await
        .expect("Timed out querying Polymarket markets endpoint")
        .expect("Failed to fetch active markets");

    assert!(
        !markets.is_empty(),
        "Expected at least one active market from {base_url}"
    );
}

#[tokio::test]
#[ignore = "requires EDGELORD_SMOKE=1 and public network access; submits no orders"]
async fn smoke_polymarket_websocket_snapshots_and_deltas_readonly() {
    if !smoke_enabled() {
        return;
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    use edgelord::{
        adapter::outbound::polymarket::{dto::response::GammaMarket, stream::PolymarketDataStream},
        domain::id::TokenId,
        port::outbound::exchange::{MarketDataStream, MarketEvent},
    };
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let markets: Vec<GammaMarket>=http.get("https://gamma-api.polymarket.com/markets?active=true&closed=false&limit=100&order=volume24hr&ascending=false").send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    let tokens: Vec<_> = markets
        .iter()
        .flat_map(|market| market.token_ids())
        .take(40)
        .map(TokenId::new)
        .collect();
    assert!(!tokens.is_empty());
    let mut stream =
        PolymarketDataStream::new("wss://ws-subscriptions-clob.polymarket.com/ws/market".into());
    timeout(Duration::from_secs(15), stream.connect())
        .await
        .unwrap()
        .unwrap();
    stream.subscribe(&tokens).await.unwrap();
    timeout(Duration::from_secs(30), async {
        let mut snapshots = 0;
        loop {
            match stream.next_event().await {
                Some(MarketEvent::BookSnapshot { book, .. }) => {
                    assert!(book
                        .asks()
                        .windows(2)
                        .all(|levels| levels[0].price() <= levels[1].price()));
                    snapshots += 1;
                }
                Some(MarketEvent::BookDelta { .. }) if snapshots > 0 => break,
                Some(MarketEvent::Disconnected { reason }) => panic!("Disconnected: {reason}"),
                None => panic!("Stream ended"),
                _ => {}
            }
        }
    })
    .await
    .expect("No live snapshot followed by a price change within 30s");
}
