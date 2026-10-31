//! US wire types. Required financial fields never default to zero.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::error::UsError;

/// A decimal amount and its explicit currency.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Amount {
    #[serde(with = "rust_decimal::serde::str")]
    pub value: Decimal,
    pub currency: String,
}

impl Amount {
    /// Reject currency mismatches rather than silently treating everything as USD.
    pub fn usd(&self) -> Result<Decimal, UsError> {
        if self.currency != "USD" {
            return Err(UsError::Protocol("expected USD amount"));
        }
        Ok(self.value)
    }
}

/// The four intents are distinct even when they have the same book direction.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum OrderIntent {
    #[serde(rename = "ORDER_INTENT_BUY_LONG")]
    BuyLong,
    #[serde(rename = "ORDER_INTENT_SELL_LONG")]
    SellLong,
    #[serde(rename = "ORDER_INTENT_BUY_SHORT")]
    BuyShort,
    #[serde(rename = "ORDER_INTENT_SELL_SHORT")]
    SellShort,
}

/// A validated FOK limit request. Construction does not submit an order.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitOrder {
    market_slug: String,
    intent: OrderIntent,
    #[serde(rename = "type")]
    order_type: &'static str,
    price: Amount,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    quantity: Decimal,
    tif: &'static str,
    manual_order_indicator: &'static str,
}

impl LimitOrder {
    /// Price is always expressed on the YES instrument, including short intents.
    /// Tick size and available quantity still require a fresh venue preflight.
    pub fn fill_or_kill(
        market_slug: String,
        intent: OrderIntent,
        yes_price: Decimal,
        quantity: Decimal,
    ) -> Result<Self, UsError> {
        validate_slug(&market_slug)?;
        if yes_price <= Decimal::ZERO || yes_price >= Decimal::ONE || quantity <= Decimal::ZERO {
            return Err(UsError::Protocol(
                "price must be inside (0, 1) and quantity positive",
            ));
        }
        Ok(Self {
            market_slug,
            intent,
            order_type: "ORDER_TYPE_LIMIT",
            price: Amount {
                value: yes_price,
                currency: "USD".into(),
            },
            quantity,
            tif: "TIME_IN_FORCE_FILL_OR_KILL",
            manual_order_indicator: "MANUAL_ORDER_INDICATOR_AUTOMATIC",
        })
    }
}

pub(crate) fn validate_slug(slug: &str) -> Result<(), UsError> {
    if slug.is_empty()
        || !slug
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(UsError::Protocol("invalid market slug"));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct Markets {
    pub markets: Vec<Market>,
}

#[derive(Debug, Deserialize)]
pub struct Market {
    pub slug: String,
    pub active: bool,
    pub closed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookResponse {
    pub market_data: MarketBook,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketBook {
    pub market_slug: String,
    pub bids: Vec<Level>,
    pub offers: Vec<Level>,
    /// Kept verbatim so an unknown state cannot be mistaken for OPEN.
    pub state: String,
    pub transact_time: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Level {
    pub px: Amount,
    #[serde(with = "rust_decimal::serde::str")]
    pub qty: Decimal,
}

impl MarketBook {
    /// Validate all levels; book state and freshness must be checked separately.
    pub fn validate(&self) -> Result<(), UsError> {
        validate_slug(&self.market_slug)?;
        for level in self.bids.iter().chain(&self.offers) {
            let price = level.px.usd()?;
            if price < Decimal::ZERO || price > Decimal::ONE || level.qty <= Decimal::ZERO {
                return Err(UsError::Protocol("invalid book price or quantity"));
            }
        }
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.state == "MARKET_STATE_OPEN"
    }
}

#[derive(Debug, Deserialize)]
pub struct Balances {
    pub balances: Vec<Balance>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    pub currency: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub current_balance: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub buying_power: Decimal,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Positions {
    pub positions: BTreeMap<String, Position>,
    pub next_cursor: Option<String>,
    pub eof: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Position {
    #[serde(with = "rust_decimal::serde::str")]
    pub net_position: Decimal,
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub net_position_decimal: Option<Decimal>,
    pub cost: Option<Amount>,
    pub realized: Amount,
    pub fees: Option<Amount>,
    pub expired: bool,
}

impl Position {
    /// Prefer the explicit decimal field when the server provides both formats.
    pub fn quantity(&self) -> Decimal {
        self.net_position_decimal.unwrap_or(self.net_position)
    }
}

#[derive(Debug, Deserialize)]
pub struct Orders {
    pub orders: Vec<Order>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub id: String,
    pub market_slug: String,
    pub intent: OrderIntent,
    /// Unknown states stay nonterminal until explicitly supported.
    pub state: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub quantity: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub cum_quantity: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub leaves_quantity: Decimal,
    pub avg_px: Option<Amount>,
    pub commission_notional_total_collected: Option<Amount>,
}

impl Order {
    /// Only a terminal state with zero leaves is final. A canceled order can
    /// still have fills; callers must account for `cum_quantity` and fees.
    pub fn is_terminal(&self) -> bool {
        self.leaves_quantity == Decimal::ZERO
            && matches!(
                self.state.as_str(),
                "ORDER_STATE_FILLED"
                    | "ORDER_STATE_CANCELED"
                    | "ORDER_STATE_REJECTED"
                    | "ORDER_STATE_EXPIRED"
            )
    }

    pub fn validate(&self) -> Result<(), UsError> {
        validate_slug(&self.market_slug)?;
        if self.id.is_empty()
            || self.quantity <= Decimal::ZERO
            || self.cum_quantity < Decimal::ZERO
            || self.leaves_quantity < Decimal::ZERO
            || self.cum_quantity > self.quantity
            || self.leaves_quantity > self.quantity - self.cum_quantity
        {
            return Err(UsError::Protocol("inconsistent order quantities"));
        }
        if self.state == "ORDER_STATE_FILLED" && self.cum_quantity != self.quantity {
            return Err(UsError::Protocol("filled order has incomplete quantity"));
        }
        if self.cum_quantity > Decimal::ZERO {
            let price = self
                .avg_px
                .as_ref()
                .ok_or(UsError::Protocol("filled order has no average price"))?
                .usd()?;
            if price < Decimal::ZERO || price > Decimal::ONE {
                return Err(UsError::Protocol("invalid average fill price"));
            }
            // An absent commission must not silently become a zero fee.
            self.commission_notional_total_collected
                .as_ref()
                .ok_or(UsError::Protocol(
                    "filled order has no collected commission",
                ))?
                .usd()?;
        }
        Ok(())
    }
}
