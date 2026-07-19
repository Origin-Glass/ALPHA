#!/bin/sh
set -eu

backup_dir=${1:-/tmp/alpha-recovery-smoke}
target_database="alpha_restore_smoke_$$"
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/alpha-recovery.XXXXXX")

cleanup() {
  docker compose exec -T db dropdb --username alpha --if-exists "$target_database" >/dev/null 2>&1 || true
  rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

canonical_json() {
  database=$1
  dataset=$2
  case "$dataset" in
    generation)
      query="SELECT jsonb_build_object(
        'jobs', COALESCE((SELECT jsonb_agg(to_jsonb(row_data) ORDER BY id) FROM content_generation_jobs row_data), '[]'::jsonb),
        'attempts', COALESCE((SELECT jsonb_agg(to_jsonb(row_data) ORDER BY job_id, attempt_number) FROM content_generation_attempts row_data), '[]'::jsonb),
        'artifacts', COALESCE((SELECT jsonb_agg(to_jsonb(row_data) ORDER BY id) FROM content_artifacts row_data), '[]'::jsonb)
      )"
      ;;
    review_receipts)
      query="SELECT jsonb_build_object(
        'ai', COALESCE((SELECT jsonb_agg(to_jsonb(row_data) ORDER BY id) FROM content_ai_review_receipts row_data), '[]'::jsonb),
        'human', COALESCE((SELECT jsonb_agg(to_jsonb(row_data) ORDER BY id) FROM content_human_review_decisions row_data), '[]'::jsonb),
        'rights', COALESCE((SELECT jsonb_agg(to_jsonb(row_data) ORDER BY id) FROM content_rights_review_receipts row_data), '[]'::jsonb),
        'pilot', COALESCE((SELECT jsonb_agg(to_jsonb(row_data) ORDER BY id) FROM content_pilot_receipts row_data), '[]'::jsonb)
      )"
      ;;
    policy_consents)
      query="SELECT jsonb_build_object(
        'versions', COALESCE((SELECT jsonb_agg(to_jsonb(row_data) ORDER BY version) FROM policy_versions row_data), '[]'::jsonb),
        'consents', COALESCE((SELECT jsonb_agg(to_jsonb(row_data) ORDER BY user_id, policy_version) FROM policy_consents row_data), '[]'::jsonb)
      )"
      ;;
    project_workspace_hashes)
      query="SELECT jsonb_build_object(
        'project_inputs', COALESCE((SELECT jsonb_agg(jsonb_build_object('id', id, 'revision', revision, 'input_hash', encode(input_hash, 'hex')) ORDER BY id, revision) FROM project_ideas), '[]'::jsonb),
        'workspaces', COALESCE((SELECT jsonb_agg(jsonb_build_object('id', id, 'version', version, 'request_hash', encode(request_hash, 'hex'), 'template_digest', encode(template_digest, 'hex')) ORDER BY id) FROM project_workspaces), '[]'::jsonb),
        'revisions', COALESCE((SELECT jsonb_agg(jsonb_build_object('workspace_id', workspace_id, 'version', version, 'artifact_hash', encode(artifact_hash, 'hex')) ORDER BY workspace_id, version) FROM workspace_revisions), '[]'::jsonb),
        'files', COALESCE((SELECT jsonb_agg(jsonb_build_object('workspace_id', workspace_id, 'path', path, 'content_hash', encode(content_hash, 'hex')) ORDER BY workspace_id, path) FROM workspace_files), '[]'::jsonb)
      )"
      ;;
    *)
      printf '%s\n' "알 수 없는 복구 검증 데이터군: $dataset" >&2
      return 64
      ;;
  esac
  docker compose exec -T db psql --username alpha --dbname "$database" \
    --set ON_ERROR_STOP=1 --tuples-only --no-align --command "$query"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

backup_file=$(./ops/backup.sh "$backup_dir")
./ops/restore.sh "$backup_file" "$target_database" >/dev/null

for dataset in generation review_receipts policy_consents project_workspace_hashes; do
  canonical_json alpha "$dataset" >"$work_dir/source-$dataset.json"
  canonical_json "$target_database" "$dataset" >"$work_dir/restored-$dataset.json"
  if ! cmp -s "$work_dir/source-$dataset.json" "$work_dir/restored-$dataset.json"; then
    printf '%s\n' "$dataset canonical 데이터가 원본과 일치하지 않습니다." >&2
    exit 1
  fi
  printf '%s %s\n' "$dataset" "$(sha256_file "$work_dir/source-$dataset.json")"
done
