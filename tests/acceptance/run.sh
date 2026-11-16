#!/usr/bin/env bash
# Bunker's acceptance run on the Mac mini, against a disposable Laravel fixture.
#   run.sh <phase>   fixture | up | deploy | controls | probe | proxy | roundtrip | reboot |
#                    unwrapped | rollback | cleanup [--final] | all
# Each phase exits 0 on match, 1 on mismatch, 2 on a setup error. `all` runs every phase in order,
# then cleanup; it stops at the first failure and leaves the VM for inspection.
# Logs append to $ACC/acceptance.log; a failing run's results are kept in results/<timestamp>/.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
# shellcheck source=env.sh
. "$HERE/env.sh"
mkdir -p "$ACC"
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
RUN=$ACC/runs/$STAMP
mkdir -p "$RUN"
LOG=$ACC/acceptance.log
PROXY_URL=http://127.0.0.1:3128

log() { printf '%s %s\n' "$(date -u +%FT%TZ)" "$*" | tee -a "$LOG" >&2; }
keep() {
  mkdir -p "$HERE/results/$STAMP"
  cp -R "$RUN/." "$HERE/results/$STAMP/" 2>/dev/null
  log "results kept in $HERE/results/$STAMP"
}
fail() { log "FAIL: $*"; keep; exit 1; }
setup() { log "SETUP ERROR: $*"; keep; exit 2; }
b() { "$BUNKER" "$@"; }
gsh() { limactl shell --tty=false --workdir / "$VM" "$@"; }
port() { sed -n "s/^$1 *= *//p" "$BUNKER_HOME/$PROJECT/bunker.toml"; }
web_code() { curl -s -o /dev/null -m 10 -w '%{http_code}' "http://127.0.0.1:$(port web)/${1:-}"; }
status_json() { b --json status "$PROJECT"; }
jl() { printf '{"check":"%s","result":"%s","detail":"%s"}\n' "$1" "$2" "${3:-}"; }

preflight() {
  for t in jq limactl curl git security nc python3 shasum; do
    command -v "$t" >/dev/null || setup "$t is not installed"
  done
  local plist=$HOME/Library/LaunchAgents/com.bunker.proxy.plist
  if [[ -f $plist ]] && ! grep -q "$BUNKER_HOME" "$plist"; then
    setup "a real bunker proxy agent is installed ($plist); launchd labels are shared, so the acceptance run would replace it"
  fi
}

LISTENERS=()
start_listeners() {
  python3 -m http.server --bind 127.0.0.1 "$LOOP_PORT" >/dev/null 2>&1 &
  LISTENERS+=($!)
  python3 -m http.server --bind 0.0.0.0 "$ALL_PORT" >/dev/null 2>&1 &
  LISTENERS+=($!)
  trap 'kill "${LISTENERS[@]}" 2>/dev/null' EXIT
  sleep 1
}

builtin_build_hosts() {
  sed -n '/BUILTIN_BUILD: &\[&str\] = &\[/,/^\];/p' "$REPO/src/proxy/policy.rs" | grep -oE '"[^"]+"' | tr -d '"'
}

phase_fixture() {
  log "== fixture"
  (cd "$REPO" && cargo build --release --locked -q) || setup "cargo build failed"
  mkdir -p "$ACC/bin" && install -m 0755 "$REPO/target/release/bunker" "$BUNKER"
  rm -rf "$ACC/fixture" && mkdir -p "$ACC/fixture"
  curl -fsSL "$FIXTURE_TARBALL" | tar -xz -C "$ACC/fixture" --strip-components=1 || setup "cannot fetch the Laravel skeleton"
  if ! { git -C "$ACC/fixture" init -q && git -C "$ACC/fixture" add -A &&
    git -C "$ACC/fixture" -c user.name=acceptance -c user.email=acceptance@example.invalid commit -qm fixture; }; then
    setup "cannot commit the fixture"
  fi
  [[ -f $BUNKER_KEYCHAIN ]] || security create-keychain -p bunker-test "$BUNKER_KEYCHAIN" || setup "cannot create the throwaway keychain"
  mkdir -p "$BUNKER_HOME/images"
  for img in "$IMAGE_CACHE"/ubuntu-24.04-arm64-*.img; do
    [[ -f $img && ! -f $BUNKER_HOME/images/${img##*/} ]] && cp -c "$img" "$BUNKER_HOME/images/"
  done
  return 0
}

phase_up() {
  log "== up"
  [[ -f $BUNKER_HOME/$PROJECT/bunker.toml ]] || b init "$PROJECT" --path "$ACC/fixture" >>"$RUN/up.log" 2>&1 || setup "bunker init failed"
  b net allow example.com "$PROJECT" >>"$RUN/up.log" 2>&1 || setup "bunker net allow failed"
  b up "$PROJECT" >>"$RUN/up.log" 2>&1 || fail "bunker up exited $? (see up.log)"
  local s; s=$(status_json); printf '%s\n' "$s" >"$RUN/status-up.json"
  [[ $(jq -r .data.vm.confinement <<<"$s") == confined ]] || fail "not confined after up"
  [[ $(jq -r '.data.forwards.proxy.sandboxed and .data.forwards.web.sandboxed' <<<"$s") == true ]] || fail "a forward is not sandboxed"
  local net=$BUNKER_HOME/$PROJECT/network.jsonl blocked=0
  for h in $(builtin_build_hosts); do
    if jq -e --arg h "$h" 'select(.kind == "decision" and .verdict == "block" and .host == $h)' "$net" >/dev/null 2>&1; then
      log "a BUILTIN_BUILD host was blocked: $h"; blocked=1
    fi
  done
  ((blocked == 0)) || fail "a build host was blocked during up"
}

phase_deploy() {
  log "== deploy"
  b deploy "$PROJECT" >>"$RUN/deploy.log" 2>&1 || fail "bunker deploy exited $? (see deploy.log)"
  [[ $(web_code) == 200 ]] || fail "the web port does not return 200 after deploy"
}

phase_controls() {
  log "== controls (from the host, unconfined: each must be reachable)"
  start_listeners
  ctl() { local name=$1; shift; if "$@" >/dev/null 2>&1; then jl "$name" reachable; else jl "$name" no_control; fi; }
  {
    ctl internet_1_1_1_1_443 nc -z -w4 1.1.1.1 443
    ctl mac_lan_ssh nc -z -w4 "$MAC_LAN" 22
    ctl router_80 nc -z -w4 "$ROUTER" 80
    ctl tailnet_peer_ssh nc -z -w4 "$TS_PEER" 22
    ctl tailnet_self_ssh nc -z -w4 "$TS_SELF" 22
    ctl host_loopback_listener curl -s -m4 -o /dev/null "http://127.0.0.1:$LOOP_PORT"
    ctl host_lan_listener curl -s -m4 -o /dev/null "http://$MAC_LAN:$ALL_PORT"
  } >"$RUN/controls.jsonl"
  cat "$RUN/controls.jsonl" >&2
  ! grep -q no_control "$RUN/controls.jsonl" || fail "a control is unreachable from the host, so its guest check would prove nothing"
}

probe_env() {
  printf '%s ' "LOOP_PORT=$LOOP_PORT" "ALL_PORT=$ALL_PORT" "PROXY_PORT=$(port proxy)" "MAC_LAN=$MAC_LAN" \
    "ROUTER=$ROUTER" "TS_PEER=$TS_PEER" "TS_SELF=$TS_SELF" "GUEST_SOCK=/tmp/bunker-acc.sock"
}

# The probes test direct paths. The guest's /etc/environment (read by sudo) routes plain curl
# through the proxy, whose 403 curl counts as success, so every proxy variable is cleared; the
# probes' proxy checks pass -x themselves.
NOPROXY=(-u http_proxy -u HTTP_PROXY -u https_proxy -u HTTPS_PROXY -u no_proxy -u NO_PROXY)

copy_probes() { # /tmp in the guest is cleared by a reboot
  limactl copy "$HERE/probe.sh" "$HERE/probe2.sh" "$VM:/tmp/" >>"$RUN/copy.log" 2>&1 || setup "limactl copy failed"
}

run_probe() { # <name> [--root-tamper]
  local name=$1; shift
  copy_probes
  # shellcheck disable=SC2046
  gsh sudo env "${NOPROXY[@]}" $(probe_env) bash /tmp/probe.sh "$@" "$PROXY_URL" >"$RUN/$name.jsonl" 2>>"$RUN/probe.err"
  "$HERE/compare.sh" "$RUN/$name.jsonl" "$HERE/expected/probe.json" >"$RUN/$name.diff" ||
    { cat "$RUN/$name.diff" >&2; fail "HARD STOP: $name does not match expected/probe.json"; }
}

phase_probe() {
  log "== probe"
  start_listeners
  gsh sh -c 'command -v dig && command -v nc && command -v curl && command -v ping' >/dev/null ||
    setup "the guest lacks dig, nc, curl or ping, so its checks would pass for the wrong reason"
  run_probe probe
  # probe2 measures what the Lima gateway exposes to the guest; root can remove the guest's own
  # firewall, so that is measured without it (and the firewall's drops would make its port scan
  # wait out a timeout per port). The table is restored right after.
  gsh sudo nft delete table inet bunker >>"$RUN/probe.err" 2>&1
  # shellcheck disable=SC2046
  gsh sudo env "${NOPROXY[@]}" $(probe_env) bash /tmp/probe2.sh >"$RUN/probe2.jsonl" 2>>"$RUN/probe.err"
  gsh sudo nft -f /etc/nftables.conf >>"$RUN/probe.err" 2>&1 || setup "cannot restore the guest firewall"
  "$HERE/compare.sh" "$RUN/probe2.jsonl" "$HERE/expected/probe2.json" >"$RUN/probe2.diff" ||
    { cat "$RUN/probe2.diff" >&2; fail "HARD STOP: probe2 does not match expected/probe2.json"; }
  # Root in the guest flushes the firewall and rewrites routes and DNS: the host still holds.
  run_probe probe-root --root-tamper
}

phase_proxy() {
  log "== proxy (internal targets through the proxy: 403, even when allowed)"
  b net allow 127.0.0.1.nip.io "$PROJECT" >>"$RUN/proxy.log" 2>&1 || setup "net allow failed"
  b net allow "$MAC_LAN.nip.io" "$PROJECT" >>"$RUN/proxy.log" 2>&1 || setup "net allow failed"
  sleep 1
  local connect=(curl -s -o /dev/null -m 10 -w '%{http_connect}' -x "$PROXY_URL")
  {
    jl connect_10_0_0_1 "$(gsh "${connect[@]}" https://10.0.0.1/)"
    jl nip_loopback_allowed "$(gsh "${connect[@]}" https://127.0.0.1.nip.io/)"
    jl nip_lan_allowed "$(gsh "${connect[@]}" "https://$MAC_LAN.nip.io/")"
    jl nip_tailnet_self "$(gsh "${connect[@]}" "https://$TS_SELF.nip.io/")"
    jl metadata_http "$(gsh curl -s -o /dev/null -m 10 -w '%{http_code}' -x "$PROXY_URL" http://169.254.169.254/)"
  } >"$RUN/proxy.jsonl"
  b net rm 127.0.0.1.nip.io "$PROJECT" >>"$RUN/proxy.log" 2>&1
  b net rm "$MAC_LAN.nip.io" "$PROJECT" >>"$RUN/proxy.log" 2>&1
  "$HERE/compare.sh" "$RUN/proxy.jsonl" "$HERE/expected/proxy.json" >"$RUN/proxy.diff" ||
    { cat "$RUN/proxy.diff" >&2; fail "HARD STOP: proxy does not match expected/proxy.json"; }
  local net=$BUNKER_HOME/$PROJECT/network.jsonl
  for h in 127.0.0.1.nip.io "$MAC_LAN.nip.io"; do
    jq -e -s --arg h "$h" 'map(select(.kind == "decision" and .host == $h)) | last | .verdict == "block" and .reason == "internal_address"' "$net" >/dev/null ||
      fail "HARD STOP: $h was allowed by the list but not refused as internal_address"
  done
  for h in 10.0.0.1 "$TS_SELF.nip.io" 169.254.169.254; do
    jq -e -s --arg h "$h" 'map(select(.kind == "decision" and .host == $h)) | last | .verdict == "block"' "$net" >/dev/null ||
      fail "HARD STOP: no block decision logged for $h"
  done
}

phase_roundtrip() {
  log "== roundtrip (an awkward value through the Keychain, .env quoting and phpdotenv)"
  # shellcheck disable=SC2016 # literal $, \ and " are the point
  local value='q"u$o\te#d\nline2' want got
  printf '%s' "$value" | b env set BUNKER_ROUNDTRIP "$PROJECT" --stdin >>"$RUN/roundtrip.log" 2>&1 || fail "env set failed"
  want=$(printf '%s' "$value" | shasum -a 256 | cut -d' ' -f1)
  # config:cache makes env() return null, so read the file the way Laravel would (phpdotenv).
  got=$(b artisan "$PROJECT" -- tinker --execute='echo hash("sha256", Dotenv\Dotenv::parse(file_get_contents(base_path(".env")))["BUNKER_ROUNDTRIP"]);' 2>>"$RUN/roundtrip.log" | grep -oE '[0-9a-f]{64}' | tail -1)
  b env unset BUNKER_ROUNDTRIP "$PROJECT" >>"$RUN/roundtrip.log" 2>&1
  [[ $got == "$want" ]] || fail "roundtrip: the guest read $got, expected $want"
}

wait_confined_and_serving() { # <seconds>
  local deadline=$((SECONDS + $1)) s
  while ((SECONDS < deadline)); do
    if [[ $(web_code) == 200 ]]; then
      s=$(status_json 2>/dev/null)
      if [[ $(jq -r '.data.vm.confinement' <<<"$s") == confined && $(jq -r '.data.forwards.proxy.sandboxed and .data.forwards.web.sandboxed' <<<"$s") == true ]]; then
        return 0
      fi
    fi
    sleep 3
  done
  return 1
}

phase_reboot() {
  log "== reboot (the guest)"
  local before id deadline
  before=$(gsh cat /proc/sys/kernel/random/boot_id) || setup "cannot read the boot id"
  gsh sudo systemctl reboot >/dev/null 2>&1
  deadline=$((SECONDS + 90))
  while ((SECONDS < deadline)); do
    id=$(gsh cat /proc/sys/kernel/random/boot_id 2>/dev/null) && [[ -n $id && $id != "$before" ]] && break
    sleep 3
  done
  [[ -n ${id:-} && $id != "$before" ]] || fail "the guest did not come back with a new boot id within 90 s"
  wait_confined_and_serving 90 || fail "not serving 200, confined, with both forwards, within 90 s of the reboot"
  start_listeners
  run_probe probe-after-reboot
}

phase_unwrapped() {
  log "== unwrapped (a plain limactl start must be caught)"
  b stop "$PROJECT" >>"$RUN/unwrapped.log" 2>&1 || fail "bunker stop failed"
  limactl start --tty=false "$VM" >>"$RUN/unwrapped.log" 2>&1 || setup "plain limactl start failed"
  local s rc
  s=$(status_json); rc=$?
  printf '%s\n' "$s" >"$RUN/status-unwrapped.json"
  [[ $rc == 5 && $(jq -r .data.vm.confinement <<<"$s") == unconfined ]] || fail "status exited $rc instead of 5 for an unwrapped VM"
  b deploy "$PROJECT" >>"$RUN/unwrapped.log" 2>&1; rc=$?
  [[ $rc == 5 ]] || fail "deploy into an unwrapped VM exited $rc instead of 5"
  limactl stop --tty=false "$VM" >>"$RUN/unwrapped.log" 2>&1 || setup "limactl stop failed"
  b start "$PROJECT" >>"$RUN/unwrapped.log" 2>&1 || fail "bunker start failed"
  wait_confined_and_serving 90 || fail "not confined and serving after bunker start"
}

phase_rollback() {
  log "== rollback"
  echo v2 >"$ACC/fixture/public/acceptance.txt"
  if ! { git -C "$ACC/fixture" add public/acceptance.txt &&
    git -C "$ACC/fixture" -c user.name=acceptance -c user.email=acceptance@example.invalid commit -qm v2; }; then
    setup "cannot commit v2"
  fi
  b deploy "$PROJECT" >>"$RUN/rollback.log" 2>&1 || fail "deploying v2 failed"
  [[ $(curl -s -m 10 "http://127.0.0.1:$(port web)/acceptance.txt") == v2 ]] || fail "v2 is not served"
  b rollback "$PROJECT" >>"$RUN/rollback.log" 2>&1 || fail "rollback failed"
  grep -q "migrations are not rolled back" "$RUN/rollback.log" || fail "rollback did not say migrations stay"
  [[ $(web_code acceptance.txt) == 404 ]] || fail "the first release is not served again after rollback"
}

phase_cleanup() {
  log "== cleanup ${1:-}"
  if [[ -f $BUNKER_HOME/$PROJECT/bunker.toml && -x $BUNKER ]]; then
    b destroy "$PROJECT" --yes >>"$RUN/cleanup.log" 2>&1 || log "destroy failed (see cleanup.log)"
  fi
  local plist=$HOME/Library/LaunchAgents/com.bunker.proxy.plist
  if [[ -f $plist ]] && grep -q "$BUNKER_HOME" "$plist"; then
    launchctl bootout "gui/$(id -u)/com.bunker.proxy" 2>/dev/null
    rm -f "$plist"
  fi
  [[ -f $BUNKER_KEYCHAIN ]] && security delete-keychain "$BUNKER_KEYCHAIN"
  rm -rf "$BUNKER_HOME" "$ACC/fixture"
  [[ ${1:-} == --final ]] && rm -rf "$ACC"
  ! limactl list -q 2>/dev/null | grep -qx "$VM" || fail "$VM still exists"
  ! launchctl list | grep -q "com.bunker.$PROJECT\." || fail "com.bunker.$PROJECT agents are still loaded"
  local leftover=("$HOME/Library/LaunchAgents/com.bunker.$PROJECT."*.plist)
  [[ ! -e ${leftover[0]} ]] || fail "com.bunker.$PROJECT plists remain"
  return 0
}

preflight
case ${1:-} in
fixture | up | deploy | controls | probe | proxy | roundtrip | reboot | unwrapped | rollback)
  "phase_$1"
  ;;
cleanup) phase_cleanup "${2:-}" ;;
all)
  for p in fixture up deploy controls probe proxy roundtrip reboot unwrapped rollback; do
    "phase_$p"
  done
  phase_cleanup
  log "all phases passed"
  ;;
*)
  sed -n '2,7p' "$0" >&2
  exit 2
  ;;
esac
