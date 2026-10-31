# Testing

Use Rust 1.93+, CMake, a C++ compiler and libclang. On Debian/Ubuntu, install `cmake libclang-dev build-essential`; on macOS install CMake and LLVM and set `LIBCLANG_PATH` to LLVM's `lib` directory when needed.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
bash tools/check-architecture.sh
python3 scripts/test-release.py
cargo package --locked
```

`tests/review_correctness.rs` reproduces the original five correctness failures. `tests/trading_correctness_tests.rs` checks all three strategy payoff guarantees against allowed resolution states, actual-cost ledger replay, settlement losses, account binding, unresolved execution recovery, malformed fills, real local WebSocket transport and separation of inferred versus verified relationships. Adapter tests reject unconfirmed order responses as fills. SQLite tests verify replay cannot double-count closing profit.

The release-script tests use fake archives and transport, verify all four formula targets, and prove a bad checksum cannot replace an existing installation. They do not establish that remote release artifacts have been published.

Read-only public REST and WebSocket smoke tests are opt-in:

```sh
EDGELORD_SMOKE=1 cargo test --locked --test polymarket_live_smoke_tests -- --ignored --nocapture
```

These tests submit no orders and require no wallet. Authenticated API, LLM and Telegram tests are separately ignored and may require credentials or create external effects. Do not indiscriminately run all ignored tests. Release CI runs deterministic all-feature tests, not credentialed tests; `--all-features` alone does not run ignored tests.

Passing tests establishes the documented bounded behavior, not profitability, throughput under arbitrary load, or live end-to-end order execution. The automated suite does not validate funded execution or redemption. Before unattended use, validate a reviewed market universe and a separately authorized, bounded-capital trial including confirmed fills, a rejected leg, restart reconciliation and actual settlement/redemption.
