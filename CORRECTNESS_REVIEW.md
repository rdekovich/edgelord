# Correctness review

## Remediation status (unreleased working tree)

The original findings below describe commit `506190a`, not the corrected working tree. All five original reproductions now pass. Final Rust 1.93 all-feature regression: **1,332 passed, 0 failed, 31 ignored** (ignored tests include credentialed integrations, public smoke tests and illustrative doctests). The two opt-in public REST/WebSocket smoke tests separately passed. Installer/formula contract tests, Actionlint, architecture checks, all-target/all-feature Clippy, no-default/individual-feature compiler checks, packaged-crate rebuild, strict rustdoc build and example-config validation passed. Platform verification here was Linux x86-64; the release CI matrix covers macOS and Linux ARM64. The three strategies now produce executable baskets with tested minimum payouts; multi-market execution requires operator-verified complete groups/relations. Combinatorial execution enumerates 2–8 binary markets and never sizes orders from projection distance.

The feed now processes snapshots, absolute deltas and resolution events; tests exercise real local WebSocket transport and public read-only REST/WebSocket feeds. Execution uses preflight checks, explicit FOK, actual confirmed fills, durable intents/outcomes, retained partial exposure and a reconciliation halt for uncertainty. Restart binds/reconciles account inventory. Settlement uses actual token payouts, and SQLite close reporting is transactional and idempotent.

Release/installer repair moves ownership links to `rdekovich/edgelord`, fixes cargo-binstall archive layout, checks installer checksums, keeps the formula in this repo, and fails releases on publication errors. Existing releases do not contain these changes. See [operations](docs/deployment/operations.md#releasing-and-installing) for remaining publication setup.

Scope remains deliberately bounded: fee-free immediate-execution contracts, dedicated EOA wallet on Polygon, one running host/account, and reviewed resolution rules. There is no automatic redemption, ambiguous-order reconciliation, or hedge/unwind. No funded trading trial or new release was performed. These limits prevent claiming unattended live readiness or profitability. See [testing](docs/testing.md) for reproducible acceptance commands.

## Original baseline findings

Reviewed commit `506190abef0e14556f93f2ad57bfe3889e09bc9c` on 2026-10-30.

Verdict: builds successfully and has working isolated components, but is not a correct end-to-end live arbitrage trader. The README's claim that all three strategies work is not supported by the live wiring or the mathematical implementation.

## Validation

- Original `cargo test --locked`: **1,294 passed, 0 failed, 3 ignored**, including doctests. Started before adding the review tests.
- Public read-only Polymarket REST smoke test: **passed** (`EDGELORD_SMOKE=1 cargo test --locked --test polymarket_live_smoke_tests -- --ignored --nocapture`).
- `bash tools/check-architecture.sh`: passed.
- `target/debug/edgelord check config --config config.toml.example`: passed, with missing-wallet warning. This validates configuration parsing, not trading readiness.
- Added `tests/review_correctness.rs`: **five tests, all fail**, demonstrating the defects below. Run `cargo test --locked --test review_correctness`.
- CMake and libclang were missing from this machine; installed in `/tmp/edgelord-review-tools`. Build invocation used `PATH=/tmp/edgelord-review-tools/bin:$PATH LIBCLANG_PATH=/tmp/edgelord-review-tools/lib/python3.13/site-packages/clang/native cargo test --locked`.
- No orders, wallet transactions, or credentialed trading tests were performed. Actual profitability and full live execution are unverified. No production code changed.

## Findings, ordered by practical impact

### P1: Accepted orders are recorded as fully filled trades

`src/adapter/outbound/polymarket/executor.rs:138` and `:271` convert every successful HTTP/SDK result into a fill, without checking response `success`, `status`, `making_amount`, or `taking_amount`. The installed SDK 0.4.2 defaults these limit orders to GTC. An order resting on the book can therefore be reported as a completed arbitrage; the single-order path invents the full requested filled quantity and price. Position recording then uses requested quantities, not actual fills.

Fix: validate application-level rejection, track order lifecycle and actual filled quantities, reconcile every leg, and retain exposure for pending orders. Choose appropriate order lifetime explicitly; fill-or-kill per leg alone does not make a multi-leg transaction atomic.

### P1: The market stream discards live book objects and price changes

`src/adapter/outbound/polymarket/dto/message.rs:38` only recognizes arrays of full book snapshots. Individual `book` objects and `price_change` objects deserialize as `Unknown`, which `src/adapter/outbound/polymarket/stream.rs:319` silently skips. Initial snapshot arrays can work, but subsequent changes do not update cached prices or trigger detection. Removing the best ask leaves it in the local book.

Reproduced by `review_single_book_event_is_recognized` and `review_price_change_event_is_recognized`. Fix: parse supported event types, apply price-level changes, and explicitly manage resynchronization and heartbeat behavior.

Protocol reference: https://docs.polymarket.com/market-data/realtime-data

### P1: Partial fills are excluded from risk exposure

`src/application/cache/position.rs:48` counts only positions whose `is_open()` is true; `src/domain/position.rs:69` returns false for `PartialFill`. The recovery path records a partial position and releases the reservation (`src/application/orchestration/execution.rs:142`). The filled, unhedged inventory then contributes zero to total exposure and the per-market risk check.

Reproduced by `review_partial_fill_counts_toward_exposure`: $40 of filled inventory yields $0 exposure. Fix: count every nonclosed inventory position and reconcile actual fills before releasing reservations.

### P1: Timeout and cancellation handling can forget real inventory/orders

`src/application/orchestration/execution.rs:184` drops the execution future on timeout, releases all exposure, and unlocks the market without reconciling submissions. A server may have accepted or filled a leg before its response was lost. The partial recovery branch at `:130` also assumes successful cancellation means no inventory remains; cancellation of the unfilled remainder cannot reverse an earlier partial fill.

Fix: persist order identifiers/intents, reconcile ambiguous outcomes, cancel outstanding quantities, retain confirmed inventory, and block new execution while state is uncertain. This finding follows from control flow and exchange order semantics; no live orders were used to reproduce it.

Order semantics: https://docs.polymarket.com/trading/place-orders

### P1: The combinatorial projection is mathematically incorrect for its supported constraints

`src/application/solver/bregman.rs:32` computes only `sum(mu * ln(mu/theta))`, and its gradient includes `+1`. For arbitrary marginal vectors whose sums need not match, this is not a nonnegative Bregman divergence; the omitted linear terms matter. `src/application/solver/frank_wolfe.rs:114` also initializes from potentially infeasible prices, violating the feasible-iterate requirement for this Frank-Wolfe scheme.

Reproduced with already feasible prices `(0.4, 0.4)` under `x + y <= 1`: the solver changes them to approximately `(0.18788, 0.21385)`, reports divergence **-0.27588**, and fails to converge in 20 iterations. A projection must preserve an already feasible point.

Fix: define the correct objective for binary marginals, use a feasible initializer, validate feasibility/convergence, and add analytic cases for each supported relation type. A cited paper does not validate this implementation.

### P1: Combinatorial projection output is not converted into an executable arbitrage portfolio

`src/application/cluster/detector.rs:237` substitutes projected probabilities for actual ask prices; `:254` passes divergence as share volume and assumes a $1 payout. It always constructs buy legs for YES tokens. General implication/mutual-exclusion constraints do not guarantee that buying every YES pays $1, and some violations need opposite-side exposure.

With asks of $0.40 and a fixture projection of $0.50, `review_cluster_uses_executable_prices` returns $0.50 order prices and 0.2 shares (the fixture divergence), regardless of available volume of 100. This example becomes zero expected profit despite a complete-set acquisition cost of $0.80. The fixture isolates trade construction from the separate solver defect.

Fix: derive a portfolio and worst-case payoff over valid resolutions, price its executable sides from the order books, size from liquidity/risk limits, and include fees. Do not treat projection distance as executable share quantity.

### P1: Restart resets risk accounting despite existing exchange positions

`src/infrastructure/orchestration/runtime.rs:44` creates a fresh `AppState`, whose position tracker and pending exposure start empty (`src/application/state.rs:145`). There is no startup hydration/reconciliation of positions or open orders before execution. SQLite trade reporting does not restore the risk tracker. Restarting can therefore permit another full allocation on top of existing inventory.

Fix: reconcile exchange holdings/open orders and durable execution records before enabling trades.

### P2: Market rebalancing cannot receive an eligible live market

The default parser used by Polymarket, `src/port/outbound/exchange.rs:294`, discards every market whose outcome count is not two. The rebalancing strategy explicitly requires at least three outcomes (`src/application/strategy/market_rebalancing.rs`). Startup uses this parser with no event-level aggregation, so this enabled-by-default strategy cannot fire through the live discovery path. Polymarket multi-outcome events need grouping and verified exclusivity/exhaustiveness across constituent markets.

### P2: Settlement handling exists but is not connected to the Polymarket stream

The application can handle `MarketEvent::MarketSettled` (`src/application/orchestration/event.rs:69`), but the production Polymarket stream never emits it and there is no alternative settlement polling path. Positions therefore do not close through that handler in live operation, realized P&L is not updated through it, and exposure does not clear on resolution. There is also no implemented automatic merge/redemption lifecycle.

## What does work

The binary complete-set detector implements the basic gross-price condition correctly for its synthetic inputs: buy equal YES and NO quantities when executable asks sum below the payout, capped by the smaller quoted size. Many configuration, storage, CLI, architecture, and mock-based orchestration checks pass. Those successes do not establish current live-book correctness, fill correctness, net profit after fees, or safe recovery.

Recommended repair order: live data parsing; actual-fill accounting and partial exposure; timeout/restart reconciliation; settlement lifecycle; event grouping for rebalancing; then rebuild and independently validate the combinatorial solver and portfolio construction. Keep combinatorial execution disabled until its invariants are demonstrated.
