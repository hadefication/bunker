# Bunker — idle hibernation

Date: 2026-11-18
Status: Candidate for a later plan — not brainstormed with the maintainer yet.

Builds on `2026-11-16-bunker-design.md`, which stays the authority. Nothing here is built.

## Why

The mini has 16 GiB and 8 cores. Each VM defaults to 2 GiB and 2 CPUs, so about 3–4 apps fit next
to everything else the mini runs. Most of the planned apps are used a few times a week, and their
VMs hold RAM the rest of the time. Lima has no suspend or pause, only stop and snapshot, but a
stopped VM uses no RAM or CPU. Stopping an idle VM and starting it again on the next request would
let more apps share the mini.

Who it suits:
- **Browser apps used now and then.** A visitor can wait out a cold start behind a page that
  refreshes itself.
- **Not always-on endpoints** such as Life OS's MCP server at `/mcp`. MCP clients handle a slow
  response or an HTML "starting" page poorly, and a scheduled or queued job there must run on time.

Hibernation is per-project opt-in and off by default.

## Goals

- A project with idle hibernation stops its VM after a set time with no inbound requests.
- The next request starts it again, through the same path `bunker start` uses, and is served once
  the VM is ready and confined.
- `bunker status` and `--json` show a hibernating project as its own state, not as stopped or
  broken.
- After a Mac reboot, a hibernating project stays asleep and still wakes on a request.

## Non-goals

- Suspend or memory snapshots. Lima doesn't offer them, and a cold boot keeps the recovery path
  tested.
- Waking on anything but an inbound HTTP request, a deploy or a bunker command.
- Making the cold start fast. It is what `bunker start` costs today.

## Anti-goals

- **A wake that skips the confinement gate.** A woken VM is reached only after the probe says
  confined, exactly as `tunnel-run` requires today. An unconfined result serves an error, logs it,
  and never retries on its own.
- **A wake that can be driven into a boot loop.** Bots, scanners or a client retrying in a tight loop
  must not keep the VM cycling. Wakes are rate-limited, a failed wake backs off, and Cloudflare Access
  in front of a project stops unauthenticated traffic before it reaches the Mac.
- **Internet-facing code with more power than it needs.** Whatever listens for requests while the VM
  is off must not itself be able to start VMs, run `limactl` or edit LaunchAgents.
- Silent loss of queued or scheduled work (see "Workers and the scheduler").

## What a request costs today

A cold `bunker start` runs the vm agent, waits for `Running`, opens both forwards, waits for
cloud-init and the guest's proxy port, then probes confinement. That is tens of seconds on the mini,
to be measured; the waits are bounded at 180 s (`Host::wait_timeout`), and the acceptance harness
allows 90 s after a guest reboot. Cloudflare drops an origin request after about 100 s, so a request
held for a whole cold start can fail at the edge.

## Approaches

- **A. Hold and wait.** A bunker-owned gate listens in front of the web forward. cloudflared points
  at the gate. A request for a hibernating project triggers a wake, the gate holds the connection
  until the VM is ready and confined, then proxies it. Transparent to every client when the boot is
  short. Near the 100 s limit, a held request turns into a Cloudflare error page. A new
  internet-facing listener on the Mac.
- **B. A "starting up" page.** The same gate, but while the VM boots it answers at once: an HTML page
  that refreshes itself every few seconds for browser navigations, and `503` with `Retry-After` for
  everything else. No edge timeout risk and the visitor sees what's happening. A POST from a stale tab
  gets the 503, not the app.
- **C. Manual or scheduled only.** No gate. `bunker stop` and `bunker start`, or an `awake` window in
  config run by a LaunchAgent (for example 07:00–23:00). No new internet-facing code. Outside the
  window the domain shows Cloudflare's tunnel error, and nothing wakes it.

**Recommendation (my call, for the maintainer to confirm):** A and B together in one gate. A browser
navigation (GET, `Accept: text/html`) gets the self-refreshing page at once; any other request is held
up to a bound well under 100 s (about 60 s), then gets `503` and `Retry-After`. Build C's pieces first,
because the gate needs them anyway: the hibernating state, `bunker status`, the worker and scheduler
rules, and how deploy, backup and the maintenance window treat a sleeping VM.

## The gate (approaches A and B)

- One gate per opted-in project, as its own LaunchAgent (`com.bunker.<project>.gate`), not inside the
  egress proxy. The proxy is the egress boundary; inbound internet traffic should not share its
  process.
- The gate listens on `ports.web`, where cloudflared already points. The web forward moves to a new
  allocated port that only the gate connects to. Projects without `[idle]` keep today's layout.
- The gate runs under its own Seatbelt profile: bind its port, connect only to the web forward's port,
  write only a wake file and its log, no exec. It asks for a wake by touching
  `~/.bunker/<project>/wake`; a separate agent with `WatchPaths` on that file runs `bunker wake
  <project>`, which is `bunker start` plus the rate limit. The internet-facing process never starts
  anything itself.
- Before proxying after a wake, the gate checks confinement (or reads a confined marker that `bunker
  wake` writes only after the probe passes). Unconfined: `503`, an alarm in the log, no retry until a
  human runs `bunker start`.
- **Tunnel while asleep.** Today `stop` removes the tunnel first and `tunnel-run` refuses to run for
  a VM that isn't confined. For a hibernating project the tunnel stays up, because it only reaches the
  gate. `tunnel-run` would accept "VM stopped, project hibernating, gate running" as well as
  "confined". That widens the one rule that keeps the domain off a bad VM, so it needs care and tests.
- **Rate limit.** At most a few wakes an hour per project (number to choose). A wake that fails or
  times out starts a back-off during which the gate answers `503` without waking. The gate serves
  `robots.txt` and `favicon.ico` itself and never wakes for them.
- **Access.** Strongly recommended for any project with `[idle]`. Without it, crawlers alone can keep
  a VM awake, which defeats the point. Whether to require it is an open question.

## Idle detection

- **Signal.** With a gate, the gate keeps the last request time and the count of open connections; it
  is in the path and costs nothing extra. Without one (approach C, or before the gate exists), the
  signal is the newest line in nginx's access log, read through `limactl shell` on a timer. Requests
  from bunker itself are excluded either way.
- **Config,** in `bunker.toml`, off when the table is absent:

  ```toml
  [idle]
  stop_after = "30m"       # no inbound request and no open connection for this long
  # awake    = "07:00-23:00"   # approach C: never hibernate inside this window
  ```

  Validated like the rest of the file; unknown keys are errors.
- **Open connections.** A WebSocket or long poll counts as activity until it closes.
- **Who stops it.** The gate (or the idle timer) touches a stop request; the same narrow agent runs a
  hibernating stop, which differs from `bunker stop` only in leaving the tunnel and gate in place and
  recording the state.

## Workers and the scheduler

This is the main correctness cost. A stopped VM runs no queue worker and no scheduler.
- Jobs queued during the last request may not have run when the idle timer fires. Before stopping,
  bunker checks the queue is empty (through artisan in the guest; exact command to choose) and waits
  if not. Delayed jobs due while asleep run late, on the next wake.
- Scheduled tasks are missed, and Laravel's scheduler does not catch up. `[idle]` with
  `[schedule].enabled = true` is refused unless the config says missed runs are acceptable (key name
  to choose), so nobody loses a scheduled task without opting in.

## Deploy, backup, maintenance, status, reboot

- **Deploy** wakes a hibernating VM, deploys, and lets the idle timer put it back to sleep.
- **Nightly backup** has nothing new to back up from a VM that hasn't woken since the last bundle, so
  it can skip it. Otherwise it wakes, backs up, and sleeps again.
- **Security updates** in the maintenance window need the VM running. Either the window wakes every
  hibernating VM, or updates run on the next wake (slower wake, briefly unpatched). Open question.
- **`bunker status`** reports `vm.state: "hibernating"` with `idle: {stop_after, last_request,
  wakes_last_hour, backoff_until}` in `--json`. New error codes are stable, for example
  `wake_rate_limited`, `wake_timeout` and `wake_unconfined`. Every new command has human output and
  `--json`.
- **Reboot.** The hibernating state is recorded under `~/.bunker/<project>/` (written with
  `write_private`). After a Mac reboot the gate and tunnel agents come back, the vm agent doesn't, and
  the first request wakes it. A project that was awake comes back awake and idles down later.
- Nothing is written into the app's repo, and no secret value appears in the gate's logs or pages.

## Open questions for the maintainer

1. Is hibernation worth a new internet-facing listener, or is approach C (manual and scheduled
   windows) enough for now?
2. Is it acceptable for the tunnel to stay up while the VM is off, given that `tunnel-run` would then
   run cloudflared for a project whose VM isn't confined (but isn't running either)?
3. Should `[idle]` require Cloudflare Access on the domain?
4. Which apps would use it? If none run a scheduler or long queues, the worker rules can stay simple.
5. Maintenance window: wake sleeping VMs nightly to patch them, or patch on the next wake?
6. Gate as one agent per project, or one shared gate with a listener per project like the proxy?
7. Default `stop_after` and the wake rate limit.
