# Bunker — design

Date: 2026-11-16
Status: approved 2026-11-16

## Intent

Bunker turns the maintainer's Mac mini (Apple M1, 16 GB, macOS 26.4) into a production host for his
personal web projects, published to the internet through Cloudflare named tunnels.

The point is to host on hardware he owns without putting the Mac at risk. Each site runs in its own
Lima VM that holds a copy of the code and every service the site needs. If a site is compromised,
the attacker gets a disposable Linux box. They don't get his home directory, SSH keys, secrets,
the dev databases running on the Mac, his LAN or his tailnet.

Bunker is a standalone project. It has no tie to Yard or any other dev tool. It replaces the earlier
bunker (`~/Code/bunker`, `hadefication/bunker`, v0.1.5), which served the dev working copy with
FrankenPHP as host LaunchAgents and had no isolation. The new binary is also called `bunker` and
replaces the old one at `~/.local/bin/bunker`.

The first tenant is **Life OS** (`~/Code/life-os`): Laravel 13, PHP 8.3+, Filament 5, Passport, an
MCP server at `/mcp`, MySQL with database-driven queue, cache and sessions, one queue worker, served
at `life-os.bangkila.com` behind Cloudflare Access. It starts with fresh data. Nothing gets
imported from the old deployment.

## Goals

- One Lima VM per project, holding the code (copied in, never mounted), its services (MySQL first)
  and its processes. Everything comes back on its own after a reboot of the VM or the Mac.
- Strong containment, enforced on the Mac's side so that root inside the guest can't undo it:
  - The guest can't reach the Mac's loopback, the Mac's LAN address, other LAN devices, the tailnet or
    other VMs.
  - All guest egress goes through a host-side proxy that applies a per-project allowlist and
    denies everything else by default.
  - The Lima host process that handles guest traffic is sandboxed.
- Deploys are explicit. A deploy replaces code only. The database, `storage/` and `.env` persist in
  the VM. Releases switch atomically and can be rolled back.
- Deploy works from the mini itself, and from another of the maintainer's Macs over tailcat.
- Secrets follow devvm's model:
  - The master copy lives in the mini's macOS Keychain.
  - Outbound credentials reach the VM only as placeholders.
  - Only the secrets the app itself computes with are written into the VM.

  No bunker command ever prints a value. The maintainer never wants agents reading `.env`, so
  bunker must not become a way to read one.
- Backups default to the mini's disk. They can also go to pluggable storage destinations (iCloud Drive
  and Google Drive first), through an interface that makes adding more easy.
- Laravel first, with the VM's insides generic enough (a web process, workers, services, hooks) that
  other stacks can be added later without redesign.

## Non-goals (v1)

- Importing data from an existing deployment. Life OS starts fresh.
- Stacks other than Laravel. Don't build them, but don't paint them out either.
- Quick (try) tunnels. Named tunnels only.
- Multi-host, clustering, high availability, or hosting for anyone other than the maintainer.
- A web UI.

## Anti-goals (would make it a failure even if it works)

- Any path from a guest to the Mac's loopback, LAN, tailnet or home directory.
- Anything written into a project's repo. Config, secrets, backups, logs and releases all live
  outside it.
- Network policy that can be widened by changing the repo, or by deploying from another Mac.
- A command that prints a secret value, or logs that leak one.
- Shipping `.env*` files from the working tree into a VM.

## Constraints and findings

- **Lima 2.3.0** (current Homebrew stable) is required. With `vmType: vz` and
  `networkOpts.hostLoopback: false`, the guest can't reach the Mac's `127.0.0.1` through
  `host.lima.internal`, and that's enforced on the host side. It doesn't block the Mac's LAN
  address, other LAN hosts or the tailnet, and Lima has no option that does. That gap is closed by
  the confinement below.
- Lima mounts the host home directory by default. Bunker VMs set `mounts: []`.
- Lima's user-mode networking is a userspace network stack inside the Lima hostagent, running on
  the Mac as the maintainer. Guest connections become real connections made by that process. This
  makes the hostagent the most plausible escape surface, and it's why it gets sandboxed.
- `sandbox-exec` exists on macOS 26.4.1 but Apple has deprecated it. If it ever goes away, the
  fallback is approach B below (a dedicated macOS user plus a firewall rule).
- **devvm** (`github.com/devvmhq/devvm`, MIT, built on Lima) already confines VMs this way on macOS.
  It runs the hostagent under `sandbox-exec` with a profile that allows only the VM's SSH port, a
  proxy on the host's loopback and Lima's unix sockets. The guest's DNS points at nothing, and all
  egress goes through a host proxy that resolves names itself. The proxy always refuses loopback,
  private, link-local, multicast, unspecified and CGNAT (`100.64.0.0/10`) addresses unless a rule
  names the address, and allowing every host doesn't lift that rule. Borrow its approach and, where
  useful, its code. Don't depend on it.
- **tailcat** (`github.com/tailscale/tailcat`, open-sourced by Tailscale on 2026-08-31) is netcat over
  Tailscale's data plane with no account, no root and no route or DNS changes.
  - By default, the `tc…` address alone is enough to connect. `serve --allow=nodekey:…` restricts it
    to listed client keys.
  - Each run generates a new key unless one is saved with `genkey`.
  - It's young and has no published audit. Treat the node-key allowlist as the security boundary.
  - It's a Go tool. Bunker shells out to the `tailcat` binary.
- PHP inside the guest comes from the ondrej PPA, which covers 7.x through 8.5. Homebrew isn't used
  in the guest.
- RAM: about 1.5–2 GB per VM puts the ceiling at roughly 4–5 live sites. One site is planned.
- cloudflared can create a DNS CNAME but can't delete one. Teardown deletes the tunnel and tells the
  user which CNAME to remove.
- `~/.cloudflared/cert.pem` (the Cloudflare login) already exists on the mini.
- Rust hygiene (maintainer's rules):
  - Scaffold with `deny.toml`, copied from `~/AI/skillset/deny.toml`, and
    `.github/workflows/supply-chain.yml`, copied from `~/AI/skillset/.github/workflows/`.
    `cargo deny check` must pass before the first commit.
- CLI rules (maintainer's rules):
  - `--version`, `-V` and a `version` subcommand, all wired to `CARGO_PKG_VERSION` plus the commit and
    build date.
  - Readable labeled output for humans, `--json` with a stable shape plus meaningful exit codes for
    scripts, and real `--help` on every command.

## Approaches considered

All four use `vz`, no mounts and `hostLoopback: false`.

- **A. VMs run as the maintainer, unconfined.** This is the simplest. The LAN, tailnet and host-service
  gap is only covered by a firewall inside the guest, which guest root can disable. A hostagent
  escape lands in his account. Rejected.
- **B. VMs run under a dedicated `bunker` macOS user** via a LaunchDaemon, with a pf rule blocking that
  user's egress to private ranges. An escape lands in an empty account. It needs one-time `sudo`
  setup and a narrow entry point between the CLI and the daemon, and it's unproven whether pf can
  match that user's traffic. **Kept as the fallback** if `sandbox-exec` confinement fails or
  disappears.
- **C. Like B, but with tailcat running inside each VM** so other Macs push straight into the guest.
  Every VM gets its own reachable endpoint, the host can't check what comes in, and more trust goes
  to a young tool. Rejected.
- **D. Chosen: VMs run as the maintainer, devvm-style confinement.**
  - The hostagent runs under a Seatbelt profile and all egress goes through bunker's host-side
    allowlist proxy.
  - cloudflared runs on the Mac, so the VM needs no tunnel egress and never holds tunnel
    credentials.
  - Remote deploys land on the host and go through the same deploy path.

  It closes the LAN gap and boxes in the network program without a second account. The remaining
  escape route is Virtualization.framework, which Apple runs in its own sandboxed process.

Apple's `container` (one VM per container) was dropped. The maintainer chose Lima, and Lima now has
what's needed.

## Design

### Layout on the Mac

```
~/.bunker/
  <project>/
    bunker.toml          # the project's config and network policy (the only config file)
    cloudflared.yml      # generated
    sandbox.sb           # generated Seatbelt profile for this VM's hostagent
    lima.yaml            # generated Lima template
    network.jsonl        # proxy decisions (allowed and blocked)
    backups/             # local destination: age-encrypted nightly bundles, mode 0600
    logs/                # host-side logs: proxy, cloudflared, bunker
  remote/                # tailcat server key and allowed client keys (see Remote deploy)
```

The Lima instance is named `bunker-<project>`. Whether `LIMA_HOME` is bunker-private is the
builder's call, as long as `limactl list` makes it clear which VMs belong to bunker.

### Config: `~/.bunker/<project>/bunker.toml`

Config never lives in the repo. Keeping the network allowlist outside the repo means a commit, a
dependency or an agent editing the project can't widen it. Remote deploys send code, never config.

```toml
project    = "/Users/inggo/Code/life-os"   # source repo on the mini
stack      = "laravel"
php        = "8.4"
extensions = ["intl", "bcmath"]
domain     = "life-os.bangkila.com"
tunnel     = "life-os"

[services]
mysql = "8.4"

[workers]
queue = "php artisan queue:work --tries=3 --timeout=30"

[schedule]
enabled = false          # Laravel scheduler; Life OS has no scheduled tasks yet

[hooks]                  # run inside the VM, in the new release, as the app user
first_deploy  = ["php artisan passport:keys"]
before_switch = ["php artisan migrate --force"]

[vm]
memory = "2GiB"
cpus   = 2
disk   = "20GiB"

[network]
build = []   # extra hosts allowed during a deploy, on top of bunker's built-in list
allow = []   # hosts the running site may reach; edited by `bunker net allow`
```

The exact keys are the builder's to refine. These requirements are fixed:
- the file lives only under `~/.bunker/<project>/`
- input is validated, and unknown keys are errors
- `bunker net allow/deny` edit this file

**Allowlist edits** (designer's call, agreed 2026-11-16):
- `bunker net allow <host>` adds to `[network].allow`; `--build` adds to `[network].build` instead.
  `deny` and `rm` edit the same lists.
- The edit is format-preserving (`toml_edit`), so comments and key order survive, and atomic (write a
  temp file, then rename).
- The proxy reloads the allowlist at once. No redeploy and no VM restart. A file that fails
  validation leaves the proxy blocking everything, which is the existing fail-closed rule.
- Editing the file by hand is equally valid. `bunker net reload` applies a hand edit.
- Every change, from the command or a reload, prints the before/after diff and is recorded in
  `network.jsonl`.
- **Agents may run `net allow`.** The allowlist is a plain file in the maintainer's home directory,
  so gating the command would not stop an agent that can write files. The CLI therefore does not
  pretend to be a lock. The control is visibility: the printed diff and the log entry. Moving the
  allowlist somewhere agents cannot write (the Keychain, a root-owned file) was considered and set
  aside for the friction it adds.

### Inside the VM

- Ubuntu 24.04 LTS cloud image.
- **Security updates in a nightly maintenance window** (designer's call, settled after review):
  - The host's nightly job, which also runs the backup, opens the OS update hosts on the proxy.
    These are the Ubuntu security and updates mirrors, the ondrej PPA, the Percona repo (MySQL)
    and NodeSource.
  - It then runs `unattended-upgrade` in the guest through `limactl shell` and closes the update
    hosts again.
  - If the guest reports `/var/run/reboot-required`, the VM restarts inside the window. That's a few
    seconds of downtime at night, which is acceptable for personal sites. It can be turned off per
    project, in which case `bunker status` flags the pending reboot.
  - Outside the window, the update hosts are closed like everything else.
- nginx in front of php-fpm (ondrej PPA, version from config). nginx was chosen over FrankenPHP
  because it supports PHP 7.x and can front non-PHP stacks later.
- MySQL from the config. Oracle's apt repo has no arm64 packages for Ubuntu 24.04 and Ubuntu ships only
  8.0, so `mysql = "8.4"` installs Percona Server for MySQL 8.4 LTS, a drop-in MySQL 8.4, from
  `repo.percona.com` (maintainer's choice, plan 2 Task 11). Its telemetry agent is a hard package
  dependency; bunker masks it before install and disables the server's telemetry. Bunker generates
  the database password at provision time, writes it into the VM's `.env`, and never shows it.
- systemd units for nginx, php-fpm, MySQL, each worker and the scheduler (when enabled), with
  logrotate. Everything is enabled to start on boot.
- Layout:

  ```
  /srv/app/releases/<timestamp>-<sha>/
  /srv/app/shared/.env
  /srv/app/shared/storage/
  /srv/app/current -> releases/…
  ```

- Privileges:
  - PHP runs as an unprivileged `app` user with no `sudo`.
  - Release directories are owned by a separate deploy user and read-only to `app`. Only
    `shared/storage` and `bootstrap/cache` (per release) are writable by it.
  - `.env` is `root:app 0640`.
- nginx carries over old bunker's hardened headers: `X-Content-Type-Options`, `X-Frame-Options`,
  `Referrer-Policy` and `Permissions-Policy`, with `server_tokens off` and `X-Powered-By` stripped.
  It sets the real client IP from `CF-Connecting-IP`, trusted only from the port-forward source.
- The guest has no DNS: provisioning disables systemd-resolved and points `/etc/resolv.conf` at a
  port nothing listens on, so a lookup fails at once. The proxy resolves every host. (With resolved
  left on, lookups waited out timeouts on the Lima gateway, which stalled every php-fpm start 17 s
  on the ondrej packages' telemetry lookup, and the Mac's DNS search domain leaked in. Found by the
  plan 2 acceptance run.)
- A guest-side nftables firewall that drops private and loopback-of-host destinations is defense in
  depth. Most web exploits land as `app`, not root. The host-side confinement is the real boundary.

### Containment (host side)

This section was updated by the containment test. Its results section below is the evidence.

- **Lima template:**
  - `vmType: vz`, `mounts: []`, `networkOpts.hostLoopback: false`
  - `hostResolver.enabled: false`, `dns: ["127.0.0.1"]`, `propagateProxyEnv: false`, a fixed
    `ssh.localPort`
  - devvm's three ignore-everything `portForwards` rules, so Lima forwards nothing itself
  - no Rosetta, no GUI, audio or clipboard
- **Sandboxed hostagent:** `limactl start` runs under `sandbox-exec` with a generated profile. Its
  network rules allow only:
  - connections to Lima's unix sockets in the VM's dir
  - connections to and listening on the VM's SSH port
  - listening on unix sockets in the VM's dir

  It denies the DNS-SD mach service. It has **no** proxy rule and no web-port rule. It starts from
  `(allow default)`; plan 2 added file-write and exec restrictions (see the hardening note under the
  threat model). See the threat model in the results
  section.
- **Forwards:** bunker opens two forwards per project. Each one is its own `ssh` process under its
  own Seatbelt profile, using Lima's ssh config with `ControlMaster=no`, `ControlPath=none`,
  `ForwardAgent=no`, `ForwardX11=no` and `PermitLocalCommand=no`, and each is supervised so it
  reconnects after a guest reboot.
  - **Proxy:** `-R` guest `127.0.0.1:<port>` to the host proxy. Its profile allows outbound only to
    the SSH port and the proxy port.
  - **Web:** `-L` host `127.0.0.1:<per-project port>` to guest `:80`, for cloudflared. Its profile
    allows outbound only to the SSH port, and binding only on the web port.
- **Wrapped starts only:** bunker always starts VMs through the wrapper. It records the hostagent pid
  from `ha.pid` and flags a VM running on any other hostagent, for example after a manual
  `limactl start`, which the test showed reopens the LAN, the tailnet and the internet.
- **Guest DNS** points at an address with nothing listening. `HTTP(S)_PROXY` is set system-wide plus
  in apt, Composer and npm config.
- **How the guest reaches the proxy** (designer's call, settled after review). The requirement is that
  the guest reaches nothing on the host except its proxy port. `hostLoopback: false` is a means, not
  the requirement. The proxy lives on host loopback, and `hostLoopback: false` would block it at
  `192.168.5.2`, so the spike tries two ways in order:
  1. Keep `hostLoopback: false` and expose the proxy through a Lima-managed channel, such as a
     reverse forward or a unix socket bridged into the guest.
  2. Leave host loopback on and rely on the Seatbelt profile, which lets the hostagent connect only
     to the proxy port on loopback. This is how devvm does it.

  Either one passes the spike. The spike fails, and approach B takes over, only if the Seatbelt
  profile can't hold the "proxy only" line. **Outcome: route 1 (V2), using an SSH reverse forward.**
  Route 2 (V1) also passed and stays the fallback.
- **Proxy:** bunker's own, written in Rust, as a LaunchAgent with one listener per project.
  - Handles HTTP CONNECT and plain HTTP, resolves names itself and connects only to addresses it
    checked.
  - Always refuses internal ranges (loopback, RFC 1918, link-local, multicast, unspecified,
    CGNAT 100.64/10, IPv6 ULA/link-local, NAT64-embedded internal IPv4), including the Mac's own
    addresses.
  - Allows hosts from:
    - bunker's built-in build list (Ubuntu mirrors, the ondrej PPA, Packagist, GitHub and its
      codeload and objects hosts, the npm registry), during provision and deploy only
    - `[network].build` during deploys
    - OS update hosts during the nightly maintenance window only
    - `[network].allow` at all times
  - Blocked requests get a 403 naming the host and the `bunker net allow` command that would permit
    it. Every decision is logged to `network.jsonl`. A broken policy file fails closed.

### Edge: Cloudflare tunnel on the host

- cloudflared runs on the Mac as a LaunchAgent per project, with `ingress` pointing the domain at
  `http://127.0.0.1:<per-project port>`. Tunnel credentials never enter the VM.
- Tunnel create, route and teardown carry over from old bunker (`~/Code/bunker/src/commands/`).
  Yard's tunnel design (`~/AI/yard/docs/superpowers/specs/2026-10-21-yard-tunnels-design.md`) covers
  per-domain certificates, tracking leftover DNS records, and the real-IP header. Copy ideas or code,
  don't depend on Yard.
- For Life OS, the hostname `life-os.bangkila.com` and its Cloudflare Access app stay as they are.
  The CNAME has to point at whichever tunnel bunker uses (a reused `life-os` or a new one), which
  needs `route dns` with overwrite or a manual DNS change.

### Deploy

`bunker deploy [<project>]` on the mini:

1. **Source:** `git archive` of the committed `HEAD` of `project`. It refuses a dirty working tree
   unless `--dirty` is given, in which case tracked files from the working tree are used. Either
   way, untracked and gitignored files never ship, so `.env*`, `vendor` and `node_modules` stay out.
   On top of that, bunker drops any `.env*` path from the archive. The release records the commit
   SHA.
2. Copy into `releases/<timestamp>-<sha>/` over the confined channel (`limactl copy` or a tar stream
   over `limactl shell`).
3. Open the build allowlist, then run `composer install --no-dev --optimize-autoloader` and, if
   `package.json` has a `build` script, `npm ci && npm run build`. Close the build allowlist.
4. Run `composer audit` and `npm audit --omit=dev`, and refuse on advisories unless `--allow-advisories`
   is given.
5. Link `shared/.env` and `shared/storage`, then run `config:cache`, `route:cache` and `view:cache`.
   This is safe now that the VM has its own copy of the code.
6. On the first deploy only:
   - generate `APP_KEY` and the Passport keypair into the Keychain, if they're absent
   - write `shared/.env` from the Keychain and `bunker.toml`, including `APP_ENV=production`,
     `APP_DEBUG=false` and the `DB_*` values
   - run the `first_deploy` hooks

   For Life OS, `passport:keys` is unnecessary once its keys come from `.env`. The builder adjusts
   the example hook.
7. Run the `before_switch` hooks (migrations). If any step fails, the old release stays live.
8. Flip the `current` symlink atomically, then reload php-fpm and restart the workers.
9. Keep the last 5 releases.

`bunker rollback` flips `current` to the previous release and restarts the workers. It doesn't
undo migrations, and says so.

### Remote deploy over tailcat

- **On the mini:** `bunker remote enable` saves a tailcat server key under `~/.bunker/remote/` and runs
  `tailcat serve` as a LaunchAgent with `--allow=nodekey:…` listing only registered client keys. It
  never runs without an allowlist. `bunker remote allow <nodekey>` and `bunker remote revoke` manage
  that list.
- **On another Mac:** `bunker remote add <name> <tc-address>` stores the address and generates a
  client key with `tailcat genkey --client`, whose node key gets registered on the mini.
  `bunker deploy <project> --remote <name>` runs from the repo: it runs `git archive` locally and
  streams the archive to the mini.
- The mini accepts exactly one action per connection: receive an archive for a named, existing
  project and run the normal deploy (steps 2–9) on it. It's a narrow, validated receiver, not a
  shell. It can't change config, the network policy or secrets. Whether this is built on
  `serve exec`, on `serve ssh` with a forced command, or on another tailcat mode is the builder's
  call, as long as these properties hold.
- Output streams back to the remote caller. A remote deploy can't see secrets either.

### Secrets (modelled on devvm)

The master copy of every secret lives in the mini's **macOS Keychain**, one item per project and key.
The items are created with an access rule that lets only the bunker binary read them without a
prompt, so any other process, including an agent running `security find-generic-password`, triggers a
macOS approval prompt. The rule survives bunker updates because releases are signed with a
Developer ID: the Keychain trusts the signing identity (the designated requirement), not one build's
hash. Verified on the mini (plan 2, Task 10): a signed bunker stored an item, `security` was refused
behind a prompt, and a rebuilt binary with a different CDHash, re-signed with the same identity, read
the item without one. An unsigned or ad-hoc signed build gets a prompt after every update. bunker
replaces an item by deleting and re-adding it, and finds it without reading its value, so an item
another program planted (trusting itself) never keeps its access and never causes a prompt.

There are two kinds of secret:

- **Outbound credentials** are values the app only sends to an outside service, such as a mail
  provider API key or an API token. These follow devvm's placeholder model:
  - The VM's `.env` holds a placeholder that ends in a random value unique to the VM.
  - The proxy swaps in the real value, and only on requests to the host the secret is bound to. It
    intercepts TLS for those hosts with a per-VM CA that the guest trusts and the host never does.

  A compromised site can't steal these credentials. It can only use them against the bound host
  while the attacker is still in. Life OS has none of these today. The mechanism is in v1 scope, but
  the builder may schedule it after the Life OS cutover.
- **In-app secrets** are values the app computes with: `APP_KEY`, the Passport keys (loaded from
  `.env` as `PASSPORT_PRIVATE_KEY` and `PASSPORT_PUBLIC_KEY`, so they don't sit in `storage/`), the DB
  password and session or signing keys. These have to be real values inside the VM.
  - Bunker generates them or accepts them into the Keychain.
  - It writes them into the VM's `shared/.env` on first deploy, after `bunker restore`, and whenever
    they change.
  - The Keychain stays the master copy, so losing the VM never loses them.

Commands:
- `bunker env set KEY` reads the value from the TTY without echoing it, or from stdin with `--stdin`,
  and stores it in the Keychain.
  - `--bind <host>` marks it as an outbound credential, so the VM gets a placeholder.
  - Without `--bind`, it's an in-app secret written into the VM.
- `bunker env unset KEY` removes it.
- `bunker env list` prints key names and their kind (bound host or in-app), never values.
- Non-secret settings such as `MAIL_MAILER` or `APP_URL` use the same command with `--plain`. They're
  stored in `bunker.toml` instead of the Keychain and are still never printed in bulk.

Rules:
- No command prints a value. Nothing in `--json` output contains one, and values are scrubbed from
  bunker's own logs.
- After any change, bunker rewrites `shared/.env`, re-caches config and restarts the workers.

### Backups

**What's backed up** is the data, not the VM. The VM can always be rebuilt from `bunker.toml` and
the repo with `bunker up`, then `bunker restore`, then `bunker deploy`. Backing up the VM image was
rejected: it's GBs instead of MBs, copying it live gives an inconsistent MySQL, a restored image
brings back stale packages, and rebuilding as the recovery path keeps that path tested.

Each backup is one **bundle** per project per run, holding:
- `mysqldump --single-transaction` of each database
- a tarball of `shared/storage`
- an export of the project's Keychain secrets, so an off-mini copy can be restored even if the mini
  and its Keychain are gone

The host pulls the dump and tarball out through `limactl shell`, because the guest can't reach the
host. Every bundle is encrypted with **age** before it's written anywhere, including the local disk,
so all destinations hold identical encrypted files.

**Backup key and recovery.**
- `bunker backup init` generates an age identity and stores it in the Keychain.
- It shows a recovery key once, on the TTY only. It refuses to run if stdin or stdout isn't a TTY, and
  never shows the key in `--json`. The maintainer saves it in a password manager.
- Without that recovery key, off-mini copies can't be decrypted after the mini is lost. The command
  says so plainly.
- This one-time display is the only exception to "never print a secret", and it exists because the
  human is the escrow.

**Destinations.** These form a pluggable interface, a Rust trait along the lines of
`Destination { put, list, get, prune }`:
- **`local`** is the default and always on: `~/.bunker/<project>/backups/`, mode 0600.
- **`folder`** writes into any folder that a sync client or mount handles. That covers:
  - **iCloud Drive**, at `~/Library/Mobile Documents/com~apple~CloudDocs/…`, which is present on the
    mini
  - **Google Drive for desktop**, at `~/Library/CloudStorage/GoogleDrive-…/…`, which isn't installed
    on the mini yet
  - Dropbox, external drives and NAS mounts

  Bunker can only confirm the file landed in the folder, not that it finished uploading.
  `bunker status` says so.
- **Native API destinations** such as Google Drive via its API, S3-compatible stores or an rclone
  wrapper come later through the same trait. Adding one must not touch the backup pipeline.

Destinations are defined once in `~/.bunker/backup.toml`, each with its own retention, and projects
pick from them:

```toml
# ~/.bunker/backup.toml
[destinations.icloud]
kind = "folder"
path = "~/Library/Mobile Documents/com~apple~CloudDocs/Bunker"
keep = 30

# ~/.bunker/<project>/bunker.toml
[backup]
time         = "03:00"          # nightly, together with the maintenance window
keep         = 7                # local retention
destinations = ["icloud"]       # local is implicit
```

Commands:
- `bunker backup <project>` runs one now.
- `bunker backup list <project>` lists bundles in every destination.
- `bunker restore <project> [<bundle>] [--from <destination>]` decrypts the bundle, reloads the
  database and storage, writes the secrets back into the Keychain if they're missing, and re-injects
  them into the VM. It works on a fresh VM, and it's in v1, because a backup you can't restore isn't
  a backup. Restoring onto a new mini needs the recovery key.
- A failed backup or destination write shows in `bunker status` and the logs. A notification channel
  is the builder's call.

### Logs

- `bunker logs [<project>] [nginx|php|worker|app|proxy|tunnel]` tails the guest logs (nginx, php-fpm,
  workers, `storage/logs`) and the host logs (proxy, cloudflared).
- Output passes through old bunker's secret scrubber. Logs are rotated on both sides.

### CLI surface (initial)

```
bunker init <project> --path <repo>     write ~/.bunker/<project>/bunker.toml (detects Laravel, PHP constraint)
bunker up <project>                     create/provision the VM (idempotent; re-run after config changes)
bunker deploy [<project>] [--remote <name>] [--dirty] [--allow-advisories]
bunker rollback <project>
bunker status [<project>]               VM, services, tunnel, proxy, confinement state, current release
bunker logs ...
bunker env set|unset|list ...
bunker net allow|deny|rm [--build] <host> | net reload | net log [--blocked]
bunker backup init | backup [list] <project> | restore <project> [<bundle>] [--from <dest>]
bunker artisan <project> -- <args>      run artisan in the current release as the app user
bunker shell <project>                  interactive shell in the VM (for the human; see note)
bunker start|stop|restart <project>
bunker destroy <project>                delete VM, tunnel, LaunchAgents; print the CNAME to remove by hand
bunker remote enable|allow|revoke|add ...
bunker version / --version / -V
```

Every command has human output by default and `--json` for scripts. `bunker shell` and
`bunker artisan` can reach `.env` inside the VM, because they're operator tools. The maintainer's
"no agent reads `.env`" rule is enforced by agent policy, not by bunker. Bunker's own output never
contains values.

### Reboot and lifecycle

- LaunchAgents start the proxy, then the confined VMs, cloudflared, the tailcat server and the
  backup schedule, all with `RunAtLoad` and `KeepAlive`.
- launchd has no ordering, so each piece retries until what it needs is up: the forwards exit on a
  failed forward (`ExitOnForwardFailure`) and launchd restarts them after `ThrottleInterval`. The
  tunnel agent runs `bunker tunnel-run <project>`, which starts cloudflared only once the confinement
  probe says the VM is confined; until then it exits and launchd retries, so after a reboot the
  domain reaches the Mac only when the VM is back and confined (plan 2, Task 15).
- The agents run the installed binary by its real path. `up` warns when that path is a build
  artifact under `target/`.
- The proxy rotates `network.jsonl` and every host log under `~/.bunker` at 10 MiB, keeping five
  copies. Files bunker opens per write (`network.jsonl`, `proxy.log`) rotate by rename, and the newest policy event (also kept in `policy-state.json`, so rotation never reads the log) is carried into the fresh `network.jsonl`, so a flood of blocked requests can't erase the policy state. `network.jsonl` also rotates on the append that crosses 10 MiB, so a flood can't grow it between ticks; agents' stdout and stderr, which launchd holds open, are copied and truncated in place. Guest logs rotate
  with logrotate.
- Like old bunker, this needs the maintainer logged in after the Mac reboots. A LaunchDaemon variant
  comes with fallback B.

## First task: verify containment

Before building on it, prove on the mini that:

1. Lima 2.3 `vz` with `mounts: []` boots Ubuntu 24.04, and the guest reaches the proxy by one of the
   two routes under "How the guest reaches the proxy".
2. With the hostagent under a devvm-style Seatbelt profile and guest DNS disabled, the guest:
   - can reach an allowed public host only through the proxy
   - can't reach the Mac's loopback, the Mac's LAN IP, the router, a tailnet `100.x` address or the
     internet directly
   - still can't, as root, after flushing guest firewall rules and changing routes or DNS
3. The web port forward still works under the profile.

If step 2 can't be made to hold, stop and switch to approach B before building further.

## Containment test results (2026-11-16)

Run on the mini (macOS 26.4.1, Lima 2.3.0, Ubuntu 24.04 arm64, `vz`, `mounts: []`), following
`docs/superpowers/plans/2026-11-16-bunker-containment-spike.md`. tinyproxy stood in for bunker's
proxy, allowlisting `example.com` and `ports.ubuntu.com`. An independent review found gaps in the
first run. A second run (the fix pass) added controls and the missing checks, and the numbers below
come from it. **Containment holds for the tested threat. Approach B isn't needed.**

**Threat model tested.** An attacker has root inside the guest but no exploit for the Lima hostagent
or for Virtualization.framework. The Seatbelt profile restricts only the hostagent's *networking*. It
starts from `(allow default)`, so a hostagent code-execution bug, for instance from guest-controlled
frames, DHCP or DNS, would still get the maintainer's file and process access. Narrowing the profile
beyond networking (file writes only under the VM dir and logs, process exec only for ssh, fewer mach
services) is plan 2 work, as long as Lima still runs under it.

**Plan 2 hardening (as built, 2026-11-16).** The hostagent profile now also denies file writes except
under the VM dir, `/dev/null`, `/dev/tty*` and `/dev/ptmx`, and even inside the VM dir it can't
rewrite `lima.yaml`. Lima's `_config` (which holds `override.yaml` and `default.yaml`) is not
writable, so a compromised hostagent can't add a mount for its next launchd restart; the mount would
be served by Virtualization.framework's own process, outside this sandbox. Its TMPDIR is `<vm dir>/tmp`.
It denies exec except for `limactl` (the symlink and its Cellar path), `/usr/bin/ssh`,
`/usr/bin/sw_vers`, `/usr/sbin/system_profiler`, `/bin/sh` and `/bin/bash`. The last four were the
bounded additions Lima 2.3.0 needed to start (`sw_vers` and `system_profiler` for the vz driver,
`sh`/`bash` for its SSH readiness script); children inherit the exec rule, so the shells can still
run only those binaries. File reads and mach services are not narrowed.

**Controls.** A check counts only if it reads `open` unconfined. Rows with no control prove nothing
and are marked that way.

| check | unconfined (control) | V2 | V2 as root |
|---|---|---|---|
| allowed host via proxy | open | open | open |
| denied host via proxy (CONNECT) | – | – | – |
| direct TCP / HTTPS to internet | open | – | – |
| direct UDP DNS to 1.1.1.1 | open | – | – |
| ICMP to internet | open | – | – |
| Mac loopback, other port, via 192.168.5.2 | open | – | – |
| proxy port via 192.168.5.2 | open | – | – |
| Mac LAN IP (HTTP; SSH :22) | open | – | – |
| router | open | – | – |
| tailnet peer and the mini's own tailnet IP | open | – | – |
| external DNS via the hostagent's resolver at 192.168.5.2, UDP and TCP | open | – | – |
| TCP ports open on the gateway 192.168.5.2 (full 1–65535 scan) | 52 (every Mac loopback listener, incl. MySQL 3306, Redis 6379, Mailpit 8025/8026, VNC 5900) | 53 only | n/a |
| hostagent internal DNS zone (`host.lima.internal`) | answers | answers | n/a |
| Lima services API on gateway:80, expose requests, host binds | not present | not present | n/a |
| vsock to host CID 2, ports 1–65535 | none | none | n/a |
| NICs besides `lo` | 1 (`eth0`) | 1 | n/a |
| 192.168.5.3, DNS and ports 22/53/80/443/2222/8080 | no control (never answered) | – | – |
| IPv6 to internet | no control (no IPv6 even unconfined) | – | – |

The probe-plus-extended-checks ran as root in V2, so the extended rows are root results. Port 53 on
the gateway in V2 is the hostagent's in-process DNS. It answers Lima's internal zone but resolves
nothing external. "As root" means the probe ran under `sudo` after:
- flushing nftables, which emptied 12 rules
- re-setting the default route, which was already via 192.168.5.2, so that was close to a no-op
- pointing resolv.conf at 192.168.5.3, which never answers

Changing guest settings can't matter, because enforcement is entirely on the host. What root added
were the gateway scan, the vsock scan and the expose attempts.

**apt** worked through the proxy for the allowlisted mirror under V2. With the mirror removed from
the allowlist, apt failed, and tinyproxy logged 4 `filtered domain "ports.ubuntu.com"` refusals. That
confirms plain-HTTP filtering. (The first run's "denied" result was really the proxy being down mid-restart.)

**Forwards.**
- The reverse proxy forward and the web forward each ran as an `ssh` under their own Seatbelt profile.
  Both worked: the web forward returned HTTP 200 on host `127.0.0.1:18100`.
- `limactl` listened on nothing but the SSH port, TCP or UDP.

**Reboot.**
- After `systemctl reboot` in the guest, the boot ID changed and SSH was seen going down.
- The VM came back on the same sandboxed hostagent (same `ha.pid`).
- Both forwards dropped. After the reverse forward reconnected, only the allowed host was reachable.

**Unwrapped start.** A plain `limactl start`, without the wrapper, reopened direct internet, DNS,
ICMP, the Mac's LAN IP and SSH, the router, and both tailnet addresses, even with
`hostLoopback: false`. Only Mac loopback stayed blocked, thanks to `hostLoopback: false`.

**V1** (devvm's route: host loopback on, the profile allows the proxy port) passed all the first-run
checks, apt included. It wasn't re-run in the fix pass, so it only carries first-run evidence: no
DNS-resolver, gateway-scan, vsock or reboot-proof checks. It stays the fallback.

The final hostagent profile needed no rules beyond the template:

```
(version 1)
(allow default)
(deny network-outbound)
(allow network-outbound (remote unix-socket (subpath "<realpath ~/.lima>/<vm>")))
(deny mach-lookup (global-name "com.apple.dnssd.service"))
(allow network-outbound (remote ip "localhost:<ssh port>"))
(deny network-bind)
(allow network-bind (local tcp "localhost:<ssh port>"))
(allow network-bind (local unix-socket (subpath "<realpath ~/.lima>/<vm>")))
```

**Findings plan 2 must carry:**
- **The gap was real and serious.** Unconfined, the guest reached `webteractiveglen` over Tailscale,
  the mini's SSH on its LAN IP, and every service on the Mac's loopback, including Yard's MySQL and
  Redis.
- **Lima's SSH multiplexing master inherits the sandbox.** Lima's `ssh.config` uses
  `ControlMaster auto`, and the master is spawned by the sandboxed `limactl start`. A forward
  multiplexed onto it runs sandboxed. That was the first V2 run's failure, observed live as
  `proxy_allowed` blocked, then 200 with a standalone connection. Its raw output was overwritten, so
  that run's evidence is the ledger note. Bunker's forwards use `ControlMaster=no` and
  `ControlPath=none`.
- **Forwards drop on guest reboot** and must be supervised so they reconnect.
- **Every VM start must go through the wrapper,** and `bunker status` must flag a VM that didn't.
  `ha.pid` comparison detects a swapped hostagent, not confinement itself. Checking the pid with
  libsandbox's `sandbox_check` is worth trying.
- **Provisioning ran unconfined here,** with DNS on. Bunker must provision through the proxy with the
  build allowlist from the first boot. That's untested.
- **The guest can't resolve its own hostname without DNS** (`sudo: unable to resolve host`). Bunker
  writes the hostname into `/etc/hosts`.
- **The real proxy needs its own tests:** CONNECT and absolute-URI requests to IP literals, to
  internal ranges, to the Mac's LAN and tailnet IPs, and to names resolving to internal addresses.
- **Harness rule for plan 2's tests:** every check expected to be blocked under confinement needs a
  passing unconfined control. Logs append rather than truncate, and a failing run's output is kept.

## Left to the builder

- Crate layout, module structure and which crates to use (subject to `cargo deny`).
- Exact `bunker.toml` key names beyond what's fixed above, and the error messages.
- How the proxy picks per-project ports, and the `LIMA_HOME` location.
- The tailcat mode used for the remote receiver, as long as the stated properties hold.
- Exact contents of the Seatbelt profile and the nginx config.
- The VM provisioning mechanism (cloud-init, a script over `limactl shell`, or both), as long as it's
  idempotent.

## Publishing

- Same repo, `hadefication/bunker`, released as **v2.0.0**. This is the maintainer's decision.
- v2 is developed on a `v2` branch cut from the v0.1.5 `main`, so `main` and its releases stay
  untouched until v2 ships.
- At release, a `v1` branch is created at the old `main`, then `main` fast-forwards to `v2`. Because
  `v2` descends from the old `main`, that's a fast-forward, not a merge, and the v0.1.x tags stay on
  `main`'s history.
- Old installs that run `self_update` get v2. That's acceptable because nothing runs on old bunker
  now: the old Life OS tunnel is already down.
- v2 must recognise an old-layout `~/.bunker/<project>/` (a Caddyfile and no `bunker.toml`), for
  example on the maintainer's other Mac, and ignore it with a one-line notice instead of
  misreading it.
- v2 is installed on the other Macs too, as the remote-deploy client.

## Changes outside this repo

- Life OS's `CLAUDE.md` "Production Runtime (Bunker)" and "Bunker Commands" sections describe old
  bunker and become stale. They get updated when Life OS moves over. Its "No deployment config lives
  in this repo" line stays true.
