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

> Edgelord detects and executes bounded arbitrage baskets. All three strategies have deterministic payoff tests; this is not a claim of profitability or unattended live readiness. Execution currently supports fee-free, immediate-execution binary $1 contracts on Polygon with an EOA wallet. Multi-market strategies require operator-reviewed resolution relationships. See [verification and remaining limits](CORRECTNESS_REVIEW.md) before enabling trading.

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
