# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

Bunker is a Rust CLI tool for deploying web apps locally on macOS. Laravel is the first supported framework; others will follow. It manages FrankenPHP (Caddy), cloudflared tunnels, and queue/scheduler workers as macOS LaunchAgents. Single compiled binary, no runtime dependencies beyond the tools it orchestrates.

## Build & Run

```bash
cargo build                    # debug build
cargo build --release          # release build
cargo run -- <command>         # run during development
cargo run -- help              # see all commands
```

Install locally:
```bash
cargo install --path .         # installs to ~/.cargo/bin/bunker
```

## Architecture

- **Rust binary** with `clap` derive for CLI parsing
- **Zero project contamination** — all config lives in `~/.bunker/<project>/`, nothing written to the target project
- **macOS LaunchAgents** for service management (3-4 plists per project: server, tunnel, queue worker, and optionally scheduler)
- **Project resolution**: explicit arg → CWD basename lookup in `~/.bunker/` → error
- **Framework trait** — framework-specific logic (detection, templates, services) is behind a trait. Adding a new framework means implementing the trait, not modifying core commands.

### Per-Project Config Layout (`~/.bunker/<project>/`)

- `bunker.conf` — key=value config sourced/parsed by the CLI
- `Caddyfile` — FrankenPHP config (hardened: security headers, dotfile blocking, gzip/zstd, JSON access logs)
- `com.<project>.{server,tunnel,queue,scheduler}.plist` — LaunchAgent definitions (scheduler is optional, gated by `SCHEDULER_ENABLED` in config)
- `logs/` — stdout/stderr for each service + caddy access log

### Key Crates

- `clap` (derive) — CLI subcommands and arg parsing
- `dialoguer` — interactive prompts for `init`
- `colored` — terminal colors
- `serde` — config serialization
- `which` — binary path detection

## CLI Commands

`bunker init` | `start` | `stop` | `restart` | `status` | `run` | `logs` | `list` | `teardown` | `edit`

- `init` — interactive setup, auto-detects paths, scaffolds config, symlinks plists
- `run` — foreground mode via `npx concurrently` (debugging)
- `logs` — tail from `~/.bunker/<project>/logs/`, supports `--service` and `--follow`
- `teardown` — stops services, removes symlinks, optionally removes config dir

## Design Constraints

- macOS only — LaunchAgents, `launchctl`, Herd/Homebrew PHP paths
- Idempotent — `bunker init` twice updates, doesn't duplicate
- External access via cloudflared named tunnels only (no direct port exposure)
- Caddyfile binds to localhost; tunnel handles public routing
- Framework-aware — Laravel first, but framework-specific logic lives behind a trait so adding new frameworks is additive

## Testing

```bash
cargo test                     # run all tests
cargo test <test_name>         # run a single test
```

Manual integration testing against a real Laravel project:
```bash
cargo run -- init              # in a Laravel project dir
cargo run -- start
cargo run -- status
cargo run -- logs --follow
cargo run -- stop
cargo run -- teardown
```
