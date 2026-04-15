# Bunker

Local production deployment for web apps on macOS. Manages FrankenPHP (Caddy), cloudflared tunnels, and background workers as macOS LaunchAgents.

Currently supports Laravel. More frameworks coming.

## Prerequisites

- [FrankenPHP](https://frankenphp.dev/)
- [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) (authenticated via `cloudflared login`)
- PHP (Herd, Homebrew, or system)

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/hadefication/bunker/main/install.sh | sh
```

### Build from source

Requires the [Rust toolchain](https://rustup.rs/).

```bash
git clone https://github.com/hadefication/bunker.git
cd bunker
cargo install --path .
```

### Shell Completions

```bash
# zsh
bunker completions zsh > ~/.zfunc/_bunker

# bash
bunker completions bash > /usr/local/etc/bash_completion.d/bunker

# fish
bunker completions fish > ~/.config/fish/completions/bunker.fish
```

## Usage

```bash
# Set up a project (run from your project directory)
bunker init

# Non-interactive (for scripts and AI agents)
bunker init --yes --domain my-app.example.com
bunker init --yes --name my-app --port 8700 --domain my-app.example.com --scheduler

# Preview without creating anything
bunker init --dry-run

# Manage services
bunker start
bunker stop
bunker restart
bunker status                    # includes health check

# Run in foreground (for debugging)
bunker run

# View logs
bunker logs
bunker logs --service=server --follow

# List all bunkered projects
bunker list

# Re-generate configs after editing bunker.conf
bunker update

# Remove a project's bunker config
bunker teardown
bunker teardown --yes            # skip prompts

# Edit config manually
bunker edit
```

## How It Works

`bunker init` walks you through setup — detects your framework, PHP and binary paths, picks an unused port, creates a cloudflared tunnel, routes DNS, and generates all config. Everything is written to `~/.bunker/<project>/`, nothing touches your project repo.

Services run as macOS LaunchAgents: auto-start on login, restart on crash, managed via `launchctl`.

## License

MIT
