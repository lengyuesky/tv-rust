import assert from "node:assert/strict";
import { test } from "node:test";
import {
  directPosterUrls,
  initialPoster,
  posterKey,
  posterReducer,
} from "../src/poster-state.ts";

test("每个地址仅自动重试一次，再使用备用直链，全部失败后停止", () => {
  let state = initialPoster([
    "https://img.example/a.jpg",
    "https://img.example/b.jpg",
  ]);
  for (let i = 0; i < 4; i++) {
    assert.equal(state.exhausted, false);
    state = posterReducer(state, { type: "error", key: posterKey(state) });
  }
  assert.equal(state.exhausted, true);
  assert.equal(state.index, 1);
  assert.equal(state.attempt, 1);
});

test("原图失败后接收新候选，迟到的旧图片事件不能覆盖它", () => {
  let state = initialPoster(["https://img.example/a.jpg"]);
  state = posterReducer(state, { type: "error", key: posterKey(state) });
  const oldKey = posterKey(state);
  state = posterReducer(state, { type: "error", key: oldKey });
  state = posterReducer(state, {
    type: "candidates",
    urls: ["https://img.example/a.jpg", "https://img.example/b.jpg"],
  });
  assert.equal(state.exhausted, false);
  assert.equal(state.index, 1);
  assert.strictEqual(
    posterReducer(state, { type: "error", key: oldKey }),
    state,
  );
  state = posterReducer(state, { type: "loaded", key: posterKey(state) });
  assert.equal(state.loaded, true);
});

test("手动重试开启新一轮，原始签名地址保持不变", () => {
  const signed = "https://img.example/p.jpg?signature=a%2Fb&expires=123";
  let state = initialPoster([signed]);
  state = posterReducer(state, { type: "error", key: posterKey(state) });
  state = posterReducer(state, { type: "error", key: posterKey(state) });
  const oldKey = posterKey(state);
  state = posterReducer(state, { type: "retry" });
  assert.equal(state.exhausted, false);
  assert.equal(state.attempt, 0);
  assert.equal(state.round, 1);
  assert.equal(state.urls[0], signed);
  assert.strictEqual(
    posterReducer(state, { type: "loaded", key: oldKey }),
    state,
  );
});

test("候选地址去重并拒绝带凭据、非HTTP和未补全的路径", () => {
  assert.deepEqual(
    directPosterUrls([
      "https://img.example/a.jpg",
      "https://img.example/a.jpg",
      "/a.jpg",
      "javascript:alert(1)",
      "https://user:pass@img.example/a.jpg",
      "file:///tmp/a.jpg",
    ]),
    ["https://img.example/a.jpg"],
  );
});
