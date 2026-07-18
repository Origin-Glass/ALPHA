#!/bin/sh
set -eu

backup_dir=${1:-/tmp/alpha-recovery-smoke}
target_database=alpha_restore_smoke
query="SELECT json_build_object(
  'migrations', (SELECT count(*) FROM _sqlx_migrations),
  'migration_checksum', (SELECT md5(string_agg(version::text || encode(checksum, 'hex'), ',' ORDER BY version)) FROM _sqlx_migrations),
  'problems', (SELECT count(*) FROM problems),
  'problem_checksum', (SELECT md5(string_agg(problem.id::text || problem.slug || encode(revision.content_hash, 'hex'), ',' ORDER BY problem.id)) FROM problems problem JOIN problem_revisions revision ON revision.id = problem.current_revision_id),
  'tests', (SELECT count(*) FROM problem_test_cases),
  'test_checksum', (SELECT md5(string_agg(id::text || input || expected_output || visibility, ',' ORDER BY id)) FROM problem_test_cases),
  'users', (SELECT count(*) FROM users),
  'submissions', (SELECT count(*) FROM submissions),
  'audit_events', (SELECT count(*) FROM audit_events)
)"

if docker compose exec -T db psql --username alpha --dbname postgres --tuples-only --no-align --command "SELECT 1 FROM pg_database WHERE datname = '$target_database'" | grep -q 1; then
  printf '%s\n' "검증 대상 데이터베이스가 이미 존재합니다: $target_database" >&2
  exit 65
fi

backup_file=$(./ops/backup.sh "$backup_dir")
source_fingerprint=$(docker compose exec -T db psql --username alpha --dbname alpha --tuples-only --no-align --command "$query")
./ops/restore.sh "$backup_file" "$target_database" >/dev/null
restored_fingerprint=$(docker compose exec -T db psql --username alpha --dbname "$target_database" --tuples-only --no-align --command "$query")

if [ "$source_fingerprint" != "$restored_fingerprint" ]; then
  printf '%s\n' "복구 데이터 지문이 원본과 일치하지 않습니다." >&2
  printf '%s\n%s\n' "$source_fingerprint" "$restored_fingerprint" >&2
  exit 1
fi

docker compose exec -T db dropdb --username alpha "$target_database"
printf '%s\n' "$source_fingerprint"
