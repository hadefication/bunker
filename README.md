# Bunker

Host your own web apps on a Mac you own, without putting the Mac at risk. Each site runs in its own
[Lima](https://lima-vm.io/) VM that holds a copy of the code and every service it needs. The VM is
confined from the Mac's side: it can't reach the Mac's loopback, its LAN, your tailnet or the
internet, except through bunker's allowlist proxy. Sites are published through Cloudflare named
tunnels.

> **v2 is in development on the `v2` branch.** The released v0.1 (FrankenPHP as LaunchAgents, no
> isolation) is on `main` until v2 ships. The design is in
> [`docs/superpowers/specs/2026-11-16-bunker-design.md`](docs/superpowers/specs/2026-11-16-bunker-design.md).

## Commands

Every command prints readable output by default. For agents and scripts, add `--json`: output is
`{"ok":true,"data":{…}}` or `{"ok":false,"error":{"code","message","hint"}}`.

| Command | What it does |
| --- | --- |
| `bunker version`, `--version`, `-V` | Print the version, commit and build date |

Exit codes: `0` ok · `1` failure or degraded · `2` usage · `3` not found · `4` refused · `5` unconfined VM.

## Build from source

Requires Rust 1.92 or newer, on macOS.

```bash
cargo build
cargo test                    # unit and integration tests; runs anywhere
cargo test -- --ignored sys_  # tests that touch sandbox-exec, the Keychain, launchd or Lima (on a Mac)
cargo deny check              # supply-chain gate; must pass before committing a lockfile change
```

## License

MIT
