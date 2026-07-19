#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  printf '%s\n' "사용법: $0 OUTPUT_DIRECTORY SOURCE_DATABASE" >&2
  exit 64
fi

output_dir=$1
source_database=$2
case "$source_database" in
  ''|[!a-zA-Z]*|*[!a-zA-Z0-9_]*)
    printf '%s\n' "원본 데이터베이스 이름에는 영문자, 숫자, 밑줄만 사용할 수 있습니다: $source_database" >&2
    exit 64
    ;;
esac
timestamp=$(date -u +%Y%m%dT%H%M%SZ)
backup_file="$output_dir/$source_database-$timestamp-$$.dump"
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/alpha-backup.XXXXXX")

cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$output_dir"
for dataset in generation review_receipts policy_consents project_workspace_hashes; do
  sh ./ops/canonical-db-json.sh "$source_database" "$dataset" >"$work_dir/before-$dataset.json"
done

if ! docker compose exec -T db pg_dump --username alpha --dbname "$source_database" --format custom --compress=9 --no-owner --no-acl >"$backup_file"; then
  rm -f "$backup_file"
  exit 1
fi

for dataset in generation review_receipts policy_consents project_workspace_hashes; do
  sh ./ops/canonical-db-json.sh "$source_database" "$dataset" >"$work_dir/after-$dataset.json"
  if ! cmp -s "$work_dir/before-$dataset.json" "$work_dir/after-$dataset.json"; then
    rm -f "$backup_file"
    printf '%s\n' "백업 중 ${source_database}의 $dataset 데이터가 변경되었습니다. 쓰기 중지 후 다시 실행하세요." >&2
    exit 75
  fi
done

docker compose exec -T db pg_restore --list <"$backup_file" >/dev/null
if command -v sha256sum >/dev/null 2>&1; then
  checksum=$(sha256sum "$backup_file" | awk '{print $1}')
else
  checksum=$(shasum -a 256 "$backup_file" | awk '{print $1}')
fi
printf '%s\n' "$checksum" >"$backup_file.sha256"

printf '%s\n' "$backup_file"
