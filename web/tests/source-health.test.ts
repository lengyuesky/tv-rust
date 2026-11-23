import assert from "node:assert/strict";
import { test } from "node:test";
import {
  sourceHealthStatus,
  type SourceHealth,
} from "../src/source-health.ts";

const health = (values: Partial<SourceHealth> = {}): SourceHealth => ({
  probed: 0,
  verified: 0,
  rejected: 0,
  web_pages: 0,
  last_probed_at: 0,
  ...values,
});

test("没有探测记录时不展示视频源可用性", () => {
  assert.equal(sourceHealthStatus(), null);
  assert.equal(sourceHealthStatus(health()), null);
});

test("至少三条探测线路全部被拒绝才标记服务器无法访问", () => {
  for (const probed of [1, 2]) {
    assert.equal(
      sourceHealthStatus(health({ probed, rejected: probed }))?.kind,
      "unknown",
    );
  }
  for (const probed of [3, 14]) {
    assert.deepEqual(
      sourceHealthStatus(health({ probed, rejected: probed })),
      {
        kind: "unreachable",
        label: "服务器无法访问",
        title: `近 7 天探测的 ${probed} 条线路全部被上游拒绝（多为地域限制），该源在这台服务器上基本不可用。`,
      },
    );
  }
  assert.equal(
    sourceHealthStatus(health({ probed: 4, rejected: 3 }))?.kind,
    "unknown",
  );
});

test("视频源区分全部可用、部分可用、网页播放器和待确认", () => {
  assert.deepEqual(
    sourceHealthStatus(health({ probed: 5, verified: 5 })),
    {
      kind: "available",
      label: "可用",
      title: "近 7 天探测的 5 条线路全部识别出真实视频。",
    },
  );
  assert.deepEqual(
    sourceHealthStatus(health({ probed: 5, verified: 2, rejected: 3 })),
    {
      kind: "partial",
      label: "部分可用 2/5",
      title: "近 7 天探测的 5 条线路中，2 条识别出真实视频。",
    },
  );
  assert.deepEqual(
    sourceHealthStatus(health({ probed: 3, web_pages: 3 })),
    {
      kind: "web",
      label: "网页播放器",
      title: "近 7 天探测的 3 条线路全部返回网页播放器，不能直接作为媒体播放。",
    },
  );
  assert.equal(
    sourceHealthStatus(health({ probed: 3, rejected: 1, web_pages: 1 }))?.label,
    "待确认",
  );
});
