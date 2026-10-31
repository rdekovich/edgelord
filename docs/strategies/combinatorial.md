# Combinatorial strategy

The trading detector enumerates executable baskets across 2–8 binary $1 markets. For each market it chooses no token, YES, or NO, with equal share quantity on every selected leg. It enumerates every binary resolution satisfying the configured constraints, and uses the minimum basket payout over those resolutions. An opportunity requires that minimum payout to exceed current executable ask cost by `gap_threshold`, with sufficient quoted liquidity.

For the resolution implication A ⇒ B, buy NO(A) + YES(B). The allowed outcomes are (NO,NO), (NO,YES), and (YES,YES), paying respectively $1, $2 and $1 per basket. If both tokens together cost $0.90, the minimum payoff margin is $0.10 per basket, before execution risk. Buying YES(A) + YES(B) has no such guarantee.

For at-most-one of A and B, NO(A) + NO(B) pays at least $1 even when neither occurs. For an exactly-one group, buying every YES pays $1. These are conditional proofs: a mistaken resolution relationship invalidates the guarantee.

Configure `strategies.verified_relations` as described in [configuration](../configuration.md#strategies). LLM-inferred relationships are candidates for review only and cannot enter the execution cache. Statistical correlation, similar wording and confidence scores are not settlement guarantees.

The implementation deliberately excludes weighted baskets, short selling, clusters larger than eight, fee-bearing contracts and delayed execution. It searches a bounded family, not every possible arbitrage. The retained Frank–Wolfe/Bregman solver is research infrastructure; its distance or projection coordinates are never converted into trade quantities.

Each leg uses FOK, but the basket is not atomic across orders. Partial or ambiguous execution halts trading and preserves inventory/exposure for reconciliation. See [operations](../deployment/operations.md).
