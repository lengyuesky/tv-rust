import assert from "node:assert/strict";
import { test } from "node:test";
import { currentQuality, qualityLabel } from "../src/playback-quality.ts";

test("画质显示具体分辨率、编码和帧率，未知数据不伪装原画", () => {
  assert.equal(qualityLabel(currentQuality()), "画质待识别");
  assert.equal(
    qualityLabel({
      width: 1920,
      height: 1080,
      codec: "h264",
      frame_rate: 30000 / 1001,
    }),
    "1080p · 1920 × 1080 · H.264 · 29.97 fps",
  );
  assert.equal(
    qualityLabel({ width: 3840, height: 2160, codec: "hevc" }),
    "4K · 3840 × 2160 · H.265",
  );
  assert.equal(qualityLabel({ width: 1920, height: NaN }), "画质待识别");
});

test("HLS 实际解码尺寸优先于首档探测，换档后不保留旧档编码信息", () => {
  const probed = { width: 1920, height: 1080, codec: "h264", frame_rate: 24 };
  const rendered = { width: 1280, height: 720 };
  assert.equal(
    qualityLabel(currentQuality(probed, rendered)),
    "720p · 1280 × 720",
  );
  assert.equal(
    qualityLabel(
      currentQuality(probed, rendered, {
        width: 1280,
        height: 720,
        codec: "hev1.1",
        frame_rate: 30,
      }),
    ),
    "720p · 1280 × 720 · H.265 · 30 fps",
  );
  assert.equal(
    qualityLabel(
      currentQuality(probed, rendered, {
        width: 1920,
        height: 1080,
        codec: "avc1.42e01e",
      }),
    ),
    "720p · 1280 × 720",
  );
});
