# Getting started

The current correctness fixes are unreleased. Build this checkout on Linux or macOS with Rust 1.93+, CMake, a C++ compiler and libclang:

```sh
cargo install --locked --path .
cp config.toml.example config.toml
edgelord check config --config config.toml
edgelord run --config config.toml --dry-run
```

The example uses public Polygon market data and disables order submission. Setting `--testnet` does not turn Polymarket's production endpoints into a paper exchange; live execution accepts only chain 137. Do not treat successful config or health checks as proof of safe trading.

Start with no wallet secret. Review [strategy configuration](configuration.md#strategies), [testing](testing.md), and [operations](deployment/operations.md) before considering live execution. All three strategies are supported within the documented bounds; multi-market strategies require verified groups and relationships.

`WALLET_PRIVATE_KEY` enables wallet-backed execution only when dry-run is disabled. Optional secrets are `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`, and the selected LLM provider's API key. Keep credentials out of source control. Use a dedicated EOA wallet; existing or externally modified inventory fails reconciliation.

Use `edgelord --help` and each command's `--help` for supported flags. SQLite reports are available through `edgelord status --db edgelord.db` and `edgelord statistics today --db edgelord.db`. Preserve the accompanying execution journal with the database.

For Homebrew, standalone archives and crates.io publication status, see [installation operations](deployment/operations.md#releasing-and-installing).
