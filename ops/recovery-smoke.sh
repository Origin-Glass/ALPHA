#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
  printf '%s\n' "사용법: $0 SOURCE_DATABASE BACKUP.dump RESTORED_DATABASE" >&2
  exit 64
fi

source_database=$1
backup_file=$2
restored_database=$3
if [ "$source_database" = "$restored_database" ]; then
  printf '%s\n' "원본과 복구 데이터베이스는 달라야 합니다: $source_database" >&2
  exit 64
fi
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/alpha-recovery.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM

if [ ! -f "$backup_file" ] || [ ! -f "$backup_file.sha256" ]; then
  printf '%s\n' "선택한 dump 또는 체크섬 파일을 찾을 수 없습니다: $backup_file" >&2
  exit 66
fi
docker compose exec -T db pg_restore --list <"$backup_file" >/dev/null

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

expected_checksum=$(awk 'NR == 1 { print $1 }' "$backup_file.sha256")
if [ "$(sha256_file "$backup_file")" != "$expected_checksum" ]; then
  printf '%s\n' "선택한 dump 체크섬이 일치하지 않습니다: $backup_file" >&2
  exit 65
fi

for dataset in generation review_receipts policy_consents project_workspace_hashes; do
  sh ./ops/canonical-db-json.sh "$source_database" "$dataset" >"$work_dir/source-$dataset.json"
  sh ./ops/canonical-db-json.sh "$restored_database" "$dataset" >"$work_dir/restored-$dataset.json"
  if ! cmp -s "$work_dir/source-$dataset.json" "$work_dir/restored-$dataset.json"; then
    printf '%s\n' "$dataset canonical 데이터가 $source_database와 $restored_database 사이에 일치하지 않습니다." >&2
    exit 1
  fi
  printf '%s %s\n' "$dataset" "$(sha256_file "$work_dir/source-$dataset.json")"
done
