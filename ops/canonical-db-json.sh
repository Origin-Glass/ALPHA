#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  printf '%s\n' "사용법: $0 DATABASE DATASET" >&2
  exit 64
fi

database=$1
dataset=$2
case "$database" in
  ''|[!a-zA-Z]*|*[!a-zA-Z0-9_]*)
    printf '%s\n' "데이터베이스 이름에는 영문자, 숫자, 밑줄만 사용할 수 있습니다: $database" >&2
    exit 64
    ;;
esac

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
    exit 64
    ;;
esac

docker compose exec -T db psql --username alpha --dbname "$database" \
  --set ON_ERROR_STOP=1 --tuples-only --no-align --command "$query"
