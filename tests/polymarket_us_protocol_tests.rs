#![cfg(feature = "polymarket-us")]

use edgelord::adapter::outbound::polymarket_us::model::*;
use rust_decimal_macros::dec;

#[test]
fn order_contract_uses_intent_slug_yes_price_and_fractional_numeric_quantity() {
    let order = LimitOrder::fill_or_kill(
        "example-market".into(),
        OrderIntent::BuyShort,
        dec!(0.6),
        dec!(0.125),
    )
    .unwrap();
    let json = serde_json::to_value(order).unwrap();
    assert_eq!(json["marketSlug"], "example-market");
    assert_eq!(json["intent"], "ORDER_INTENT_BUY_SHORT");
    assert_eq!(json["price"]["value"], "0.6");
    assert_eq!(json["quantity"].to_string(), "0.125");
    assert_eq!(json["tif"], "TIME_IN_FORCE_FILL_OR_KILL");
    assert!(json.get("symbol").is_none());
    assert!(json.get("action").is_none());
    assert!(
        LimitOrder::fill_or_kill("x".into(), OrderIntent::BuyLong, dec!(1.01), dec!(1)).is_err()
    );
}

#[test]
fn acknowledgements_are_not_fills_and_fees_are_required_for_filled_orders() {
    assert!(serde_json::from_str::<Order>(r#"{"id":"accepted"}"#).is_err());
    let mut order: Order = serde_json::from_str(
        r#"{
        "id":"order-1","marketSlug":"market-a","intent":"ORDER_INTENT_BUY_LONG",
        "state":"ORDER_STATE_CANCELED","quantity":10,"cumQuantity":2.5,"leavesQuantity":0,
        "avgPx":{"value":"0.6","currency":"USD"}
    }"#,
    )
    .unwrap();
    assert!(order.is_terminal());
    assert!(order.validate().is_err());
    order.commission_notional_total_collected = Some(Amount {
        value: dec!(0.02),
        currency: "USD".into(),
    });
    order.validate().unwrap();
    assert_eq!(order.cum_quantity, dec!(2.5));
    order.state = "ORDER_STATE_FILLED".into();
    assert!(order.validate().is_err());
    order.state = "NEW_UNKNOWN_STATE".into();
    assert!(!order.is_terminal());
}

#[test]
fn books_require_the_real_envelope_currency_and_nonzero_depth() {
    let response: BookResponse = serde_json::from_str(r#"{"marketData":{
        "marketSlug":"example-market","bids":[{"px":{"value":"0.23","currency":"USD"},"qty":"8.76"}],
        "offers":[],"state":"MARKET_STATE_OPEN","transactTime":"2026-10-02T20:10:00Z"
    }}"#).unwrap();
    response.market_data.validate().unwrap();
    assert!(response.market_data.is_open());
    assert_eq!(response.market_data.bids[0].qty, dec!(8.76));
    assert!(serde_json::from_str::<BookResponse>(r#"{"bids":[],"offers":[]}"#).is_err());
    let mut book = response.market_data;
    book.bids[0].px.currency = "USDC".into();
    assert!(book.validate().is_err());
    book.state = "MARKET_STATE_NEW_UNKNOWN".into();
    assert!(!book.is_open());
}
