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
| `bunker init <project> --path <repo> [--domain D] [--tunnel T] [--php V] [--force]` | Detect the Laravel app (PHP from composer.json's constraint, extensions, Node, Passport), pick free ports and write a commented `~/.bunker/<project>/bunker.toml` (mode 0600). The tunnel defaults to the project name when `--domain` is given |
| `bunker up <project> [--overwrite-dns]` | Create (first time) and start the project's confined VM: downloads and verifies the Ubuntu image, writes the Lima template and Seatbelt profiles, starts the proxy, the VM and both SSH forwards as LaunchAgents, and checks the VM is confined. Then, only if it is, provisions it: PHP and nginx (ondrej PPA), MySQL 8.4 (Percona Server, since Oracle has no arm64 packages), Node when configured, Composer, the `app` and `deploy` users, workers and the scheduler, with the build hosts open for 45 minutes at most. Generates the database password and app key into the Keychain. Output goes to `~/.bunker/<project>/logs/provision.log`. With `domain` and `tunnel` set, it then finds or creates the Cloudflare named tunnel (needs `cloudflared tunnel login` once), points the domain at it with a CNAME (an existing record is refused unless `--overwrite-dns`), writes `cloudflared.yml` (the domain to the web forward, anything else 404) and runs cloudflared as a LaunchAgent. Safe to re-run |
| `bunker start <project>` | Start a stopped VM and its forwards; the tunnel comes back only once the VM checks out confined |
| (after a reboot) | Log in and everything comes back on its own: the proxy, each VM, its forwards and its tunnel are LaunchAgents with `RunAtLoad`. A tunnel starts only once its VM checks out confined. Host logs under `~/.bunker` rotate at 10 MiB, five copies kept |
| `bunker stop <project>` | Stop the tunnel (first, so the domain stops reaching the Mac), the forwards and the VM. Nothing comes back at login until `start` or `up` |
| `bunker restart <project>` | Stop, then start |
| `bunker deploy <project> [--dirty] [--allow-advisories]` | Ship the repo's `HEAD` as a new release (`/srv/app/releases/<time>-<sha>`): tracked files only, never any `.env*` path. Runs `composer install`, the npm build (when `node` is set), `composer audit` and `npm audit` with the build hosts open, then links shared storage and `.env`, caches config, routes and views, runs the hooks, switches `current` in one rename, reloads php-fpm, restarts workers and keeps five releases. The first deploy generates `APP_KEY` and writes `.env`. A failure before the switch removes the half-built release. Audit advisories refuse the deploy unless `--allow-advisories`; uncommitted changes to tracked files need `--dirty`. One line per step; output in `~/.bunker/<project>/logs/deploy.log` |
| `bunker rollback <project>` | Point `current` back at the release before it, reload php-fpm and restart workers. Migrations are not rolled back |
| `bunker status [project]` | For one project, or every project: the VM and whether its hostagent is confined, both forwards (running, sandboxed), the proxy (listening, policy readable), the tunnel, nginx, php-fpm, MySQL and each worker in the guest, and the live and previous release, with warnings. Exit 5 if a VM is unconfined, 1 if anything expected is down |
| `bunker logs [project] [nginx\|php\|worker\|app\|proxy\|tunnel] [-f] [-n N]` | Recent log lines (100 by default; `app` when no source is named), following with `-f`. Guest logs come through limactl, the proxy and tunnel logs from `~/.bunker`. Every line is scrubbed first: the project's Keychain values, `Authorization` and `Bearer` credentials, password, secret, token and API-key values, and private keys show as `[redacted]` |
| `bunker exec [project] <command…>` | Run a command as the app user in the live release (`/srv/app/current`), attached to this terminal, with its exit status: `bunker exec php artisan migrate:status`, `bunker exec life-os composer show`. The first word is the project only if one has that name; otherwise it's the project whose repo holds this directory. Arguments reach the command as given; put `--` after the project if the command shares a project's name |
| `bunker shell <project>` | An interactive shell in the VM (needs a terminal) |
| `bunker destroy <project> [--yes]` | Delete the VM (its database and storage go with it), its agents and generated files. Keeps `bunker.toml`, backups and logs. Asks you to type the project name unless `--yes`. With a tunnel, it also stops cloudflared and deletes the tunnel; the CNAME stays, so remove it in the Cloudflare dashboard |
| `bunker env set KEY [project] [--stdin]` | Store a secret in the macOS Keychain (service `bunker.<project>`). It's read without echo, or from stdin with `--stdin`, and never printed. The VM's `.env` is rewritten if the VM is running |
| `bunker env set KEY VALUE [project] --plain` | A non-secret setting such as `MAIL_MAILER`, stored in `bunker.toml` `[env]`. Keys bunker manages (`APP_ENV`, `APP_KEY`, `DB_*`) are refused |
| `bunker env unset KEY [project]` / `bunker env list [project]` | Remove a key; list names and kinds (never values) |
| `bunker net allow <host> [project] [--build]` | Let the running site reach a host (`host`, `*.domain` for subdomains, or a public IP; ports 80 and 443 unless you write `host:port`). `--build` allows it only while a deploy runs. Removes it from `deny` |
| `bunker net deny <host> [project]` | Block a host even when a wildcard allows it; removes it from `allow` and `build` |
| `bunker net rm <host> [project]` | Remove a host from all three lists |
| `bunker net reload [project]` | Validate a hand edit of `bunker.toml`, print what changed since the last policy, and log it |
| `bunker net log [project] [--blocked] [-n N]` | Recent proxy decisions, policy changes and leases from `network.jsonl` (default 50) |
| `bunker version`, `--version`, `-V` | Print the version, commit and build date |

Config lives only in `~/.bunker/<project>/bunker.toml` (`BUNKER_HOME` overrides `~/.bunker`),
never in the repo. Unknown keys and invalid values are errors. Commands that take a project find it
by name, or from the repo you run them in.

Allowlist edits keep the file's comments and order, refuse internal addresses (exit 4) and any
result that would not validate, and print a `+`/`-` diff. Every change is appended to
`~/.bunker/<project>/network.jsonl`. Agents may run `net allow`: the allowlist is a file in your
home directory either way, so the control is that every change is visible, not that the command is
locked.

Secret values reach the VM only on a process's stdin, never on a command line. `shared/.env` is
written `root:app 0640`, with bunker's managed keys (`APP_ENV=production`, `APP_DEBUG=false`,
`APP_URL`, the `DB_*` settings) followed by your `[env]` settings and secrets.

A VM only ever starts through its LaunchAgent, under `sandbox-exec`. A VM started any other way
(a manual `limactl start`) is not confined: `up`, `start` and `restart` report it and exit `5`.

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
