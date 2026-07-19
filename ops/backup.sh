#!/bin/sh
set -eu
umask 077

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
manifest_file="$backup_file.semantic.json"

mkdir -p "$output_dir"
work_dir=$(mktemp -d "$output_dir/.alpha-backup.XXXXXX")
staged_dump="$work_dir/backup.dump"
staged_dump_checksum="$work_dir/backup.dump.sha256"
staged_manifest="$work_dir/backup.dump.semantic.json"
staged_manifest_checksum="$work_dir/backup.dump.semantic.json.sha256"

cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

for dataset in generation review_receipts policy_consents project_workspace_hashes; do
  sh ./ops/canonical-db-json.sh "$source_database" "$dataset" >"$work_dir/before-$dataset.json"
done

if ! docker compose exec -T db pg_dump --username alpha --dbname "$source_database" --format custom --compress=9 --no-owner --no-acl >"$staged_dump"; then
  exit 1
fi

for dataset in generation review_receipts policy_consents project_workspace_hashes; do
  sh ./ops/canonical-db-json.sh "$source_database" "$dataset" >"$work_dir/after-$dataset.json"
  if ! cmp -s "$work_dir/before-$dataset.json" "$work_dir/after-$dataset.json"; then
    printf '%s\n' "백업 중 ${source_database}의 $dataset 데이터가 변경되었습니다. 쓰기 중지 후 다시 실행하세요." >&2
    exit 75
  fi
done

docker compose exec -T db pg_restore --list <"$staged_dump" >/dev/null
checksum=$(sha256_file "$staged_dump")
generation_digest=$(sha256_file "$work_dir/before-generation.json")
review_digest=$(sha256_file "$work_dir/before-review_receipts.json")
policy_digest=$(sha256_file "$work_dir/before-policy_consents.json")
workspace_digest=$(sha256_file "$work_dir/before-project_workspace_hashes.json")

printf '%s  %s\n' "$checksum" "$(basename "$backup_file")" >"$staged_dump_checksum"
printf '{"format_version":1,"source_database":"%s","dump_sha256":"%s","datasets":{"generation":"%s","review_receipts":"%s","policy_consents":"%s","project_workspace_hashes":"%s"}}\n' \
  "$source_database" "$checksum" "$generation_digest" "$review_digest" "$policy_digest" "$workspace_digest" >"$staged_manifest"
printf '%s  %s\n' "$(sha256_file "$staged_manifest")" "$(basename "$manifest_file")" >"$staged_manifest_checksum"

mv "$staged_dump_checksum" "$backup_file.sha256"
mv "$staged_manifest" "$manifest_file"
mv "$staged_manifest_checksum" "$manifest_file.sha256"
mv "$staged_dump" "$backup_file"

printf '%s\n' "$backup_file"
