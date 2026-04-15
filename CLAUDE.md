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
cargo test                     # run all tests
cargo test <test_name>         # run a single test
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
- **Non-interactive mode** — `--yes` flag and CLI args on `init`/`teardown` for AI agents and scripting
- **Dry-run** — `--dry-run` on `init` previews all generated config without side effects

### Per-Project Config Layout (`~/.bunker/<project>/`)

- `bunker.conf` — key=value config parsed by the CLI
- `Caddyfile` — FrankenPHP config (hardened: security headers, HSTS, dotfile blocking, direct PHP blocking, gzip/zstd, JSON access logs)
- `cloudflared.yml` — cloudflared tunnel config with ingress rules mapping hostname to localhost:port
- `com.<project>.{server,tunnel,queue,scheduler}.plist` — LaunchAgent definitions (scheduler is optional, gated by `SCHEDULER_ENABLED` in config)
- `logs/` — stdout/stderr for each service + caddy access log

### Key Crates

- `clap` (derive) + `clap_complete` — CLI subcommands, arg parsing, shell completions
- `dialoguer` — interactive prompts for `init`
- `colored` — terminal colors
- `anyhow` — error handling
- `regex` — UUID extraction from cloudflared output
- `serde` + `serde_json` — config and JSON parsing
- `which` — binary path detection

## CLI Commands

```
bunker init [--yes] [--dry-run] [--name X] [--port X] [--domain X] [--tunnel X] [--scheduler]
bunker start [project]
bunker stop [project]
bunker restart [project]
bunker status [project]          # includes health check + domain
bunker run [project]             # foreground via npx concurrently
bunker logs [project] [--service=name] [--follow]
bunker list
bunker update [project]          # re-generate configs from bunker.conf
bunker teardown [project] [--yes]
bunker edit [project]
bunker completions <shell>       # bash, zsh, fish
```

## Input Validation

All user-supplied values are validated before use:
- **Project names**: `[a-z0-9-]` only
- **Tunnel names**: `[a-zA-Z0-9-]`, no leading dash
- **Domains**: must contain `.`, `[a-zA-Z0-9.-]`, no leading dash
- **Paths**: must be absolute, no newlines or null bytes
- **Plist values**: XML-escaped before interpolation
- **Caddyfile paths**: quoted to handle spaces

## Cloudflare Integration

- `init` creates the tunnel, generates `cloudflared.yml` with ingress rules, and routes DNS with `-f` (force overwrite)
- `teardown` removes DNS route and deletes the tunnel
- Named tunnels require a config YAML with ingress rules — the `--url` flag only works for quick tunnels

## Design Constraints

- macOS only — LaunchAgents, `launchctl`, Herd/Homebrew PHP paths
- Idempotent — `bunker init` twice updates, doesn't duplicate
- External access via cloudflared named tunnels only (no direct port exposure)
- Caddyfile binds to localhost; tunnel handles public routing
- Framework-aware — Laravel first, but framework-specific logic lives behind a trait so adding new frameworks is additive
