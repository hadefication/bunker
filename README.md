# Bunker

Local production deployment for web apps on macOS. Manages FrankenPHP (Caddy), cloudflared tunnels, and background workers as macOS LaunchAgents.

Currently supports Laravel. More frameworks coming.

## Prerequisites

- [FrankenPHP](https://frankenphp.dev/)
- [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) (authenticated via `cloudflared login`)
- PHP (Herd, Homebrew, or system)
- [Rust toolchain](https://rustup.rs/) (for building from source)

## Install

```bash
git clone git@github.com:hadefication/bunker.git
cd bunker
cargo install --path .
```

## Usage

```bash
# Set up a project (run from your project directory)
bunker init

# Manage services
bunker start
bunker stop
bunker restart
bunker status

# Run in foreground (for debugging)
bunker run

# View logs
bunker logs
bunker logs --service=server --follow

# List all bunkered projects
bunker list

# Remove a project's bunker config
bunker teardown

# Edit config manually
bunker edit
```

## How It Works

`bunker init` walks you through setup — detects your framework, PHP and binary paths, picks an unused port, creates a cloudflared tunnel, and routes DNS. All config is written to `~/.bunker/<project>/`, nothing touches your project repo.

Services run as macOS LaunchAgents: auto-start on login, restart on crash, managed via `launchctl`.

## License

MIT
