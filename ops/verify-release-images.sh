#!/bin/sh
set -eu

: "${RELEASE_COMPOSE_CONFIG:?RELEASE_COMPOSE_CONFIG is required}"
: "${APPROVED_IMAGE_REGISTRIES:?APPROVED_IMAGE_REGISTRIES is required}"
: "${COSIGN_PUBLIC_KEY:?COSIGN_PUBLIC_KEY is required}"
: "${EXPECTED_SIGNER_IDENTITY:?EXPECTED_SIGNER_IDENTITY is required}"
: "${EXPECTED_SOURCE_REPOSITORY:?EXPECTED_SOURCE_REPOSITORY is required}"
: "${EXPECTED_COMMIT:?EXPECTED_COMMIT is required}"
: "${APPROVED_BUILDERS:?APPROVED_BUILDERS is required}"
command -v cosign >/dev/null
command -v docker >/dev/null
command -v ruby >/dev/null

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/alpha-release-policy.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM
ruby ops/release-policy.rb images "$RELEASE_COMPOSE_CONFIG" >"$work_dir/images"

approved_registry() {
  image=$1
  case "$image" in
    */*) registry=${image%%/*} ;;
    *) registry=docker.io ;;
  esac
  old_ifs=$IFS
  IFS=,
  for approved in $APPROVED_IMAGE_REGISTRIES; do
    if [ "$registry" = "$approved" ]; then
      IFS=$old_ifs
      return 0
    fi
  done
  IFS=$old_ifs
  return 1
}

while IFS= read -r image; do
  approved_registry "$image" || {
    printf '%s\n' "release image registry is not approved: $image" >&2
    exit 1
  }
  if command -v sha256sum >/dev/null 2>&1; then
    name=$(printf '%s' "$image" | sha256sum | awk '{print $1}')
  else
    name=$(printf '%s' "$image" | shasum -a 256 | awk '{print $1}')
  fi
  docker buildx imagetools inspect "$image" >/dev/null
  cosign verify --output=json --key "$COSIGN_PUBLIC_KEY" "$image" >"$work_dir/$name-signature.json"
  ruby ops/release-policy.rb signature "$image" "$work_dir/$name-signature.json"
  cosign verify-attestation --output=json --key "$COSIGN_PUBLIC_KEY" --type slsaprovenance "$image" >"$work_dir/$name-provenance.json"
  ruby ops/release-policy.rb provenance "$image" "$work_dir/$name-provenance.json"
  cosign verify-attestation --output=json --key "$COSIGN_PUBLIC_KEY" --type spdxjson "$image" >"$work_dir/$name-sbom.json"
  ruby ops/release-policy.rb sbom "$image" "$work_dir/$name-sbom.json"
done <"$work_dir/images"
