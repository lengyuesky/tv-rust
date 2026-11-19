export function directPosterUrls(values: unknown[], limit = 8): string[] {
  const result = new Set<string>();
  for (const value of values) {
    if (typeof value !== "string" || !value.trim() || value.length > 8192)
      continue;
    try {
      const url = new URL(value.trim());
      if (
        ["http:", "https:"].includes(url.protocol) &&
        !url.username &&
        !url.password
      )
        result.add(value.trim());
    } catch {
      // 地址由服务端按视频源补全；不把无效地址当成本应用的图片路径。
    }
  }
  return [...result].slice(0, limit);
}

export type PosterState = {
  urls: string[];
  index: number;
  attempt: number;
  round: number;
  loaded: boolean;
  exhausted: boolean;
  lookup: "idle" | "loading" | "done";
};
export type PosterAction =
  | { type: "error" | "loaded"; key: string }
  | { type: "lookup" }
  | { type: "candidates"; urls: unknown[] }
  | { type: "retry" };

export function initialPoster(urls: unknown[]): PosterState {
  const direct = directPosterUrls(urls, 48);
  return {
    urls: direct,
    index: 0,
    attempt: 0,
    round: 0,
    loaded: false,
    exhausted: !direct.length,
    lookup: "idle",
  };
}
export const posterKey = (s: PosterState) =>
  `${s.round}:${s.index}:${s.attempt}`;

export function posterReducer(
  state: PosterState,
  action: PosterAction,
): PosterState {
  switch (action.type) {
    case "loaded":
      return action.key === posterKey(state) && !state.exhausted
        ? { ...state, loaded: true }
        : state;
    case "error":
      if (action.key !== posterKey(state) || state.exhausted) return state;
      if (state.attempt === 0) return { ...state, attempt: 1, loaded: false };
      if (state.index + 1 < state.urls.length)
        return { ...state, index: state.index + 1, attempt: 0, loaded: false };
      return { ...state, exhausted: true, loaded: false };
    case "lookup":
      return { ...state, lookup: "loading" };
    case "candidates": {
      // 为后来查询到的同作品封面保留容量，CDN 展开不能占满原来的八条上限。
      const urls = directPosterUrls(
        [...state.urls, ...directPosterUrls(action.urls, 24)],
        48,
      );
      return urls.length > state.urls.length
        ? {
            ...state,
            urls,
            index: state.urls.length,
            attempt: 0,
            exhausted: false,
            loaded: false,
            lookup: "done",
          }
        : { ...state, lookup: "done" };
    }
    case "retry":
      return { ...initialPoster(state.urls), round: state.round + 1 };
  }
}
