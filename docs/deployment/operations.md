# Operations

This runbook focuses on operating edgelord safely in production.

## Routine Commands

Commands that don't need secrets:

```console
$ systemctl status edgelord
$ journalctl -u edgelord -f
$ edgelord status --db /opt/edgelord/data/edgelord.db
$ edgelord statistics today --db /opt/edgelord/data/edgelord.db
```

Commands that need secrets:

```console
$ dugout run -- edgelord wallet status
$ dugout run -- edgelord check health --config /opt/edgelord/config/config.toml
$ dugout run -- edgelord check connection --config /opt/edgelord/config/config.toml
```

## Incident Triage Order

1. Confirm process health and restart state.
2. Confirm exchange/API connectivity.
3. Confirm wallet availability and gas/capital balances.
4. Confirm risk gates are not intentionally blocking execution.

## Hardening Checklist

- Service runs as non-root user
- Secrets managed via dugout (no plaintext `.env` files)
- Dugout identity file restricted (`chmod 600 ~/.dugout/identity`)
- SSH is key-based and hardened
- Host firewall is enabled
- Dependency and OS patch cadence is defined
- Backups exist for non-secret operational config

## Change Management

Before each deployment:

1. `cargo fmt --all -- --check`
2. `cargo test`
3. `check config`, `check health`, and `check live` against production config
4. Restart service and validate logs/status

## Recommended Rollout Pattern

- Stage in `dry_run` first.
- Observe behavior and metrics.
- Enable execution with low exposure caps.
- Increase limits only after stable operation windows.

## Execution and recovery limits

Live execution currently supports fee-free, immediate-execution contracts, Polygon chain 137, and a dedicated EOA wallet. Fresh REST depth, market status, tick size, minimum order size and zero taker fees are checked before orders are submitted. Each leg is FOK; a basket of orders is not atomic. An accepted HTTP response is not a confirmed fill.

Back up `edgelord.db` **and** `edgelord.db.execution.jsonl` together while the process is stopped. The journal durably records intent before submission and confirmed outcomes before releasing reservations. It is bound to the account; process locks prevent concurrent use on the same host. These locks do not coordinate separate hosts: run only one trader for an account.

Startup reconstructs open inventory and pending exposure and compares it to exchange holdings and open orders. Mismatch, an unresolved intent, or a partial basket blocks further trading. Ordinary circuit-breaker reset and Telegram resume cannot clear a reconciliation halt. Inspect the journal, exchange order/trade statuses and actual balances before any recovery. Do not delete or edit the journal to bypass a halt. Automated repair of ambiguous orders and automatic hedge/unwind are not implemented; preserving exposure is intentional.

Settlement uses actual token quantities and winning-token payouts, with WebSocket resolution events and REST polling. SQLite closing statistics are transactional and idempotent on restart; if a crash delayed reporting, the recovery day's report receives the close. Journal/accounting settlement is not redemption: automatic merging/redemption and cash collection are not implemented. Closed but unredeemed on-chain inventory can prevent restart reconciliation and needs manual redemption/reconciliation. External/manual wallet trading is unsupported while the bot owns the account.

Read-only smoke tests and deterministic payoff tests do not establish funded execution, latency competitiveness or profitability. Before unattended operation, a separately authorized bounded-capital trial must cover fills, rejection/partial execution, restart and settlement/redemption.

## Releasing and installing

This repository is also its Homebrew tap. No `usealtoal` repository or separate tap token is required:

```sh
brew tap rdekovich/edgelord https://github.com/rdekovich/edgelord.git
brew install --HEAD rdekovich/edgelord/edgelord
```

Use that command only after the new formula/code have reached `main`. Initially the formula is HEAD-only so it cannot silently install an older release lacking the correctness fixes. It builds from source with Homebrew Rust, CMake and LLVM. Linux and macOS are supported; Windows is not currently supported because execution/account locking uses Unix APIs.

Release preparation:

1. Ensure the crates.io `edgelord` owner account has publication rights. A GitHub repository transfer does not transfer crates.io ownership. Ownership could not be verified during this review because the registry API returned HTTP 403.
2. Add repository Actions secret `CARGO_REGISTRY_TOKEN` using a token scoped to publishing this crate. At review time this repository exposed only LLM secret names; no crate publishing secret was configured. Never put the token in a file or PR.
3. Set the version in `Cargo.toml`, update `Cargo.lock`, run the commands in [testing](../testing.md), and commit those changes. Tags must match the committed version; the workflow no longer rewrites source versions or ignores publication errors.
4. Push an explicitly approved release tag such as `v0.1.0`. The workflow checks the tag, tests and packages the crate, builds and smoke-tests four native release binaries, then publishes the crate. Publication failure prevents the GitHub release. No credentialed trading tests run in this workflow.
5. The workflow publishes archives/checksums and `edgelord.rb`, then pushes `homebrew/vVERSION` in this repository. Review and merge that branch into `main` to promote the stable formula. Branch creation needs the workflow's `contents: write` permission; branch protections may require a maintainer to create/merge the PR. There is no cross-repository token.

After a release containing these changes is published and the formula update merged:

```sh
brew install rdekovich/edgelord/edgelord
cargo install --locked edgelord
cargo binstall edgelord
curl -LsSf https://raw.githubusercontent.com/rdekovich/edgelord/main/scripts/install.sh | sh
```

The standalone installer downloads the archive and its SHA-256 and verifies it before replacing the executable. It installs in `$HOME/.edgelord/bin`; add that directory to your PATH. Override `EDGELORD_INSTALL_DIR` or set `EDGELORD_VERSION` (without `v`) to select an installation directory/release. Shell startup files are not modified. Checksums detect corruption; they are not independent publisher signatures.

New Linux archives use GNU libc, not musl: x86-64 builds use Ubuntu 22.04 (glibc 2.35), ARM64 uses Ubuntu 24.04 (glibc 2.39). The installer is intended for these releases; historical musl/Windows artifacts are not a supported installation of the corrected code. macOS builds use native Apple Silicon and Intel runners. The CI matrix is responsible for platform validation; only Linux x86-64 was exercised locally.

A release is not transactional across crates.io and GitHub. If crate publication succeeds but a later step fails, rerun only the failed release/formula job; do not bypass publication failures or overwrite immutable crate versions. If a formula branch was already pushed, review that branch rather than force-pushing it. No new crate, tag or binary was published as part of this review.

References: [Homebrew custom-remote taps](https://docs.brew.sh/Taps.html), [Cargo publishing](https://doc.rust-lang.org/cargo/commands/cargo-publish.html).
