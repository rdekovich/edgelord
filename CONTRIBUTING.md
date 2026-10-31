# Contributing to edgelord

## Setup

[Rust](https://rustup.rs/) 1.93+, CMake, a C++ compiler and libclang are required on Linux or macOS. See [testing](docs/testing.md) for platform prerequisites and the complete acceptance commands.

```console
$ git clone https://github.com/rdekovich/edgelord
$ cd edgelord
$ cargo build --locked
```

## Testing

```console
$ cargo test --locked --all-features
```

## Code Style

- `cargo fmt` before committing
- `cargo clippy --locked --all-targets --all-features -- -D warnings` must pass
- Doc comments on all public items
- Follow existing patterns in the codebase

## Architecture

Edgelord uses hexagonal architecture. See [ARCHITECTURE.md](ARCHITECTURE.md) for details.

| Layer | Purpose |
|-------|---------|
| `domain/` | Pure types, no I/O |
| `port/` | Inbound and outbound contracts |
| `application/` | Use-case orchestration |
| `adapter/` | CLI, exchange, storage implementations |
| `infrastructure/` | Runtime wiring and bootstrap |

## Commits

Single-line, conventional commit format:

```
<type>(<scope>): <description>
```

**Types:** `feat`, `fix`, `refactor`, `docs`, `test`, `chore`

**Examples:**
```
feat(strategy): add market rebalancing detector
fix(executor): handle partial fill edge case
docs(readme): update installation instructions
```

## Pull Requests

- One concern per PR
- Describe what changed and why
- Add tests for new functionality
- Ensure CI passes

## Reporting Issues

Use [GitHub Issues](https://github.com/rdekovich/edgelord/issues). Include:

- edgelord version (`edgelord --version`)
- OS and architecture
- Steps to reproduce
