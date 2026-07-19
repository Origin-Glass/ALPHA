#!/bin/sh
set -eu

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  printf '%s\n' "사용법: $0 BACKUP.dump RESTORED_DATABASE [LIVE_SOURCE_DATABASE]" >&2
  exit 64
fi

backup_file=$1
restored_database=$2
live_source_database=${3:-}
manifest_file="$backup_file.semantic.json"
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/alpha-recovery.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

verify_checksum() {
  file=$1
  checksum_file=$2
  if [ ! -f "$file" ] || [ ! -f "$checksum_file" ]; then
    printf '%s\n' "파일 또는 체크섬을 찾을 수 없습니다: $file" >&2
    exit 66
  fi
  expected=$(awk 'NR == 1 { print $1 }' "$checksum_file")
  if [ "$(sha256_file "$file")" != "$expected" ]; then
    printf '%s\n' "체크섬이 일치하지 않습니다: $file" >&2
    exit 65
  fi
}

verify_checksum "$backup_file" "$backup_file.sha256"
verify_checksum "$manifest_file" "$manifest_file.sha256"
docker compose exec -T db pg_restore --list <"$backup_file" >/dev/null

dump_checksum=$(sha256_file "$backup_file")
manifest_dump_checksum=$(ruby -rjson -e '
  data = JSON.parse(File.read(ARGV.fetch(0)))
  expected_keys = %w[datasets dump_sha256 format_version source_database]
  expected_datasets = %w[generation policy_consents project_workspace_hashes review_receipts]
  abort "invalid manifest keys" unless data.keys.sort == expected_keys
  abort "invalid manifest version" unless data["format_version"] == 1
  abort "invalid source database" unless data["source_database"].is_a?(String) && data["source_database"].match?(/\A[a-zA-Z][a-zA-Z0-9_]*\z/)
  abort "invalid dump digest" unless data["dump_sha256"].is_a?(String) && data["dump_sha256"].match?(/\A[0-9a-f]{64}\z/)
  datasets = data["datasets"]
  abort "invalid dataset set" unless datasets.is_a?(Hash) && datasets.keys.sort == expected_datasets
  abort "invalid dataset digest" unless datasets.values.all? { |digest| digest.is_a?(String) && digest.match?(/\A[0-9a-f]{64}\z/) }
  puts data.fetch("dump_sha256")
' "$manifest_file")
if [ "$dump_checksum" != "$manifest_dump_checksum" ]; then
  printf '%s\n' "semantic manifest가 선택한 dump와 연결되지 않습니다." >&2
  exit 65
fi

for dataset in generation review_receipts policy_consents project_workspace_hashes; do
  sh ./ops/canonical-db-json.sh "$restored_database" "$dataset" >"$work_dir/restored-$dataset.json"
  restored_digest=$(sha256_file "$work_dir/restored-$dataset.json")
  manifest_digest=$(ruby -rjson -e 'puts JSON.parse(File.read(ARGV.fetch(0))).fetch("datasets").fetch(ARGV.fetch(1))' "$manifest_file" "$dataset")
  if [ "$restored_digest" != "$manifest_digest" ]; then
    printf '%s\n' "$dataset canonical digest가 backup-time manifest와 일치하지 않습니다." >&2
    exit 1
  fi
  printf '%s %s\n' "$dataset" "$restored_digest"

  if [ -n "$live_source_database" ]; then
    sh ./ops/canonical-db-json.sh "$live_source_database" "$dataset" >"$work_dir/live-$dataset.json"
    live_digest=$(sha256_file "$work_dir/live-$dataset.json")
    if [ "$live_digest" != "$manifest_digest" ]; then
      printf '%s %s\n' "rpo_divergence:$dataset" "$live_digest"
    fi
  fi
done
