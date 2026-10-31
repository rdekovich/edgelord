//! Strategy listing and explanation.

use serde_json::json;
use tabled::{Table, Tabled};

use crate::adapter::inbound::cli::output;
use crate::error::Result;

const STRATEGY_SINGLE_CONDITION: &str = "single_condition";
const STRATEGY_MARKET_REBALANCING: &str = "market_rebalancing";
const STRATEGY_COMBINATORIAL: &str = "combinatorial";

#[derive(Tabled)]
struct StrategyRow {
    #[tabled(rename = "Name")]
    name: &'static str,
    #[tabled(rename = "Signal")]
    signal: &'static str,
    #[tabled(rename = "Typical Edge")]
    edge: &'static str,
}

fn normalize(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

/// List available strategies.
pub fn list() -> Result<()> {
    if output::is_quiet() && !output::is_json() {
        return Ok(());
    }

    if output::is_json() {
        output::json_output(json!({
            "command": "strategies.list",
            "strategies": [
                {
                    "name": STRATEGY_SINGLE_CONDITION,
                    "signal": "YES + NO < $1",
                    "typical_edge": "not guaranteed",
                },
                {
                    "name": STRATEGY_MARKET_REBALANCING,
                    "signal": "sum(outcomes) < $1",
                    "typical_edge": "not guaranteed",
                },
                {
                    "name": STRATEGY_COMBINATORIAL,
                    "signal": "cross-market constraints",
                    "typical_edge": "not guaranteed",
                },
            ],
        }));
        return Ok(());
    }

    output::header(env!("CARGO_PKG_VERSION"));
    output::section("Available strategies");

    let strategies = vec![
        StrategyRow {
            name: STRATEGY_SINGLE_CONDITION,
            signal: "YES + NO < $1",
            edge: "not guaranteed",
        },
        StrategyRow {
            name: STRATEGY_MARKET_REBALANCING,
            signal: "sum(outcomes) < $1",
            edge: "not guaranteed",
        },
        StrategyRow {
            name: STRATEGY_COMBINATORIAL,
            signal: "cross-market constraints",
            edge: "not guaranteed",
        },
    ];

    let table = Table::new(strategies).to_string();
    output::lines(&table);

    output::hint(&format!(
        "run {} for details",
        output::highlight("edgelord strategies explain <name>")
    ));

    Ok(())
}

/// Explain a specific strategy.
pub fn explain(name: &str) -> Result<()> {
    if output::is_quiet() && !output::is_json() {
        return Ok(());
    }

    let normalized_name = normalize(name);

    if output::is_json() {
        let payload = match normalized_name.as_str() {
            STRATEGY_SINGLE_CONDITION => json!({
                "command": "strategies.explain",
                "strategy": STRATEGY_SINGLE_CONDITION,
                "summary": "Detects arbitrage in binary (YES/NO) markets when YES + NO < $1.00 payout",
                "config_path": "strategies.single_condition",
            }),
            STRATEGY_MARKET_REBALANCING => json!({
                "command": "strategies.explain",
                "strategy": STRATEGY_MARKET_REBALANCING,
                "summary": "Detects arbitrage in multi-outcome markets when total outcome price < $1.00 payout",
                "config_path": "strategies.market_rebalancing",
            }),
            STRATEGY_COMBINATORIAL => json!({
                "command": "strategies.explain",
                "strategy": STRATEGY_COMBINATORIAL,
                "summary": "Detects cross-market opportunities using operator-verified constraints and bounded YES/NO basket enumeration",
                "config_path": "strategies.combinatorial",
            }),
            _ => json!({
                "command": "strategies.explain",
                "status": "unknown_strategy",
                "requested": name,
                "normalized": normalized_name,
                "available": [
                    STRATEGY_SINGLE_CONDITION,
                    STRATEGY_MARKET_REBALANCING,
                    STRATEGY_COMBINATORIAL,
                ],
            }),
        };
        output::json_output(payload);
        return Ok(());
    }

    output::header(env!("CARGO_PKG_VERSION"));

    match normalized_name.as_str() {
        STRATEGY_SINGLE_CONDITION => explain_single_condition(),
        STRATEGY_MARKET_REBALANCING => explain_market_rebalancing(),
        STRATEGY_COMBINATORIAL => explain_combinatorial(),
        _ => {
            output::error(&format!("Unknown strategy: {}", name));
            output::hint(&format!(
                "available strategies: {STRATEGY_SINGLE_CONDITION}, {STRATEGY_MARKET_REBALANCING}, {STRATEGY_COMBINATORIAL}"
            ));
            return Ok(());
        }
    }

    Ok(())
}

fn explain_single_condition() {
    output::section(STRATEGY_SINGLE_CONDITION);
    output::lines(
        "Detects arbitrage in binary (YES/NO) markets where:
YES price + NO price < $1.00 payout

Example:
  YES @ $0.45 + NO @ $0.52 = $0.97
  Payout = $1.00
  Edge = $0.03 (3%)

Configuration:
  [strategies.single_condition]
  min_edge = 0.05    # 5% minimum edge
  min_profit = 0.50  # $0.50 minimum profit",
    );
}

fn explain_market_rebalancing() {
    output::section(STRATEGY_MARKET_REBALANCING);
    output::lines(
        "Detects arbitrage in multi-outcome markets where:
sum(all outcome prices) < $1.00 payout

Requires reviewed strategies.exhaustive_groups: every condition must be
mutually exclusive AND exhaustive, with exactly one $1 YES winner.

Example (3-outcome market):
  Option A @ $0.30 + Option B @ $0.35 + Option C @ $0.32 = $0.97
  Payout = $1.00
  Edge = $0.03 (3%)

Configuration:
  [strategies.market_rebalancing]
  min_edge = 0.03",
    );
}

fn explain_combinatorial() {
    output::section(STRATEGY_COMBINATORIAL);
    output::lines(
        "Enumerates equal-quantity YES/NO baskets across 2-8 binary $1 markets.
A basket must cost less than its minimum payout over every allowed resolution.

Example: if A implies B, NO(A) + YES(B) pays at least $1.

Requires operator-reviewed strategies.verified_relations; LLM guesses cannot
permit trades. Configure real condition IDs and verify resolution rules.

Configuration:
  [strategies]
  enabled = [\"combinatorial\"]
  [[strategies.verified_relations]]
  type = \"implies\"
  if_yes = \"condition-a\"
  then_yes = \"condition-b\"
  [strategies.combinatorial]
  gap_threshold = 0.02

Each leg is FOK; a multi-leg basket is not atomic.",
    );
}
