# Acceptance on the Mac mini

`run.sh` proves, on the real machine, that bunker's containment and lifecycle hold end to end
against a disposable Laravel fixture. It never touches the real `~/.bunker` (it uses
`BUNKER_HOME=$ACC/home`), the login Keychain (a throwaway keychain under `$ACC`), or a real
`com.bunker.proxy` agent (it refuses to run if one is installed, since launchd labels are shared).

```
tests/acceptance/run.sh all              # every phase in order, then cleanup (about 30 minutes)
tests/acceptance/run.sh <phase>          # fixture up deploy controls probe proxy roundtrip
                                         # reboot unwrapped rollback, each on its own
tests/acceptance/run.sh cleanup --final  # destroy the fixture VM and remove $ACC
```

Each phase exits 0 on a match, 1 on a mismatch and 2 on a setup error. `all` stops at the first
failure and leaves the VM for inspection; run `cleanup` afterwards. A probe or proxy mismatch is a
hard stop: containment did not hold.

- `env.sh`: the mini's addresses from plan 1 (LAN, router, tailnet peer and self) and the paths.
  Override any of them in the environment. `IMAGE_CACHE` names a directory of already verified
  Ubuntu images to clone instead of downloading.
- `probe.sh`, `probe2.sh`: plan 1's guest checks, one JSON line each:
  `{"check","result":"reachable"|"blocked","detail"}`. `--root-tamper` makes root in the guest
  flush the firewall and rewrite routes and DNS first; the host side must still hold.
- `expected/`: what each run must show. Everything is blocked except `proxy_allowed`; in probe2,
  the Lima gateway's internal DNS zone answers and only its port 53 is open (plan 1's accepted
  results). An expectation may pin a check's detail as `{"result","detail"}`.
- Every guest check expected blocked has a control: the `controls` phase reaches the same target
  from the host, unconfined. An unreachable control fails the run, because its check would prove
  nothing. probe2's checks target the Lima gateway, which exists only inside the VM, so they have
  no host control.
- Logs append to `$ACC/acceptance.log` and are never truncated; a failing run's files are copied
  to `results/<timestamp>/` (gitignored).
