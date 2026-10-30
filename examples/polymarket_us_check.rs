//! Read-only protocol probe. Run with --account only to inspect authenticated data.
use edgelord::adapter::outbound::polymarket_us::{auth::Credentials, client::UsClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg != "--account") {
        return Err("Usage: polymarket_us_check [--account]".into());
    }
    let authenticated = args.iter().any(|arg| arg == "--account");
    let credentials = if authenticated {
        Some(Credentials::from_env()?)
    } else {
        None
    };
    let client = UsClient::new(credentials)?;
    let markets = client.markets(5).await?;
    println!(
        "Polymarket US · READ ONLY · {} market records",
        markets.len()
    );
    for market in markets
        .iter()
        .filter(|market| market.active && !market.closed)
    {
        let book = client.book(&market.slug).await?;
        println!(
            "{} · {} · {} bids / {} offers",
            book.market_slug,
            book.state,
            book.bids.len(),
            book.offers.len()
        );
    }
    if authenticated {
        for balance in client.balances().await? {
            println!(
                "{} · balance {} · buying power {}",
                balance.currency, balance.current_balance, balance.buying_power
            );
        }
        println!(
            "{} open orders · {} position records",
            client.open_orders().await?.len(),
            client.positions().await?.len()
        );
    }
    println!("Protocol check finished. This does not verify order execution or trading readiness.");
    Ok(())
}
