export type VideoQuality = {
  width?: number;
  height?: number;
  codec?: string;
  frame_rate?: number;
  bitrate?: number;
};

const positive = (n: unknown, max = Number.MAX_SAFE_INTEGER) =>
  typeof n === "number" && Number.isFinite(n) && n > 0 && n <= max ? n : 0;

export function compareQuality(a: VideoQuality = {}, b: VideoQuality = {}) {
  const size = (quality: VideoQuality) => {
    const width = positive(quality.width, 32768),
      height = positive(quality.height, 32768);
    return width && height ? [height, width] : [0, 0];
  };
  const left = size(a),
    right = size(b);
  return right[0] - left[0] || right[1] - left[1];
}

export function currentQuality(
  probed: VideoQuality = {},
  rendered?: VideoQuality,
  level?: VideoQuality,
): VideoQuality {
  const width = positive(rendered?.width, 32768);
  const height = positive(rendered?.height, 32768);
  const decoded = width > 0 && height > 0;
  // 多档 HLS 的探测可能来自第一档，实际解码尺寸变化后不能继续套用该档编码信息。
  const same = !decoded || (width === probed.width && height === probed.height);
  const matchingLevel =
    level && (!decoded || (width === level.width && height === level.height))
      ? level
      : undefined;
  return {
    width: decoded ? width : positive(probed.width, 32768),
    height: decoded ? height : positive(probed.height, 32768),
    codec: matchingLevel?.codec || (same ? probed.codec : ""),
    frame_rate:
      positive(matchingLevel?.frame_rate, 1000) ||
      (same ? positive(probed.frame_rate, 1000) : 0),
    bitrate:
      positive(matchingLevel?.bitrate) || (same ? positive(probed.bitrate) : 0),
  };
}

export function qualityLabel(quality: VideoQuality): string {
  const width = Math.round(positive(quality.width, 32768));
  const height = Math.round(positive(quality.height, 32768));
  if (!width || !height) return "画质待识别";
  const tier = height === 2160 ? "4K" : height === 4320 ? "8K" : `${height}p`;
  const codec = (quality.codec || "").toLowerCase();
  const name = /^(h264|avc1|avc3)/.test(codec)
    ? "H.264"
    : /^(hevc|h265|hvc1|hev1)/.test(codec)
      ? "H.265"
      : /^(av1|av01)/.test(codec)
        ? "AV1"
        : /^(vp9|vp09)/.test(codec)
          ? "VP9"
          : codec
            ? codec.toUpperCase()
            : "";
  const fps = positive(quality.frame_rate, 1000);
  return [
    tier,
    `${width} × ${height}`,
    name,
    fps ? `${Number(fps.toFixed(2))} fps` : "",
  ]
    .filter(Boolean)
    .join(" · ");
}
