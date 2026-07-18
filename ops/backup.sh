#!/bin/sh
set -eu

output_dir=${1:-backups}
timestamp=$(date -u +%Y%m%dT%H%M%SZ)
backup_file="$output_dir/alpha-$timestamp-$$.dump"

mkdir -p "$output_dir"
docker compose exec -T db pg_dump --username alpha --dbname alpha --format custom --compress=9 --no-owner --no-acl >"$backup_file"

docker compose exec -T db pg_restore --list <"$backup_file" >/dev/null
if command -v sha256sum >/dev/null 2>&1; then
  checksum=$(sha256sum "$backup_file" | awk '{print $1}')
else
  checksum=$(shasum -a 256 "$backup_file" | awk '{print $1}')
fi
printf '%s\n' "$checksum" >"$backup_file.sha256"

printf '%s\n' "$backup_file"
