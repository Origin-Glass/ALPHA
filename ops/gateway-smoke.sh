#!/usr/bin/env sh
set -eu

base_url=${1:-http://127.0.0.1:8080}

live=$(curl --fail --silent --show-error "$base_url/health/live")
ready=$(curl --fail --silent --show-error "$base_url/health/ready")

if [ "$live" != '{"service":"alpha-api","status":"ok"}' ]; then
    echo "공개 게이트웨이 생존 상태 응답이 API JSON이 아닙니다." >&2
    exit 1
fi

if [ "$ready" != '{"service":"alpha-api","status":"ready"}' ]; then
    echo "공개 게이트웨이 준비 상태 응답이 API JSON이 아닙니다." >&2
    exit 1
fi
