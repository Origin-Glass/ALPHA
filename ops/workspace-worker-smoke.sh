#!/usr/bin/env bash
set -euo pipefail

base_url="${1:-http://127.0.0.1:18080}"
smoke_dir="$(mktemp -d)"
trap 'rm -rf "$smoke_dir"' EXIT
cookie_jar="$smoke_dir/cookies"

login="$(curl --fail --silent --show-error -c "$cookie_jar" -H 'content-type: application/json' -d '{"handle":"workspace-smoke"}' "$base_url/api/v1/auth/test-session")"
csrf="$(printf '%s' "$login" | jq -er '.csrf_token')"
curl --fail --silent --show-error -b "$cookie_jar" -H "x-csrf-token: $csrf" -H 'content-type: application/json' -d '{"version":"2026-07-18","choices":{"terms":true,"privacy":true}}' "$base_url/api/v1/auth/terms" >/dev/null

workspace_key="$(uuidgen | tr '[:upper:]' '[:lower:]')"
workspace="$(curl --fail --silent --show-error -b "$cookie_jar" -H "x-csrf-token: $csrf" -H 'content-type: application/json' -d "{\"title\":\"Compose worker smoke\",\"template_slug\":\"python-cli-v1\",\"project_id\":null,\"files\":[{\"path\":\"main.py\",\"content\":\"print(42)\",\"symlink\":false}],\"idempotency_key\":\"$workspace_key\"}" "$base_url/api/v1/workspaces")"
workspace_id="$(printf '%s' "$workspace" | jq -er '.id')"
run_key="$(uuidgen | tr '[:upper:]' '[:lower:]')"
run="$(curl --fail --silent --show-error -b "$cookie_jar" -H "x-csrf-token: $csrf" -H 'content-type: application/json' -d "{\"expected_version\":1,\"idempotency_key\":\"$run_key\"}" "$base_url/api/v1/workspaces/$workspace_id/runs")"
run_id="$(printf '%s' "$run" | jq -er '.id')"

for _ in $(seq 1 30); do
  result="$(curl --fail --silent --show-error -b "$cookie_jar" "$base_url/api/v1/workspace-runs/$run_id")"
  status="$(printf '%s' "$result" | jq -er '.status')"
  if [[ "$status" == "succeeded" ]]; then
    [[ "$(printf '%s' "$result" | jq -r '.stdout')" == "42"$'\n' ]]
    exit 0
  fi
  [[ "$status" != "failed" && "$status" != "cancelled" && "$status" != "expired" ]]
  sleep 1
done

echo "workspace worker did not consume run $run_id" >&2
exit 1
