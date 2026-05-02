# Repository Guidelines

## Project Structure & Module Organization

Bunker is a Rust 2024 CLI application. The binary entry point is `src/main.rs`, with CLI argument definitions in `src/cli.rs`. Command implementations live in `src/commands/` (`init`, `status`, `logs`, `teardown`, etc.). Shared configuration parsing is in `src/config.rs`, generated service/config templates are in `src/templates.rs`, and framework-specific behavior is isolated under `src/framework/` with Laravel support in `src/framework/laravel.rs`.

There is no checked-in test directory yet; add tests near the code they exercise using Rust unit tests, or add integration tests under `tests/` when validating full CLI behavior.

## Build, Test, and Development Commands

- `cargo build` builds the debug binary.
- `cargo build --release` builds an optimized release binary.
- `cargo run -- <command>` runs the CLI during development, for example `cargo run -- status`.
- `cargo run -- help` prints the current command surface.
- `cargo test` runs all tests.
- `cargo fmt` formats Rust code.
- `cargo clippy --all-targets --all-features` checks for common Rust issues.
- `cargo install --path .` installs the local binary to `~/.cargo/bin/bunker`.

## Coding Style & Naming Conventions

Use standard Rust formatting via `cargo fmt`; keep imports and modules organized by `rustfmt`. Prefer clear `snake_case` for functions, variables, modules, and files. Use `PascalCase` for types and enum variants. Keep command files focused on one CLI command or lifecycle area.

Avoid shell interpolation for subprocesses. Use `Command::new()` with direct argv, validate user-provided paths and names before use, and preserve Bunker’s rule that target project repositories are not modified.

## Testing Guidelines

Use Rust’s built-in test framework. Name tests after the behavior being verified, such as `rejects_invalid_project_name` or `renders_scheduler_plist_when_enabled`. For command logic, prefer tests around parsing, validation, generated config, and framework detection. Run `cargo test`, `cargo fmt`, and `cargo clippy --all-targets --all-features` before opening a PR.

## Commit & Pull Request Guidelines

Recent history uses short imperative commit subjects, for example `Fix formatting (cargo fmt)` and `Add info command, fix table formatting, harden security`. Keep subjects concise and behavior-focused. Do not add `Co-Authored-By` trailers.

Pull requests should include a clear summary, test results, and any macOS, Cloudflare, FrankenPHP, or Laravel assumptions. Include screenshots or terminal output when changing tables, prompts, logs, or user-facing CLI text.

## Security & Configuration Tips

Never read or edit `.env` files unless explicitly asked. Bunker stores generated project config in `~/.bunker/<project>/`; do not add generated Caddyfiles, plists, tunnel config, or logs to this repository.
