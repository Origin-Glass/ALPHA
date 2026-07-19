#!/bin/sh
set -eu

database=${1:?사용법: $0 DATABASE}
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/alpha-recovery-assert.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM

for dataset in generation review_receipts policy_consents project_workspace_hashes; do
  sh ./ops/canonical-db-json.sh "$database" "$dataset" >"$work_dir/$dataset.json"
done

jq -e '
  any(.jobs[]; .id == "60000000-0000-7000-8000-000000000002" and .request_spec.fixture == "recovery-v1" and .status == "completed") and
  any(.attempts[]; .id == "60000000-0000-7000-8000-000000000003" and .provider_snapshot.model == "recovery-model" and .status == "completed") and
  any(.artifacts[]; .id == "60000000-0000-7000-8000-000000000004" and .payload.title == "Canonical recovery artifact")
' "$work_dir/generation.json" >/dev/null

jq -e '
  any(.ai[]; .id == "60000000-0000-7000-8000-000000000007" and .outcome == "pass" and .review_response.fixture == "recovery-v1") and
  any(.human[]; .id == "60000000-0000-7000-8000-000000000008" and .decision == "approve") and
  any(.rights[]; .id == "60000000-0000-7000-8000-000000000009" and .decision == "approve" and .commercial_use_allowed and .redistribution_allowed) and
  any(.pilot[]; .id == "60000000-0000-7000-8000-00000000000a" and .decision == "pass" and .participants == 5)
' "$work_dir/review_receipts.json" >/dev/null

jq -e '(.versions | length > 0) and (.consents | length > 0)' "$work_dir/policy_consents.json" >/dev/null
jq -e '
  any(.project_inputs[]; .id == "60000000-0000-7000-8000-000000000020" and .revision == 1 and .input_hash == ("f1" * 32)) and
  (.workspaces | length > 0) and (.revisions | length > 0) and (.files | length > 0)
' "$work_dir/project_workspace_hashes.json" >/dev/null
