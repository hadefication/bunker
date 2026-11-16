# Repository Guidelines

Bunker v2 is being built on the `v2` branch from the plan in
`docs/superpowers/plans/2026-11-16-bunker-core.md`, against the design in
`docs/superpowers/specs/2026-11-16-bunker-design.md`. The spec is the authority.

## Project Structure

A Rust 2024 crate with a library (`src/lib.rs`) and a thin binary (`src/main.rs`).
- `src/cli.rs`: clap definitions. `src/commands/`: one file per command, thin.
- `src/output.rs`: human output, the `--json` envelope, exit codes. `src/error.rs`: `BunkerError`.
- `src/paths.rs`: `bunker_home()`, `ProjectDir`, and `write_private` (0600, atomic) for every file
  bunker writes. `src/config/`: `bunker.toml` types, validation, repo detection, port allocation.
- `src/net/`: format-preserving `[network]` edits and the `network.jsonl` event log.
  `src/time.rs`: RFC 3339 timestamps (no date crate).
- `src/proxy/`: the egress proxy. `addr::is_internal` (shared with config validation), `policy`
  (allow/build/deny matching), `lease` (time-boxed build and maintenance openings), `resolve`
  (`Resolver`/`Connector` traits so tests prove it connects only to checked addresses), `server`
  (hyper 1, one listener per project), `own` (the Mac's addresses via getifaddrs).
  `bunker proxy serve` is hidden; launchd runs it as `com.bunker.proxy`.
- `src/lima/`: the Lima template (containment settings fixed), the Seatbelt profiles for the
  hostagent and both SSH forwards, and the Ubuntu image bunker downloads and verifies itself
  (a sandboxed `limactl start` has no network). Images live in `~/.bunker/images/`.
- `src/host.rs`: `Host`, everything a command needs from the machine (runner, confinement probe,
  paths, limactl, poll timing). Tests build one with `host::testing::host`.
- `src/lima/vm.rs`: limactl version, instances, the vm and forward agents, readiness wait.
- `src/guest/`: `render` builds the provisioning script and the files it installs from
  `templates/` (nginx site, php-fpm pool, worker and scheduler units, nftables, logrotate);
  `provision` runs it as root in the guest inside a Build lease, the DB password on stdin.
  Repo signing keys and Composer are pinned by fingerprint and sha256 in the script.
- `src/deploy/`: `source` (the release tar: tracked files minus `.env*`, `--dirty` for the working
  tree), `steps` (the pipeline as data: name, guest user, script, lease), `lock` (flock on
  `deploy.lock`), `run` (`deploy` and `rollback`). Root steps never chmod or chown inside a
  release; deploy owns it and runs third-party composer and npm scripts.
  `src/lima/confine.rs`: `ConfinementProbe`; `SystemProbe` reads `ha.pid` and asks
  `sandbox_check` whether that process is sandboxed.
- `src/secrets/`: `SecretStore` (`KeychainStore`, `MemoryStore` for tests), `generate_missing`
  (APP_KEY, DB_PASSWORD, Passport keys via openssl on stdin), `render_env` (quoting for phpdotenv),
  `meta::record` (names and kinds in bunker.toml). `BUNKER_KEYCHAIN` points tests at a throwaway
  keychain file; never the login keychain.
- `src/launchd.rs`: labels, plist rendering, install/remove/kickstart/status via `launchctl` in
  `gui/<uid>`. `BUNKER_LAUNCH_AGENTS_DIR` overrides `~/Library/LaunchAgents`.
- `src/run.rs`: every external program goes through the `Runner` trait. Tests use
  `RecordingRunner` to check exact invocations and `assert_no_secret_in_argv`.
- `tests/`: integration tests (`assert_cmd`).

## Build, Test, and Development Commands

- `cargo build`, `cargo test`, `cargo fmt`, `cargo clippy --all-targets -- -D warnings`.
- `cargo test -- --ignored sys_<name>` runs a test that touches the real system (sandbox-exec,
  Keychain, launchd, Lima). Those are `#[ignore]` and named `sys_*`.
- `cargo deny check` after any `Cargo.lock` change.

## Rules

- Nothing is ever written into a project's repo. Config lives in `~/.bunker/<project>/bunker.toml`.
- No secret value in argv, stdout, `--json`, logs or test output. Values reach the guest on stdin.
- No `.env` reads. Never add `Co-Authored-By` or any agent attribution to commits.
- Every command has human output and `--json`; errors carry a stable `code` and an exit code.
