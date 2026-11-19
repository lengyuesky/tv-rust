import assert from "node:assert/strict";
import { test } from "node:test";
import { defaultPreferences, doubanPosterUrls } from "../src/image-source.ts";
import {
  directPosterUrls,
  initialPoster,
  posterKey,
  posterReducer,
} from "../src/poster-state.ts";

test("豆瓣封面使用账号首选 CDN 并保留路径与查询参数", () => {
  const original =
    "https://img3.doubanio.com/view/photo/s_ratio_poster/public/p123.jpg?size=large&token=a%2Fb";
  assert.deepEqual(doubanPosterUrls([original], defaultPreferences), [
    original.replace("img3.doubanio.com", "img.doubanio.cmliussss.net"),
    original.replace("img3.doubanio.com", "img.doubanio.cmliussss.com"),
  ]);
  assert.equal(
    new URL(
      doubanPosterUrls([original], {
        ...defaultPreferences,
        douban_image_source: "cmliussss-cdn-ali",
      })[0],
    ).hostname,
    "img.doubanio.cmliussss.com",
  );
});

test("自定义源沿用 LunaTV URL 编码格式，失败时仍有内置 CDN", () => {
  const original =
    "https://img9.doubanio.com/view/photo/public/a.jpg?token=a%2Fb";
  const prefix = "https://proxy.example/fetch?url=";
  const urls = doubanPosterUrls([original], {
    douban_image_source: "custom",
    douban_image_custom_url: prefix,
  });
  assert.equal(urls[0], prefix + encodeURIComponent(original));
  assert.deepEqual(
    urls.slice(1).map((url) => new URL(url).hostname),
    ["img.doubanio.cmliussss.net", "img.doubanio.cmliussss.com"],
  );
});

test("只替换可信豆瓣图片主机，普通签名图和近似主机保持原样", () => {
  const urls = [
    "https://signed.example/a.jpg?sig=a%2Fb&exp=123",
    "https://img3.doubanio.com.attacker.example/a.jpg",
    "https://other.example/a.jpg?url=https%3A%2F%2Fimg3.doubanio.com",
    "https://img3.doubanio.com:8443/a.jpg",
  ];
  assert.deepEqual(
    doubanPosterUrls(directPosterUrls(urls), defaultPreferences),
    urls,
  );
});

test("CDN 展开后仍能接收同作品备用地址，重试保留完整候选", () => {
  const urls = doubanPosterUrls(
    Array.from(
      { length: 8 },
      (_, index) => `https://img3.doubanio.com/view/photo/public/${index}.jpg`,
    ),
    defaultPreferences,
  );
  let state = initialPoster(urls);
  assert.equal(state.urls.length, 16);
  for (let i = 0; i < 32; i++)
    state = posterReducer(state, { type: "error", key: posterKey(state) });
  assert.equal(state.exhausted, true);
  state = posterReducer(state, {
    type: "candidates",
    urls: ["https://reliable.example/same-work.jpg"],
  });
  assert.equal(
    state.urls[state.index],
    "https://reliable.example/same-work.jpg",
  );
  assert.equal(state.exhausted, false);
  const retry = posterReducer(state, { type: "retry" });
  assert.deepEqual(retry.urls, state.urls);
});
