# Bunker containment test (plan 1 of the series) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove on the Mac mini that a Lima VM can be confined so that its only route off the guest is
bunker's proxy, even for root inside the guest. Pick which proxy route bunker uses, or stop and hand
the fallback to approach B back to the maintainer.

**Architecture:** A throwaway experiment, driven end to end by an agent with no human at the
terminal. Phases (fixtures, baseline, V1, V2, forward, restart, cleanup) are subcommands of one
driver script. Each phase emits JSON results and exits non-zero on any mismatch with a committed
expectation file. The agent only runs commands and reads exit codes and JSON.

**Tech Stack:** Lima 2.3.0 (Homebrew), `/usr/bin/sandbox-exec`, Ubuntu 24.04 arm64 guest, tinyproxy
(stand-in for bunker's proxy), bash, `jq`, Python 3 `http.server` as listeners.

**Spec:** `docs/superpowers/specs/2026-11-16-bunker-design.md`. See "Containment (host side)",
"How the guest reaches the proxy" and "First task: verify containment".

**The series:** this is plan 1. Later plans get written from this plan's results:
2. core (scaffold, config, VM and proxy, deploy, tunnel, in-app secrets, Life OS live)
3. backups, restore and destinations, plus the maintenance window
4. remote deploy over tailcat
5. outbound credential placeholders
6. v2 publishing

## Rules for the driving agent

- **No interactive commands.**
  - Every `limactl` call passes `--tty=false`.
  - Background processes start with `nohup … &`, write a pid file under `$SPIKE/run/`, and log to
    `$SPIKE/logs/`.
  - Never run a bare `sleep`. Waiting is done with bounded retry loops, such as
    `curl --retry 20 --retry-connrefused --retry-delay 1` or a `for i in $(seq 60)` loop around a
    command that has its own timeout.
- **Judge by exit code and JSON only.**
  - Every phase prints JSON lines to stdout, which `run.sh` also saves to `$SPIKE/results/<phase>.jsonl`.
  - Human-readable notes go to stderr.
  - A phase exits 0 if its results match `$SPIKE/expected/<phase>.json`, 1 on mismatch, and 2 on a
    setup error.
- **Bounded adjustment.** If a confined start fails, the agent may add allow rules to the Seatbelt
  profile, one at a time:
  - only rules whose need appears in `~/.lima/bunker-spike/ha.stderr.log`
  - only for unix sockets under the VM's Lima dir, or for `localhost:60022`
  - at most 3 rules in total
  - every rule recorded in `$SPIKE/results/profile-changes.md`

  Never add a rule that allows a non-loopback IP, or any `network-outbound` to `*`. If 3 rules
  aren't enough, that variant has failed.
- **Hard stops, where the agent ends the run and reports to the maintainer:**
  - any command needs `sudo` (Task 6 is human-gated)
  - Lima < 2.3.0
  - the baseline phase doesn't detect the gap (the probe is broken twice in a row)
  - both V1 and V2 fail
  - any instruction would need reading `.env`, `~/.secrets` or `~/Backups/auth`
- **Idempotent.**
  - `run.sh fixtures up` and `run.sh cleanup` can be run any number of times.
  - Before starting, the agent runs `run.sh cleanup` to clear leftovers from an earlier attempt.
- **No commits.** The agent never runs `git commit` or `git push`. Committing is the maintainer's
  call, asked at the end.

## Global Constraints

- Spike files live in
  `/private/tmp/claude-501/-Users-inggo-AI-bunker/ba7a2e9f-72c1-4402-9ae7-1be815f4a239/scratchpad/spike/`
  (`$SPIKE`). The only repo change is the results section added to the spec in Task 5.
- Lima must be ≥ 2.3.0, with `vmType: vz` and `mounts: []`.
- Fixed values, defined once in `$SPIKE/env.sh` and sourced by every script:

  ```bash
  VM=bunker-spike
  SSH_PORT=60022
  PROXY_PORT=18888
  LOOP_PORT=18080
  ALL_PORT=18081
  WEB_PORT=18100
  RPROXY_PORT=3128
  MAC_LAN=192.168.254.114
  ROUTER=192.168.254.254
  TS_PEER=100.80.217.65
  TS_SELF=100.112.139.61
  LIMA_VM_DIR="$(cd ~/.lima && pwd -P)/$VM"   # symlink-resolved; Seatbelt matches real paths
  ```

  If `ipconfig getifaddr en1` no longer equals `MAC_LAN`, or `route -n get default` no longer
  reports `ROUTER`, update `env.sh` from those commands before running anything.

## Review Focus

- **IPv6 egress:** a guest reaching the internet or the LAN over IPv6 when only IPv4 was tested.
  The probe has `ipv6_internet`.
- **DNS side channel:** the guest resolving names through Lima's resolver (`192.168.5.3`) or host
  mDNSResponder. The probe has `lima_dns` and `system_resolver`.
- **Root undoing guest-side settings:** every confined phase runs the probe twice, the second time as
  root after `--root-tamper`.
- **Port forward under the sandbox:** the `forward` phase checks the SSH `-L` forward and that Lima
  binds nothing but `$SSH_PORT`.
- **Restart dropping confinement:** the `restart` phase checks that an unwrapped start re-opens the
  gap (so bunker must detect it), and checks what a guest reboot does.

---

### Task 1: Driver, probe, comparator and expectations

**Files:**
- Create: `$SPIKE/env.sh` (values above)
- Create: `$SPIKE/probe.sh` (runs inside the guest)
- Create: `$SPIKE/compare.sh`
- Create: `$SPIKE/run.sh`
- Create: `$SPIKE/expected/{baseline,v1,v1-root,v2,v2-root,forward,restart-unwrapped}.json`

**Interfaces:**
- Produces:
  - `probe.sh [--root-tamper] <proxy-url>` (in the guest): one JSON line per check,
    `{"check":"<name>","result":"reachable"|"blocked"}`, then exit 0.
  - `compare.sh <results.jsonl> <expected.json>`: exits 0 when every check named in the expected
    file has the expected result, and 1 otherwise, printing `{"check":…,"expected":…,"got":…}` for
    each mismatch. Checks missing from the expected file are recorded but not compared.
  - `run.sh <phase>`, where phase is one of `fixtures up|down`, `baseline`, `v1`, `v2`, `forward`,
    `restart`, `cleanup`.

- [ ] **Step 1: Write `probe.sh` with these checks (4-second timeout each)**

| check | reachable when (inside guest) |
|---|---|
| `proxy_allowed` | `curl -sS -m4 -x "$PROXY" -o /dev/null -w '%{http_code}' https://example.com` prints `200` |
| `proxy_denied` | the same request to `https://github.com` prints `200` (a 403 or a failure counts as blocked) |
| `direct_tcp_internet` | `nc -z -w4 1.1.1.1 443` succeeds |
| `direct_https_internet` | `curl -sS -m4 -o /dev/null https://1.1.1.1` succeeds |
| `direct_udp_dns` | `dig +short +time=2 +tries=1 @1.1.1.1 example.com` prints an address |
| `lima_dns` | the same query `@192.168.5.3` prints an address |
| `system_resolver` | `getent hosts example.com` succeeds |
| `host_loopback_other_port` | `curl -sS -m4 -o /dev/null http://192.168.5.2:$LOOP_PORT` succeeds |
| `proxy_via_lima_ip` | `nc -z -w4 192.168.5.2 $PROXY_PORT` succeeds |
| `host_lan_ip` | `curl -sS -m4 -o /dev/null http://$MAC_LAN:$ALL_PORT` succeeds |
| `host_lan_ssh` | `nc -z -w4 $MAC_LAN 22` succeeds |
| `router` | `nc -z -w4 $ROUTER 80` succeeds |
| `tailnet_peer` | `nc -z -w4 $TS_PEER 22` succeeds |
| `tailnet_self` | `nc -z -w4 $TS_SELF 22` succeeds |
| `ipv6_internet` | `curl -6 -sS -m4 -o /dev/null 'https://[2606:4700:4700::1111]'` succeeds |
| `icmp_internet` | `ping -c1 -W2 1.1.1.1` succeeds |

The probe takes the `env.sh` values from its environment. `run.sh` passes them with
`limactl shell --tty=false $VM env K=V… bash /tmp/probe.sh …`.

With `--root-tamper` (the probe must run under `sudo`), it first runs:
- `nft flush ruleset`
- `ip route replace default via 192.168.5.2`
- `printf 'nameserver 192.168.5.3\n' > /etc/resolv.conf`

- [ ] **Step 2: Write the expectation files**

- `baseline.json`: `host_loopback_other_port`, `host_lan_ip`, `router` and `direct_tcp_internet`
  must all be `reachable`. This proves the probe sees the gap. `tailnet_peer` isn't listed, so it's
  recorded only.
- `v1.json` and `v1-root.json`: `proxy_allowed` must be `reachable` and `proxy_via_lima_ip`
  `reachable`. Every other check must be `blocked`.
- `v2.json` and `v2-root.json` (the proxy URL is `http://127.0.0.1:$RPROXY_PORT`): `proxy_allowed`
  must be `reachable`, and every other check `blocked`, including `proxy_via_lima_ip`.
- `forward.json`: `{"web_forward":"reachable","lima_extra_listeners":"blocked"}`.
- `restart-unwrapped.json`: `host_lan_ip` must be `reachable`. This proves an unwrapped start loses
  confinement, which bunker must detect.

- [ ] **Step 3: Write `compare.sh`** with `jq`, to the interface above.

- [ ] **Step 4: Self-test the comparator with no VM involved**

Run:
```bash
printf '{"check":"router","result":"blocked"}\n' > /tmp/r.jsonl
echo '{"router":"blocked"}' > /tmp/e.json && $SPIKE/compare.sh /tmp/r.jsonl /tmp/e.json; echo "exit=$?"
echo '{"router":"reachable"}' > /tmp/e.json && $SPIKE/compare.sh /tmp/r.jsonl /tmp/e.json; echo "exit=$?"
```

Expected: `exit=0`, then a mismatch line followed by `exit=1`.

- [ ] **Step 5: Write `run.sh` phases** (bodies follow Tasks 2–5). Each phase sources `env.sh`, writes
  `$SPIKE/results/<phase>.jsonl`, calls `compare.sh` against the matching expectation and exits with
  its code.

---

### Task 2: Fixtures and unconfined baseline (the "red" run)

**Files:**
- Create: `$SPIKE/lima.yaml`, `$SPIKE/tinyproxy.conf`, `$SPIKE/allow.txt`

**Interfaces:**
- Consumes: `run.sh`, `probe.sh` and `compare.sh` from Task 1.
- Produces: a stopped VM `$VM` with its image cached and test tools installed, and
  `results/baseline.jsonl`.

- [ ] **Step 1: Install and check versions**

Run: `brew install lima tinyproxy jq && limactl --version`
Expected: `limactl version 2.3.x` or newer. Lower is a hard stop.

- [ ] **Step 2: Write `lima.yaml`**

- `template://ubuntu-24.04` base, `vmType: vz`, `mounts: []`, `ssh.localPort: 60022`,
  `ssh.forwardAgent: false`
- `containerd: {system: false, user: false}`, `video.display: none`, `rosetta.enabled: false`
- `hostResolver.enabled: false`, `dns: ["127.0.0.1"]`
- `portForwards: [{guestIPMustBeZero: false, guestPortRange: [1, 65535], ignore: true}]`
- a system `provision` script that installs `curl netcat-openbsd dnsutils nftables iputils-ping`
  and copies nothing from the host

- [ ] **Step 3: Write the tinyproxy config**

`Listen 127.0.0.1`, `Port 18888`, `Filter "$SPIKE/allow.txt"`, `FilterDefaultDeny Yes`,
`FilterExtended On`, `ConnectPort 443`, `ConnectPort 80`, `LogFile "$SPIKE/logs/tinyproxy.log"`.
`allow.txt` holds `^example\.com$` and `^ports\.ubuntu\.com$`.

- [ ] **Step 4: Implement `run.sh fixtures up|down`, then run `up`**

`up` starts the following under nohup with pid files, skipping any whose pid is alive:
- `python3 -m http.server --bind 127.0.0.1 $LOOP_PORT`
- `python3 -m http.server --bind 0.0.0.0 $ALL_PORT`
- `tinyproxy -d -c $SPIKE/tinyproxy.conf`

It then waits on `curl --retry 20 --retry-connrefused --retry-delay 1` and verifies the proxy.

Run: `$SPIKE/run.sh fixtures up; echo "exit=$?"`
Expected: `exit=0`. The JSON shows `example.com` returning 200 through the proxy and `github.com`
returning 403.

- [ ] **Step 5: Implement `run.sh baseline`, then run it**

The phase does three things:
1. Starts the VM unconfined with `limactl start --tty=false --name=$VM --set '.dns=[]' $SPIKE/lima.yaml`.
   DNS stays on for this one boot, so provisioning can reach apt, and the phase records that it did
   so. `limactl start` blocks until the VM is ready.
2. Copies the probe in with `limactl copy`, then runs it with proxy URL
   `http://192.168.5.2:$PROXY_PORT`.
3. Stops the VM.

Run: `$SPIKE/run.sh baseline; echo "exit=$?"`
Expected: `exit=0`. If it exits 1, fix `probe.sh` once and re-run. A second exit 1 is a hard stop.

---

### Task 3: Variant V1, host loopback on, Seatbelt allows only proxy and SSH (devvm's route)

**Files:**
- Create: `$SPIKE/v1.sb`

**Interfaces:**
- Consumes: the stopped `$VM` and fixtures.
- Produces: `results/v1.jsonl`, `results/v1-root.jsonl` and `results/apt-v1.json`, then a stopped VM.

- [ ] **Step 1: Generate `v1.sb` from `env.sh`.** It's adapted from devvm's
  `internal/backend/lima/seatbelt.go` (MIT, `NetProxy` plus `NoBinds`):

```
(version 1)
(allow default)
(deny network-outbound)
(allow network-outbound (remote unix-socket (subpath "$LIMA_VM_DIR")))
(deny mach-lookup (global-name "com.apple.dnssd.service"))
(allow network-outbound (remote ip "localhost:$SSH_PORT"))
(allow network-outbound (remote ip "localhost:$PROXY_PORT"))
(deny network-bind)
(allow network-bind (local tcp "localhost:$SSH_PORT"))
(allow network-bind (local unix-socket (subpath "$LIMA_VM_DIR")))
```

- [ ] **Step 2: Implement `run.sh v1`**

The phase runs these steps in order:
1. `limactl edit --tty=false $VM --set '.dns=["127.0.0.1"]'`
2. `/usr/bin/sandbox-exec -f $SPIKE/v1.sb limactl start --tty=false $VM`. If this fails, apply the
   bounded adjustment rule, then retry.
3. Run the probe, then `sudo` the probe with `--root-tamper`.
4. Run the apt check:
   - Run `sudo apt-get -o Acquire::http::Proxy=$P -o Acquire::https::Proxy=$P update` in the guest,
     where `$P` is `http://192.168.5.2:$PROXY_PORT`, and expect success.
   - Remove `ports.ubuntu.com` from `allow.txt`, restart tinyproxy, run the update again and expect
     failure.
   - Restore the line.

   Results go into `apt-v1.json` as `{"apt_allowed":"reachable","apt_denied":"blocked"}`.
5. Stop the VM.

Compare `v1.jsonl` with `v1.json` and `v1-root.jsonl` with `v1-root.json`, and check the apt results.
The phase exits 0 only if all of them match.

- [ ] **Step 3: Run it**

Run: `$SPIKE/run.sh v1; echo "exit=$?"`
Expected: `exit=0`. An exit 1 means V1 failed. Record it and continue to Task 4 either way.

---

### Task 4: Variant V2, `hostLoopback: false` plus a reverse SSH forward; forward and restart checks

**Files:**
- Create: `$SPIKE/v2.sb` (`v1.sb` without the `localhost:$PROXY_PORT` outbound rule)

**Interfaces:**
- Consumes: the stopped `$VM` and fixtures.
- Produces: `results/v2.jsonl`, `results/v2-root.jsonl`, `results/forward.jsonl` and
  `results/restart.jsonl`.

- [ ] **Step 1: Implement and run `run.sh v2`**

The phase:
1. Sets `.networkOpts.hostLoopback=false` with `limactl edit --tty=false`.
2. Starts the VM under `v2.sb`, applying the bounded adjustment rule if needed.
3. Starts the reverse forward under nohup with a pid file. The forward runs outside the sandbox:
   `ssh -F $LIMA_VM_DIR/ssh.config -N -o ExitOnForwardFailure=yes -R 127.0.0.1:$RPROXY_PORT:127.0.0.1:$PROXY_PORT lima-$VM`
4. Waits until `ss -ltn` in the guest shows `127.0.0.1:$RPROXY_PORT`, with a bounded retry of 20.
5. Runs the probe with `http://127.0.0.1:$RPROXY_PORT`, plain and as root with `--root-tamper`.
6. Leaves the VM running for the next step.

Run: `$SPIKE/run.sh v2; echo "exit=$?"`
Expected: `exit=0`. An exit 1 means V2 failed.

- [ ] **Step 2: Implement and run `run.sh forward`, against whichever variant passed (prefer V2)**

The phase:
1. Starts `python3 -m http.server 80` in the guest under `sudo nohup`.
2. Starts `ssh -F … -N -o ExitOnForwardFailure=yes -L 127.0.0.1:$WEB_PORT:127.0.0.1:80 lima-$VM` under
   nohup.
3. Records `web_forward` as reachable if `curl --retry 10 --retry-connrefused -s -o /dev/null -w '%{http_code}' http://127.0.0.1:$WEB_PORT`
   returns 200.
4. Records `lima_extra_listeners` as blocked if `lsof -nP -a -c limactl -iTCP -sTCP:LISTEN` lists no
   port other than `$SSH_PORT`.

Run: `$SPIKE/run.sh forward; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 3: Implement and run `run.sh restart`**

The phase runs two cycles.

Unwrapped cycle:
1. `limactl stop`, then `limactl start` with no sandbox-exec.
2. Run the probe and compare it with `restart-unwrapped.json`.

This cycle is expected to show the gap.

Wrapped cycle:
1. Stop, then start again under the passing variant's profile.
2. Note `ha.pid`, then run `sudo reboot` in the guest.
3. Wait for the guest to answer again, up to 60 tries of `limactl shell --tty=false $VM true`.
4. Record `{"reboot_same_hostagent": true|false}` and re-run the probe.

Expected after reboot: either the same hostagent pid with every check still blocked, or the VM
stopped. Both are acceptable findings. Any `reachable` check other than `proxy_allowed` after the
reboot fails the phase.

Run: `$SPIKE/run.sh restart; echo "exit=$?"`
Expected: `exit=0`.

---

### Task 5: Record results in the spec and clean up

**Files:**
- Modify: `docs/superpowers/specs/2026-11-16-bunker-design.md`. Add
  `## Containment test results (2026-11-16)` directly after "First task: verify containment".

**Interfaces:**
- Consumes: everything in `$SPIKE/results/`.
- Produces: the chosen variant, the exact final Seatbelt profile, the Lima settings, the DNS during
  provisioning note, the `tailnet_peer` baseline result, and the restart findings. Plan 2 is
  written from these.

- [ ] **Step 1: Write the results section**

Include:
- a compact table, one row per check, with a column for each of baseline, V1, V1-root, V2 and V2-root
- the forward and restart results
- `profile-changes.md`, verbatim
- the chosen variant: V2 if both pass, otherwise whichever passed
- if neither passed: a line saying so and that Task 6 needs the maintainer

- [ ] **Step 2: Implement and run `run.sh cleanup`**

The phase:
- deletes the VM with `limactl delete -f $VM`
- kills every pid in `$SPIKE/run/`
- runs `brew uninstall tinyproxy`

It then checks that `limactl list -q` doesn't contain `$VM` and that
`lsof -nP -iTCP:$LOOP_PORT,$ALL_PORT,$WEB_PORT,$PROXY_PORT,$SSH_PORT -sTCP:LISTEN` prints nothing.
Lima and jq stay installed.

Run: `$SPIKE/run.sh cleanup; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 3: Report to the maintainer**

Report the chosen variant, the table, and anything surprising, such as `tailnet_peer` being reachable
unconfined. Then ask whether to commit the spec and the two plans.

---

### Task 6 (human-gated; only if both V1 and V2 failed): fallback B feasibility

This task needs `sudo` to create a macOS user and load a pf anchor. **The agent stops here and asks
the maintainer.** It continues only if the maintainer runs the `sudo` steps themselves (for example by
typing `! sudo …`) or explicitly approves them.

**Files:**
- Create: `$SPIKE/pf-bunker.conf`

- [ ] **Step 1: Write the anchor**

```
block drop out quick proto {tcp udp} from any to {127.0.0.0/8 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 100.64.0.0/10 169.254.0.0/16} user _bunkerspike
```

- [ ] **Step 2: Maintainer runs the setup**

Create user `_bunkerspike`, then load the anchor with
`sudo pfctl -a bunker-spike -f $SPIKE/pf-bunker.conf` and `sudo pfctl -E`. Note the token.

- [ ] **Step 3: Run the probe**

Start the VM as `_bunkerspike` with its own `LIMA_HOME`, unconfined by Seatbelt, and run the probe.
Expected: every private-range check blocked and `direct_tcp_internet` reachable.

- [ ] **Step 4: Record the result in the spec and remove everything**

Maintainer runs `sudo pfctl -a bunker-spike -F all` and `sudo pfctl -X <token>`, then deletes the
user and the VM.
