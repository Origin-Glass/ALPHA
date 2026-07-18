#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  printf '%s\n' "사용법: $0 BACKUP.dump NEW_DATABASE" >&2
  exit 64
fi

backup_file=$1
target_database=$2
case "$target_database" in
  alpha|postgres|template0|template1)
    printf '%s\n' "운영 또는 시스템 데이터베이스에는 복구할 수 없습니다." >&2
    exit 64
    ;;
  [a-zA-Z]*)
    ;;
  *)
    printf '%s\n' "새 데이터베이스 이름은 영문자로 시작해야 합니다." >&2
    exit 64
    ;;
esac
case "$target_database" in
  *[!a-zA-Z0-9_]*)
    printf '%s\n' "새 데이터베이스 이름에는 영문자, 숫자, 밑줄만 사용할 수 있습니다." >&2
    exit 64
    ;;
esac

if [ ! -f "$backup_file" ]; then
  printf '%s\n' "백업 파일을 찾을 수 없습니다: $backup_file" >&2
  exit 66
fi

checksum_file="$backup_file.sha256"
if [ ! -f "$checksum_file" ]; then
  printf '%s\n' "체크섬 파일을 찾을 수 없습니다: $checksum_file" >&2
  exit 66
fi
expected_checksum=$(awk 'NR == 1 { print $1 }' "$checksum_file")
if command -v sha256sum >/dev/null 2>&1; then
  actual_checksum=$(sha256sum "$backup_file" | awk '{print $1}')
else
  actual_checksum=$(shasum -a 256 "$backup_file" | awk '{print $1}')
fi
if [ "$actual_checksum" != "$expected_checksum" ]; then
  printf '%s\n' "백업 체크섬이 일치하지 않습니다." >&2
  exit 65
fi
printf '%s\n' "$backup_file: OK"

exists=$(docker compose exec -T db psql --username alpha --dbname postgres --tuples-only --no-align --command "SELECT 1 FROM pg_database WHERE datname = '$target_database'")
if [ "$exists" = "1" ]; then
  printf '%s\n' "대상 데이터베이스가 이미 존재합니다: $target_database" >&2
  exit 65
fi

docker compose exec -T db createdb --username alpha "$target_database"
docker compose exec -T db pg_restore --username alpha --dbname "$target_database" --exit-on-error --no-owner --no-acl <"$backup_file"

docker compose exec -T db psql --username alpha --dbname "$target_database" --tuples-only --no-align --command "SELECT COUNT(*) FROM _sqlx_migrations"
