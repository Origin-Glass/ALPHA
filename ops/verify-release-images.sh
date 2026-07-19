#!/bin/sh
set -eu

: "${RELEASE_IMAGE_REFS:?RELEASE_IMAGE_REFS is required}"
: "${APPROVED_IMAGE_REGISTRIES:?APPROVED_IMAGE_REGISTRIES is required}"
: "${COSIGN_PUBLIC_KEY:?COSIGN_PUBLIC_KEY is required}"
command -v cosign >/dev/null
command -v docker >/dev/null
printf '%s\n' "$RELEASE_IMAGE_REFS" | grep -Eq '[^[:space:]]'

approved_registry() {
  image=$1
  case "$image" in
    */*) registry=${image%%/*} ;;
    *) registry=docker.io ;;
  esac
  case "$registry" in
    *.*|*:*|localhost|docker.io) ;;
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

printf '%s\n' "$RELEASE_IMAGE_REFS" | while IFS= read -r image; do
  [ -n "$image" ] || continue
  if ! printf '%s\n' "$image" | grep -Eq '@sha256:[0-9a-f]{64}$'; then
    printf '%s\n' "release image must use repository@sha256: digest: $image" >&2
    exit 1
  fi
  case "$image" in
    *@sha256:0000000000000000000000000000000000000000000000000000000000000000)
      printf '%s\n' "zero digest is forbidden: $image" >&2
      exit 1
      ;;
  esac
  approved_registry "$image" || {
    printf '%s\n' "release image registry is not approved: $image" >&2
    exit 1
  }
  docker buildx imagetools inspect "$image" >/dev/null
  cosign verify --key "$COSIGN_PUBLIC_KEY" "$image" >/dev/null
  cosign verify-attestation --key "$COSIGN_PUBLIC_KEY" --type slsaprovenance "$image" >/dev/null
  cosign verify-attestation --key "$COSIGN_PUBLIC_KEY" --type spdxjson "$image" >/dev/null
done
