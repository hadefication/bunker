#!/usr/bin/env bash
# compare.sh <results.jsonl> <expected.json>
# Every check named in expected must have the expected result. An expected value may be a string
# (the result) or {"result","detail"} to pin the detail too. Exit 0 match, 1 mismatch, 2 bad input.
set -u
res=$1 exp=$2
[[ -r $res && -r $exp ]] || { echo "compare: missing $res or $exp" >&2; exit 2; }
out=$(jq -c -n --slurpfile r "$res" --slurpfile e "$exp" '
  ($r | map({(.check): {result: .result, detail: (.detail // "")}}) | add // {}) as $got
  | $e[0] | to_entries[]
  | (if (.value | type) == "object" then .value else {result: .value} end) as $want
  | ($got[.key] // {result: "missing", detail: ""}) as $have
  | select($have.result != $want.result or ($want.detail != null and $have.detail != $want.detail))
  | {check: .key, expected: $want, got: $have}') || exit 2
if [[ -n $out ]]; then printf '%s\n' "$out"; exit 1; fi
exit 0
