<p align="center">
  <img src="assets/banner.png" alt="edgelord" width="1000">
</p>

<p align="center">
  <em>A prediction market arbitrage detection and execution CLI, written in Rust.</em>
</p>

<p align="center">
  <a href="https://github.com/rdekovich/edgelord/actions"><img src="https://github.com/rdekovich/edgelord/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/edgelord"><img src="https://img.shields.io/crates/v/edgelord.svg" alt="Crates.io"></a>
  <a href="https://github.com/rdekovich/edgelord/blob/main/LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue" alt="License"></a>
</p>

---

> Edgelord detects and executes bounded arbitrage baskets. All three strategies have deterministic payoff tests; this is not a claim of profitability or unattended live readiness. The International execution adapter still depends on the [archived V1 SDK](https://github.com/Polymarket/rs-clob-client), which upstream marks nonfunctional; live trading requires migration and verification against V2. The optional US integration currently provides read-only protocol checks, not trading. Multi-market strategies require operator-reviewed resolution relationships. See [testing and verification limits](docs/testing.md) before enabling trading.

---

## Highlights

- **Real-time arbitrage detection** across binary and multi-outcome markets
- **Three strategies** with explicit market-group configuration: single-condition, market rebalancing, combinatorial
- **LLM-powered inference** for relationship candidates; candidates cannot authorize trades
- **Risk management** with position limits, exposure caps, and circuit breakers
- **Telegram notifications** for trades, opportunities, and alerts
- **Durable execution journal** for restart recovery, plus SQLite reporting

## Installation

The correctness changes in this checkout are not yet published. To use this version, build this checkout on Linux or macOS with Rust 1.93+, CMake, a C++ compiler, and libclang:

```bash
cargo install --locked --path .
```

Homebrew is hosted in this repository. Once these changes are merged:

```bash
brew tap rdekovich/edgelord https://github.com/rdekovich/edgelord.git
brew install --HEAD rdekovich/edgelord/edgelord
```

The initial formula builds from source. After a new release and its formula update are merged, omit `--HEAD` to install the release binary. See [release and installation operations](docs/deployment/operations.md#releasing-and-installing) for crates.io, cargo-binstall, and checksum-verified standalone installation. Existing published versions do **not** contain this remediation.

## Quick Start

```console
$ cp config.toml.example config.toml
$ edgelord check config --config config.toml
$ edgelord run --config config.toml --dry-run
```

## Polymarket US

The optional Rust US boundary uses the [official US API contracts](https://github.com/Polymarket/polymarket-us-typescript). It provides signed read-only account requests and public market/book access. It is **not connected to `edgelord run`** and cannot submit orders. Changing the International API URL to a US URL does not enable US support.

From this checkout, check public connectivity without credentials:

```sh
cargo run --locked --features polymarket-us --example polymarket_us_check
```

To also inspect balances, open orders and paginated positions, provide `POLYMARKET_US_KEY_ID` and `POLYMARKET_US_SECRET_KEY` through your secret manager, then append `-- --account`. These are US developer portal credentials, separate from Polygon wallet keys and crates.io publishing tokens. The probe prints account balances to your terminal but never prints credentials.

US trading integration still requires signed-position accounting, collected fees, order reconciliation and a verified private data stream. US has one YES instrument per market; buying YES and selling YES does not reproduce the International two-token single-condition basket. Market rebalancing and combinatorial strategies need venue-specific execution validation before activation.

## Strategies

| Strategy | Executable basket | Required assumption |
|----------|-------------------|---------------------|
| `single_condition` | Buy YES + NO below $1 | One binary condition |
| `market_rebalancing` | Buy every YES below $1 | Reviewed, exclusive **and exhaustive** group |
| `combinatorial` | Enumerate YES/NO baskets against all allowed resolutions | Reviewed constraints; 2–8 binary markets |

```console
$ edgelord strategies list
$ edgelord strategies explain single_condition
```

## Commands

```console
$ edgelord init                  # Setup wizard
$ edgelord run --dry-run         # Detect without submitting orders
$ edgelord status                # Current state and today's P&L
$ edgelord statistics today      # Today's statistics
$ edgelord statistics week       # 7-day summary
$ edgelord check config          # Validate configuration
$ edgelord check live            # Live readiness checks
$ edgelord wallet status         # Token approvals
$ edgelord wallet approve 1000   # Approve $1000 for trading
```

See `edgelord --help` for all commands.

## Configuration

Create a config file with `edgelord init` or manually:

```toml
dry_run = true
exchange = "polymarket"
database = "edgelord.db"

[strategies]
enabled = ["single_condition"]

[risk]
max_position_per_market = 100
max_total_exposure = 500
min_profit_threshold = 0.50
max_slippage = 0.02
```

See [docs/configuration.md](docs/configuration.md) for full reference.

## Architecture

```
domain/         Pure types, no external dependencies
port/           Inbound and outbound contracts
adapter/        CLI, exchange integrations, notifications
application/    Use-case orchestration
infrastructure/ Config, bootstrap, runtime wiring
```

Hexagonal architecture with clean separation between domain logic and external integrations.

## Extending

Implement `port::inbound::strategy::Strategy` to add custom strategies:

```rust
impl Strategy for MyStrategy {
    fn detect(&self, ctx: &StrategyContext) -> Vec<Opportunity> {
        // Your detection logic
    }
}
```

See [docs/strategies/overview.md](docs/strategies/overview.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.
