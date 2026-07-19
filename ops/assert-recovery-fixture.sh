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

jq -e '
  any(.versions[]; .version == "recovery-fixture-v1" and .title_ko == "복구 검증 정책" and .required == true) and
  any(.consents[]; .policy_version == "recovery-fixture-v1" and .policy_title_ko == "복구 검증 정책" and
      .choices.terms == true and .choices.privacy == true and .choices.fixture == "recovery-v1")
' "$work_dir/policy_consents.json" >/dev/null
jq -e '. as $root |
  any(.project_inputs[]; .id == "60000000-0000-7000-8000-000000000020" and .revision == 1 and .input_hash == ("f1" * 32)) and
  any(.workspaces[]; .id == "60000000-0000-7000-8000-000000000030" and .project_id == null and
      .template_id == "23000000-0000-7000-8000-000000000004" and .template_revision == 1 and .version == 1 and
      .request_hash == ("a1" * 32) and .template_digest == ("44" * 32)) and
  any(.revisions[]; .workspace_id == "60000000-0000-7000-8000-000000000030" and .version == 1 and
      .artifact_hash == ("b2" * 32) and .user_id == ($root.workspaces[] | select(.id == "60000000-0000-7000-8000-000000000030") | .user_id)) and
  any(.files[]; .workspace_id == "60000000-0000-7000-8000-000000000030" and .path == "main.py" and
      .path_key == "main.py" and .content_hash == ("c3" * 32) and
      .user_id == ($root.workspaces[] | select(.id == "60000000-0000-7000-8000-000000000030") | .user_id))
' "$work_dir/project_workspace_hashes.json" >/dev/null
