# Deployment Guide

This section describes deployment prerequisites and current execution limits. The corrected implementation has deterministic and read-only live verification; funded execution and unattended operation have not been validated.

## Guides

- [Wallet Setup](wallet.md) — Prepare a dedicated Polygon wallet and funding model.
- [Telegram Integration](telegram.md) — Configure alerting and runtime bot control commands.
- [Operations](operations.md) — Monitoring, incident response, and hardening checklist.

## Live Trading Prerequisites

- Non-testnet configuration (`environment = "mainnet"`, `chain_id = 137`)
- `dry_run = false`
- Wallet available through private key or keystore
- `edgelord check health --config <path>` returns healthy
- `edgelord check live --config <path>` returns no blockers
- Reviewed exhaustive groups/relations for enabled multi-market strategies
- Dedicated EOA account with holdings and open orders reconciled to the execution journal
- Fee-free immediate-execution contracts and a separately validated bounded-capital trial

Health/readiness commands alone do not establish these conditions. Read [execution and recovery limits](operations.md#execution-and-recovery-limits) before disabling dry-run.

## Regulatory and Compliance Note

This documentation is technical guidance, not legal advice. Ensure your operating jurisdiction and user profile are compatible with the exchanges and assets you use.
