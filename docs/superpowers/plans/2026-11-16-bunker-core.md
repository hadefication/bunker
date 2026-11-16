# Bunker core (plan 2 of the series) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build bunker v2's core on the `v2` branch and put Life OS live on it: config, the network
allowlist, the host proxy, confined Lima VMs, guest provisioning, in-app secrets, deploy and
rollback, the Cloudflare tunnel, and status, logs, artisan and shell.

**Architecture:** One Rust binary. Pure modules (config, policy, renderers, deploy steps, scrubber)
carry the logic and the unit tests. Everything that touches the system goes through a `Runner` trait,
so tests record the exact `limactl`, `launchctl`, `cloudflared` and `git` invocations and can prove
that no secret value ever reaches a command line. launchd supervises every long-running piece: the
proxy, each VM's sandboxed hostagent (`limactl start --foreground`), both SSH forwards and
cloudflared. bunker itself runs no daemon except `bunker proxy serve`, which launchd starts.

**Tech Stack:** Rust 2024 (rustc 1.92 on the mini), clap 4, serde, toml, toml_edit, tokio, hyper 1
plus hyper-util, tar, sha2, security-framework (Keychain), rpassword, zeroize, libc, time,
assert_cmd and tempfile (dev only). On the mini: Lima 2.3.0, `/usr/bin/sandbox-exec`, cloudflared
2026.9.3, `/usr/bin/openssl`, `/usr/bin/curl`. In the guest: Ubuntu 24.04 arm64, nginx, php-fpm
from the ondrej PPA, MySQL 8.4 from repo.mysql.com, Node 22 from NodeSource, Composer.

**Spec:** `docs/superpowers/specs/2026-11-16-bunker-design.md`. The containment results section
(and its "Findings plan 2 must carry") is binding here.

**Out of scope here, on purpose:** backups (Life OS goes live with none until plan 3, so plan 3
comes next), the nightly maintenance window (plan 2 disables the guest's own apt timers, so guest
security updates wait for plan 3), remote deploy, and `env set --bind`.

**The series:** plan 1 (the containment test) is done. This is plan 2. Later plans:
3. backups, restore and destinations, plus the nightly maintenance window
4. remote deploy over tailcat
5. outbound credential placeholders (`env set --bind`)
6. v2 publishing (release, `self_update`, the `v1` branch, `main` fast-forward)

## Rules for the driving agent

- **Branch and commits.** All work is on `v2`. Each task ends with one commit (lean prose message,
  no type prefix, no trailers of any kind), and only if the maintainer authorized per-task commits
  at the execution handoff. Without that, stop after each task's tests pass and ask. Never push.
- **No interactive commands.** `limactl` always gets `--tty=false` except in `bunker shell` and
  interactive `bunker artisan`. Waiting is a bounded retry loop around a command with its own
  timeout, never a bare `sleep`.
- **System-touching tests are ignored by default.** A test that needs `sandbox-exec`, the Keychain,
  `launchctl`, `limactl` or the network is `#[ignore]` and named `sys_*`. `cargo test` runs the
  rest anywhere, CI included. Each task lists the `sys_*` tests it adds, and the task isn't done
  until they pass on the mini with `cargo test -- --ignored sys_<name>`.
- **Human-gated steps** are marked **[HUMAN]**. The agent prepares them, stops and asks. They are:
  macOS Keychain approval prompts, code signing with the Developer ID identity, overwriting the
  live `life-os.bangkila.com` DNS record, anything that needs `sudo` on the Mac, rebooting the Mac,
  and checking the site in a browser behind Cloudflare Access.
- **Hard stops,** where the agent ends the run and reports:
  - a confined VM reaches anything except its proxy (any acceptance probe in Task 16 reads
    `reachable` where `blocked` is expected)
  - any step would need reading `.env`, `~/.secrets` or `~/Backups/auth`, or printing a secret
  - Lima older than 2.3.0
- **Secrets in this work.** Values used in tests are made-up fixtures. No real secret is ever typed
  by the agent: the maintainer enters real values with `bunker env set` on a TTY.
- **Leave the mini clean.** VMs, LaunchAgents and tunnels created by tests are removed by the task
  that made them, or by `tests/acceptance/run.sh cleanup`.

## Global Constraints

- Package `bunker`, version `2.0.0-alpha.0` (plan 6 sets `2.0.0`), edition 2024,
  `rust-version = "1.92"`, macOS only, `license = "MIT"`.
- `bunker --version`, `bunker -V` and `bunker version` all print
  `bunker 2.0.0-alpha.0 (<short sha>[-dirty], <YYYY-MM-DD>)` to stdout and exit 0. A `build.rs`
  injects the sha and the build date; there's no version literal anywhere else in the source.
- `deny.toml` and `.github/workflows/supply-chain.yml` are copied from `~/AI/yard/` (the
  `~/AI/skillset` copies the spec names no longer exist), and `cargo deny check` passes before the
  first commit and after every lockfile change. New crates are vetted per the Rust dependency rule.
- Every command has human output by default and `--json`. JSON is an envelope:
  `{"ok":true,"data":{…}}` or `{"ok":false,"error":{"code":"<snake_case>","message":"…","hint":"…"|null}}`.
  Human output ends errors with the hint and names `--json` for scripts.
- Exit codes: `0` ok · `1` failure or degraded · `2` usage (clap) · `3` not found (project, VM,
  release) · `4` refused (dirty tree, advisories, policy, lock held, not yet supported, needs a TTY)
  · `5` unconfined VM.
- Paths: `~/.bunker` unless `BUNKER_HOME` is set (tests always set it). LaunchAgents go to
  `~/Library/LaunchAgents` unless `BUNKER_LAUNCH_AGENTS_DIR` is set. Lima uses its default
  `LIMA_HOME` (`~/.lima`); instances are named `bunker-<project>`.
- launchd labels: `com.bunker.proxy`, and `com.bunker.<project>.vm`, `.fwd-proxy`, `.fwd-web`,
  `.tunnel`.
- Ports, allocated by `init` and stored in `[ports]`: SSH from 60100–60199, proxy from 18800–18899,
  web from 18900–18999. Guest side: the proxy is reached at `127.0.0.1:3128`, nginx listens on
  `127.0.0.1:80`.
- Guest layout and users: `/srv/app/{releases,shared,current}` per the spec; php-fpm and workers
  run as `app` (no sudo); releases are owned by `deploy`; `shared/.env` is `root:app 0640`.
- Keychain: service `bunker.<project>`, account = the env key. Names and kinds of secrets live in
  `bunker.toml` `[secrets]`; plain settings live in `[env]`. Values never live in `bunker.toml`.
- **No secret value in argv, stdout, `--json`, bunker's logs or test output.** Values travel to the
  guest on stdin only. Tests enforce it with `RecordingRunner::assert_no_secret_in_argv`.
- Files bunker writes under `~/.bunker` are mode 0600 (directories 0700), written atomically
  (temp file in the same directory, then rename).
- Commit messages: lean prose, lowercase subject, no bullets, no trailers.

## Review Focus

1. **A name resolving to both public and internal addresses** (DNS rebinding, or a CDN plus a stale
   private record). The proxy must refuse the request and connect only to addresses it checked.
   Test: Task 5 `mixed_public_and_internal_resolution_is_refused`.
2. **A deploy that dies halfway** (composer fails, Ctrl-C, SSH drops). The build allowlist must
   close, `current` must stay on the old release, and the lock must free. Test: Task 12
   `build_lease_closed_after_failed_composer`; leases also expire on their own (Task 4
   `expired_lease_is_inactive`).
3. **`bunker.toml` hand-edited into an invalid state while the proxy runs.** That project's guest
   loses all egress and a `policy_error` is logged; other projects keep working. Test: Task 5
   `broken_config_fails_closed_and_logs_policy_error`.
4. **Secret values with quotes, `$`, `\`, `#` or newlines** (PEM keys) in `shared/.env`. They must
   round-trip exactly through Laravel's dotenv. Tests: Task 10 `render_env_quotes_special_characters`
   and `render_env_keeps_multiline_pem`, plus the guest round-trip in Task 16.
5. **A VM started outside bunker** (a manual `limactl start`). Status must flag it with exit 5,
   and deploy must refuse to run against it. Tests: Task 8 `sys_confinement_detects_unsandboxed_pid`,
   Task 12 `deploy_refuses_when_unconfined`, Task 16 phase `unwrapped`.

## File structure

```
build.rs                      sha + date for --version
deny.toml                     from ~/AI/yard
.github/workflows/ci.yml      fmt, clippy -D warnings, test --locked; on main and v2
.github/workflows/supply-chain.yml   from ~/AI/yard
src/main.rs                   entry, exit-code mapping
src/cli.rs                    clap definitions
src/output.rs                 Format, Report, envelope, ExitCode
src/error.rs                  BunkerError, Result
src/run.rs                    Cmd, Output, Runner, SystemRunner, RecordingRunner
src/paths.rs                  bunker_home, ProjectDir
src/config/{mod,validate,detect,ports,constraint}.rs
src/net/{edit,log}.rs         allowlist edits, network.jsonl events
src/proxy/{addr,policy,lease,resolve,server}.rs
src/launchd.rs
src/lima/{template,sandbox,image,vm,confine}.rs
src/secrets/{store,keychain,dotenv,generate}.rs
src/guest/{provision,render}.rs
src/deploy/{source,steps,run,lock}.rs
src/tunnel/{cloudflared,config}.rs
src/logs/{scrub,tail}.rs
src/commands/{init,net,proxy,up,lifecycle,destroy,env,deploy,rollback,status,logs,artisan,shell,version}.rs
templates/{provision.sh,nginx-site.conf,php-fpm-pool.conf,worker.service,scheduler.service,scheduler.timer,nftables.conf,logrotate-app}
tests/*.rs                    integration tests (assert_cmd)
tests/acceptance/             run.sh, env.sh, probe.sh, probe2.sh, compare.sh, expected/*.json
```

Old v0.1 helpers worth copying are reachable with `git show 5be4977:<path>`:
`src/config.rs` (`validate_project_name`, `validate_tunnel_name`, `validate_domain`,
`validate_path`, `write_restricted`, `port_available`) and `src/templates.rs` (XML escaping, the
header list).

---

### Task 1: v2 scaffold, output contract, runner

**Files:**
- Delete: `src/**` (v0.1). Keep `install.sh` and `.github/workflows/release.yml` untouched for plan 6.
- Create: `Cargo.toml` (rewritten), `build.rs`, `deny.toml`, `.github/workflows/supply-chain.yml`,
  `src/{main,cli,output,error,run}.rs`, `src/commands/version.rs`
- Modify: `.github/workflows/ci.yml` (add `v2` to branches, `cargo test --locked`, clippy
  `-- -D warnings`), `CLAUDE.md`, `AGENTS.md`, `README.md` (v2 skeleton: what bunker is now, a
  command table filled in task by task, a "v2 is in development on this branch" line)
- Test: `tests/cli_version.rs`, unit tests in `src/output.rs` and `src/run.rs`

**Interfaces:**
- Produces:
  - `enum ExitCode { Ok = 0, Failure = 1, Usage = 2, NotFound = 3, Refused = 4, Unconfined = 5 }`
  - `struct BunkerError { code: &'static str, message: String, hint: Option<String>, exit: ExitCode }`
    with `BunkerError::new(code, exit, message)` and `.hint(impl Into<String>)`;
    `type Result<T> = std::result::Result<T, BunkerError>`
  - `enum Format { Human, Json }`; `trait Report { fn human(&self, w: &mut dyn Write) -> io::Result<()>; fn json(&self) -> serde_json::Value; }`;
    `fn emit(format: Format, r: &dyn Report)`; `fn emit_error(format: Format, e: &BunkerError)`
  - `struct Cmd { program: String, args: Vec<String>, stdin: Option<Vec<u8>>, env: Vec<(String, String)>, interactive: bool }`
    with a builder (`Cmd::new(p).args([...]).stdin(bytes)`); `struct Output { status: i32, stdout: Vec<u8>, stderr: Vec<u8> }`
  - `trait Runner: Send + Sync { fn run(&self, cmd: &Cmd) -> Result<Output>; }`, `SystemRunner`
    (interactive cmds inherit the TTY), and `RecordingRunner` (queued responses matched by program
    plus an args prefix; records every `Cmd`; `fn calls(&self) -> Vec<Cmd>`;
    `fn assert_no_secret_in_argv(&self, secrets: &[&str])` panics if any program or arg contains
    one)

- [ ] **Step 1: Write the failing tests**

```rust
// tests/cli_version.rs
#[test] fn version_flag_prints_version_commit_and_date()      // --version, -V and `version`:
// stdout matches ^bunker 2\.0\.0-alpha\.0 \([0-9a-f]{7,}(-dirty)?, \d{4}-\d{2}-\d{2}\)\n$, exit 0
// src/output.rs
#[test] fn error_json_has_code_message_hint()   // {"ok":false,"error":{"code":"x","message":"m","hint":"h"}}
#[test] fn ok_json_wraps_data()                 // {"ok":true,"data":{...}}
// src/run.rs
#[test] fn recording_runner_flags_secret_in_argv()   // #[should_panic] when an arg contains the secret
#[test] fn recording_runner_allows_secret_on_stdin() // no panic when only stdin carries it
```

- [ ] **Step 2: Run them and watch them fail**
Run: `cargo test` — Expected: compile errors or FAIL (nothing exists yet).

- [ ] **Step 3: Implement the scaffold.** Copy `deny.toml` and `supply-chain.yml` from `~/AI/yard`
  (keep the `[bans] deny` block verbatim; trim the license allowlist to what `cargo deny list`
  resolves). `build.rs` runs `git rev-parse --short HEAD`, `git status --porcelain` and the date,
  and sets `BUNKER_COMMIT` and `BUNKER_BUILD_DATE`, with `unknown` when git is missing.
  `main.rs` maps `BunkerError.exit` to the process exit code; clap errors exit 2. Only the
  `version` subcommand and the global `--json` flag exist after this task.

- [ ] **Step 4: Verify**
Run: `cargo test && cargo clippy -- -D warnings && cargo deny check`
Expected: all pass; `cargo deny check` prints `advisories ok, bans ok, licenses ok, sources ok`.

- [ ] **Step 5: Commit** (if authorized): `start bunker v2 on the v2 branch` with a body explaining
  that the v0.1 code is removed and why the reference files came from Yard.

---

### Task 2: Paths, config, validation, detection, init

**Files:**
- Create: `src/paths.rs`, `src/config/{mod,validate,detect,ports,constraint}.rs`, `src/commands/init.rs`
- Test: unit tests in each module, `tests/cli_init.rs`

**Interfaces:**
- Consumes: Task 1's `BunkerError`, `Report`, `Runner`
- Produces:
  - `fn bunker_home() -> PathBuf`; `struct ProjectDir { pub name: String, pub root: PathBuf }`
    with path methods `config()`, `cloudflared_yml()`, `tunnel_credentials()`, `sandbox_sb()`,
    `fwd_proxy_sb()`, `fwd_web_sb()`, `lima_yaml()`, `network_log()`, `leases_dir()`,
    `logs_dir()`, `deploy_lock()`
  - `struct Config { project: PathBuf, stack: Stack, php: String, extensions: Vec<String>, node: Option<String>, domain: Option<String>, tunnel: Option<String>, services: Services, workers: BTreeMap<String, String>, schedule: Schedule, hooks: Hooks, vm: Vm, ports: Ports, network: Network, env: BTreeMap<String, String>, secrets: BTreeMap<String, SecretMeta> }`
    where `Ports { ssh: u16, proxy: u16, web: u16 }`, `Network { build: Vec<String>, allow: Vec<String>, deny: Vec<String> }`,
    `SecretMeta { kind: SecretKind /* InApp | Bound */, host: Option<String> }`. All structs use
    `#[serde(deny_unknown_fields)]`. `domain` and `tunnel` are optional: with neither, `up` skips
    the tunnel (the acceptance fixture relies on this).
  - `Config::load(&ProjectDir) -> Result<Config>`, `Config::parse(&str) -> Result<Config>` (parse then validate)
  - `validate_project_name`, `validate_tunnel_name`, `validate_domain`, `validate_path` (copied
    from v0.1), plus `validate_host_pattern(&str)`, `validate_php(&str)` against
    `SUPPORTED_PHP = ["7.0","7.1","7.2","7.3","7.4","8.0","8.1","8.2","8.3","8.4","8.5"]`,
    `validate_extension(&str)` (`^[a-z0-9_]+$`), `validate_size(&str)` (`^\d+(MiB|GiB)$`),
    `validate_env_key(&str)` (`^[A-Z][A-Z0-9_]*$`)
  - Host pattern syntax: `host`, `*.domain` (subdomains only, at least two labels after the `*.`),
    optional `:port`, or a public IP literal. Internal IP literals are refused at validation.
  - `fn resolve_project(arg: Option<&str>, cwd: &Path) -> Result<ProjectDir>`: the argument names
    a directory under `bunker_home()` that has `bunker.toml`; with no argument, the project whose
    `project` path equals the cwd or one of its ancestors. A directory with a `Caddyfile` and no
    `bunker.toml` is an old v0.1 project: skip it and print once, to stderr,
    `~/.bunker/<name> is a bunker v0.1 project; v2 ignores it`.
  - `fn php_from_constraint(c: &str) -> Result<String>`: the highest `SUPPORTED_PHP` version
    satisfying a Composer constraint. It handles `^`, `~`, `>=`, `>`, `<=`, `<`, exact, `||`, and
    AND by space or comma. Anything else is an error with the hint `pass --php <version>`.
  - `fn detect(repo: &Path) -> Result<Detected { stack: Stack, php: String, extensions: Vec<String>, node: Option<String>, passport: bool }>`:
    Laravel means `artisan` exists and `composer.json` requires `laravel/framework`. `passport` means
    it requires `laravel/passport`. `node` is `Some("22")` when `package.json` has a `build` script.
  - `fn allocate_ports(taken: &[Ports], is_free: &dyn Fn(u16) -> bool) -> Result<Ports>`
  - `bunker init <project> --path <repo> [--domain D] [--tunnel T] [--php V] [--force]`: writes a
    commented `bunker.toml` (0600) and creates `~/.bunker/<project>/` (0700). It refuses an
    existing config without `--force` (exit 4).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn unknown_key_is_an_error()   // "[vm]\nmemry = \"2GiB\"" -> code "config_invalid", message contains "memry"
#[test] fn spec_example_config_parses()   // the spec's bunker.toml plus [ports] -> php "8.4", workers["queue"], network.allow empty
#[test] fn host_pattern_validation()   // ok: "example.com", "*.github.com", "api.example.com:8443", "93.184.215.14"
                                       // err: "*", "*.com", "exa mple.com", "http://x.com", "10.0.0.1", "example.com:0"
#[test] fn php_from_constraint_table()   // "^8.3"->"8.5", "~7.4"->"7.4", ">=8.1 <8.3"->"8.2", "^7.4 || ^8.0"->"8.5", "8.2.*"->"8.2", "dev-main"->err
#[test] fn detects_laravel_passport_and_node()   // fixture repo: artisan, composer.json (laravel/framework, laravel/passport, php ^8.3), package.json with build
#[test] fn resolve_project_from_nested_cwd()     // cwd = <repo>/app/Http -> that project
#[test] fn old_layout_is_skipped_with_notice()   // dir with Caddyfile only -> not resolved; stderr has the notice once
#[test] fn port_allocation_skips_taken_and_busy()   // taken ssh 60100, is_free(60101)=false -> ssh 60102
// tests/cli_init.rs (BUNKER_HOME = tempdir)
#[test] fn init_writes_0600_config_and_refuses_overwrite()   // second init exits 4; --force succeeds
#[test] fn init_json_reports_detected_values()               // data.php, data.stack, data.ports
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test config:: init` — Expected: FAIL.
- [ ] **Step 3: Implement** the signatures above. `init` builds the file with `toml_edit` so it can carry comments like the spec's example.
- [ ] **Step 4: Verify.** Run: `cargo test` — Expected: PASS. Update README's command table (`init`).
- [ ] **Step 5: Commit** (if authorized).

---

### Task 3: Allowlist editing and the network log

**Files:**
- Create: `src/net/{edit,log}.rs`, `src/commands/net.rs`
- Test: unit tests, `tests/cli_net.rs`

**Interfaces:**
- Consumes: `Config::parse`, `validate_host_pattern`, `ProjectDir`
- Produces:
  - `enum ListName { Allow, Build, Deny }`; `enum Op { Add, Remove }`;
    `struct Change { list: ListName, before: Vec<String>, after: Vec<String> }`
  - `fn edit_lists(doc: &str, edits: &[(ListName, Op, &str)]) -> Result<(String, Vec<Change>)>`,
    format-preserving (`toml_edit`). It refuses if the edited document fails `Config::parse`.
  - `fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<()>`
  - `enum NetEvent` serialized as one JSON object per line, all with `ts` (RFC 3339 UTC) and `project`:
    `decision { host, port, phase, verdict: "allow"|"block", reason }`,
    `policy { source: "edit"|"reload"|"proxy_load", allow, build, deny }`,
    `policy_error { message }`, `lease { kind, action: "open"|"close"|"expired", ttl_secs }`
  - `fn append_event(path: &Path, e: &NetEvent) -> Result<()>` (`O_APPEND`, created 0600)
  - `fn last_policy(path: &Path) -> Result<Option<NetEvent>>`
  - Commands:
    - `bunker net allow <host> [project] [--build]` adds to `allow` (or to `build`) and removes the
      host from `deny`.
    - `bunker net deny <host> [project]` adds to `deny` and removes it from `allow` and `build`.
    - `bunker net rm <host> [project]` removes it from all three lists.
    - `bunker net reload [project]` validates the file, prints the diff against `last_policy`, and
      appends `policy { source: "reload" }`.
    - `bunker net log [project] [--blocked] [-n N]` (default 50).

    Every edit prints `+ host` and `- host` lines, appends `policy { source: "edit" }`, and in JSON
    returns `{project, changes: [{list, before, after}]}`. An edit that changes nothing says
    `already allowed` (or `not listed`) and exits 0.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn allow_preserves_comments_and_order()   // doc with "# keep me" comments; after allow, every original line is still present in order and the host is appended to allow
#[test] fn allow_is_idempotent()                  // second allow: no change, Change.before == after
#[test] fn deny_removes_from_allow_and_build()
#[test] fn edit_refuses_invalid_document_and_leaves_file()   // file has an unknown key -> exit 1 code config_invalid, bytes unchanged
#[test] fn allow_rejects_internal_ip()            // "10.0.0.5" -> exit 4 code host_internal
#[test] fn edit_appends_policy_event()            // last line of network.jsonl: kind policy, source edit, allow contains host
#[test] fn write_atomic_keeps_mode_0600()
#[test] fn net_log_blocked_filters_decisions()    // seeded log: only verdict block lines printed
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test net` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test` — Expected: PASS. README: `net` rows.
- [ ] **Step 5: Commit** (if authorized).

---

### Task 4: Address classification, policy and leases

**Files:**
- Create: `src/proxy/{addr,policy,lease}.rs`
- Test: unit tests

**Interfaces:**
- Consumes: `Network`, `ProjectDir::leases_dir`, `append_event`
- Produces:
  - `fn is_internal(ip: IpAddr, own: &[IpAddr]) -> bool`: true for loopback, RFC 1918, link-local,
    CGNAT `100.64.0.0/10`, multicast, broadcast, unspecified, `0.0.0.0/8`, `192.0.0.0/24`,
    IPv6 ULA `fc00::/7`, link-local `fe80::/10`, IPv4-mapped and IPv4-compatible forms of any of
    those, NAT64 `64:ff9b::/96` embedding an internal IPv4, and any address in `own`.
  - `const BUILTIN_BUILD: &[&str]`: `ports.ubuntu.com`, `ppa.launchpadcontent.net`,
    `keyserver.ubuntu.com`, `repo.mysql.com`, `deb.nodesource.com`, `getcomposer.org`,
    `repo.packagist.org`, `packagist.org`, `github.com`, `api.github.com`, `codeload.github.com`,
    `objects.githubusercontent.com`, `registry.npmjs.org`.
    `const MAINTENANCE: &[&str]`: `ports.ubuntu.com`, `ppa.launchpadcontent.net`,
    `repo.mysql.com`, `deb.nodesource.com`. Task 11's first confined provision may add to these;
    each addition is a ledgered ruling with the blocked host shown in `network.jsonl`.
  - `struct Phases { build: bool, maintenance: bool }`;
    `struct Policy { allow, build, deny: Vec<Pattern> }` built from `Network`;
    `fn decide(&self, host: &str, port: u16, phases: Phases) -> Decision { verdict: Verdict, reason: Reason }`
    with `Reason::{Listed(ListName | Builtin | Maintenance), Denied, NotListed, Port}`.
    Matching: `deny` wins; exact host or `*.domain` (subdomains only); a pattern without a port
    allows 80 and 443 only; `host:port` allows only that port; hosts compare case-insensitively
    with any trailing dot removed.
  - `enum LeaseKind { Build, Maintenance }`;
    `fn open_lease(dir: &ProjectDir, kind: LeaseKind, ttl: Duration) -> Result<LeaseGuard>` writes
    the expiry (unix seconds) to `leases/<kind>` and logs `lease open`. Dropping the guard removes
    the file and logs `lease close`. `fn lease_active(dir, kind, now: SystemTime) -> bool` is false
    for a missing, expired or unreadable file.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn internal_ranges_table()
// true: 127.0.0.1, 10.1.2.3, 172.16.0.1, 172.31.255.255, 192.168.254.114, 169.254.1.1, 100.64.0.1,
//       100.127.255.255, 0.0.0.0, 0.1.2.3, 224.0.0.1, 255.255.255.255, ::1, ::, fc00::1, fd12::1,
//       fe80::1, ff02::1, ::ffff:10.0.0.1, 64:ff9b::a00:1, and 203.0.113.7 when own = [203.0.113.7]
// false: 93.184.215.14, 1.1.1.1, 100.128.0.1, 172.32.0.1, 2606:4700:4700::1111, 64:ff9b::101:101
#[test] fn deny_wins_over_allow()                   // allow *.github.com, deny gist.github.com
#[test] fn wildcard_matches_subdomains_not_apex()   // *.example.com: a.example.com yes, example.com no
#[test] fn build_list_needs_build_phase()           // [network].build and BUILTIN_BUILD only when phases.build
#[test] fn maintenance_list_needs_maintenance_phase()
#[test] fn ports_other_than_80_443_need_explicit_entry()   // example.com:8443 blocked unless "example.com:8443" listed
#[test] fn host_matching_ignores_case_and_trailing_dot()
#[test] fn expired_lease_is_inactive()
#[test] fn lease_guard_removes_file_on_drop()
#[test] fn garbage_lease_file_is_inactive()
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test proxy::` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test` — Expected: PASS.
- [ ] **Step 5: Commit** (if authorized).

---

### Task 5: The proxy server

**Files:**
- Create: `src/proxy/{resolve,server,mod}.rs`, `src/commands/proxy.rs` (hidden `bunker proxy serve`)
- Test: `tests/proxy_server.rs`

**Interfaces:**
- Consumes: `Policy`, `is_internal`, `lease_active`, `append_event`, `Config::load`, `ProjectDir`
- Produces:
  - `trait Resolver: Send + Sync` with an object-safe async `resolve(host, port) -> io::Result<Vec<SocketAddr>>`
    (boxed future); `SystemResolver` uses `tokio::net::lookup_host`.
  - `trait Connector: Send + Sync` with an object-safe async `connect(addr: SocketAddr) -> io::Result<TcpStream>`;
    `SystemConnector`.
  - `struct ProjectListener { project: String, addr: SocketAddr, dir: ProjectDir }`
  - `async fn serve(listeners: Vec<ProjectListener>, resolver: Arc<dyn Resolver>, connector: Arc<dyn Connector>, own: Arc<dyn Fn() -> Vec<IpAddr> + Send + Sync>, shutdown: impl Future) -> Result<()>`
  - `bunker proxy serve` binds `127.0.0.1:<ports.proxy>` for every project found under
    `bunker_home()`, uses `getifaddrs` (refreshed every 30 s) for `own`, and logs to
    `~/.bunker/logs/proxy.log`.
- Behaviour, per request:
  1. Reload the project's policy if `bunker.toml`'s mtime changed. If parsing fails, block
     everything for that project and log `policy_error` once per change. A successful load logs
     `policy { source: "proxy_load" }`.
  2. Accept `CONNECT host:port` and absolute-URI HTTP. An origin-form request gets 400.
  3. `decide(host, port, phases)`, with phases from the leases.
  4. Resolve, or use the IP literal. If **any** resolved address is internal, block with reason
     `internal_address`. Connect only to an address from that checked set.
  5. CONNECT: reply 200 and copy bytes both ways. HTTP: forward over the checked connection in
     origin-form, dropping `Proxy-*` headers.
  6. Blocked: 403, `text/plain`, body
     `bunker: blocked <host>:<port> (<reason>). To allow it: bunker net allow <host> <project>`,
     or, for an internal address, `bunker: blocked <host>:<port>: internal addresses are never allowed`.
  7. Log one `decision` event per request.

- [ ] **Step 1: Write the failing tests.** The fake resolver maps names to chosen IPs. The fake
  connector maps the public address `93.184.215.14:443` or `:80` to a local test server, and
  refuses everything else, so any connect to an unchecked address fails the test.

```rust
#[tokio::test] async fn connect_to_allowed_host_tunnels_bytes()
#[tokio::test] async fn connect_to_unlisted_host_gets_403_naming_allow_command()   // body contains "bunker net allow api.stripe.com fixture"
#[tokio::test] async fn absolute_uri_http_is_forwarded_without_proxy_headers()      // upstream sees "GET /p HTTP/1.1", no Proxy-Authorization
#[tokio::test] async fn internal_ip_literal_is_refused()                              // CONNECT 10.0.0.1:443 -> 403 internal
#[tokio::test] async fn name_resolving_to_internal_is_refused()                       // allowed "evil.test" -> 127.0.0.1
#[tokio::test] async fn mixed_public_and_internal_resolution_is_refused()             // [93.184.215.14, 192.168.1.1] -> 403, connector never called
#[tokio::test] async fn mac_own_address_is_refused()                                  // own = [203.0.113.7], name -> 203.0.113.7
#[tokio::test] async fn nat64_embedded_internal_is_refused()                          // -> 64:ff9b::a00:1
#[tokio::test] async fn policy_reloads_on_file_change_without_restart()               // blocked, then net-allow edit, then allowed
#[tokio::test] async fn broken_config_fails_closed_and_logs_policy_error()            // project A broken -> 403; project B on its own listener still 200
#[tokio::test] async fn build_host_allowed_only_with_lease()                          // packagist.org blocked, open_lease(Build) -> allowed, drop -> blocked
#[tokio::test] async fn every_decision_is_logged()
#[tokio::test] async fn origin_form_request_is_rejected_400()
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test --test proxy_server` — Expected: FAIL.
- [ ] **Step 3: Implement** with hyper 1 (server `http1`, `with_upgrades` for CONNECT) and hyper-util.
- [ ] **Step 4: Verify.** Run: `cargo test` — Expected: PASS.
- [ ] **Step 5: Commit** (if authorized).

---

### Task 6: launchd agents

**Files:**
- Create: `src/launchd.rs`
- Test: unit tests, `sys_plist_lints`

**Interfaces:**
- Consumes: `Runner`
- Produces:
  - `enum Role { Proxy, Vm, FwdProxy, FwdWeb, Tunnel }`;
    `fn label(project: Option<&str>, role: Role) -> String` (see Global Constraints)
  - `struct Agent { label: String, program_args: Vec<String>, env: BTreeMap<String, String>, keep_alive: bool, run_at_load: bool, throttle_secs: Option<u32>, stdout: PathBuf, stderr: PathBuf }`
  - `fn render_plist(a: &Agent) -> String` (XML-escaped as in v0.1)
  - `fn install(r: &dyn Runner, a: &Agent) -> Result<()>`: writes `<agents dir>/<label>.plist`
    (0644), runs `launchctl bootout gui/<uid>/<label>` (tolerating "not loaded"), then
    `launchctl bootstrap gui/<uid> <plist>`
  - `fn remove(r, label) -> Result<()>` (bootout, then delete the plist; missing is fine);
    `fn kickstart(r, label) -> Result<()>` (`kickstart -k`)
  - `fn status(r, label) -> Result<AgentState { loaded: bool, pid: Option<i32>, last_exit: Option<i32> }>`,
    parsed from `launchctl print gui/<uid>/<label>`

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn plist_escapes_xml_and_lists_args()       // arg "a&b<c>" rendered escaped; KeepAlive and ThrottleInterval present when set
#[test] fn install_runs_bootout_then_bootstrap()    // RecordingRunner order
#[test] fn remove_tolerates_not_loaded()            // bootout exit 113 (or 3) "Could not find service" -> Ok
#[test] fn status_parses_pid_and_last_exit()        // fixture text of `launchctl print` with "pid = 4242" and "last exit code = 0"
#[test] #[ignore] fn sys_plist_lints()              // plutil -lint on a rendered plist
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test launchd` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test && cargo test -- --ignored sys_plist_lints` — Expected: PASS.
- [ ] **Step 5: Commit** (if authorized).

---

### Task 7: Lima template, Seatbelt profiles, Ubuntu image

**Files:**
- Create: `src/lima/{template,sandbox,image}.rs`
- Test: unit tests, `sys_fwd_profile_confines_nc`

**Interfaces:**
- Consumes: `Config`, `Runner`, `paths`
- Produces:
  - `struct ImageRef { path: PathBuf, sha256: String }`;
    `fn ensure_image(r: &dyn Runner, dir: &Path) -> Result<ImageRef>`: fetches
    `https://cloud-images.ubuntu.com/releases/24.04/release/SHA256SUMS` with `/usr/bin/curl`, finds
    `ubuntu-24.04-server-cloudimg-arm64.img`, downloads it into `~/.bunker/images/` if missing or
    mismatched, and verifies it (sha2, streaming). A mismatch after download is error
    `image_checksum`.
    **Why a local image** (ruling carried from plan 1): the sandboxed `limactl start` has no
    network, so it can't download `template://ubuntu-24.04` itself.
  - `fn render_lima_yaml(cfg: &Config, image: &ImageRef, vm_name: &str) -> String`:
    `vmType: vz`, `images: [{location: <path>, arch: aarch64, digest: "sha256:<…>"}]`, cpus, memory
    and disk from `[vm]`, `mounts: []`, `ssh: {localPort: <ports.ssh>, forwardAgent: false}`,
    `containerd: {system: false, user: false}`, `video.display: none`, `rosetta.enabled: false`,
    `propagateProxyEnv: false`, `hostResolver.enabled: false`, `dns: ["127.0.0.1"]`,
    `networkOpts: {hostLoopback: false}` (the key plan 1 set with `limactl edit`), the three
    ignore-everything `portForwards`, and no `provision` entries.
  - `fn hostagent_profile(lima_vm_dir_real: &Path, ssh_port: u16) -> String`: the spec's final
    profile, exactly (spec lines 611–621). Task 9 may extend it.
  - `fn fwd_proxy_profile(ssh_port: u16, proxy_port: u16) -> String` and
    `fn fwd_web_profile(ssh_port: u16, web_port: u16) -> String`: plan 1's `fwd-rproxy.sb` and
    `fwd-web.sb` with the ports filled in.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn lima_yaml_has_containment_settings()   // contains each line above; no "template://"; no "provision:"
#[test] fn hostagent_profile_matches_spike_final()  // == fixture tests/fixtures/hostagent.sb with <vm dir>=/x/.lima/bunker-t, port 60100
#[test] fn fwd_profiles_allow_only_their_ports()    // proxy profile: outbound only localhost:60100 and :18800, no network-bind allow; web: outbound :60100, bind :18900
#[test] fn image_checksum_mismatch_is_refused()     // RecordingRunner serves a SUMS file that doesn't match the local file -> image_checksum
#[test] #[ignore] fn sys_fwd_profile_confines_nc()
// control: `nc -z -w2 1.1.1.1 443` unsandboxed exits 0
// under fwd_web_profile: same command exits non-zero; `nc -z 127.0.0.1 <ssh port>` against a test listener exits 0
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test lima::` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test && cargo test -- --ignored sys_fwd_profile_confines_nc` — Expected: PASS.
- [ ] **Step 5: Commit** (if authorized).

---

### Task 8: VM lifecycle and confinement (`up` phase A, start, stop, restart, destroy)

**Files:**
- Create: `src/lima/{vm,confine}.rs`, `src/commands/{up,lifecycle,destroy}.rs`
- Test: unit tests, `sys_confinement_detects_unsandboxed_pid`

**Interfaces:**
- Consumes: Tasks 2, 6 and 7
- Produces:
  - `fn limactl_version(r) -> Result<(u32, u32, u32)>`; error `lima_too_old` (exit 1) below 2.3.0
  - `fn vm_name(project: &str) -> String` (`bunker-<project>`); `fn lima_vm_dir(name) -> Result<PathBuf>`
    (realpath of `$LIMA_HOME/<name>`)
  - `fn agents_for(project: &str, cfg: &Config, pd: &ProjectDir, limactl: &Path) -> [Agent; 3]`:
    - **vm:** `["/usr/bin/sandbox-exec","-f",<sandbox.sb>,<limactl>,"start","--foreground","--tty=false",<vm>]`
    - **fwd-proxy:** `["/usr/bin/sandbox-exec","-f",<fwd-proxy.sb>,"/usr/bin/ssh","-F",<vm dir>/ssh.config,"-o","ControlMaster=no","-o","ControlPath=none","-o","ForwardAgent=no","-o","ForwardX11=no","-o","PermitLocalCommand=no","-o","ExitOnForwardFailure=yes","-o","ServerAliveInterval=5","-o","ServerAliveCountMax=3","-N","-R","127.0.0.1:3128:127.0.0.1:<proxy>","lima-<vm>"]`
    - **fwd-web:** the same with `-L 127.0.0.1:<web>:127.0.0.1:80` and the web profile

    All three have `keep_alive` and `run_at_load` set, with `throttle_secs: Some(5)`. launchd is
    the supervisor that reconnects forwards after a guest reboot.
  - `enum Confinement { Confined { hostagent: i32 }, Unconfined { hostagent: i32, reason: String }, Stopped }`;
    `trait ConfinementProbe: Send + Sync { fn confinement(&self, project: &str) -> Result<Confinement>; }`
    with `SystemProbe` (which Tasks 12 and 14 take as `&dyn ConfinementProbe`, so tests pass a
    fake). `SystemProbe` reads `<vm dir>/ha.pid`, and calls
    `sandbox_check(pid, NULL, 0)` (FFI into libSystem) to confirm that process is sandboxed. It also
    reports each forward's launchd pid and whether that pid is sandboxed.
  - `bunker up <project>`, phase A (this task):
    1. validate config and Lima version
    2. `ensure_image`
    3. write `lima.yaml` and the three profiles
    4. `limactl create --tty=false --name <vm> <lima.yaml>` if the instance doesn't exist
    5. install or kickstart `com.bunker.proxy` (so it binds the new project's port)
    6. install the vm agent, then wait up to 180 s for `limactl list --json` to show `Running`
       and `limactl shell --tty=false <vm> true` to succeed
    7. install both forward agents
    8. write `127.0.1.1 <hostname>` into the guest's `/etc/hosts`
    9. report `{project, vm, confinement}` (`confinement` from `SystemProbe`)

    Re-running only rewrites what changed. It never calls `limactl create` for an existing
    instance, and it never starts a VM except through the vm agent.
  - `bunker start|stop|restart <project>`: start installs the vm agent, then the forwards. Stop
    boots out the forwards, then the vm agent, then runs `limactl stop --tty=false <vm>`.
  - `bunker destroy <project> [--yes]`: confirmation means typing the project name on a TTY, or
    `--yes`. It stops the VM, runs `limactl delete --force --tty=false <vm>`, removes the three
    agents and the generated files, and keeps `bunker.toml` and `backups/`. Task 13 adds the tunnel
    teardown.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn agents_have_expected_program_arguments()   // exact vectors above for project "t", ports 60100/18800/18900
#[test] fn fwd_ssh_never_multiplexes()                // both forward agents contain "ControlMaster=no" and "ControlPath=none"
#[test] fn up_refuses_old_lima()                      // limactl --version "limactl version 2.2.0" -> lima_too_old
#[test] fn up_rerun_does_not_recreate()               // instance listed -> no "limactl create" call
#[test] fn up_never_runs_unwrapped_start()            // no recorded call has program limactl with first arg "start"
#[test] fn stop_boots_out_forwards_then_vm_then_stops()   // call order
#[test] fn destroy_without_yes_needs_tty()            // non-TTY, no --yes -> exit 4 code needs_tty
#[test] #[ignore] fn sys_confinement_detects_unsandboxed_pid()   // `sleep 30` plain -> not sandboxed; under sandbox-exec with a trivial profile -> sandboxed
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test lima:: up stop destroy` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test && cargo test -- --ignored sys_confinement_detects_unsandboxed_pid` — Expected: PASS.
- [ ] **Step 5: Smoke test on the mini.** Run: `BUNKER_HOME=<scratch> bunker init smoke --path <an empty git repo with one commit> && bunker up smoke`.
  Expected: `bunker up` exits 0, and `bunker up smoke --json` reports `data.confinement == "confined"`. Then run `bunker destroy smoke --yes`,
  and expect no `bunker-smoke` in `limactl list` and no `com.bunker.smoke.*` agents.
- [ ] **Step 6: Commit** (if authorized). README: `up`, `start`, `stop`, `restart`, `destroy`.

---

### Task 9: Hostagent profile hardening beyond networking (bounded)

**Files:**
- Modify: `src/lima/sandbox.rs`
- Test: `sys_hardened_profile_blocks_home_writes`, plus Task 8's smoke test re-run

**Interfaces:**
- Produces: `fn hostagent_profile(...)` gains file-write and process-exec restrictions. Signature unchanged.

The spec's threat model says the profile restricts networking only, and that narrowing it is plan 2
work, as long as Lima still runs under it.

- [ ] **Step 1: Write the failing test**

```rust
#[test] #[ignore] fn sys_hardened_profile_blocks_home_writes()
// sandbox-exec -f <profile> /usr/bin/touch ~/bunker-sbx-probe  -> non-zero, file absent
// sandbox-exec -f <profile> /usr/bin/touch <vm dir>/sbx-probe   -> 0
// sandbox-exec -f <profile> /usr/bin/curl -sI https://example.com -> non-zero (networking still denied)
```

- [ ] **Step 2: Run it, watch it fail.** Expected: the home write succeeds under today's profile.
- [ ] **Step 3: Add the rules.** Add `(deny file-write*)` with allows for the VM dir,
  `$LIMA_HOME/_config`, the per-user temp dir (`confstr(_CS_DARWIN_USER_TEMP_DIR)`), `/dev/null`,
  `/dev/tty*` and `/dev/ptmx`. Add `(deny process-exec)` with allows for the `limactl` binary and
  `/usr/bin/ssh`.
- [ ] **Step 4: Bounded adjustment.** Re-run Task 8's smoke test. If Lima fails, read
  `<vm dir>/ha.stderr.log` and add one allow at a time, for paths or binaries the log names. Never
  add a network allow. At most 5 additions; ledger each. If 5 aren't enough, drop the failing class
  of restriction (file or exec) entirely, keep the other, and record that in a ruling and in the
  spec's threat-model paragraph.
- [ ] **Step 5: Verify.** Run: `cargo test -- --ignored sys_hardened_profile_blocks_home_writes`, then the smoke test.
  Expected: both pass. Update the fixture for `hostagent_profile_matches_spike_final` and rename it
  `hostagent_profile_matches_fixture`.
- [ ] **Step 6: Commit** (if authorized).

---

### Task 10: Secrets: store, generation, `.env` rendering, `env` commands

**Files:**
- Create: `src/secrets/{store,keychain,dotenv,generate}.rs`, `src/commands/env.rs`
- Test: unit tests, `tests/cli_env.rs`, `sys_keychain_roundtrip`

**Interfaces:**
- Consumes: `Config`, `ProjectDir`, `Runner`, `edit_lists`-style `toml_edit` helpers, `vm_name`
- Produces:
  - `trait SecretStore: Send + Sync`, with `get(project, key) -> Result<Option<Zeroizing<Vec<u8>>>>`,
    `set(project, key, value: &[u8]) -> Result<()>` and `delete(project, key) -> Result<bool>`
  - `KeychainStore`, generic passwords via security-framework, service `bunker.<project>`. If
    `BUNKER_KEYCHAIN` is set, it uses that keychain file (tests only). `MemoryStore` is for tests.
  - `fn generate_missing(project: &str, cfg: &Config, detected_passport: bool, store: &dyn SecretStore, r: &dyn Runner) -> Result<Vec<String>>`.
    It creates only keys that are absent:
    - `APP_KEY`: `base64:` plus 32 random bytes
    - `DB_PASSWORD`: 32 random alphanumerics
    - with Passport, `PASSPORT_PRIVATE_KEY` and `PASSPORT_PUBLIC_KEY`, from
      `/usr/bin/openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:4096`, captured from
      stdout, with the public key from `openssl pkey -pubout` fed the private key on stdin

    It records each name in `[secrets]` as `{ kind = "in-app" }`.
  - `fn render_env(project: &str, cfg: &Config, store: &dyn SecretStore) -> Result<Zeroizing<String>>`:
    1. Managed keys first: `APP_NAME=<project>`, `APP_ENV=production`, `APP_DEBUG=false`,
       `APP_URL=https://<domain>` (when set), `DB_CONNECTION=mysql`, `DB_HOST=127.0.0.1`,
       `DB_PORT=3306`, `DB_DATABASE=app`, `DB_USERNAME=app`, `LOG_CHANNEL=daily`.
    2. Then `[env]`.
    3. Then every `[secrets]` in-app value.

    Every value is double-quoted, with `\` → `\\`, `"` → `\"` and `$` → `\$`. Newlines stay literal
    inside the quotes. `PROTECTED = [APP_ENV, APP_KEY, DB_PASSWORD, DB_USERNAME, DB_DATABASE, DB_HOST]`
    can't be set as plain settings.
  - `fn push_env(r: &dyn Runner, project: &str, rendered: &str, deployed: bool) -> Result<()>`:
    `limactl shell --tty=false <vm> sudo install -m 0640 -o root -g app /dev/stdin /srv/app/shared/.env`
    with the rendered text on stdin. When `deployed`, it then runs `config:cache` as `app` and
    restarts the workers.
  - Commands:
    - `bunker env set KEY [project] [--stdin]`: reads the value from the TTY with rpassword, or
      from stdin with `--stdin`. Neither available means exit 4 `needs_tty`; an empty value means
      exit 4.
    - `bunker env set KEY VALUE --plain`: writes to `[env]`.
    - `bunker env set … --bind <host>`: exit 4 `not_yet_supported`, with the message
      `outbound credentials arrive in a later release`.
    - `bunker env unset KEY`.
    - `bunker env list`: names and kinds only.

    After any change: `render_env`, then `push_env` if the VM exists.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn render_env_quotes_special_characters()   // value a"b$c\d#e -> line KEY="a\"b\$c\\d#e"
#[test] fn render_env_keeps_multiline_pem()         // PEM value appears between quotes with literal newlines
#[test] fn render_env_sets_production_flags()       // APP_ENV="production", APP_DEBUG="false"
#[test] fn plain_cannot_override_protected_keys()   // env set APP_KEY x --plain -> exit 4 code key_protected
#[test] fn env_set_bind_is_refused_for_now()        // exit 4 code not_yet_supported
#[test] fn env_list_never_prints_values()           // MemoryStore value "s3cr3t-fixture-value": absent from human and --json output
#[test] fn push_env_sends_values_on_stdin_only()    // RecordingRunner.assert_no_secret_in_argv(&["s3cr3t-fixture-value"])
#[test] fn generate_creates_only_missing()          // existing APP_KEY untouched; DB_PASSWORD created
#[test] fn app_key_format()                         // "base64:" + base64 decoding to 32 bytes
#[test] fn openssl_private_key_never_in_argv()      // the generated PEM appears only in stdin of the pubout call
#[test] #[ignore] fn sys_keychain_roundtrip()       // throwaway keychain in tempdir via BUNKER_KEYCHAIN: set, get, delete
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test secrets env` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test && cargo test -- --ignored sys_keychain_roundtrip` — Expected: PASS.
- [ ] **Step 5 [HUMAN]: Keychain access rule across updates.** The spec wants only bunker able to
  read the items without a prompt. The agent prepares, the maintainer runs:
  1. `cargo build --release`, then sign it:
     `codesign --force --options runtime --timestamp --sign "Developer ID Application: Glen Gringgo Bangkila (2VJCCC449X)" target/release/bunker`
  2. `printf probe-value | target/release/bunker env set BUNKER_ACL_PROBE aclprobe --stdin` (on a throwaway project `aclprobe`)
  3. `security find-generic-password -s bunker.aclprobe -a BUNKER_ACL_PROBE -w`. Expected: macOS
     asks for approval. Click **Deny**.
  4. Rebuild and re-sign, then `target/release/bunker env list aclprobe`. Expected: no prompt.

  If steps 3 and 4 behave, record in the spec that Developer ID signing keeps the access rule
  across updates. If not, fall back per the spec (plain login Keychain items) and update the spec.
  Clean up with `bunker env unset BUNKER_ACL_PROBE aclprobe` and remove `~/.bunker/aclprobe`.
- [ ] **Step 6: Commit** (if authorized). README: `env`.

---

### Task 11: Guest provisioning (`up` phase B)

**Files:**
- Create: `src/guest/{provision,render}.rs`, `templates/{provision.sh,nginx-site.conf,php-fpm-pool.conf,worker.service,scheduler.service,scheduler.timer,nftables.conf,logrotate-app}`
- Modify: `src/commands/up.rs`
- Test: unit tests, the first confined provision on the smoke VM

**Interfaces:**
- Consumes: `Config`, `SecretStore`, `generate_missing`, `open_lease`, `Runner`, `vm_name`
- Produces:
  - `fn render_provision(cfg: &Config) -> String`: an idempotent bash script (`set -euo pipefail`,
    never `set -x`). Config values are interpolated only after validation. Its steps:
    1. `/etc/hosts` hostname line.
    2. Proxy settings: `/etc/environment` gets `http_proxy`, `https_proxy` and the upper-case
       forms set to `http://127.0.0.1:3128`, with `no_proxy=localhost,127.0.0.1`; plus
       `/etc/apt/apt.conf.d/95bunker-proxy`, global Composer config, and npm config for `deploy`.
    3. Disable `apt-daily`, `apt-daily-upgrade` and the unattended-upgrades timer. Plan 3 owns the
       maintenance window.
    4. The guest nftables firewall from `templates/nftables.conf`: established/related accept, `lo`
       accept, then output drop to internal ranges. This is defense in depth only.
    5. Users `app` and `deploy` (no sudo); `/srv/app/{releases,shared/storage/{app,framework/{cache,sessions,views},logs}}`
       with the spec's ownership.
    6. The ondrej PPA, MySQL 8.4 (`repo.mysql.com`, `mysql-8.4-lts`) and NodeSource `node_22.x`
       repos, each with a `signed-by` keyring. The key is fetched, then its fingerprint is checked
       against a constant pinned from the vendor's published fingerprint, with the source URL in a
       comment. A fingerprint mismatch aborts.
    7. `php<ver>-{fpm,cli,mysql,mbstring,xml,curl,zip,bcmath,intl}` plus `[extensions]`; nginx;
       `mysql-server`; `nodejs` when `node` is set; Composer as a pinned version, sha256-verified,
       into `/usr/local/bin/composer`.
    8. Pool `app` (`templates/php-fpm-pool.conf`, `user = app`, `listen.owner = www-data`,
       `clear_env = yes`); site `templates/nginx-site.conf`; default site removed.
    9. MySQL database `app` and user `app@localhost`, with the password fed to `mysql` on a heredoc.
       Never `mysql -e` or `-p<value>`.
    10. Worker units (`bunker-worker-<name>.service`) and, when enabled, the scheduler timer, all
        with `ConditionPathExists=/srv/app/current/artisan`; logrotate for `storage/logs` and nginx.
    11. Enable everything.

    The DB password reaches the script as the first line of stdin
    (`IFS= read -r BUNKER_DB_PASSWORD`), never in the script text and never in argv.
  - `fn render_nginx(cfg) -> String`: `listen 127.0.0.1:80`, `server_tokens off`, and the four
    headers with `always`: `X-Content-Type-Options "nosniff"`, `X-Frame-Options "DENY"`,
    `Referrer-Policy "strict-origin-when-cross-origin"`,
    `Permissions-Policy "camera=(), microphone=(), geolocation=()"`. Also
    `fastcgi_hide_header X-Powered-By`, `set_real_ip_from 127.0.0.1`,
    `real_ip_header CF-Connecting-IP`, root `/srv/app/current/public`, and a dotfile deny except
    `.well-known`.
  - `fn render_worker(name: &str, cmd: &str) -> String`: `User=app`,
    `WorkingDirectory=/srv/app/current`, `Restart=always`, `RestartSec=3`.
  - `fn provision(r, store, project, cfg) -> Result<()>`: `generate_missing`, then opens a Build
    lease (TTL 45 min), runs `limactl shell --tty=false <vm> sudo bash -s` with stdin = DB password
    line + script, and drops the lease. `bunker up` runs phase A and then this.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn nginx_has_hardened_headers_and_real_ip()        // each directive above present
#[test] fn worker_unit_runs_as_app_with_restart()
#[test] fn provision_script_is_valid_bash()                // `bash -n` on the rendered script exits 0
#[test] fn provision_script_never_puts_password_on_a_command_line()   // no "mysql -e", no "-p$", no "IDENTIFIED BY '" outside the heredoc
#[test] fn provision_sets_proxy_for_apt_composer_npm()
#[test] fn provision_runs_inside_build_lease()             // RecordingRunner hook: lease file exists during the bash call, gone after
#[test] fn provision_sends_db_password_on_stdin_only()     // assert_no_secret_in_argv
#[test] fn repo_keys_are_fingerprint_pinned()              // each repo block compares the fingerprint before writing the keyring
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test guest` — Expected: FAIL.
- [ ] **Step 3: Implement.** Run `shellcheck` on the rendered script if it's installed. Its findings are fixed or ledgered.
- [ ] **Step 4: Verify.** Run: `cargo test` — Expected: PASS.
- [ ] **Step 5: First confined provision on the mini.** This is the open item in "Findings plan 2
  must carry". Run `BUNKER_HOME=<scratch> bunker up smoke` from scratch on a Laravel-shaped fixture
  (`composer create-project laravel/laravel <scratch>/smoke --no-install --no-scripts`, then
  `git init` and a commit).
  Expected:
  - exit 0, with `SystemProbe` reporting `Confined` before the provision call
  - `jq -c 'select(.kind=="decision" and .verdict=="block")' ~/…/smoke/network.jsonl` shows
    nothing needed. A block that broke provisioning is fixed by adding that host to `BUILTIN_BUILD`
    (ledgered ruling, with the log line).
  - a second `bunker up smoke` exits 0 and changes nothing (idempotence)

  Keep the smoke VM for Task 12.
- [ ] **Step 6: Commit** (if authorized).

---

### Task 12: Deploy and rollback

**Files:**
- Create: `src/deploy/{source,steps,run,lock}.rs`, `src/commands/{deploy,rollback}.rs`
- Test: unit tests, `tests/cli_deploy.rs`, a deploy to the smoke VM

**Interfaces:**
- Consumes: `Config`, `SecretStore`, `generate_missing`, `render_env`, `push_env`, `open_lease`, `confinement`, `Runner`
- Produces:
  - `struct Archive { tar: tempfile::NamedTempFile, sha: String, dirty: bool }`;
    `fn build_archive(repo: &Path, dirty: bool) -> Result<Archive>`:
    - A tracked modification (`git status --porcelain --untracked-files=no` non-empty) is refused
      with exit 4 `dirty_tree` unless `dirty` is set.
    - Clean: `git archive --format=tar HEAD`. Dirty: a tar of `git ls-files -z` read from the
      working tree.
    - Both drop every entry with any path component starting with `.env`.
  - `fn release_name(now: OffsetDateTime, sha: &str, dirty: bool) -> String`, e.g.
    `20261116T030405Z-abc1234`, with a `-dirty` suffix when dirty
  - `enum As { Deploy, App, Root }`; `struct Step { name: &'static str, run_as: As, cmd: String, needs_lease: bool }`;
    `fn steps(cfg: &Config, release: &str, first: bool, allow_advisories: bool) -> Vec<Step>`.
    The step names, in order:
    1. `upload`
    2. `composer_install` (lease)
    3. `npm_build` (lease, only when `node` is set and `package.json` has `build`)
    4. `composer_audit` (lease)
    5. `npm_audit` (lease, with node)
    6. `link_shared`
    7. `write_env` (first deploy only)
    8. `first_deploy_hooks` (first only)
    9. `caches`
    10. `before_switch_hooks`
    11. `switch`
    12. `reload`
    13. `prune`

    **Ruling** vs the spec's order: `write_env` runs before `caches`, because `config:cache` needs
    `.env` and on a first deploy it doesn't exist at spec step 5.

    `link_shared` replaces the release's `storage` with a symlink to `shared/storage`, links
    `.env` to `shared/.env`, and makes the release's `bootstrap/cache` group `app`, mode 2775. The
    rest of the release stays owned by `deploy` and read-only to `app`.
  - `struct DeployOpts { dirty: bool, allow_advisories: bool }`;
    `fn deploy(r: &dyn Runner, store: &dyn SecretStore, probe: &dyn ConfinementProbe, project: &str, cfg: &Config, opts: DeployOpts) -> Result<DeployReport { release, sha, dirty, steps: Vec<StepResult { name, ok, secs }>, url: Option<String> }>`.
    The order of work:
    1. Take `flock` on `deploy.lock` (held → exit 4 `deploy_in_progress`).
    2. Require `Confined` (else exit 5 `unconfined`).
    3. Upload: `mkdir` the release dir as `deploy`, then `tar -x` from stdin.
    4. Open the Build lease around the lease steps only.
    5. First deploy means `/srv/app/shared/.env` is absent. Run `generate_missing`, then
       `render_env` and `push_env`.
    6. `switch`: `ln -s releases/<r> /srv/app/current.tmp && mv -T /srv/app/current.tmp /srv/app/current`.
    7. `reload`: `systemctl reload php<ver>-fpm` and restart `bunker-worker-*`.
    8. `prune` keeps 5, never `current` or the previous release.

    A failure before `switch` removes the half-built release and leaves `current` alone. Audit
    advisories refuse with exit 4 `advisories` unless `--allow-advisories`.
  - `fn rollback(r, project, cfg) -> Result<RollbackReport>`: switches to the release before
    `current`, then reloads and restarts. With none, exit 4 `no_previous_release`. Output always
    says `migrations are not rolled back`.
  - Human output streams one line per step. `--json` returns the report.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn archive_drops_env_files()      // temp repo with force-added .env, .env.production, config/.env.example, app/x.php -> only app/x.php remains
#[test] fn dirty_tree_refused_without_flag()     // exit 4 code dirty_tree
#[test] fn dirty_flag_ships_tracked_worktree_not_untracked()
#[test] fn release_name_format()
#[test] fn steps_order_first_deploy()     // exact names list above (with node)
#[test] fn steps_order_later_deploy()     // no write_env, no first_deploy_hooks
#[test] fn build_lease_closed_after_failed_composer()   // runner fails composer_install -> lease file absent, no "mv -T" call, release dir removed
#[test] fn concurrent_deploy_is_refused() // lock held by the test -> exit 4 deploy_in_progress
#[test] fn deploy_refuses_when_unconfined()   // fake confinement Unconfined -> exit 5, no upload call
#[test] fn switch_is_an_atomic_rename()
#[test] fn prune_keeps_five_and_never_current()
#[test] fn audit_advisories_refuse_unless_allowed()
#[test] fn rollback_without_previous_is_refused()
#[test] fn rollback_says_migrations_not_undone()
#[test] fn first_deploy_env_on_stdin_only()   // assert_no_secret_in_argv with the MemoryStore's values
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test deploy rollback` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test` — Expected: PASS.
- [ ] **Step 5: Deploy the smoke fixture.** Run: `bunker deploy smoke`, then `curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:<web>/`.
  Expected: deploy exits 0, and curl prints `200`. A second deploy, then `bunker rollback smoke`,
  serves the first release (check the `current` symlink target). Then run `bunker destroy smoke --yes`.
- [ ] **Step 6: Commit** (if authorized). README: `deploy`, `rollback`.

---

### Task 13: Cloudflare tunnel (`up` phase C, destroy)

**Files:**
- Create: `src/tunnel/{cloudflared,config}.rs`
- Modify: `src/commands/{up,destroy}.rs`
- Test: unit tests

**Interfaces:**
- Consumes: `Config` (`domain`, `tunnel`, `ports.web`), `launchd`, `ProjectDir::tunnel_credentials`
- Produces:
  - `struct Cloudflared { bin: PathBuf, cert: PathBuf /* ~/.cloudflared/cert.pem */ }`, with:
    - `find(name) -> Result<Option<String /* id */>>`: `tunnel --origincert <cert> list --name <n> -o json`;
      parse the JSON between the first `[` and the last `]`, as Yard does
    - `create(name, creds: &Path)`: `create --credentials-file <creds> <name>`, then chmod 0600
    - `route(id, host, overwrite) -> Result<()>`: `route dns [-f] <id> <host>`. Output containing
      `already exists` without `overwrite` gives exit 4 `dns_exists` with the hint
      `bunker up <project> --overwrite-dns`.
    - `delete(id)`: `cleanup <id>`, then `delete <id>`
  - `fn render_cloudflared_yml(id: &str, creds: &Path, domain: &str, web_port: u16) -> String`:
    `tunnel`, `credentials-file`, and `ingress` with `hostname: <domain>` → `service: http://127.0.0.1:<web>`,
    then `service: http_status:404`
  - Agent `com.bunker.<p>.tunnel`: `cloudflared tunnel --no-autoupdate --config <yml> run`, KeepAlive
  - `bunker up <project> [--overwrite-dns]`, phase C, runs only when `domain` and `tunnel` are both
    set. It finds or creates the tunnel. An existing tunnel without a credentials file gives
    exit 4 `tunnel_credentials_missing`, with a hint to delete it or supply the file. Then it
    routes, writes the yml and installs the agent.
  - `destroy` also removes the tunnel agent, deletes the tunnel and prints
    `Remove the CNAME for <domain> in the Cloudflare dashboard; cloudflared can't delete DNS records.`

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn yml_routes_domain_to_web_port_with_404_fallback()
#[test] fn route_existing_without_overwrite_is_refused_with_hint()   // runner stderr "...already exists..." -> exit 4 dns_exists, hint contains --overwrite-dns
#[test] fn route_with_overwrite_passes_dash_f()
#[test] fn credentials_written_0600()
#[test] fn up_without_domain_skips_tunnel()       // no cloudflared calls
#[test] fn destroy_prints_cname_to_remove()
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test tunnel` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test` — Expected: PASS. README: `up --overwrite-dns`.
- [ ] **Step 5: Commit** (if authorized).

---

### Task 14: Status, logs, artisan, shell

**Files:**
- Create: `src/logs/{scrub,tail}.rs`, `src/commands/{status,logs,artisan,shell}.rs`
- Test: unit tests, `tests/cli_status.rs`

**Interfaces:**
- Consumes: `confinement`, `launchd::status`, `SecretStore`, `Runner`, `last_policy`
- Produces:
  - `bunker status [project] [--json]` returns this shape:
    `{project, vm:{state, confinement:"confined"|"unconfined"|"stopped", hostagent_pid}, forwards:{proxy:{loaded,pid,sandboxed}, web:{loaded,pid,sandboxed}}, proxy:{loaded, listening, policy:"ok"|"error"}, tunnel:{configured, loaded, pid}|null, services:{nginx, php_fpm, mysql, workers:{<name>:state}}, release:{current, sha, previous}|null, warnings:[string]}`.
    With no project, it returns an array of those. Exit 5 if any VM is unconfined, 1 if anything
    expected is down, else 0. Guest service states come from one
    `limactl shell … systemctl is-active …` call.
  - `struct Scrubber`; `fn Scrubber::new(known: Vec<(String /* key */, Zeroizing<String>)>) -> Scrubber`;
    `fn scrub<'a>(&self, line: &'a str) -> Cow<'a, str>`:
    - known values of 6 or more characters become `[redacted:<KEY>]`
    - the rest of the line after `Authorization:`, and the token after `Bearer `, become `[redacted]`
    - so do values of `password|passwd|secret|token|api[_-]?key` in `k=v` or `"k":"v"` form
    - lines between `-----BEGIN … PRIVATE KEY-----` and `-----END …-----` become `[redacted]`

    **Ruling:** the spec's "old bunker's secret scrubber" doesn't exist (v0.1 only stripped headers
    in Caddy), so this is new.
  - `bunker logs [project] [nginx|php|worker|app|proxy|tunnel] [-f] [-n N]`. Guest sources are read
    through `limactl shell`; host sources are files under `~/.bunker`. Every line passes through
    `Scrubber`, built from the project's Keychain values.
  - `bunker artisan <project> -- <args…>`: `limactl shell [--tty=false unless stdin is a TTY] <vm> sudo -u app php /srv/app/current/artisan <quoted args>`,
    passing the exit code through. Each arg is POSIX single-quoted.
  - `bunker shell <project>`: interactive `limactl shell <vm>`. Without a TTY, exit 4 `needs_tty`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn scrubber_redacts_known_values()
#[test] fn scrubber_redacts_patterns_and_pem_blocks()
#[test] fn scrubber_keeps_ordinary_lines()        // returns Cow::Borrowed
#[test] fn status_json_shape()                    // faked runner + confinement; every key above present
#[test] fn status_exit_5_when_unconfined()
#[test] fn artisan_args_are_single_quoted()       // "--execute=echo 'x'" -> '--execute=echo '\''x'\'''
#[test] fn shell_refuses_without_tty()
```

- [ ] **Step 2: Run, watch them fail.** Run: `cargo test logs status artisan shell` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test` — Expected: PASS. README: `status`, `logs`, `artisan`, `shell`.
- [ ] **Step 5: Commit** (if authorized).

---

### Task 15: Reboot and lifecycle wiring

**Files:**
- Modify: `src/commands/up.rs`, `src/launchd.rs`
- Test: unit tests

**Interfaces:**
- Produces: the `com.bunker.proxy` agent (`bunker proxy serve`, KeepAlive, RunAtLoad), installed by
  the first `up`. Every VM, forward and tunnel agent is `RunAtLoad`, so a login after a Mac reboot
  brings everything back. launchd has no ordering, so the forwards' `ExitOnForwardFailure` plus
  `ThrottleInterval` retries until the VM and the proxy are up. The proxy agent's program is the
  installed binary path (`std::env::current_exe` resolved through symlinks); `up` warns when that
  path is inside a `target/` directory.
- Produces: host-side log rotation. `bunker proxy serve` runs a housekeeping tick every 10 minutes
  that rotates `network.jsonl` and every `*.log` under `~/.bunker/**/logs/` at 10 MiB, keeping 5
  copies (v0.1's limits). Agents' stdout and stderr point into `logs/`. Guest-side rotation is
  logrotate (Task 11).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn rotation_keeps_five_copies_at_ten_mib()   // tempdir: an 11 MiB file -> file.1 created, a fresh file, at most .1–.5
#[test] fn every_agent_runs_at_load()
#[test] fn proxy_agent_points_at_installed_binary()
#[test] fn up_warns_when_binary_is_a_build_artifact()   // current_exe under /x/target/release -> warning in report
```

- [ ] **Step 2: Run, watch them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Verify.** Run: `cargo test` — Expected: PASS.
- [ ] **Step 5: Commit** (if authorized).

---

### Task 16: Acceptance on the mini (disposable fixture)

**Files:**
- Create: `tests/acceptance/{run.sh,env.sh,probe.sh,probe2.sh,compare.sh,README.md}`,
  `tests/acceptance/expected/{probe,probe2,proxy}.json`
- Source: plan 1's scripts, at
  `/private/tmp/claude-501/-Users-inggo-AI-bunker/ba7a2e9f-72c1-4402-9ae7-1be815f4a239/scratchpad/spike/{probe.sh,probe2.sh,compare.sh}`.
  If that scratchpad is gone, rewrite them from the spec's results table: one JSON line per check,
  `{"check","result":"reachable"|"blocked","detail"}`.

**Harness rules** (from the spec): logs append and never truncate; a failing run's output is kept
under `tests/acceptance/results/<timestamp>/` (gitignored). Every check expected to be blocked has a
**control**: the same target, probed from the host (unconfined), must be reachable, or the check
is reported `no_control` and proves nothing. `env.sh` holds the mini's values from plan 1
(`MAC_LAN=192.168.254.114`, `ROUTER=192.168.254.254`, `TS_PEER=100.80.217.65`, `TS_SELF=100.112.139.61`)
and uses `BUNKER_HOME=$ACC/home` so nothing touches the real `~/.bunker`.

Phases (`run.sh <phase>`; each exits 0 on match, 1 on mismatch, 2 on setup error):
- `fixture`: `composer create-project laravel/laravel $ACC/fixture --no-install --no-scripts`,
  `git init`, commit.
- `up`: `bunker init fixture --path $ACC/fixture`, `bunker net allow example.com fixture`, then
  `bunker up fixture`. Asserts `bunker status fixture --json` reports `confinement == "confined"`
  and both forwards `sandboxed: true`, and that `network.jsonl` has no `block` decision for a
  `BUILTIN_BUILD` host.
- `deploy`: `bunker deploy fixture`; web port returns 200.
- `controls`: from the host, `nc -z` or curl each target (1.1.1.1:443, `$MAC_LAN:22`,
  `$ROUTER:80`, `$TS_PEER:22`, `$TS_SELF:22`, a test listener on host loopback). Each must be
  reachable.
- `probe`: copy `probe.sh` and `probe2.sh` in through `limactl copy`, then run them as root in the
  guest with `HTTPS_PROXY=http://127.0.0.1:3128`. Compare with `expected/probe*.json`: everything
  `blocked` except `proxy_allowed`.
- `proxy`: from the guest, through the proxy: `CONNECT 10.0.0.1:443`,
  `https://127.0.0.1.nip.io` and `https://$MAC_LAN.nip.io` (both added with `net allow` first, to
  prove the internal check beats the allowlist), `https://$TS_SELF.nip.io`, and
  `http://169.254.169.254/`. Each must return 403, with `reason: internal_address` (or `not_listed`)
  in `network.jsonl`.
- `roundtrip`: `printf '%s' 'q"u$o\te#d\nline2' | bunker env set BUNKER_ROUNDTRIP fixture --stdin`.
  Then the sha256 of the guest's `env('BUNKER_ROUNDTRIP')`, read via
  `bunker artisan fixture -- tinker --execute='echo hash("sha256", env("BUNKER_ROUNDTRIP"));'`, must
  equal the host's sha256 of the same fixture string.
- `reboot`: `limactl shell … sudo systemctl reboot`. Wait for the boot ID to change. Within 90 s,
  the web port returns 200, `confinement == "confined"` and both forwards are back. Then re-run
  `probe`.
- `unwrapped`: `bunker stop fixture`, then plain `limactl start --tty=false bunker-fixture`.
  `bunker status fixture --json` must exit 5 with `confinement == "unconfined"`, and
  `bunker deploy fixture` must exit 5. Then `limactl stop`, `bunker start fixture`, and confined
  again.
- `rollback`: commit a change to the fixture, deploy, `bunker rollback fixture`; the first
  release's content is served again.
- `cleanup [--final]`: `bunker destroy fixture --yes`, then remove `$ACC/home` and the fixture.
  Leaves no `bunker-fixture` VM and no `com.bunker.fixture.*` agents.

- [ ] **Step 1: Write the scripts and expectation files.**
- [ ] **Step 2: Run every phase in order.** Run: `tests/acceptance/run.sh all`, which runs the phases above, then `cleanup`.
  Expected: exit 0. Any `probe` or `proxy` mismatch is a **hard stop**.
- [ ] **Step 3: Record results.** Add a short "Plan 2 acceptance" paragraph to the spec's results
  section: date, the phases run, and one line each for any finding.
- [ ] **Step 4: Commit** (if authorized): scripts, expectations, the spec paragraph.

---

### Task 17: Life OS goes live

**Files:**
- Modify (outside this repo, committed there only with the maintainer's OK): `~/Code/life-os/CLAUDE.md`
  sections "Production Runtime (Bunker)" and "Bunker Commands"

Uses the real `~/.bunker` and the installed binary. `cargo install --path . --locked --root ~/.local`
replaces the old v0.1 binary at `~/.local/bin/bunker`. Sign it first as in Task 10 step 5 if that
path was kept.

- [ ] **Step 1: Init.** Run: `bunker init life-os --path ~/Code/life-os --domain life-os.bangkila.com --tunnel life-os`.
  Expected: exit 0. Then edit `bunker.toml` hooks to `first_deploy = []` (Life OS loads Passport
  keys from `.env`) and `before_switch = ["php artisan migrate --force"]`, and set `[workers]
  queue` per the spec.
- [ ] **Step 2 [HUMAN]: Plain settings.** The maintainer runs `bunker env set <KEY> <VALUE> --plain life-os`
  for the Life OS settings that aren't secrets (the Filament admin path and email allowlist,
  `MAIL_MAILER`, `MAIL_FROM_*`). `warden list` in `~/Code/life-os` and `config/filament.php` show
  the key names. Real secret values, if any, go through `bunker env set KEY life-os` on the TTY.
- [ ] **Step 3: Up without the tunnel first.** Temporarily remove `domain` and `tunnel` from
  `bunker.toml`, run `bunker up life-os`, then restore them. Expected: exit 0, confined.
- [ ] **Step 4 [HUMAN]: DNS.** Run: `bunker up life-os`. Expected: exit 4 `dns_exists` (the CNAME
  still points at the deleted old tunnel). The maintainer approves, then the agent runs
  `bunker up life-os --overwrite-dns`. Expected: exit 0, and the tunnel agent is loaded.
- [ ] **Step 5: Deploy.** Run: `bunker deploy life-os`. Expected: exit 0 and the migrations run.
  Then `bunker status life-os --json`: exit 0, with `confinement == "confined"`.
- [ ] **Step 6: Edge checks from the mini.**
  - `curl -s -o /dev/null -w '%{http_code}' https://life-os.bangkila.com/.well-known/oauth-authorization-server`
    Expected: `200` (the Access bypass path).
  - `curl -s -o /dev/null -w '%{http_code}' https://life-os.bangkila.com/` Expected: `302` or `403`
    from Cloudflare Access, not a 5xx or a Cloudflare 1033 error.
- [ ] **Step 7 [HUMAN]: First user and browser check.** The maintainer runs
  `bunker artisan life-os -- make:filament-user` on a TTY, signs in through Access in a browser,
  and confirms the admin panel and the MCP connection (the `life-os` MCP server in Claude Code)
  work.
- [ ] **Step 8 [HUMAN, recommended]: Mac reboot.** Reboot the mini and log in. Expected: without
  any command, `bunker status life-os --json` exits 0 and the site answers.
- [ ] **Step 9: Life OS docs.** Rewrite Life OS's two stale CLAUDE.md sections to describe bunker
  v2: the VM, `bunker deploy life-os`, `bunker logs`, `bunker artisan`, and that config lives in
  `~/.bunker/life-os/bunker.toml`. Keep "No deployment config lives in this repo". Ask before
  committing in that repo.

---

### Task 18: Docs as built, cleanup

**Files:**
- Modify: `README.md`, `CLAUDE.md`, `AGENTS.md`, `docs/superpowers/specs/2026-11-16-bunker-design.md`

- [ ] **Step 1: README, CLAUDE.md and AGENTS.md describe v2 as built:** the architecture, the
  command reference with `--json` and exit codes, the layout, and how to run `sys_*` tests and the
  acceptance harness.
- [ ] **Step 2: The spec matches what was built.** Fold in this plan's rulings that change
  user-visible behaviour:
  - the `[network].deny` list and the `host:port` syntax, with ports 80/443 by default
  - build and maintenance leases
  - `up --overwrite-dns`
  - the local Ubuntu image
  - launchd as the supervisor
  - `write_env` before `caches`
  - the maintenance window and guest security updates waiting for plan 3
  - the Keychain result from Task 10
  - the scrubber being new
- [ ] **Step 3: Check the docs against the code.** Grep the docs for every command, flag, config
  key and launchd label; fix mismatches.
- [ ] **Step 4: Clean up.** Run `cargo clean --profile dev`, and remove `target/release` unless it's the
  installed binary's source (it isn't; Task 17 installs to `~/.local`). Report the space freed.
  Keep `~/.bunker/images/`.
- [ ] **Step 5: Commit** (if authorized).
