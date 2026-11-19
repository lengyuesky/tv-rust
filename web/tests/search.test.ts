import assert from "node:assert/strict";
import { test } from "node:test";
import { mergeSearchItems } from "../src/search.ts";

test("新加载页的精确片名排在旧页包含匹配之前，重复条目只保留一份", () => {
  const partial = {
    id: "partial",
    title: "乔安你好",
    search_title: "乔安你好",
    search_rank: 1,
  };
  const exact = {
    id: "exact",
    title: "你好",
    search_title: "你好",
    search_rank: 0,
  };
  const result = mergeSearchItems(
    [partial],
    [exact, { ...partial, year: "2019" }],
  );
  assert.deepEqual(
    result.map((v) => v.id),
    ["exact", "partial"],
  );
  assert.equal(result[1].year, "2019");
});

test("同等级结果顺序不受上游响应顺序影响", () => {
  const a = {
    id: "a",
    search_title: "你好",
    search_rank: 0,
    year: "2021",
    source_name: "测试源",
  };
  const b = { ...a, id: "b" };
  assert.deepEqual(mergeSearchItems([], [b, a]), mergeSearchItems([], [a, b]));
});
