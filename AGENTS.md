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
- `src/proxy/`: the egress proxy (`addr::is_internal` is shared with config validation).
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
