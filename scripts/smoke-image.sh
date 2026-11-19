#!/usr/bin/env bash
set -euo pipefail

: "${TEST_IMAGE:?缺少待验证的镜像}"
: "${EXPECTED_ARCH:?缺少待验证的架构}"
name="tv-rust-smoke-${EXPECTED_ARCH}-$$"
work=$(mktemp -d)
cleanup() {
  docker logs "$name" 2>/dev/null || true
  docker rm --force --volumes "$name" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

test "$(docker run --rm --entrypoint dpkg "$TEST_IMAGE" --print-architecture)" = "$EXPECTED_ARCH"
test "$(docker run --rm --entrypoint id "$TEST_IMAGE" -u)" = "10001"
docker run --detach --name "$name" --init \
  --publish 127.0.0.1::3000 --read-only \
  --tmpfs /tmp:size=256m,mode=1777 \
  --security-opt no-new-privileges:true --cap-drop ALL "$TEST_IMAGE"
port=$(docker inspect --format '{{(index (index .NetworkSettings.Ports "3000/tcp") 0).HostPort}}' "$name")
base="http://127.0.0.1:$port"
curl --fail --silent --show-error --retry 30 --retry-all-errors --retry-delay 1 \
  --retry-max-time 60 --connect-timeout 2 --max-time 5 \
  --output "$work/health.json" "$base/api/health"
jq -e '.ok == true' "$work/health.json"
curl --fail --silent --show-error --max-time 5 "$base/api/status" | jq -e '.initialized == false'
curl --fail --silent --show-error --max-time 5 "$base/" > "$work/index.html"
[[ "$(<"$work/index.html")" == *'id="root"'* ]]
docker exec "$name" ffprobe -version
