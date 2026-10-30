# Strategy Guide

Implementation scope: single-condition buys YES+NO; rebalancing buys an operator-verified exhaustive group; combinatorial enumerates equal-quantity YES/NO baskets over 2–8 binary markets. The live path uses guaranteed resolution payouts and executable asks, not Frank–Wolfe distance. Relationship inference is advisory. Mathematical payoff coverage does not guarantee that every leg fills or that profitable opportunities exist. See [configuration](../configuration.md#strategies) and [verification](../../CORRECTNESS_REVIEW.md).


## What is Prediction Market Arbitrage?

Prediction markets price uncertain events as tradeable contracts. Each contract pays a fixed amount (typically $1) if its associated outcome occurs. In a well-functioning market, the prices of all mutually exclusive and exhaustive outcomes should sum to the payout value—this reflects the fundamental constraint that exactly one outcome must occur.

When prices temporarily deviate from this constraint, arbitrage opportunities emerge. If every required leg fills below the minimum settlement payout, the basket has a positive gross payoff under the assumed resolution rules. Orders are submitted separately; execution and resolution risks remain.

## The Fundamental Arbitrage Condition

For a market with *n* mutually exclusive outcomes O₁, O₂, ..., Oₙ where exactly one must occur:

**No arbitrage condition:**
$$\sum_{i=1}^{n} P(O_i) = \text{payout}$$

**Arbitrage opportunity exists when:**
$$\sum_{i=1}^{n} \text{ask}_i < \text{payout}$$

The **edge** represents the gross payoff margin per basket, conditional on full execution:
$$\text{edge} = \text{payout} - \sum_{i=1}^{n} \text{ask}_i$$

## Why Do Arbitrage Opportunities Exist?

Prediction markets are not perfectly efficient. Several factors create temporary price dislocations:

1. **Information asymmetry**: News affects different outcomes at different speeds
2. **Liquidity fragmentation**: Order books for different outcomes update independently
3. **Market maker latency**: Automated systems don't instantly rebalance all prices
4. **Behavioral factors**: Traders focus attention on high-probability outcomes, neglecting the tails

These inefficiencies create windows—typically lasting milliseconds to seconds—where prices violate the no-arbitrage condition.

## Available Strategies

Edgelord implements three strategies that detect different types of arbitrage opportunities:

| Strategy | Market Type | Detects |
|----------|-------------|---------|
| [Single-Condition](single-condition.md) | Binary (YES/NO) | ask_YES + ask_NO < 1 |
| [Market Rebalancing](market-rebalancing.md) | Multi-outcome (3+) | Σ ask_i < payout |
| [Combinatorial](combinatorial.md) | Related market clusters | Cross-market constraint violations |

## Strategy Selection

Configure each strategy explicitly. Multi-market strategies require reviewed resolution groups or constraints; the inference engine is optional and cannot authorize execution. See [configuration](../configuration.md#strategies).

## Shared Risk Considerations

All strategies share common execution risks:

- **Edge decay**: The opportunity may disappear between detection and execution
- **Partial fills**: Not all legs may execute, leaving directional exposure
- **Volume constraints**: Tradeable size equals the minimum depth across all legs

## Configuration

Enable strategies in your configuration:

```toml
[strategies]
enabled = ["single_condition", "market_rebalancing"]
```

Each strategy has its own configuration section for fine-tuning detection thresholds.
