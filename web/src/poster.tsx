import { useContext, useEffect, useReducer, useRef } from "react";
import { Film, Play, RefreshCw } from "lucide-react";
import { api } from "./api";
import {
  directPosterUrls,
  initialPoster,
  posterKey,
  posterReducer,
} from "./poster-state";
import { doubanPosterUrls, type UserPreferences } from "./image-source";
import { PreferencesContext } from "./preferences";

type Item = Record<string, any>;

export function Poster({ film }: { film: Item }) {
  const preferences = useContext(PreferencesContext);
  const urls =
    film.authorized === false
      ? []
      : doubanPosterUrls(
          directPosterUrls([
            film.cover,
            film.poster,
            ...(film.poster_candidates || []),
          ]),
          preferences,
        );
  // 图片地址更新后清除旧的失败状态；进度变化不会重新加载图片。
  const identity = JSON.stringify([
    film.id,
    film.recommendation_kind,
    urls,
    preferences,
    film.authorized,
  ]);
  return (
    <PosterImage
      key={identity}
      film={film}
      urls={urls}
      preferences={preferences}
    />
  );
}

function PosterImage({
  film,
  urls,
  preferences,
}: {
  film: Item;
  urls: unknown[];
  preferences: UserPreferences;
}) {
  const [state, dispatch] = useReducer(posterReducer, urls, initialPoster);
  const key = posterKey(state);
  const src = state.urls[state.index];
  const image = useRef<HTMLImageElement>(null);
  useEffect(() => {
    if (!src || state.exhausted || state.loaded) return;
    // 懒加载图片进入视口后才开始计时，避免离屏封面被误判为失败。
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const target = image.current;
    if (!target) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting) && !timeout) {
          timeout = setTimeout(() => dispatch({ type: "error", key }), 8000);
        }
      },
      { rootMargin: "200px" },
    );
    observer.observe(target);
    return () => {
      observer.disconnect();
      clearTimeout(timeout);
    };
  }, [src, key, state.exhausted, state.loaded]);
  useEffect(() => {
    if (film.authorized === false) return;
    if (!state.exhausted || state.lookup !== "idle") return;
    dispatch({ type: "lookup" });
    const controller = new AbortController();
    const path = film.recommendation_kind
      ? `/recommendations/${encodeURIComponent(film.id)}/posters?kind=${film.recommendation_kind}`
      : `/films/${encodeURIComponent(film.id)}/posters`;
    api(path, "GET", undefined, false, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted)
          dispatch({
            type: "candidates",
            urls: doubanPosterUrls(
              directPosterUrls(value.urls || []),
              preferences,
            ),
          });
      })
      .catch(() => {
        if (!controller.signal.aborted)
          dispatch({ type: "candidates", urls: [] });
      });
    return () => controller.abort();
    // lookup 的变化不应中止正在进行的查询；仅每轮耗尽地址时查询一次。
  }, [state.exhausted, state.round, film.id, film.recommendation_kind]);
  return (
    <div className="poster">
      {src && !state.exhausted ? (
        <img
          ref={image}
          key={key}
          src={src}
          loading="lazy"
          decoding="async"
          referrerPolicy="no-referrer"
          onLoad={() => dispatch({ type: "loaded", key })}
          onError={() => dispatch({ type: "error", key })}
          alt={film.title}
        />
      ) : (
        <div className="poster-fallback">
          <Film size={30} />
          <span>{film.title}</span>
          <small>
            {film.authorized === false
              ? "当前账号未获使用权限"
              : state.lookup === "done"
                ? "暂无可用封面"
                : "正在查找封面…"}
          </small>
          {state.lookup === "done" && (
            <button
              type="button"
              className="poster-retry"
              aria-label={`重试${film.title}的封面`}
              onClick={(e) => {
                e.stopPropagation();
                dispatch({ type: "retry" });
              }}
            >
              <RefreshCw size={13} /> 重试封面
            </button>
          )}
        </div>
      )}
      <span className="poster-play">
        <Play size={23} fill="currentColor" />
      </span>
      {film.rate && <span className="score">{film.rate}</span>}
      {film.available === false && (
        <span className="tag-off">
          {film.authorized === false ? "未获授权" : "来源失效"}
        </span>
      )}
      {film.source_count > 1 && (
        <span className="source-count">{film.source_count} 个源</span>
      )}
      {film.progress?.duration > 0 && (
        <div className="progress-bar">
          <i
            style={{
              width:
                Math.min(
                  100,
                  (film.progress.position / film.progress.duration) * 100,
                ) + "%",
            }}
          />
        </div>
      )}
    </div>
  );
}
