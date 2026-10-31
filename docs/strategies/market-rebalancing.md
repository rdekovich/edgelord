# Market rebalancing

Buy one YES share in every member of a mutually exclusive **and exhaustive** event group when the sum of executable asks is below $1. Exactly one member must resolve YES and pay $1 under every allowed resolution. At-most-one is insufficient: if every member can resolve NO, the basket can pay zero.

For asks of $0.30, $0.35 and $0.31, one basket costs $0.96 and pays $1, conditional on the resolution assumptions and all required fills. If the smallest available ask size is 150 shares, the quote supports at most 150 baskets: cost $144, payout $150, gross margin $6. Risk limits can reduce the executable quantity further. Volume is shares per leg, not dollars.

Polymarket discovery exposes binary conditions. Configure `strategies.exhaustive_groups` with at least three distinct condition IDs; startup constructs an event market from their YES tokens while retaining the individual binary markets. Every member must survive discovery/filtering, or startup fails. Review actual settlement rules, deadlines, exceptional outcomes and cancellation behavior. A shortlist of likely winners is not an exhaustive set.

```toml
[strategies]
enabled = ["single_condition", "market_rebalancing"]
exhaustive_groups = [["condition-a", "condition-b", "condition-c"]]

[strategies.market_rebalancing]
min_edge = 0.03
min_profit = 1.00
max_outcomes = 10
```

Replace placeholders with reviewed condition IDs. A stale or missing book cannot authorize execution. Live preflight rejects fee-bearing or delayed markets and insufficient depth. Each leg is FOK, but the whole basket is not atomic; a partial fill leaves directional inventory and halts execution for reconciliation.

See [configuration](../configuration.md#strategies), [tests](../testing.md), and [recovery operations](../deployment/operations.md#execution-and-recovery-limits).
