#![cfg(feature = "polymarket")]

use edgelord::adapter::outbound::polymarket::{
    executor::PolymarketExecutor,
    settings::{Environment, PolymarketRuntimeConfig},
};

#[tokio::test]
async fn retired_protocol_fails_before_credentials_or_network_are_used() {
    let result = PolymarketExecutor::new(&PolymarketRuntimeConfig {
        private_key: "not-a-key".into(),
        chain_id: 137,
        api_url: "http://127.0.0.1:1".into(),
        environment: Environment::Mainnet,
    })
    .await;
    let error = result.err().expect("retired executor must fail");
    assert!(error.to_string().contains("V1 execution is disabled"));
}
