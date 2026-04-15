# Bunker — Product Requirements Document

## Overview

Bunker is a CLI tool for deploying Laravel applications locally on macOS using FrankenPHP (Caddy), cloudflared tunnels, and Laravel queue workers — managed as macOS LaunchAgents.

It replaces manual setup of Caddyfiles, plist files, cloudflared configs, and shell scripts with a single `bunker init` command. All deployment configuration lives outside the project repo in `~/.bunker/<project>/`.

## Problem

Deploying a Laravel app locally for production-like access (tunneled to a public domain) requires:
- Writing a Caddyfile for FrankenPHP
- Creating 3 macOS LaunchAgent plist files (server, tunnel, queue worker)
- Configuring cloudflared tunnel and DNS
- Writing management scripts for start/stop/restart/status
- Managing logs across multiple services

This is tedious, error-prone, and has to be repeated per project. The configuration files reference absolute paths, ports, tunnel names, and PHP binaries — all project-specific.

## Solution

A single CLI tool (`bunker`) that:
1. Interactively gathers project config (port, domain, tunnel name)
2. Auto-detects what it can (project path, PHP binary, frankenphp/cloudflared paths)
3. Scaffolds all config into `~/.bunker/<project>/`
4. Symlinks LaunchAgent plists into `~/Library/LaunchAgents/`
5. Manages the full lifecycle: init, start, stop, restart, status, logs, teardown

## Architecture

### Implementation

Rust CLI compiled to a single binary. Installed to `~/.local/bin/bunker` (or wherever the user's PATH includes).

Key crates:
- `clap` (derive) — CLI parsing, subcommands, help text
- `dialoguer` — interactive prompts during `init`
- `colored` — terminal output
- `serde` — config serialization
- `which` — binary detection

Framework-specific logic is behind a trait so adding new frameworks doesn't touch core command code.

### Runtime Dependencies

- `frankenphp`
- `cloudflared`
- `php` (Herd, Homebrew, or system)
- `launchctl` (macOS built-in)
- `npx` + `concurrently` (for foreground `run` mode only)

### Directory Structure

```
~/.bunker/
└── <project-name>/
    ├── bunker.conf          # project config (key=value, sourced by the CLI)
    ├── Caddyfile
    ├── cloudflared.yml      # cloudflared tunnel config with ingress rules
    ├── com.<project>.server.plist
    ├── com.<project>.tunnel.plist
    ├── com.<project>.queue.plist
    ├── com.<project>.scheduler.plist   # optional, when SCHEDULER_ENABLED=true
    └── logs/
        ├── caddy-access.log
        ├── frankenphp-stdout.log
        ├── frankenphp-stderr.log
        ├── cloudflared-stdout.log
        ├── cloudflared-stderr.log
        ├── queue-stdout.log
        ├── queue-stderr.log
        ├── scheduler-stdout.log    # when SCHEDULER_ENABLED=true
        └── scheduler-stderr.log    # when SCHEDULER_ENABLED=true
```

### Config File (`bunker.conf`)

```bash
PROJECT_NAME="my-app"
PROJECT_PATH="/Users/you/Code/my-app"
PORT=8700
DOMAIN="my-app.example.com"
TUNNEL_NAME="my-app"
TUNNEL_UUID="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
PHP_PATH="/opt/homebrew/bin/php"
FRANKENPHP_PATH="/usr/local/bin/frankenphp"
CLOUDFLARED_PATH="/opt/homebrew/bin/cloudflared"
SCHEDULER_ENABLED="false"
```

## CLI Commands

### `bunker init`

Interactive setup for the current project directory.

1. Detect project name from directory basename (kebab-case)
2. Detect PHP, frankenphp, cloudflared paths — confirm with user
3. Ask for port (suggest unused one)
4. Ask for domain
5. Ask for tunnel name (default: project name)
6. Ask if the project uses scheduled tasks (y/N) — sets `SCHEDULER_ENABLED`
7. Create or reuse cloudflared tunnel — capture UUID
8. Write `bunker.conf`
9. Generate Caddyfile from template
10. Generate 3 plist files (+ scheduler plist if enabled) from templates
11. Symlink plists into `~/Library/LaunchAgents/`
12. Remind user to:
    - Add CNAME DNS record (or offer to run `cloudflared tunnel route dns`)
    - Create `.env.production` in the project

### `bunker start [project]`

Load the LaunchAgents. If `project` is omitted and CWD is a bunkered project, use that.

### `bunker stop [project]`

Unload the LaunchAgents.

### `bunker restart [project]`

Stop + start.

### `bunker status [project]`

Show which services are running (PID, exit code) via `launchctl list`. Includes scheduler if enabled.

### `bunker run [project]`

Foreground mode — runs all services (3 or 4 depending on scheduler) via `npx concurrently` with colored output. Useful for debugging. Ctrl+C stops all.

### `bunker logs [project] [--service=server|tunnel|queue|scheduler|access] [--follow]`

Tail logs from `~/.bunker/<project>/logs/`. Defaults to all services. `--follow` for live tailing.

### `bunker list`

List all bunkered projects with their status (running/stopped), port, and domain.

### `bunker teardown [project]`

1. Stop services
2. Remove symlinks from `~/Library/LaunchAgents/`
3. Ask before removing `~/.bunker/<project>/`
4. Remind user about cloudflared tunnel deletion and DNS cleanup

### `bunker edit [project]`

Open `~/.bunker/<project>/` in the user's `$EDITOR` for manual config tweaks.

### `bunker update [project]`

Re-generate Caddyfile, plists, and cloudflared.yml from existing `bunker.conf`. Use after manually editing config via `bunker edit`. Restart services afterward to apply.

### `bunker completions <shell>`

Generate shell completions for bash, zsh, or fish. Example: `bunker completions zsh > ~/.zfunc/_bunker`

## Project Resolution

Commands that accept `[project]` resolve in this order:
1. Explicit argument: `bunker start my-app`
2. CWD detection: if `~/.bunker/<basename of CWD>/bunker.conf` exists, use that
3. Error: "Not a bunkered project. Run `bunker init` first."

## Design Principles

- **Zero project contamination** — nothing written to the project repo. All config lives in `~/.bunker/`.
- **Portable** — single compiled binary, no runtime needed.
- **Idempotent** — running `bunker init` twice updates config, doesn't duplicate.
- **Discoverable** — `bunker list` shows everything, `bunker status` shows health.
- **macOS-native** — uses LaunchAgents for service management (auto-start on login, restart on crash).

## Caddyfile Template

Hardened for production:
- Binds to localhost only (external access via tunnel)
- Security headers (nosniff, DENY frame, strict referrer, permissions policy)
- Blocks dotfiles, vendor, storage, artisan, direct PHP access, backup/swap files
- JSON access logging with rotation (10MB, keep 5)
- gzip/zstd compression
- 10MB request body limit

## Scope Boundaries

### In Scope
- Laravel projects on macOS
- FrankenPHP + Caddy as the web server
- cloudflared named tunnels for public access
- Laravel queue worker as the third service
- Laravel scheduler (`schedule:work`) as an optional fourth service
- macOS LaunchAgents for service management

### Out of Scope (for now)
- Non-Laravel projects
- Linux systemd support
- Docker-based deployments
- Multiple workers/schedulers per project
- SSL certificate management (handled by Cloudflare)
- Cloudflare Access / Zero Trust configuration (user does this in dashboard)
- `.env.production` management (user creates this themselves)

## Companion Skill

The Claude Code skill (`~/.claude/skills/bunker/SKILL.md`) becomes a thin wrapper:

```
/bunker setup    → runs `bunker init` interactively
/bunker status   → runs `bunker status`
/bunker teardown → runs `bunker teardown`
```

The skill's role is to invoke the CLI and interpret output — not to template files or manage config directly.

## Prior Art

This design was extracted from a real Laravel project's production setup, which uses:
- FrankenPHP with a dedicated port
- cloudflared named tunnel for public access
- Laravel queue worker with 3 retries, 30s timeout
- Composer scripts for lifecycle management
- macOS LaunchAgents for auto-start

Bunker generalizes this pattern for any Laravel project.
