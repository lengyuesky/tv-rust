#!/usr/bin/env bash
set -euo pipefail

: "${RUNNER_TEMP:?缺少 runner 临时目录}"
: "${GITHUB_PATH:?缺少 runner 路径输出}"
root="$RUNNER_TEMP/ffprobe"
if [ ! -x "$root/ffprobe" ]; then
  archive="$RUNNER_TEMP/ffprobe.tar.xz"
  curl --fail --silent --show-error --location --retry 3 --retry-all-errors \
    --retry-max-time 60 --connect-timeout 5 --max-time 30 --output "$archive" \
    https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-10-31-12-59/ffmpeg-n8.1.3-27-g8dca1586f4-linux64-lgpl-shared-8.1.tar.xz
  echo "493e89a48da02b9a7d217b60f7f60c3c917c9bed73459d8a106e5e987da082ef  $archive" | sha256sum --check
  mkdir -p "$root"
  tar -xJf "$archive" -C "$root" --strip-components=1 --wildcards '*/bin/ffprobe' '*/lib/*'
  cat > "$root/ffprobe" <<'WRAPPER'
#!/usr/bin/env bash
set -euo pipefail
tools=$(cd -- "$(dirname -- "$0")" && pwd)
export LD_LIBRARY_PATH="$tools/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$tools/bin/ffprobe" "$@"
WRAPPER
  chmod +x "$root/ffprobe"
fi
"$root/ffprobe" -version
echo "$root" >> "$GITHUB_PATH"

# 使用本地受控素材核对当前 CPU 与工具的 AES-128 解析能力。
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp tests/fixtures/hls-probe/key.bin "$work/key.key"
cp tests/fixtures/hls-probe/segment-0.ts "$work/segment.ts"
sed -e 's/key.bin/key.key/' -e 's/segment-0.ts/segment.ts/' \
  tests/fixtures/hls-probe/media.m3u8 > "$work/probe.m3u8"
"$root/ffprobe" -v error -protocol_whitelist file,crypto -allowed_extensions key,ts,m3u8 \
  -analyzeduration 1500000 -probesize 524288 -show_streams -of json "$work/probe.m3u8" > "$work/probe.json"
jq -e 'any(.streams[]; .codec_type == "video" and .width == 64) and any(.streams[]; .codec_type == "audio")' "$work/probe.json"
