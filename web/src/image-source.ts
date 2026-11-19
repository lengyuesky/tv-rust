export type ImageSource =
  "cmliussss-cdn-tencent" | "cmliussss-cdn-ali" | "custom";
export type UserPreferences = {
  douban_image_source: ImageSource;
  douban_image_custom_url: string;
};

export const defaultPreferences: UserPreferences = {
  douban_image_source: "cmliussss-cdn-tencent",
  douban_image_custom_url: "",
};

export const imageSources: {
  value: ImageSource;
  label: string;
  host?: string;
}[] = [
  {
    value: "cmliussss-cdn-tencent",
    label: "豆瓣 CDN By CMLiussss（腾讯云）",
    host: "img.doubanio.cmliussss.net",
  },
  {
    value: "cmliussss-cdn-ali",
    label: "豆瓣 CDN By CMLiussss（阿里云）",
    host: "img.doubanio.cmliussss.com",
  },
  { value: "custom", label: "自定义图片源" },
];

export function doubanPosterUrls(
  values: string[],
  preferences: UserPreferences,
): string[] {
  const result = new Set<string>();
  for (const original of values) {
    const url = new URL(original);
    const douban =
      /^img\d+\.doubanio\.com$/.test(url.hostname) ||
      imageSources.some((source) => source.host === url.hostname);
    if (!douban || url.port) {
      result.add(original);
      continue;
    }
    if (
      preferences.douban_image_source === "custom" &&
      preferences.douban_image_custom_url
    ) {
      result.add(
        preferences.douban_image_custom_url + encodeURIComponent(original),
      );
    }
    const primary =
      preferences.douban_image_source === "cmliussss-cdn-ali" ? 1 : 0;
    for (const index of [primary, 1 - primary]) {
      const replacement = new URL(original);
      replacement.protocol = "https:";
      replacement.hostname = imageSources[index].host!;
      result.add(replacement.toString());
    }
  }
  return [...result];
}
