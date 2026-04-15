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
- **macOS LaunchAgents** for service management (4-5 plists per project: server, tunnel, queue worker, logrotate, and optionally scheduler)
- **Project resolution**: explicit arg → CWD basename lookup in `~/.bunker/` → error
- **Framework trait** — framework-specific logic (detection, templates, services) is behind a trait. Adding a new framework means implementing the trait, not modifying core commands.
- **Non-interactive mode** — `--yes` flag and CLI args on `init`/`teardown` for AI agents and scripting
- **Dry-run** — `--dry-run` on `init` previews all generated config without side effects

### Per-Project Config Layout (`~/.bunker/<project>/`)

- `bunker.conf` — key=value config parsed by the CLI
- `Caddyfile` — FrankenPHP config (hardened: security headers, HSTS, dotfile blocking, direct PHP blocking, gzip/zstd, console access logs with sensitive header stripping)
- `cloudflared.yml` — cloudflared tunnel config with ingress rules mapping hostname to localhost:port
- `com.bunker.<project>.{server,tunnel,queue,logrotate,scheduler}.plist` — LaunchAgent definitions (scheduler is optional, gated by `SCHEDULER_ENABLED` in config; logrotate runs daily at 3 AM)
- `logs/` — stdout/stderr for each service + caddy access log (rotated: 10MB cap, 5 copies)

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
- **Project names**: `[a-z0-9-]` only, no leading dash, max 64 chars
- **Tunnel names**: `[a-zA-Z0-9-]`, no leading dash, max 64 chars
- **Domains**: must contain `.`, `[a-zA-Z0-9.-]`, no leading dash, no consecutive/trailing dots, max 253 chars (63 per label)
- **Paths**: must be absolute, no `..` segments, no null bytes/newlines, no shell metacharacters (`"'`$\;|&><()`)
- **Plist values**: XML-escaped (including single quotes, control character stripping) before interpolation
- **Caddyfile paths**: quoted to handle spaces

## Security

- **File permissions**: `~/.bunker/` directories are 0o700, all generated files are 0o600
- **Atomic symlinks**: plist symlinks use create-tmp-then-rename to prevent TOCTOU races
- **Plist namespace**: labels prefixed `com.bunker.*` to avoid Apple namespace collisions
- **No shell invocation**: all `Command::new()` uses direct argv, never `sh -c`
- **Log hygiene**: Caddy access logs strip Authorization, Cookie, Set-Cookie, X-Api-Key headers
- **Log rotation**: per-project logrotate LaunchAgent caps logs at 10MB with 5 rotated copies

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
