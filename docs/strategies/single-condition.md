# Single-condition strategy

For one binary $1 condition, buy equal numbers of YES and NO shares when the sum of executable asks is below $1. Exactly one of the two tokens must pay $1 at resolution.

If YES costs $0.40 and NO costs $0.50, one basket costs $0.90 and has a $1 settlement payout. At 100 shares per leg, cost is $90 and the gross payoff margin is $10. Available quantity is limited by the smaller ask size, then by total and per-market exposure budgets. This is a payoff calculation, not a claim that such prices persist or that all orders fill.

```toml
[strategies]
enabled = ["single_condition"]

[strategies.single_condition]
min_edge = 0.05
min_profit = 0.50
```

The detector uses current asks and equal quantities. Execution rechecks fresh REST depth, minimum size, price increments and market status. Current live support rejects fees and execution delays. Each leg is FOK, but the two-leg basket is not atomic: one leg can fill while the other fails. Confirmed fills become actual inventory; partial or uncertain execution halts further orders and retains exposure.

The execution journal restores inventory across restart. Settlement accounts for actual filled quantities and token payouts. Automatic redemption is not implemented. See [testing](../testing.md) and [operations](../deployment/operations.md#execution-and-recovery-limits).
