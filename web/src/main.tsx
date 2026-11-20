import React, {
  useState,
  useEffect,
  useRef,
  useCallback,
  useMemo,
  useImperativeHandle,
  forwardRef,
} from "react";
import { createRoot } from "react-dom/client";
import type Hls from "hls.js";
import {
  Play,
  Search,
  Heart,
  Clock,
  Settings,
  LogOut,
  ArrowLeft,
  ArrowRight,
  RefreshCw,
  Plus,
  Trash2,
  ShieldCheck,
  Radio,
  Clapperboard,
  Film,
  MonitorPlay,
  Activity,
  Check,
  AlertCircle,
  Download,
  Users,
  Link as LinkIcon,
  X,
  Loader2,
  ChevronDown,
  ChevronUp,
  ArrowDownUp,
  History,
} from "lucide-react";
import "./style.css";
import { api } from "./api";
import { Poster } from "./poster";
import { mergeSearchItems } from "./search";
import {
  defaultPreferences,
  imageSources,
  type UserPreferences,
} from "./image-source";
import { PreferencesContext } from "./preferences";
import {
  Confirm,
  Modal,
  SkeletonCards,
  Toasts,
  clearToasts,
  modalOpen,
  store,
  toast,
} from "./ui";
import {
  compareQuality,
  currentQuality,
  qualityLabel,
  type VideoQuality,
} from "./playback-quality";

type Obj = Record<string, any>;
const time = (s: number) =>
  `${Math.floor((s || 0) / 60)}:${String(Math.floor((s || 0) % 60)).padStart(2, "0")}`;
const date = (s: number) =>
  new Date(s * 1000).toLocaleString("zh-CN", { hour12: false });
const lineLabel = (name: string, index: number) =>
  /\p{Script=Han}/u.test(name) ? name : `线路 ${index + 1}`;
let finishActivePlayback: (() => Promise<void>) | null = null;
const go = (path: string) => {
  void Promise.resolve(finishActivePlayback?.())
    .then(() => {
      location.hash = path;
    })
    .catch(() => {});
};
function useRoute() {
  const [navigation, set] = useState({
    route: location.hash.slice(1) || "/",
    key: 0,
  });
  useEffect(() => {
    let sequence = 0,
      restore = "";
    const fn = async (event: HashChangeEvent) => {
      const request = ++sequence;
      const route = new URL(event.newURL).hash.slice(1) || "/";
      restore ||= new URL(event.oldURL).hash || "#/";
      try {
        await finishActivePlayback?.();
        if (request !== sequence) return;
        // 自动换源后会清理 URL 参数；再次进入相同换源地址仍是一次新导航。
        set((old) => ({ route, key: old.key + 1 }));
        restore = "";
        window.scrollTo(0, 0);
      } catch {
        // 最终快照未保存时保留播放器，让用户看到同步错误并重试。
        if (request === sequence) {
          history.replaceState(null, "", restore);
          restore = "";
        }
      }
    };
    addEventListener("hashchange", fn);
    return () => removeEventListener("hashchange", fn);
  }, []);
  return navigation;
}
function Notice({
  children,
  kind = "info",
}: {
  children: React.ReactNode;
  kind?: string;
}) {
  return (
    <div className={`notice ${kind}`}>
      <AlertCircle size={17} />
      <span>{children}</span>
    </div>
  );
}
function Empty({
  title,
  children,
  action,
}: {
  title: string;
  children: React.ReactNode;
  action?: React.ReactNode;
}) {
  return (
    <div className="empty">
      <div className="empty-icon">
        <Clapperboard size={35} />
      </div>
      <h3>{title}</h3>
      <p>{children}</p>
      {action}
    </div>
  );
}
function Busy() {
  return (
    <div className="loading">
      <Loader2 className="spin" size={23} />
      正在加载…
    </div>
  );
}
function Brand() {
  return (
    <a className="brand" href="#/">
      <span className="brand-icon">
        <Play size={19} fill="currentColor" />
      </span>
      TV<span>RUST</span>
      <small>私人放映室</small>
    </a>
  );
}

function Login({ setup, onLogin }: { setup: boolean; onLogin: () => void }) {
  const [username, setUsername] = useState(""),
    [password, setPassword] = useState(""),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      if (setup) await api("/setup", "POST", { username, password });
      await api("/login", "POST", { username, password });
      onLogin();
    } catch (e) {
      setError(String((e as Error).message));
    } finally {
      setBusy(false);
    }
  };
  return (
    <main className="login-page">
      <div className="login-art">
        <Brand />
        <div>
          <span className="eyebrow">PRESS PLAY. MAKE IT YOURS.</span>
          <h1>
            好故事，
            <br />
            在这里继续。
          </h1>
          <p>
            从一份订阅开始。
            <br />
            收藏、观看，在你的设备间自由接续。
          </p>
          <div className="login-orbit">
            <Film size={80} />
            <div className="orbit-dot" />
          </div>
        </div>
        <small>Rust 驱动 · 直链优先 · 飞牛播放器</small>
      </div>
      <div className="login-form">
        <div>
          <span className="eyebrow">WELCOME TO TV RUST</span>
          <h2>{setup ? "创建你的放映室" : "欢迎回来"}</h2>
          <p>
            {setup
              ? "设置管理员账号，之后即可导入你的视频源。"
              : "登录后，接着看你的好故事。"}
          </p>
          <form onSubmit={submit}>
            <label>
              用户名
              <input
                required
                autoComplete="username"
                minLength={2}
                maxLength={32}
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                placeholder="你的账号"
              />
            </label>
            <label>
              密码
              <input
                required
                type="password"
                minLength={8}
                maxLength={128}
                autoComplete={setup ? "new-password" : "current-password"}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="至少 8 个字符"
              />
            </label>
            {error && <Notice kind="error">{error}</Notice>}
            <button className="primary wide" disabled={busy}>
              {busy ? (
                <Loader2 className="spin" size={18} />
              ) : (
                <ArrowRight size={18} />
              )}{" "}
              {setup ? "创建并进入" : "进入放映室"}
            </button>
          </form>
          <p className="form-foot">
            {setup
              ? "首次启动没有预置视频源。"
              : "网页和飞牛播放器使用同一套账号。"}
          </p>
        </div>
      </div>
    </main>
  );
}
function Cards({
  items,
  onOpen,
  onRemove,
  removing,
  resume,
}: {
  items: Obj[];
  onOpen?: (f: Obj) => void;
  onRemove?: (f: Obj) => void;
  removing?: string;
  resume?: boolean;
}) {
  return (
    <div className="cards">
      {items.map((f) => (
        <article key={f.group_key || f.id} className="card">
          <Poster film={f} />
          <button
            className="card-open"
            aria-label={`打开${f.title}`}
            title={
              resume && f.available !== false && f.progress?.episode_id
                ? `从 ${time(f.progress.position)} 继续播放`
                : undefined
            }
            onClick={() =>
              onOpen
                ? onOpen(f)
                : go(
                    f.authorized === false
                      ? "/search?q=" + encodeURIComponent(f.title)
                      : resume &&
                          f.available !== false &&
                          f.progress?.episode_id
                        ? // 有观看进度的卡片一步续播，播放开始后地址会还原为影片页。
                          `/film/${f.id}?autoplay=1&play_episode=${encodeURIComponent(f.progress.episode_id)}`
                        : "/film/" + f.id,
                  )
            }
          />
          {onRemove && (
            <button
              className="card-remove"
              aria-label={`移除${f.title}的记录`}
              disabled={!!removing}
              onClick={() => onRemove(f)}
            >
              <Trash2 size={15} />
            </button>
          )}
          <strong>{f.title}</strong>
          <div className="card-meta">
            <span>{f.year || f.remarks || "等待探索"}</span>
            <span>
              {f.progress ? time(f.progress.position) : f.source_name || ""}
            </span>
          </div>
        </article>
      ))}
    </div>
  );
}
function Home({ me }: { me: Obj }) {
  const [sources, setSources] = useState<Obj[]>([]),
    [favorites, setFavorites] = useState<Obj[]>([]),
    [history, setHistory] = useState<Obj[]>([]),
    [rec, setRec] = useState<Obj[]>([]),
    [kind, setKindState] = useState(() =>
      store.get("home-kind") === "tv" ? "tv" : "movie",
    ),
    [recommendationRetry, setRecommendationRetry] = useState(0),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(true),
    [q, setQ] = useState("");
  useEffect(() => {
    Promise.all([api("/sources"), api("/favorites"), api("/continue-watching")])
      .then(([s, f, h]) => {
        setSources(s.sources);
        setFavorites(f.items);
        setHistory(h.items);
      })
      .catch((e) => setError(e.message));
  }, []);
  useEffect(() => {
    let active = true;
    setBusy(true);
    setError("");
    api("/recommendations?kind=" + kind)
      .then((r) => {
        if (active) setRec(r.items);
      })
      .catch((e) => {
        if (active) {
          setError(e.message);
          setRec([]);
        }
      })
      .finally(() => {
        if (active) setBusy(false);
      });
    return () => {
      active = false;
    };
  }, [kind, recommendationRetry]);
  const setKind = (value: string) => {
    store.set("home-kind", value);
    setKindState(value);
  };
  const search = (e: React.FormEvent) => {
    e.preventDefault();
    if (q.trim()) go("/search?q=" + encodeURIComponent(q.trim()));
  };
  return (
    <>
      <section className="hero">
        <div className="hero-copy">
          <span className="eyebrow">
            <span className="live-dot" /> YOUR NEXT GREAT STORY
          </span>
          <h1>
            把时间，
            <br />
            留给好故事。
          </h1>
          <p>
            搜索你想看的电影与剧集，
            <br className="mobile-break" />
            剩下的，交给你的私人放映室。
          </p>
          <form className="hero-search" onSubmit={search}>
            <Search size={20} />
            <input
              aria-label="搜索影片"
              placeholder="搜索片名，开启一段好时光"
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
            <button className="primary" aria-label="开始搜索">
              <ArrowRight size={20} />
            </button>
          </form>
          <div className="hero-stats">
            <span>
              <Radio size={14} />
              {sources.length} 个可用源
            </span>
            <span>
              <MonitorPlay size={14} />
              网页与飞牛接续
            </span>
            <span>
              <ShieldCheck size={14} />
              {me.forbid_proxy ? "仅直链播放" : "直链优先"}
            </span>
          </div>
        </div>
        <div className="hero-visual" aria-hidden="true">
          <div className="film-frame frame-back">
            <span>DISCOVER</span>
          </div>
          <div className="film-frame frame-front">
            <div className="frame-number">01</div>
            <Play size={65} strokeWidth={1} fill="currentColor" />
            <span>
              THE NEXT SCENE
              <br />
              <b>IS YOURS.</b>
            </span>
          </div>
          <div className="visual-caption">YOUR LIBRARY. YOUR MOMENT.</div>
        </div>
      </section>
      {sources.length === 0 && (
        <div className="source-empty">
          <div className="icon-block">
            <LinkIcon size={22} />
          </div>
          <div>
            <strong>你的片源，由你选择</strong>
            <p>
              {me.user.admin
                ? "导入一份订阅，即可聚合搜索其中的视频源。"
                : "当前账号没有可用源，请管理员导入或分配订阅。"}
            </p>
          </div>
          {me.user.admin && (
            <button className="outline" onClick={() => go("/settings/sources")}>
              <Plus size={16} />
              导入订阅
            </button>
          )}
        </div>
      )}
      {history.length > 0 && (
        <section aria-label="继续观看">
          <div className="section-title">
            <h2>
              继续观看 <span>CONTINUE WATCHING</span>
            </h2>
            <a href="#/history">
              全部记录 <ArrowRight size={16} />
            </a>
          </div>
          <Cards items={history} resume />
        </section>
      )}
      <section>
        <div className="section-title">
          <h2>
            值得一看 <span>DISCOVER</span>
          </h2>
          <div className="segmented">
            <button
              className={kind === "movie" ? "active" : ""}
              onClick={() => setKind("movie")}
            >
              电影
            </button>
            <button
              className={kind === "tv" ? "active" : ""}
              onClick={() => setKind("tv")}
            >
              剧集
            </button>
          </div>
        </div>
        <p className="section-sub">豆瓣热门 · 选择作品，搜索你的可用视频源</p>
        {busy ? (
          <SkeletonCards />
        ) : rec.length ? (
          <Cards
            items={rec}
            onOpen={(f) => go("/search?q=" + encodeURIComponent(f.title))}
          />
        ) : (
          <div className="quiet-state">
            <Radio size={24} />
            <div>
              <strong>推荐暂时休息一下</strong>
              <p>{error || "稍后再来看看。"} 搜索和个人收藏仍可使用。</p>
            </div>
            <button
              className="ghost"
              onClick={() => setRecommendationRetry((value) => value + 1)}
            >
              <RefreshCw size={16} />
              重试
            </button>
          </div>
        )}
      </section>
      {favorites.length > 0 && (
        <section>
          <div className="section-title">
            <h2>
              我的收藏 <span>MY LIST</span>
            </h2>
            <a href="#/favorites">
              查看全部 <ArrowRight size={16} />
            </a>
          </div>
          <Cards items={favorites.slice(0, 6)} />
        </section>
      )}
    </>
  );
}
const recentKey = (userId: string) => "recent-searches:" + userId;
const readRecent = (userId: string): string[] => {
  try {
    const value = JSON.parse(store.get(recentKey(userId)) || "[]");
    return Array.isArray(value)
      ? value.filter((item) => typeof item === "string").slice(0, 8)
      : [];
  } catch {
    return [];
  }
};
function SearchPage({ route, userId }: { route: string; userId: string }) {
  const params = new URLSearchParams(route.split("?")[1] || "");
  const query = (params.get("q") || "").trim();
  const switchFrom = params.get("switch_from") || "";
  const switchQuery = switchFrom
    ? "&switch_from=" + encodeURIComponent(switchFrom)
    : "";
  const openFilm = (id: string) =>
    go(
      "/film/" +
        id +
        (switchFrom ? "?switch_from=" + encodeURIComponent(switchFrom) : ""),
    );
  const [q, setQ] = useState(query),
    [result, setResult] = useState<Obj | null>(null),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [page, setPage] = useState(1),
    [expanded, setExpanded] = useState<string | null>(null),
    [recent, setRecent] = useState(() => readRecent(userId));
  const input = useRef<HTMLInputElement>(null);
  const requestVersion = useRef(0);
  const pending = useRef<AbortController | null>(null);
  const load = useCallback(
    async (pg: number) => {
      pending.current?.abort();
      const controller = new AbortController();
      pending.current = controller;
      const version = ++requestVersion.current;
      setBusy(true);
      setError("");
      try {
        const r = await api(
          "/search?q=" + encodeURIComponent(query) + "&page=" + pg,
          "GET",
          undefined,
          false,
          controller.signal,
        );
        if (version !== requestVersion.current || controller.signal.aborted)
          return;
        setResult((old) => ({
          ...r,
          items: mergeSearchItems(pg > 1 && old ? old.items : [], r.items),
        }));
        setPage(pg);
        if (pg === 1 && r.items.length) {
          // 只记录确实搜到内容的片名，按账号保存在当前浏览器。
          const next = [
            query,
            ...readRecent(userId).filter((item) => item !== query),
          ].slice(0, 8);
          store.set(recentKey(userId), JSON.stringify(next));
          setRecent(next);
        }
      } catch (e) {
        if (version === requestVersion.current && !controller.signal.aborted)
          setError((e as Error).message);
      } finally {
        if (version === requestVersion.current) setBusy(false);
      }
    },
    [query, userId],
  );
  useEffect(() => {
    if (!query) input.current?.focus();
    setQ(query);
    setResult(null);
    setExpanded(null);
    setPage(1);
    setError("");
    if (query) void load(1);
    else setBusy(false);
    return () => {
      ++requestVersion.current;
      pending.current?.abort();
    };
  }, [query, load]);
  const grouped = new Map<string, Obj[]>();
  for (const f of result?.items || []) {
    const key =
      f.group_key || `${f.title.trim().toLowerCase()}|${f.year}|${f.kind}`;
    const group = grouped.get(key) || [];
    if (!group.some((v) => v.id === f.id)) group.push(f);
    grouped.set(key, group);
  }
  return (
    <>
      <div className="page-heading">
        <span className="eyebrow">SEARCH YOUR SOURCES</span>
        <h1>找到你想看的</h1>
        <form
          className="search-form"
          onSubmit={(e) => {
            e.preventDefault();
            if (q.trim())
              go("/search?q=" + encodeURIComponent(q.trim()) + switchQuery);
          }}
        >
          <Search size={20} />
          <input
            aria-label="影片关键词"
            ref={input}
            type="search"
            enterKeyHint="search"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="电影、电视剧、动漫…"
          />
          <button className="primary" disabled={busy || !q.trim()}>
            搜索
          </button>
        </form>
      </div>
      {!query && recent.length > 0 && (
        <section className="recent-searches" aria-label="最近搜索">
          <div>
            <span>
              <History size={14} />
              最近搜索
            </span>
            <button
              className="ghost"
              onClick={() => {
                store.remove(recentKey(userId));
                setRecent([]);
              }}
            >
              清空
            </button>
          </div>
          <div className="chips">
            {recent.map((item) => (
              <button
                key={item}
                onClick={() =>
                  go("/search?q=" + encodeURIComponent(item) + switchQuery)
                }
              >
                {item}
              </button>
            ))}
          </div>
        </section>
      )}
      {!query && !recent.length && (
        <p className="muted small">
          输入片名开始搜索。结果按作品合并，可在每部作品中选择来源。
        </p>
      )}
      {error && (
        <Notice kind="error">
          {error}
          <button className="inline-action" onClick={() => void load(1)}>
            重试
          </button>
        </Notice>
      )}
      {result && (
        <>
          <div className="search-summary">
            <span>
              “{query}” · {grouped.size} 个作品
            </span>
            <details>
              <summary>
                {result.sources.filter((s: Obj) => s.ok).length}/
                {result.sources.length} 个源响应 <ChevronDown size={13} />
              </summary>
              <div className="source-status">
                {result.sources.map((s: Obj) => (
                  <div key={s.id} className={s.ok ? "" : "failed"}>
                    {s.ok ? <Check size={14} /> : <AlertCircle size={14} />}
                    <span>{s.name}</span>
                    <small>
                      {s.ok
                        ? `${s.count} 条${s.filtered_count ? ` · 已过滤 ${s.filtered_count} 条无关结果` : ""}`
                        : s.error}
                    </small>
                  </div>
                ))}
              </div>
            </details>
          </div>
          <Cards
            items={[...grouped.entries()].map(([key, fs]) => ({
              ...fs[0],
              source_count: fs.length,
              group_key: key,
              poster_candidates: fs.map((f) => f.poster),
            }))}
            onOpen={(f) => {
              const fs = grouped.get(f.group_key)!;
              if (fs.length === 1) openFilm(f.id);
              else setExpanded(f.group_key);
            }}
          />
          {!result.items.length && !busy && (
            <Empty title="还没有找到结果">
              可以尝试更短的片名，或检查订阅中是否有可用的视频源。
            </Empty>
          )}
          {result.has_more && (
            <div className="center">
              <button
                className="outline"
                disabled={busy}
                onClick={() => load(page + 1)}
              >
                加载下一页
              </button>
            </div>
          )}
        </>
      )}
      {busy && (result ? <Busy /> : <SkeletonCards count={12} />)}
      {expanded && (
        <Modal onClose={() => setExpanded(null)}>
          <span className="eyebrow">CHOOSE A SOURCE</span>
          <h2>{grouped.get(expanded)?.[0].title}</h2>
          <p>选择来源。各版本的集数、画质可能不同。</p>
          <div className="choose-sources">
            {grouped.get(expanded)?.map((f) => (
              <button key={f.id} onClick={() => openFilm(f.id)}>
                <span>
                  <strong>{f.source_name}</strong>
                  <small>
                    {[f.year, f.remarks].filter(Boolean).join(" · ") ||
                      "暂无版本信息"}
                  </small>
                </span>
                <ArrowRight size={18} />
              </button>
            ))}
          </div>
        </Modal>
      )}
    </>
  );
}
type PlayerState = {
  id: string;
  url: string;
  mode: string;
  kind: string;
  position: number;
  duration: number;
  allow_proxy: boolean;
  warning?: string;
  quality?: VideoQuality;
  source_name?: string;
  line_name?: string;
  episode_title?: string;
};
type PlayerHandle = { finish: () => Promise<void> };
const speeds = ["0.75", "1", "1.25", "1.5", "2"];
type PlayerProps = {
  session: PlayerState;
  onClose: () => void;
  onNext?: () => void;
  onPrev?: () => void;
  onQuality?: (id: string, quality: VideoQuality) => void;
};
const Player = forwardRef<PlayerHandle, PlayerProps>(function Player(
  { session, onClose, onNext, onPrev, onQuality },
  ref,
) {
  const video = useRef<HTMLVideoElement>(null),
    hls = useRef<Hls | null>(null),
    active = useRef(session),
    position = useRef(session.position),
    ready = useRef(false),
    snapshot = useRef<{ position: number; duration: number } | null>(null),
    saves = useRef<Promise<void>>(Promise.resolve()),
    ending = useRef<Promise<void> | null>(null),
    switchingNow = useRef(false),
    retried = useRef(false),
    reportedPlaying = useRef(false);
  const [state, setState] = useState(session),
    [error, setError] = useState(""),
    [syncError, setSyncError] = useState(""),
    [switching, setSwitching] = useState(false),
    [renderedQuality, setRenderedQuality] = useState<VideoQuality>({}),
    [levelQuality, setLevelQuality] = useState<VideoQuality | undefined>(),
    [speed, setSpeed] = useState(() => {
      // 倍速在换集、换源后沿用上一次的选择。
      const saved = store.get("playback-speed") || "1";
      return speeds.includes(saved) ? saved : "1";
    });
  active.current = state;
  const event = useCallback(
    (e: string) => {
      void api(`/plays/${session.id}/event`, "POST", { event: e }).catch(
        () => {},
      );
    },
    [session.id],
  );
  const remember = useCallback(() => {
    const v = video.current;
    if (
      ready.current &&
      v &&
      Number.isFinite(v.currentTime) &&
      v.currentTime >= 0
    )
      snapshot.current = {
        position: v.currentTime,
        duration: Number.isFinite(v.duration)
          ? v.duration
          : active.current.duration,
      };
  }, []);
  const save = useCallback(() => {
    if (ending.current) return ending.current;
    remember();
    const current = snapshot.current;
    if (!current) return saves.current;
    saves.current = saves.current.then(async () => {
      try {
        await api(`/plays/${session.id}/progress`, "POST", current, true);
        setSyncError("");
      } catch {
        setSyncError("观看进度暂未同步，请检查连接；下一次保存会重新尝试。");
      }
    });
    return saves.current;
  }, [session.id, remember]);
  const finish = useCallback(() => {
    if (ending.current) return ending.current;
    remember();
    video.current?.pause();
    // 结束会话必须排在最后一次进度保存之后，避免关闭或切集时丢失位置。
    const current = snapshot.current;
    ending.current = save()
      .then(async () => {
        await api(`/plays/${session.id}/end`, "POST", current || {}, true);
        setSyncError("");
      })
      .catch((e) => {
        ending.current = null;
        setSyncError(
          "最终观看进度未能同步，播放已暂停。连接恢复后请重试关闭、切集或换源。",
        );
        throw e;
      });
    return ending.current;
  }, [session.id, save, remember]);
  useImperativeHandle(ref, () => ({ finish }), [finish]);
  const fallback = useCallback(
    async (reason = "manual") => {
      if (
        switchingNow.current ||
        ending.current ||
        active.current.mode === "proxy"
      )
        return;
      if (reason !== "manual" && retried.current) return;
      if (!active.current.allow_proxy) {
        setError("管理员已禁止转发；直链不可用时请切换视频源。");
        event("proxy_blocked");
        return;
      }
      retried.current = true;
      switchingNow.current = true;
      setSwitching(true);
      setError("");
      remember();
      position.current = snapshot.current?.position ?? position.current;
      try {
        const r = await api(`/plays/${session.id}/fallback`, "POST", {
          reason,
        });
        setState((s) => ({ ...s, url: r.url, mode: "proxy" }));
      } catch (e) {
        setError((e as Error).message);
      } finally {
        switchingNow.current = false;
        setSwitching(false);
      }
    },
    [session.id, event, remember],
  );
  useEffect(() => {
    const v = video.current!;
    let disposed = false;
    ready.current = false;
    hls.current?.destroy();
    v.pause();
    v.removeAttribute("src");
    v.load();
    reportedPlaying.current = false;
    setRenderedQuality({});
    setLevelQuality(undefined);
    const dimensions = () => {
      if (!disposed && v.videoWidth > 0 && v.videoHeight > 0)
        setRenderedQuality({ width: v.videoWidth, height: v.videoHeight });
    };
    const loaded = () => {
      dimensions();
      if (position.current > 0 && Number.isFinite(v.duration))
        v.currentTime = Math.min(
          position.current,
          Math.max(0, v.duration - 0.1),
        );
      ready.current = true;
      v.playbackRate = Number(speed);
      void v.play().catch(() => {});
    };
    const fail = () => {
      if (disposed) return;
      event("error");
      if (active.current.mode === "direct" && v.error?.code === 2)
        void fallback("network");
      else setError("当前媒体播放失败。可以切换来源，或手动尝试转发。");
    };
    v.addEventListener("loadedmetadata", loaded);
    v.addEventListener("resize", dimensions);
    v.addEventListener("timeupdate", remember);
    v.addEventListener("error", fail);
    if (state.kind === "hls") {
      // 播放 HLS 时再加载播放器库，首页和搜索无需下载这部分代码。
      void import("hls.js")
        .then(({ default: Hls }) => {
          if (disposed) return;
          if (!Hls.isSupported()) {
            v.src = state.url;
            return;
          }
          const instance = new Hls({
            enableWorker: true,
            maxBufferLength: 30,
            manifestLoadingMaxRetry: 1,
            levelLoadingMaxRetry: 1,
            fragLoadingMaxRetry: 2,
          });
          hls.current = instance;
          instance.loadSource(state.url);
          instance.attachMedia(v);
          instance.on(Hls.Events.LEVEL_SWITCHED, (_event, data) => {
            if (disposed) return;
            const level = instance.levels[data.level];
            if (level)
              setLevelQuality({
                width: level.width,
                height: level.height,
                codec: level.videoCodec,
                frame_rate: level.frameRate,
                bitrate: level.bitrate,
              });
            dimensions();
          });
          instance.on(Hls.Events.ERROR, (_event, data) => {
            if (!data.fatal || disposed) return;
            event("error");
            if (
              data.type === Hls.ErrorTypes.NETWORK_ERROR &&
              active.current.mode === "direct"
            )
              void fallback("network");
            else
              setError(
                data.type === Hls.ErrorTypes.MEDIA_ERROR
                  ? "浏览器无法解码此媒体，请换源或使用飞牛播放器。"
                  : "视频资源加载失败，请重试或换源。",
              );
          });
        })
        .catch(() => {
          if (!disposed) setError("播放器组件加载失败，请刷新后重试。");
        });
    } else {
      v.src = state.url;
    }
    return () => {
      remember();
      ready.current = false;
      disposed = true;
      v.removeEventListener("loadedmetadata", loaded);
      v.removeEventListener("resize", dimensions);
      v.removeEventListener("timeupdate", remember);
      v.removeEventListener("error", fail);
      hls.current?.destroy();
      hls.current = null;
    };
    // 切换播放地址时重建播放器，回调经 ref 获取当前会话。
  }, [state.url, state.kind]);
  useEffect(() => {
    finishActivePlayback = finish;
    const timer = setInterval(() => void save().catch(() => {}), 10000);
    const hidden = () => {
      if (document.visibilityState === "hidden") void save().catch(() => {});
    };
    const leaving = (e: PageTransitionEvent) => {
      remember();
      if (!e.persisted) {
        // 窗口关闭时立即发出包含最终快照的请求，后到的旧进度会被已结束会话拒绝。
        void api(
          `/plays/${session.id}/end`,
          "POST",
          snapshot.current || {},
          true,
        ).catch(() => {});
      }
    };
    document.addEventListener("visibilitychange", hidden);
    window.addEventListener("pagehide", leaving);
    return () => {
      if (finishActivePlayback === finish) finishActivePlayback = null;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", hidden);
      window.removeEventListener("pagehide", leaving);
      void finish().catch(() => {});
    };
  }, [session.id, save, finish, remember]);
  const close = () => {
    void finish()
      .then(onClose)
      .catch(() => {});
  };
  const next = () =>
    void finish()
      .then(() => onNext?.())
      .catch(() => {});
  const prev = () =>
    void finish()
      .then(() => onPrev?.())
      .catch(() => {});
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const v = video.current;
      const target = e.target as HTMLElement | null;
      // 视频自身聚焦时由浏览器原生控件处理；输入、按钮和弹窗内不拦截按键。
      if (
        !v ||
        e.ctrlKey ||
        e.metaKey ||
        e.altKey ||
        modalOpen() ||
        target === v ||
        target?.closest("input,textarea,select,button,a,[contenteditable]")
      )
        return;
      const seek = (delta: number) => {
        if (Number.isFinite(v.duration))
          v.currentTime = Math.min(
            Math.max(0, v.currentTime + delta),
            v.duration,
          );
      };
      switch (e.key) {
        case " ":
        case "k":
          if (v.paused) void v.play().catch(() => {});
          else v.pause();
          break;
        case "ArrowLeft":
          seek(-10);
          break;
        case "ArrowRight":
          seek(10);
          break;
        case "ArrowUp":
          v.volume = Math.min(1, v.volume + 0.1);
          break;
        case "ArrowDown":
          v.volume = Math.max(0, v.volume - 0.1);
          break;
        case "m":
          v.muted = !v.muted;
          break;
        case "f":
          if (document.fullscreenElement) void document.exitFullscreen();
          else void v.requestFullscreen?.().catch(() => {});
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    document.addEventListener("keydown", key);
    return () => document.removeEventListener("keydown", key);
  }, []);
  const quality = useMemo(
    () => currentQuality(state.quality, renderedQuality, levelQuality),
    [state.quality, renderedQuality, levelQuality],
  );
  useEffect(
    () => onQuality?.(session.id, quality),
    [session.id, quality, onQuality],
  );
  const label = qualityLabel(quality);
  return (
    <div className="player-shell">
      <div className="player-top">
        <span className={`mode-pill ${state.mode}`}>
          <span />
          {state.mode === "direct" ? "直链播放" : "转发播放"}
        </span>
        <span className="player-quality" aria-label="当前画质">
          {switching
            ? "正在切换转发…"
            : label === "画质待识别"
              ? label
              : `${renderedQuality.width ? "实际画质" : "探测画质"}：${label}`}
        </span>
        <button className="ghost" onClick={close}>
          <X size={17} />
          关闭播放器
        </button>
      </div>
      <video
        ref={video}
        controls
        playsInline
        autoPlay
        onPlaying={() => {
          setError("");
          if (!reportedPlaying.current) {
            reportedPlaying.current = true;
            event("playing");
          }
        }}
        onPause={() => void save().catch(() => {})}
        onSeeked={() => void save().catch(() => {})}
        onEnded={() => {
          if (onNext) next();
          else void save().catch(() => {});
        }}
      />
      {error && <Notice kind="error">{error}</Notice>}
      {syncError && <Notice kind="error">{syncError}</Notice>}
      {state.warning && <p className="small muted">{state.warning}</p>}
      <div className="player-tools">
        <label>
          倍速
          <select
            value={speed}
            onChange={(e) => {
              setSpeed(e.target.value);
              store.set("playback-speed", e.target.value);
              if (video.current)
                video.current.playbackRate = Number(e.target.value);
            }}
          >
            {speeds.map((v) => (
              <option key={v} value={v}>
                {v}×
              </option>
            ))}
          </select>
        </label>
        {document.pictureInPictureEnabled && (
          <button
            className="ghost"
            onClick={() =>
              void video.current?.requestPictureInPicture().catch(() => {})
            }
          >
            画中画
          </button>
        )}
        <button
          className="ghost"
          disabled={switching || state.mode === "proxy" || !state.allow_proxy}
          title="直链卡顿或无法加载时，改由本站转发当前视频"
          onClick={() => fallback()}
        >
          转发重试
        </button>
        {!state.allow_proxy && (
          <span className="muted small">管理员已禁止转发</span>
        )}
        {(onPrev || onNext) && (
          <span className="episode-nav">
            {onPrev && (
              <button className="ghost" onClick={prev}>
                <ArrowLeft size={16} /> 上一集
              </button>
            )}
            {onNext && (
              <button className="outline" onClick={next}>
                下一集 <ArrowRight size={16} />
              </button>
            )}
          </span>
        )}
        <span className="muted small shortcut-hint">
          空格 暂停 · ← → 快退/快进 10 秒 · F 全屏 · M 静音
        </span>
        <a href="#/logs" className="muted small">
          查看播放日志
        </a>
      </div>
    </div>
  );
});
function PlaybackSources({
  film,
  session,
  line,
  busy,
  episodeId,
  liveQuality,
  onSelect,
}: {
  film: Obj;
  session: PlayerState | null;
  line: string;
  busy: boolean;
  episodeId: string;
  liveQuality?: VideoQuality;
  onSelect: (id: string, line?: string) => void;
}) {
  const [items, setItems] = useState<Obj[]>([]),
    [status, setStatus] = useState<Obj[]>([]),
    [loading, setLoading] = useState(false),
    [error, setError] = useState(""),
    [qualityRevision, setQualityRevision] = useState(0),
    [qualityResults, setQualityResults] = useState<{
      context: string;
      items: Record<string, Obj>;
    }>({ context: "", items: {} }),
    [qualityLoading, setQualityLoading] = useState(false);
  const qualityStore = useRef(qualityResults);
  const manualQualityRefresh = useRef("");
  const controller = useRef<AbortController | null>(null);
  const sequence = useRef(0);
  const load = useCallback(
    async (discover: boolean, refresh = false) => {
      controller.current?.abort();
      const request = ++sequence.current;
      const abort = new AbortController();
      controller.current = abort;
      setLoading(true);
      setError("");
      try {
        const result = await api(
          `/films/${film.id}/sources${refresh ? "?refresh=1" : discover ? "?discover=1" : ""}`,
          "GET",
          undefined,
          false,
          abort.signal,
        );
        if (request === sequence.current && !abort.signal.aborted) {
          setItems(result.items);
          setStatus(result.sources);
        }
      } catch (error) {
        if (!abort.signal.aborted) setError((error as Error).message);
      } finally {
        if (request === sequence.current && !abort.signal.aborted)
          setLoading(false);
      }
    },
    [film.id],
  );
  useEffect(() => {
    let active = true;
    // 先显示已保存的来源，再自动补齐其他候选，避免等待上游时隐藏可选名称。
    void load(false).then(() => {
      if (active) void load(true);
    });
    return () => {
      active = false;
      controller.current?.abort();
    };
  }, [load]);
  const candidates = useMemo(
    () =>
      [film, ...items.filter((item) => item.id !== film.id)].map<{
        id: string;
        source_name: string;
      }>((item) => ({ id: item.id, source_name: item.source_name })),
    [film, items],
  );
  const candidateIds = candidates
    .map((item) => item.id)
    .sort()
    .join(",");
  const qualityContext = `${film.id}:${episodeId}`;
  useEffect(() => {
    const abort = new AbortController();
    if (qualityStore.current.context !== qualityContext) {
      manualQualityRefresh.current = "";
      qualityStore.current = { context: qualityContext, items: {} };
      setQualityResults(qualityStore.current);
    }
    if (!episodeId) {
      setQualityLoading(false);
      return () => abort.abort();
    }
    const ids = candidateIds
      .split(",")
      .filter(
        (id) => qualityStore.current.items[id]?.revision !== qualityRevision,
      );
    let next = 0;
    const refresh = manualQualityRefresh.current === qualityContext;
    setQualityLoading(ids.length > 0);
    const worker = async () => {
      while (!abort.signal.aborted && next < ids.length) {
        const id = ids[next++];
        try {
          const query = new URLSearchParams({
            switch_from: film.id,
            switch_episode: episodeId,
          });
          if (refresh) query.set("refresh", "1");
          const result = await api(
            `/films/${id}/source-quality?${query}`,
            "GET",
            undefined,
            false,
            abort.signal,
          );
          if (
            abort.signal.aborted ||
            qualityStore.current.context !== qualityContext
          )
            return;
          qualityStore.current = {
            context: qualityContext,
            items: {
              ...qualityStore.current.items,
              [id]: { ...result, revision: qualityRevision },
            },
          };
          setQualityResults(qualityStore.current);
        } catch (error) {
          if (abort.signal.aborted) return;
          if ((error as Obj).status === 403 || (error as Obj).status === 404) {
            setItems((old) => old.filter((item) => item.id !== id));
          }
          if (qualityStore.current.context === qualityContext) {
            qualityStore.current = {
              context: qualityContext,
              items: {
                ...qualityStore.current.items,
                [id]: {
                  quality: {},
                  status: "unknown",
                  revision: qualityRevision,
                },
              },
            };
            setQualityResults(qualityStore.current);
          }
        }
      }
    };
    // 逐个返回结果，慢源不会挡住已有来源；媒体探测另受服务端全局限制。
    void Promise.all(
      Array.from({ length: Math.min(4, ids.length) }, worker),
    ).then(() => {
      if (!abort.signal.aborted) setQualityLoading(false);
    });
    return () => abort.abort();
  }, [film.id, episodeId, candidateIds, qualityContext, qualityRevision]);
  const lines = [
    ...new Set<string>(film.episodes.map((episode: Obj) => episode.line_name)),
  ];
  const results =
    qualityResults.context === qualityContext ? qualityResults.items : {};
  const displayed = candidates
    .map((item) => ({
      ...item,
      measured: results[item.id],
      quality:
        item.id === film.id &&
        session &&
        liveQuality?.width &&
        liveQuality?.height
          ? liveQuality
          : results[item.id]?.quality ||
            (item.id === film.id ? session?.quality : undefined) ||
            {},
    }))
    .sort(
      (a, b) =>
        compareQuality(a.quality, b.quality) ||
        a.source_name.localeCompare(b.source_name, "zh-CN") ||
        a.id.localeCompare(b.id),
    );
  return (
    <section className="playback-sources" aria-label="播放源">
      <div className="playback-source-top">
        <div>
          <strong>当前播放源：{film.source_name}</strong>
          <small>{session?.episode_title || "选择单集开始播放"}</small>
        </div>
        {lines.length > 1 && (
          <label>
            播放线路
            <select
              aria-label="快速切换播放线路"
              disabled={busy}
              value={session?.line_name || line}
              onChange={(event) => onSelect(film.id, event.target.value)}
            >
              {lines.map((name, index) => (
                <option key={name} value={name}>
                  {lineLabel(name, index)}
                </option>
              ))}
            </select>
          </label>
        )}
        <button
          className="ghost"
          disabled={loading || busy}
          onClick={() => {
            manualQualityRefresh.current = qualityContext;
            setQualityRevision((old) => old + 1);
            void load(true, true);
          }}
        >
          <RefreshCw className={loading ? "spin" : ""} size={15} />
          刷新来源
        </button>
      </div>
      <div
        className="playback-source-options"
        role="group"
        aria-label="可切换播放源"
      >
        {displayed.map((item) => (
          <button
            key={item.id}
            className={item.id === film.id ? "active" : ""}
            disabled={busy || item.id === film.id}
            aria-current={item.id === film.id ? "true" : undefined}
            aria-label={item.source_name}
            aria-describedby={`source-quality-${item.id}`}
            title={qualityLabel(item.quality)}
            onClick={() =>
              onSelect(
                item.id,
                item.measured?.status === "measured"
                  ? item.measured.line_name
                  : undefined,
              )
            }
          >
            <strong>
              {item.id === film.id && <Check size={14} />}
              {item.source_name}
            </strong>
            <small id={`source-quality-${item.id}`}>
              {item.measured?.status === "episode_required"
                ? "需选集 · 画质待识别"
                : qualityLabel(item.quality)
                    .split(" · ")
                    .slice(0, 2)
                    .join(" · ")}
            </small>
          </button>
        ))}
      </div>
      {error && <Notice kind="error">{error}</Notice>}
      {loading && (
        <p className="small muted" role="status">
          正在查找其他可用源…
        </p>
      )}
      {qualityLoading && (
        <p className="small muted" role="status">
          正在识别来源画质，按分辨率从高到低排列…
        </p>
      )}
      {!loading && displayed.length <= 1 && (
        <p className="small muted">
          暂未找到标题、年份和版本一致的其他来源，可刷新或在搜索页查找。
        </p>
      )}
      {status.some((source) => !source.ok) && (
        <p className="small muted">
          {status.filter((source) => !source.ok).length}{" "}
          个源暂未响应，已显示其余可用结果。
        </p>
      )}
    </section>
  );
}

function FilmPage({
  id,
  switchFrom,
  switchEpisode,
  switchLine,
  autoPlay,
  playEpisode,
}: {
  id: string;
  switchFrom: string;
  switchEpisode: string;
  switchLine: string;
  autoPlay: boolean;
  playEpisode: string;
}) {
  const player = useRef<PlayerHandle>(null);
  const [liveQuality, setLiveQuality] = useState<{
    id: string;
    quality: VideoQuality;
  } | null>(null);
  const updateQuality = useCallback(
    (id: string, quality: VideoQuality) => setLiveQuality({ id, quality }),
    [],
  );
  const operation = useRef(false),
    pageActive = useRef(true);
  const [film, setFilm] = useState<Obj | null>(null),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [line, setLine] = useState(""),
    [session, setSession] = useState<PlayerState | null>(null),
    [playingId, setPlayingId] = useState(""),
    [selecting, setSelecting] = useState(false),
    [reversed, setReversed] = useState(
      () => store.get("episodes-reversed") === "1",
    ),
    [sourceChoice, setSourceChoice] = useState<{
      film: Obj;
      line: string;
    } | null>(null);
  useEffect(() => {
    pageActive.current = true;
    return () => {
      pageActive.current = false;
    };
  }, []);
  useEffect(() => {
    let active = true;
    setSession(null);
    setFilm(null);
    setError("");
    const query = new URLSearchParams();
    if (switchFrom) query.set("switch_from", switchFrom);
    if (switchEpisode) query.set("switch_episode", switchEpisode);
    if (switchLine) query.set("switch_line", switchLine);
    api(`/films/${id}${query.size ? "?" + query.toString() : ""}`)
      .then(async (f) => {
        if (!active) return;
        setFilm(f);
        setLine(
          f.episodes?.find(
            (e: Obj) =>
              e.id === (f.switch_hint?.episode_id || f.progress?.episode_id),
          )?.line_name ||
            f.episodes?.[0]?.line_name ||
            "",
        );
        if (autoPlay) {
          const episodeId =
            playEpisode &&
            f.episodes.some((episode: Obj) => episode.id === playEpisode)
              ? playEpisode
              : f.switch_hint?.matched
                ? f.switch_hint.episode_id
                : "";
          if (!episodeId || !f.available) return;
          operation.current = true;
          setBusy(true);
          try {
            const started = await api("/plays", "POST", {
              episode_id: episodeId,
            });
            if (!active) {
              void api(`/plays/${started.id}/end`, "POST", {}).catch(() => {});
              return;
            }
            // 同一影片内部切线路承接本次位置；跨来源仍用各自的进度。
            if (id === switchFrom && f.switch_hint?.matched && !playEpisode)
              started.position =
                f.switch_hint.from_position || started.position;
            setSession(started);
            setPlayingId(episodeId);
            setLine(
              f.episodes.find((episode: Obj) => episode.id === episodeId)
                ?.line_name || "",
            );
            setFilm({ ...f, switch_hint: null });
            history.replaceState(null, "", `#/film/${id}`);
          } catch (error) {
            if (active) setError((error as Error).message);
          } finally {
            operation.current = false;
            if (active) setBusy(false);
          }
        }
      })
      .catch((e) => {
        if (active) setError(e.message);
      });
    return () => {
      active = false;
    };
  }, [id, switchFrom, switchEpisode, switchLine, autoPlay, playEpisode]);
  const refreshProgress = async () => {
    try {
      const f = await api("/films/" + id);
      setFilm(
        (old) => old && { ...old, progress: f.progress, switch_hint: null },
      );
    } catch {
      setError(
        "观看已结束，详情页暂未刷新进度。重新打开影片即可读取已保存的位置。",
      );
    }
  };
  const play = async (eid: string) => {
    if (operation.current) return;
    operation.current = true;
    setBusy(true);
    setError("");
    try {
      await player.current?.finish();
      setSession(null);
      const s = await api("/plays", "POST", { episode_id: eid });
      if (!pageActive.current) {
        void api(`/plays/${s.id}/end`, "POST", {}).catch(() => {});
        return;
      }
      setSession(s);
      setPlayingId(eid);
      setFilm((f) => f && { ...f, switch_hint: null });
      setSelecting(false);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      operation.current = false;
      setBusy(false);
    }
  };
  const episodeGrid = useRef<HTMLDivElement>(null);
  const filmTitle = film?.title;
  useEffect(() => {
    if (filmTitle) document.title = `${filmTitle} · TV Rust`;
  }, [filmTitle]);
  const sessionId = session?.id;
  useEffect(() => {
    // 从页面下方的选集开始播放时，把播放器带回视野。
    if (sessionId) window.scrollTo({ top: 0, behavior: "smooth" });
  }, [sessionId]);
  const lastEpisode = film?.progress?.episode_id;
  useEffect(() => {
    // 长剧集的选集区域有独立滚动，定位到正在播放或上次观看的单集。
    const grid = episodeGrid.current;
    const target = grid?.querySelector<HTMLElement>(
      "button.active, button.last",
    );
    if (grid && target && grid.scrollHeight > grid.clientHeight)
      grid.scrollTop =
        target.offsetTop - grid.clientHeight / 2 + target.clientHeight / 2;
  }, [playingId, lastEpisode, line, reversed, selecting, film?.id]);
  if (!film)
    return error ? (
      <>
        <Notice kind="error">{error}</Notice>
        <button className="outline" onClick={() => go("/")}>
          <ArrowLeft size={16} />
          返回首页
        </button>
      </>
    ) : (
      <Busy />
    );
  const episodes: Obj[] = film.episodes.filter(
    (e: Obj) => e.line_name === line,
  );
  const index = episodes.findIndex((e) => e.id === playingId);
  const hint = film.switch_hint;
  const current = hint
    ? episodes.find((e: Obj) => e.id === hint.episode_id)
    : episodes.find((e: Obj) => e.id === playingId) ||
      episodes.find((e: Obj) => e.id === film.progress?.episode_id) ||
      episodes[0];
  const sourceRoute = (target: string, chosenEpisode = "", targetLine = "") => {
    const query = new URLSearchParams({ switch_from: id });
    const originalEpisode = session ? playingId : current?.id;
    if (originalEpisode) query.set("switch_episode", originalEpisode);
    if (targetLine) query.set("switch_line", targetLine);
    if (session) query.set("autoplay", "1");
    if (chosenEpisode) query.set("play_episode", chosenEpisode);
    return `/film/${target}?${query.toString()}`;
  };
  const selectSource = async (target: string, targetLine = "") => {
    if (operation.current) return;
    if (!session) {
      if (target === id) setLine(targetLine);
      else go(sourceRoute(target, "", targetLine));
      return;
    }
    operation.current = true;
    setBusy(true);
    setError("");
    try {
      if (target !== id && !targetLine && playingId) {
        const query = new URLSearchParams({
          switch_from: id,
          switch_episode: playingId,
          selected: "1",
        });
        const measured = await api(`/films/${target}/source-quality?${query}`);
        if (!pageActive.current) return;
        if (measured.status === "measured") targetLine = measured.line_name;
      }
      const route = sourceRoute(target, "", targetLine);
      const prepared = await api(`/films/${route.slice(6)}`);
      if (!pageActive.current) return;
      if (!prepared.available)
        throw new Error(prepared.unavailable_reason || "该来源暂不可用");
      if (prepared.switch_hint?.matched) {
        await player.current?.finish();
        if (pageActive.current) go(route);
      } else {
        setSourceChoice({
          film: prepared,
          line: targetLine || prepared.episodes[0]?.line_name || "",
        });
      }
    } catch (error) {
      if (pageActive.current) setError((error as Error).message);
    } finally {
      operation.current = false;
      if (pageActive.current) setBusy(false);
    }
  };
  const selectSourceEpisode = async (episodeId: string) => {
    if (operation.current || !sourceChoice) return;
    operation.current = true;
    setBusy(true);
    setError("");
    try {
      await player.current?.finish();
      if (pageActive.current)
        go(sourceRoute(sourceChoice.film.id, episodeId, sourceChoice.line));
    } catch (error) {
      if (pageActive.current) setError((error as Error).message);
    } finally {
      operation.current = false;
      if (pageActive.current) setBusy(false);
    }
  };
  const favorite = async () => {
    try {
      await api(
        "/films/" + id + "/favorite",
        film.favorite ? "DELETE" : "PUT",
        {},
      );
      setFilm({ ...film, favorite: !film.favorite });
      toast(film.favorite ? "已取消收藏" : "已加入收藏");
    } catch (e) {
      setError((e as Error).message);
    }
  };
  const sourcesNode = (
    <PlaybackSources
      key="sources"
      film={film}
      session={session}
      line={line}
      busy={busy}
      episodeId={(session ? playingId : current?.id) || ""}
      liveQuality={
        liveQuality?.id === session?.id ? liveQuality?.quality : undefined
      }
      onSelect={(target, targetLine) => void selectSource(target, targetLine)}
    />
  );
  const detailNode = (
    <div className="film-detail" key="detail">
      <div className="detail-poster">
        <Poster film={film} />
      </div>
      <div className="detail-info">
        <span className="eyebrow">
          {film.kind === "TV" ? "SERIES" : "MOVIE"} · {film.source_name}
        </span>
        <h1>{film.title}</h1>
        <div className="film-meta">
          <span>{film.year || "年份未知"}</span>
          <span>{film.category}</span>
          <span>{film.remarks}</span>
        </div>
        <p className="overview">
          {film.overview || "这个视频源暂未提供简介。"}
        </p>
        {film.actor && (
          <p className="credits">
            <span>演员</span>
            {film.actor}
          </p>
        )}
        {film.director && (
          <p className="credits">
            <span>导演</span>
            {film.director}
          </p>
        )}
        <div className="detail-actions">
          <button
            className="primary"
            disabled={busy || !current || !film.available}
            onClick={() => current && void play(current.id)}
          >
            {busy ? (
              <Loader2 className="spin" size={18} />
            ) : (
              <Play size={18} fill="currentColor" />
            )}
            {hint
              ? hint.matched
                ? "播放对应集 · " + hint.episode_title
                : "请先选择对应单集"
              : current?.id === film.progress?.episode_id &&
                  film.progress?.position > 0
                ? "继续观看 · " + time(film.progress.position)
                : "立即播放"}
          </button>
          <button
            className={film.favorite ? "outline liked" : "outline"}
            onClick={favorite}
          >
            <Heart size={17} fill={film.favorite ? "currentColor" : "none"} />
            {film.favorite ? "已收藏" : "收藏"}
          </button>
          <button
            className="ghost"
            onClick={() =>
              go(
                "/search?q=" +
                  encodeURIComponent(film.title) +
                  "&switch_from=" +
                  encodeURIComponent(id),
              )
            }
          >
            <RefreshCw size={16} />
            搜索其他源
          </button>
        </div>
        <p className="muted small">
          换源后请核对集数与版本。无法确认对应单集时重新选集。
        </p>
      </div>
    </div>
  );
  const noticesNode = (
    <React.Fragment key="notices">
      {error && <Notice kind="error">{error}</Notice>}
      {hint && (
        <Notice>
          {hint.from_episode &&
            `原来源 ${hint.from_source} · ${hint.from_episode} · ${time(hint.from_position)}。`}
          {hint.message}
        </Notice>
      )}
      {film.warning && <Notice>{film.warning}</Notice>}
      {film.available === false && (
        <Notice kind="error">
          {film.unavailable_reason || "来源已被移除或禁用"}
          ，收藏和观看记录仍会保留。请搜索其他视频源。
        </Notice>
      )}
    </React.Fragment>
  );
  const episodesNode = (
    <section aria-label="选集播放" key="episodes">
      <div className="section-title">
        <h2>
          选集播放 <span>EPISODES</span>
        </h2>
        <div className="episode-tools">
          <span className="muted small">{episodes.length} 个视频</span>
          {episodes.length > 1 && (
            <button
              className="ghost"
              aria-pressed={reversed}
              title="切换选集排列顺序"
              onClick={() => {
                store.set("episodes-reversed", reversed ? "0" : "1");
                setReversed(!reversed);
              }}
            >
              <ArrowDownUp size={15} />
              {reversed ? "倒序" : "正序"}
            </button>
          )}
          {episodes.length > 24 && (
            <button
              className="ghost"
              aria-expanded={selecting}
              onClick={() => setSelecting(!selecting)}
            >
              {selecting ? "收起" : "展开全部"}
              {selecting ? <ChevronUp size={15} /> : <ChevronDown size={15} />}
            </button>
          )}
        </div>
      </div>
      {episodes.length ? (
        <div
          ref={episodeGrid}
          className={`episode-grid ${selecting ? "expanded" : ""}`}
        >
          {(reversed ? [...episodes].reverse() : episodes).map((e) => (
            <button
              key={e.id}
              disabled={busy || !film.available}
              className={
                session && playingId === e.id
                  ? "active"
                  : !session && lastEpisode === e.id
                    ? "last"
                    : ""
              }
              title={
                !session && lastEpisode === e.id && film.progress?.position > 0
                  ? `上次看到 ${time(film.progress.position)}`
                  : undefined
              }
              onClick={() => play(e.id)}
            >
              {e.title}
              {session && playingId === e.id && <Play size={13} />}
            </button>
          ))}
        </div>
      ) : (
        <Empty title="没有找到播放地址">
          这个条目可能需要网页解析，尝试搜索其他视频源。
        </Empty>
      )}
    </section>
  );
  return (
    <>
      <button
        className="back"
        onClick={() => (history.length > 1 ? history.back() : go("/"))}
      >
        <ArrowLeft size={16} />
        返回
      </button>
      {session && (
        <Player
          ref={player}
          key={session.id}
          session={session}
          onQuality={updateQuality}
          onClose={() => {
            setSession(null);
            void refreshProgress();
          }}
          onPrev={index > 0 ? () => play(episodes[index - 1].id) : undefined}
          onNext={
            index >= 0 && index < episodes.length - 1
              ? () => play(episodes[index + 1].id)
              : undefined
          }
        />
      )}
      {/* 播放时选集紧跟播放器；未播放时先展示影片信息。带 key 的节点换位不会重置状态。 */}
      {session
        ? [sourcesNode, noticesNode, episodesNode, detailNode]
        : [detailNode, noticesNode, sourcesNode, episodesNode]}
      {sourceChoice && (
        <Modal
          onClose={() => setSourceChoice(null)}
          locked={busy}
          closeLabel="关闭换源选集"
        >
          <h2>选择新来源的单集</h2>
          <p>
            {sourceChoice.film.source_name} · {sourceChoice.film.title}
          </p>
          <Notice>
            {sourceChoice.film.switch_hint?.message ||
              "无法确认对应集数，请选择要播放的单集。"}
          </Notice>
          <div className="line-tabs" role="group" aria-label="新来源播放线路">
            {[
              ...new Set<string>(
                sourceChoice.film.episodes.map(
                  (episode: Obj) => episode.line_name,
                ),
              ),
            ].map((name, index) => (
              <button
                key={name}
                className={sourceChoice.line === name ? "active" : ""}
                disabled={busy}
                onClick={() => setSourceChoice({ ...sourceChoice, line: name })}
              >
                {lineLabel(name, index)}
              </button>
            ))}
          </div>
          <div className="episode-grid expanded">
            {sourceChoice.film.episodes
              .filter((episode: Obj) => episode.line_name === sourceChoice.line)
              .map((episode: Obj) => (
                <button
                  key={episode.id}
                  disabled={busy}
                  onClick={() => void selectSourceEpisode(episode.id)}
                >
                  {episode.title}
                </button>
              ))}
          </div>
          {!sourceChoice.film.episodes.length && (
            <p>该来源没有可用的播放地址，请选择其他来源。</p>
          )}
          {error && <Notice kind="error">{error}</Notice>}
        </Modal>
      )}
    </>
  );
}
function Collection({ kind }: { kind: "favorites" | "history" }) {
  const [items, setItems] = useState<Obj[] | null>(null),
    [error, setError] = useState(""),
    [removing, setRemoving] = useState(""),
    [pending, setPending] = useState<Obj | null>(null);
  const remove = async (film: Obj) => {
    if (removing) return;
    setRemoving(film.id);
    setError("");
    try {
      await api(
        `/films/${film.id}/${kind === "favorites" ? "favorite" : "history"}`,
        "DELETE",
        {},
      );
      setItems(
        (current) => current?.filter((item) => item.id !== film.id) || [],
      );
      toast(kind === "favorites" ? "已取消收藏" : "观看记录已移除");
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setRemoving("");
      setPending(null);
    }
  };
  useEffect(() => {
    setItems(null);
    api("/" + kind)
      .then((r) => setItems(r.items))
      .catch((e) => setError(e.message));
  }, [kind]);
  return (
    <>
      <div className="page-heading">
        <span className="eyebrow">
          {kind === "favorites" ? "MY WATCHLIST" : "YOUR WATCHING JOURNEY"}
        </span>
        <h1>
          {kind === "favorites"
            ? "留给下一次的好故事"
            : "看到哪里，就从哪里继续"}
        </h1>
        <p>
          {kind === "favorites"
            ? "你的收藏会同步到飞牛播放器。"
            : "播放进度按账号保存，在网页与飞牛之间接续。"}
        </p>
      </div>
      {error && <Notice kind="error">{error}</Notice>}
      {items === null ? (
        <Busy />
      ) : items.length ? (
        <Cards
          items={items}
          // 取消收藏可以随时加回；观看进度移除后无法找回，先确认。
          onRemove={kind === "favorites" ? remove : setPending}
          removing={removing}
          resume={kind === "history"}
        />
      ) : (
        <Empty
          title={kind === "favorites" ? "收藏夹还很安静" : "还没有观看记录"}
          action={
            <button className="outline" onClick={() => go("/search")}>
              去探索 <ArrowRight size={16} />
            </button>
          }
        >
          找到感兴趣的影片，开启你的观看清单。
        </Empty>
      )}
      {pending && (
        <Confirm
          title="移除这条观看记录？"
          action="移除记录"
          busy={!!removing}
          onClose={() => setPending(null)}
          onConfirm={() => void remove(pending)}
        >
          《{pending.title}》
          {pending.source_name ? ` · ${pending.source_name}` : ""}{" "}
          的观看进度将被清除，收藏不受影响。
        </Confirm>
      )}
    </>
  );
}
function Logs() {
  const [data, setData] = useState<Obj | null>(null),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  const load = () => {
    setBusy(true);
    api("/logs")
      .then(setData)
      .catch((e) => setError(e.message))
      .finally(() => setBusy(false));
  };
  useEffect(load, []);
  const labels: Obj = {
    issued: "已提供地址",
    progress: "已收到进度",
    transferring: "正在转发",
    fallback: "已选择转发",
    ended: "已结束",
    access_revoked: "使用权限已撤回",
  };
  const retry = async (id: string) => {
    try {
      await api("/plays/" + id + "/fallback", "POST", { reason: "manual" });
      setError("");
      load();
    } catch (e) {
      setError((e as Error).message);
    }
  };
  return (
    <>
      <div className="page-heading row-heading">
        <div>
          <span className="eyebrow">PLAYBACK ACTIVITY</span>
          <h1>每一次播放，都有迹可循</h1>
          <p>
            直链与转发分别记录。日志保留 30 天，地址交付不等于实际播放成功。
          </p>
        </div>
        <button className="outline" onClick={load} disabled={busy}>
          <RefreshCw className={busy ? "spin" : ""} size={16} />
          刷新
        </button>
      </div>
      {error && <Notice kind="error">{error}</Notice>}
      {data?.forbid_proxy && (
        <Notice>
          禁止转发已开启。自动回退、手动重试和已签发的转发地址均受此设置约束。
        </Notice>
      )}
      {!data ? (
        <Busy />
      ) : !data.items.length ? (
        <Empty title="播放日志为空">
          开始观看后，这里会显示播放方式和观察到的状态。
        </Empty>
      ) : (
        <div className="log-list">
          {data.items.map((p: Obj) => (
            <article className="log-card" key={p.id}>
              <div className="log-top">
                <div>
                  <span className={`mode-pill ${p.mode}`}>
                    <span />
                    {p.mode === "direct" ? "直链" : "转发"}
                  </span>
                  <strong>{p.title}</strong>
                  <small>{p.episode}</small>
                </div>
                <span className="muted small">{date(p.started_at)}</span>
              </div>
              <div className="log-meta">
                <span>
                  {p.client === "fntv" ? "飞牛播放器" : "网页"} · {p.username}
                </span>
                <span>{p.source}</span>
                <span>{labels[p.state] || p.state}</span>
                <span>
                  {p.reports > 0
                    ? `最近上报 ${time(p.position)}`
                    : "尚无客户端进度回传"}
                </span>
              </div>
              {p.reason && <p className="small">{p.reason}</p>}
              {p.authorized === false && (
                <p className="text-error">{p.unavailable_reason}</p>
              )}
              <details>
                <summary>
                  查看过程 <ChevronDown size={14} />
                </summary>
                <ol className="timeline">
                  {p.events.map((e: Obj, i: number) => (
                    <li key={i}>
                      <time>
                        {new Date(e.at * 1000).toLocaleTimeString("zh-CN", {
                          hour12: false,
                        })}
                      </time>
                      <span>{e.mode === "direct" ? "直链" : "转发"}</span>
                      <p>{e.message}</p>
                    </li>
                  ))}
                </ol>
              </details>
              {p.client === "fntv" && p.mine && !p.ended && (
                <div className="log-retry">
                  <button
                    className="ghost"
                    disabled={data.forbid_proxy || p.mode === "proxy"}
                    onClick={() => retry(p.id)}
                  >
                    为飞牛准备转发重试
                  </button>
                  <small>选择后，在飞牛播放器中重试当前影片。</small>
                </div>
              )}
            </article>
          ))}
        </div>
      )}
    </>
  );
}
type SubscriptionUsage = {
  mode: "all" | "selected" | "admins";
  user_ids: string[];
};
const allUsers = (): SubscriptionUsage => ({ mode: "all", user_ids: [] });

function UsageSelector({
  usage,
  onChange,
  users,
  disabled,
}: {
  usage: SubscriptionUsage;
  onChange: (usage: SubscriptionUsage) => void;
  users: Obj[];
  disabled: boolean;
}) {
  const members = users.filter((user) => !user.admin);
  return (
    <fieldset className="usage-selector" disabled={disabled}>
      <legend>使用用户</legend>
      <div className="segmented">
        <button
          type="button"
          aria-pressed={usage.mode === "all"}
          className={usage.mode === "all" ? "active" : ""}
          onClick={() => onChange(allUsers())}
        >
          所有用户
        </button>
        <button
          type="button"
          aria-pressed={usage.mode === "selected"}
          className={usage.mode === "selected" ? "active" : ""}
          onClick={() =>
            onChange({ mode: "selected", user_ids: usage.user_ids })
          }
        >
          指定用户
        </button>
        <button
          type="button"
          aria-pressed={usage.mode === "admins"}
          className={usage.mode === "admins" ? "active" : ""}
          onClick={() => onChange({ mode: "admins", user_ids: [] })}
        >
          仅管理员
        </button>
      </div>
      <p className="small muted">
        管理员始终可用。所有用户包含以后创建的账号。
      </p>
      {usage.mode === "admins" && (
        <p className="small muted">
          仅管理员账号可以使用，所有普通成员均不可用，无需选择成员。
        </p>
      )}
      {usage.mode === "selected" && (
        <>
          <div className="usage-members">
            {members.map((user) => (
              <label key={user.id}>
                <input
                  type="checkbox"
                  checked={usage.user_ids.includes(user.id)}
                  onChange={(event) =>
                    onChange({
                      mode: "selected",
                      user_ids: event.target.checked
                        ? [...usage.user_ids, user.id]
                        : usage.user_ids.filter((id) => id !== user.id),
                    })
                  }
                />
                <span>
                  {user.username}
                  {user.disabled && <small>已停用 · 启用后恢复授权</small>}
                </span>
              </label>
            ))}
          </div>
          {!members.length && <Notice>请先在“家庭账号”中创建普通成员。</Notice>}
          <p className="small muted">
            至少选择一位成员。修改使用范围会停止已失去授权的本站转发。
          </p>
        </>
      )}
    </fieldset>
  );
}

function SettingsPage({
  me,
  refreshMe,
  updatePreferences,
  routeTab,
}: {
  me: Obj;
  refreshMe: () => void;
  updatePreferences: (value: UserPreferences) => void;
  routeTab: string;
}) {
  const tabs = me.user.admin
    ? [
        ["sources", "订阅与视频源"],
        ["images", "封面设置"],
        ["playback", "播放设置"],
        ["users", "家庭账号"],
        ["connection", "飞牛与数据"],
        ["account", "我的账号"],
      ]
    : [
        ["images", "封面设置"],
        ["playback", "播放设置"],
        ["connection", "飞牛连接"],
        ["account", "我的账号"],
      ];
  // 当前标签来自地址，刷新、后退和分享链接都能回到同一处。
  const tab = tabs.some(([id]) => id === routeTab)
    ? routeTab
    : me.user.admin
      ? "sources"
      : "playback";
  const [data, setData] = useState<Obj | null>(null),
    [users, setUsers] = useState<Obj[]>([]),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [sourceFilter, setSourceFilter] = useState(""),
    [removingSubscription, setRemovingSubscription] = useState<Obj | null>(
      null,
    ),
    [importing, setImporting] = useState(false),
    [mode, setMode] = useState("url");
  const [usage, setUsage] = useState<SubscriptionUsage>(allUsers),
    [editingUsage, setEditingUsage] = useState<Obj | null>(null),
    [imagePreferences, setImagePreferences] = useState<UserPreferences>(
      me.preferences || defaultPreferences,
    );
  const savedPreferences = JSON.stringify(me.preferences || defaultPreferences);
  useEffect(() => {
    setImagePreferences(JSON.parse(savedPreferences));
  }, [savedPreferences]);
  const openImport = () => {
    setUsage(allUsers());
    setError("");
    setImporting(true);
  };
  const [name, setName] = useState(""),
    [url, setUrl] = useState(""),
    [content, setContent] = useState(""),
    [username, setUsername] = useState(""),
    [password, setPassword] = useState(""),
    [oldPassword, setOldPassword] = useState("");
  const actionBusy = useRef(false);
  const load = useCallback(async () => {
    if (me.user.admin) {
      const [subscriptions, users] = await Promise.allSettled([
        api("/subscriptions"),
        api("/users"),
      ]);
      if (subscriptions.status === "fulfilled") setData(subscriptions.value);
      if (users.status === "fulfilled") setUsers(users.value);
      if (subscriptions.status === "rejected") throw subscriptions.reason;
      if (users.status === "rejected") throw users.reason;
    }
  }, [me.user.admin]);
  useEffect(() => {
    void load().catch((e) => setError(e.message));
  }, [load]);
  const action = async (fn: () => Promise<unknown>, success = "已保存") => {
    if (actionBusy.current) return;
    actionBusy.current = true;
    setBusy(true);
    setError("");
    clearToasts();
    try {
      await fn();
      await load();
      toast(success);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      actionBusy.current = false;
      setBusy(false);
    }
  };
  const changePolicy = () =>
    action(async () => {
      await api("/settings", "PATCH", { forbid_proxy: !me.forbid_proxy });
      refreshMe();
    });
  const connection = me.public_url || location.origin;
  const keyword = sourceFilter.trim().toLowerCase();
  const visibleSources: Obj[] = (data?.sources || []).filter(
    (s: Obj) =>
      !keyword || `${s.name} ${s.source_key}`.toLowerCase().includes(keyword),
  );
  return (
    <>
      <div className="page-heading">
        <span className="eyebrow">MAKE IT YOURS</span>
        <h1>放映室设置</h1>
        <p>管理片源、播放方式与家庭账号。</p>
      </div>
      <div className="settings-tabs">
        {tabs.map(([id, label]) => (
          <button
            key={id}
            aria-current={tab === id ? "page" : undefined}
            onClick={() => {
              go("/settings/" + id);
              setError("");
              clearToasts();
              if (me.user.admin && (id === "sources" || id === "users")) {
                void load().catch((error) => setError(error.message));
              }
            }}
            className={tab === id ? "active" : ""}
          >
            {label}
          </button>
        ))}
      </div>
      {error && !importing && !editingUsage && (
        <Notice kind="error">{error}</Notice>
      )}
      {tab === "sources" && me.user.admin && (
        <>
          <div className="section-title">
            <h2>配置订阅</h2>
            <button className="primary" onClick={openImport}>
              <Plus size={16} />
              导入订阅
            </button>
          </div>
          {!data ? (
            <Busy />
          ) : !data.subscriptions.length ? (
            <Empty
              title="从你的第一份订阅开始"
              action={
                <button className="outline" onClick={openImport}>
                  <Plus size={16} />
                  添加订阅
                </button>
              }
            >
              支持 LunaTV JSON、Base58
              文本和远程订阅地址。你决定启用哪些视频源。
            </Empty>
          ) : (
            <div className="subscription-list">
              {data.subscriptions.map((s: Obj) => (
                <article className="subscription" key={s.id}>
                  <div className="icon-block">
                    <LinkIcon size={22} />
                  </div>
                  <div className="subscription-main">
                    <h3>{s.name}</h3>
                    <p className="url">{s.url || "本地文本配置"}</p>
                    <small>
                      上次成功 {date(s.updated_at)} · 刷新间隔{" "}
                      {Math.round(s.interval_seconds / 60)} 分钟
                    </small>
                    <small className="subscription-usage">
                      {s.usage.mode === "all"
                        ? "所有用户可用"
                        : s.usage.mode === "admins"
                          ? "仅管理员可用"
                          : `指定 ${s.usage.user_ids.length} 位成员 · ${s.usage.user_ids.map((id: string) => users.find((user) => user.id === id)?.username || "已移除账号").join("、")}`}{" "}
                      {s.usage.mode !== "admins" && "· 管理员始终可用"}
                    </small>
                    {s.error && (
                      <p className="text-error">
                        {s.error} · 继续使用上次有效配置
                      </p>
                    )}
                  </div>
                  <button
                    className="ghost"
                    disabled={busy}
                    onClick={() => {
                      setEditingUsage(s);
                      setUsage(s.usage);
                      setError("");
                    }}
                  >
                    使用用户
                  </button>
                  <button
                    className="icon-button"
                    title="刷新订阅"
                    disabled={busy || !s.url}
                    onClick={() =>
                      action(
                        () =>
                          api(
                            "/subscriptions/" + s.id + "/refresh",
                            "POST",
                            {},
                          ),
                        "订阅已更新",
                      )
                    }
                  >
                    <RefreshCw size={18} />
                  </button>
                  <button
                    className="icon-button"
                    title="移除订阅"
                    disabled={busy}
                    onClick={() => setRemovingSubscription(s)}
                  >
                    <Trash2 size={17} />
                  </button>
                </article>
              ))}
            </div>
          )}
          {data && data.sources.length > 0 && (
            <>
              <div className="section-title">
                <h2>
                  视频源{" "}
                  <span>
                    已启用{" "}
                    {
                      data.sources.filter(
                        (s: Obj) =>
                          s.enabled && s.present && s.upstream_enabled,
                      ).length
                    }{" "}
                    / {data.sources.length}
                  </span>
                </h2>
                {data.sources.length > 8 && (
                  <input
                    className="source-filter"
                    type="search"
                    aria-label="筛选视频源"
                    placeholder="筛选视频源名称"
                    value={sourceFilter}
                    onChange={(event) => setSourceFilter(event.target.value)}
                  />
                )}
              </div>
              {!visibleSources.length && (
                <p className="muted small">没有名称匹配的视频源。</p>
              )}
              <div className="source-table">
                {visibleSources.map((s: Obj) => (
                  <div className="source-row" key={s.id}>
                    <div>
                      <strong>{s.name}</strong>
                      <small>
                        {
                          data.subscriptions.find(
                            (v: Obj) => v.id === s.subscription_id,
                          )?.name
                        }{" "}
                        · {s.source_key}
                      </small>
                      {s.note && <small>{s.note}</small>}
                    </div>
                    <span className="muted small">
                      {!s.present
                        ? "订阅中已移除"
                        : !s.upstream_enabled
                          ? "上游已停用"
                          : s.enabled
                            ? "已启用"
                            : "已禁用"}
                    </span>
                    <button
                      className={`switch ${s.enabled ? "on" : ""}`}
                      aria-label={`${s.enabled ? "禁用" : "启用"} ${s.name}`}
                      disabled={busy || !s.present || !s.upstream_enabled}
                      onClick={() =>
                        action(() =>
                          api("/sources/" + s.id, "PATCH", {
                            enabled: !s.enabled,
                          }),
                        )
                      }
                    >
                      <i />
                    </button>
                  </div>
                ))}
              </div>
            </>
          )}
        </>
      )}
      {tab === "images" && (
        <form
          className="settings-panel narrow"
          onSubmit={(event) => {
            event.preventDefault();
            void action(async () => {
              const result = await api(
                "/me/preferences",
                "PATCH",
                imagePreferences,
              );
              updatePreferences(result.preferences);
            }, "封面设置已保存到当前账号");
          }}
        >
          <h3>豆瓣图片源</h3>
          <p>选择封面加载服务，保存到你的账号，在不同设备间同步。</p>
          <label>
            图片源
            <select
              aria-label="图片源"
              value={imagePreferences.douban_image_source}
              disabled={busy}
              onChange={(event) =>
                setImagePreferences({
                  ...imagePreferences,
                  douban_image_source: event.target
                    .value as UserPreferences["douban_image_source"],
                })
              }
            >
              {imageSources.map((source) => (
                <option key={source.value} value={source.value}>
                  {source.label}
                </option>
              ))}
            </select>
          </label>
          {imagePreferences.douban_image_source === "custom" && (
            <label>
              自定义图片源前缀
              <input
                aria-label="自定义图片源前缀"
                required
                type="url"
                maxLength={2048}
                value={imagePreferences.douban_image_custom_url}
                disabled={busy}
                onChange={(event) =>
                  setImagePreferences({
                    ...imagePreferences,
                    douban_image_custom_url: event.target.value,
                  })
                }
                placeholder="https://proxy.example.com/fetch?url="
              />
              <small>
                与 LunaTV 格式一致，在此前缀后追加编码后的原始图片地址。
              </small>
            </label>
          )}
          <p className="small muted">
            封面由浏览器直接获取。选中服务失败后自动尝试其他内置
            CDN，再查找同作品备用封面。
          </p>
          <p className="small muted">
            保存后立即生效；其他设备打开页面或切回前台时同步。
          </p>
          <button className="primary" disabled={busy}>
            {busy ? (
              <Loader2 className="spin" size={16} />
            ) : (
              <Check size={16} />
            )}
            保存封面设置
          </button>
        </form>
      )}
      {tab === "playback" && (
        <div className="settings-panel">
          <div className="setting-row">
            <div className="icon-block">
              <ShieldCheck size={24} />
            </div>
            <div>
              <h3>禁止转发</h3>
              <p>
                开启后，网页和飞牛都只能直链播放。直链失败时显示错误，不再自动或手动转发。
              </p>
              <small>开启会终止正在进行的转发；关闭后仍保持直链优先。</small>
            </div>
            <button
              role="switch"
              aria-checked={me.forbid_proxy}
              aria-label="禁止转发"
              className={`switch ${me.forbid_proxy ? "on" : ""}`}
              disabled={busy || !me.user.admin}
              onClick={changePolicy}
            >
              <i />
            </button>
          </div>
          <div className="policy-notes">
            <h3>当前播放规则</h3>
            <p>
              <Check size={16} />
              优先由播放器直接访问视频源。
            </p>
            <p>
              <Check size={16} />
              {me.forbid_proxy
                ? "已禁止所有视频转发。"
                : "仅在明确失败或用户主动选择时尝试转发。"}
            </p>
            <p>
              <Check size={16} />
              播放日志分别记录地址交付、回退和客户端进度。
            </p>
            <p>
              <Check size={16} />
              转发保留原始媒体画质，不转码。
            </p>
          </div>
          {!me.user.admin && <Notice>播放策略由管理员统一管理。</Notice>}
        </div>
      )}
      {tab === "users" && me.user.admin && (
        <div className="two-column">
          <div className="settings-panel">
            <h3>家庭成员</h3>
            {users.map((u) => (
              <div className="user-row" key={u.id}>
                <span className="avatar">{u.username[0].toUpperCase()}</span>
                <div>
                  <strong>{u.username}</strong>
                  <small>
                    {u.admin ? "管理员" : "普通成员"}
                    {u.disabled ? " · 已停用" : ""}
                  </small>
                </div>
                {u.id !== me.user.id && (
                  <button
                    className="ghost"
                    disabled={busy}
                    onClick={() =>
                      action(() =>
                        api("/users/" + u.id, "PATCH", {
                          disabled: !u.disabled,
                        }),
                      )
                    }
                  >
                    {u.disabled ? "启用" : "停用"}
                  </button>
                )}
              </div>
            ))}
          </div>
          <form
            className="settings-panel"
            onSubmit={(e) => {
              e.preventDefault();
              action(async () => {
                await api("/users", "POST", { username, password });
                setUsername("");
                setPassword("");
              }, "成员已创建");
            }}
          >
            <h3>添加家庭成员</h3>
            <p>新成员拥有自己的收藏和观看进度。</p>
            <label>
              用户名
              <input
                required
                minLength={2}
                value={username}
                onChange={(e) => setUsername(e.target.value)}
              />
            </label>
            <label>
              初始密码
              <input
                required
                type="password"
                minLength={8}
                autoComplete="new-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
            <button className="primary" disabled={busy}>
              <Plus size={16} />
              创建账号
            </button>
          </form>
        </div>
      )}
      {tab === "connection" && (
        <div className="two-column">
          <div className="settings-panel">
            <div className="icon-block">
              <MonitorPlay size={25} />
            </div>
            <h3>连接飞牛播放器</h3>
            <p>在飞牛中添加服务器，填写下面的地址，并使用当前账号登录。</p>
            <div className="copy-value">
              <code>{connection}</code>
              <button
                className="ghost"
                onClick={() =>
                  void navigator.clipboard
                    .writeText(connection)
                    .then(() => toast("连接地址已复制"))
                    .catch(() => setError("浏览器不允许复制，请手动选择地址"))
                }
              >
                复制
              </button>
            </div>
            <p className="small muted">
              另一台设备请使用 Docker 主机的局域网地址。HTTPS
              选项与地址协议保持一致。
            </p>
            <div className="connection-steps">
              <span>
                <b>1</b>添加服务器地址
              </span>
              <span>
                <b>2</b>使用相同账号登录
              </span>
              <span>
                <b>3</b>浏览收藏或直接搜索
              </span>
            </div>
          </div>
          {me.user.admin && (
            <div className="settings-panel">
              <div className="icon-block">
                <Download size={25} />
              </div>
              <h3>备份你的放映室</h3>
              <p>
                下载一致性数据库快照，包含账号、配置、收藏、历史和播放日志。
              </p>
              <a className="outline button-link" href="/api/backup">
                <Download size={16} />
                下载数据备份
              </a>
              <p className="small muted">
                恢复时停止容器，用备份替换数据卷中的 tv.sqlite，并移除旧的
                WAL/SHM 文件后启动。详细步骤见项目 README。
              </p>
            </div>
          )}
        </div>
      )}
      {tab === "account" && (
        <form
          className="settings-panel narrow"
          onSubmit={(e) => {
            e.preventDefault();
            action(async () => {
              await api("/password", "POST", {
                current_password: oldPassword,
                password,
              });
              location.reload();
            }, "密码已更新，请重新登录");
          }}
        >
          <h3>修改我的密码</h3>
          <p>账号：{me.user.username}。修改后，所有设备需要重新登录。</p>
          <label>
            当前密码
            <input
              required
              type="password"
              autoComplete="current-password"
              value={oldPassword}
              onChange={(e) => setOldPassword(e.target.value)}
            />
          </label>
          <label>
            新密码
            <input
              required
              type="password"
              autoComplete="new-password"
              minLength={8}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </label>
          <button className="primary" disabled={busy}>
            更新密码
          </button>
        </form>
      )}
      {importing && (
        <Modal onClose={() => setImporting(false)} locked={busy}>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              action(async () => {
                await api("/subscriptions", "POST", {
                  name,
                  url: mode === "url" ? url : "",
                  content: mode === "text" ? content : "",
                  usage,
                });
                setImporting(false);
                setName("");
                setUrl("");
                setContent("");
              }, "订阅导入成功");
            }}
          >
            <span className="eyebrow">ADD YOUR SOURCE</span>
            <h2>导入配置订阅</h2>
            <p>支持 LunaTV 配置格式，自动识别 JSON 和 Base58。</p>
            <label>
              订阅名称
              <input
                required
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="为这份订阅起个名字"
                maxLength={128}
              />
            </label>
            <div className="segmented">
              <button
                type="button"
                className={mode === "url" ? "active" : ""}
                onClick={() => setMode("url")}
              >
                订阅地址
              </button>
              <button
                type="button"
                className={mode === "text" ? "active" : ""}
                onClick={() => setMode("text")}
              >
                配置文本 / 文件
              </button>
            </div>
            {mode === "url" ? (
              <label>
                订阅 URL
                <input
                  required
                  type="url"
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  placeholder="https://…/config.json"
                />
              </label>
            ) : (
              <label>
                JSON 或 Base58 配置
                <textarea
                  required
                  rows={8}
                  value={content}
                  onChange={(e) => setContent(e.target.value)}
                  placeholder={'{"cache_time":7200,"api_site":{…}}'}
                />
                <input
                  type="file"
                  accept=".json,.txt"
                  onChange={(e) => {
                    const f = e.target.files?.[0];
                    if (!f) return;
                    if (f.size <= 4 * 1024 * 1024) {
                      setError("");
                      void f.text().then(setContent);
                    } else setError("文件不能超过 4 MiB");
                  }}
                />
              </label>
            )}
            <UsageSelector
              usage={usage}
              onChange={setUsage}
              users={users}
              disabled={busy}
            />
            {error && <Notice kind="error">{error}</Notice>}
            <button
              className="primary wide"
              disabled={
                busy || (usage.mode === "selected" && !usage.user_ids.length)
              }
            >
              {busy ? (
                <Loader2 className="spin" size={16} />
              ) : (
                <Plus size={16} />
              )}
              导入配置
            </button>
          </form>
        </Modal>
      )}
      {editingUsage && (
        <Modal onClose={() => setEditingUsage(null)} locked={busy}>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void action(async () => {
                await api("/subscriptions/" + editingUsage.id, "PATCH", {
                  usage,
                });
                setEditingUsage(null);
              }, "订阅使用范围已保存");
            }}
          >
            <h2>修改订阅使用用户</h2>
            <p>{editingUsage.name}</p>
            <UsageSelector
              usage={usage}
              onChange={setUsage}
              users={users}
              disabled={busy}
            />
            {error && <Notice kind="error">{error}</Notice>}
            <button
              className="primary wide"
              disabled={
                busy || (usage.mode === "selected" && !usage.user_ids.length)
              }
            >
              {busy ? (
                <Loader2 className="spin" size={16} />
              ) : (
                <Check size={16} />
              )}
              保存使用用户
            </button>
          </form>
        </Modal>
      )}
      {removingSubscription && (
        <Confirm
          title="移除这份订阅？"
          action="移除订阅"
          busy={busy}
          onClose={() => setRemovingSubscription(null)}
          onConfirm={() =>
            void action(
              () =>
                api("/subscriptions/" + removingSubscription.id, "DELETE", {}),
              "订阅已移除，收藏和历史仍保留",
            ).finally(() => setRemovingSubscription(null))
          }
        >
          「{removingSubscription.name}」及其{" "}
          {
            (data?.sources || []).filter(
              (s: Obj) => s.subscription_id === removingSubscription.id,
            ).length
          }{" "}
          个视频源将不再用于搜索和播放。收藏与观看记录会保留并标记为来源失效；重新导入会作为新订阅，不会恢复原有关联。
        </Confirm>
      )}
    </>
  );
}
function HeaderSearch({ hidden }: { hidden: boolean }) {
  const [q, setQ] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    // 按 “/” 直接开始搜索；正在输入或弹窗打开时不抢焦点。
    const key = (e: KeyboardEvent) => {
      if (
        e.key !== "/" ||
        e.ctrlKey ||
        e.metaKey ||
        e.altKey ||
        modalOpen() ||
        (e.target as HTMLElement | null)?.closest(
          "input,textarea,select,[contenteditable]",
        )
      )
        return;
      const target =
        input.current?.offsetParent != null
          ? input.current
          : document.querySelector<HTMLInputElement>(
              ".hero-search input, .search-form input",
            );
      if (target) {
        e.preventDefault();
        target.focus();
      }
    };
    document.addEventListener("keydown", key);
    return () => document.removeEventListener("keydown", key);
  }, []);
  return (
    <form
      // 首页和搜索页已有大搜索框，这里只保留 “/” 快捷键。
      className={hidden ? "header-search hidden" : "header-search"}
      role="search"
      onSubmit={(e) => {
        e.preventDefault();
        if (!q.trim()) return;
        go("/search?q=" + encodeURIComponent(q.trim()));
        setQ("");
        input.current?.blur();
      }}
    >
      <Search size={16} />
      <input
        ref={input}
        type="search"
        enterKeyHint="search"
        aria-label="搜索片名"
        placeholder="搜索片名"
        value={q}
        onChange={(e) => setQ(e.target.value)}
      />
      <kbd aria-hidden="true">/</kbd>
    </form>
  );
}
function UserMenu({ me, onLogout }: { me: Obj; onLogout: () => void }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (e: Event) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
  return (
    <div className="user-menu" ref={root}>
      <button
        className="avatar"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`账号菜单：${me.user.username}`}
        onClick={() => setOpen(!open)}
      >
        {me.user.username[0].toUpperCase()}
      </button>
      {open && (
        <div className="menu" role="menu" onClick={() => setOpen(false)}>
          <div className="menu-head">
            <strong>{me.user.username}</strong>
            <small>{me.user.admin ? "管理员" : "家庭成员"}</small>
          </div>
          <a role="menuitem" href="#/settings/account">
            <Users size={16} />
            我的账号
          </a>
          <a role="menuitem" href="#/settings">
            <Settings size={16} />
            设置
          </a>
          <a role="menuitem" href="#/logs">
            <Activity size={16} />
            播放日志
          </a>
          <button role="menuitem" onClick={onLogout}>
            <LogOut size={16} />
            退出登录
          </button>
        </div>
      )}
    </div>
  );
}
function App() {
  const { route, key: routeKey } = useRoute();
  const [initialized, setInitialized] = useState<boolean | null>(null),
    [me, setMe] = useState<Obj | null>(null),
    [ready, setReady] = useState(false),
    [error, setError] = useState("");
  const meRequest = useRef(0);
  const refreshMe = useCallback(() => {
    const request = ++meRequest.current;
    api("/me")
      .then((value) => {
        if (meRequest.current === request) setMe(value);
      })
      .catch(() => {
        if (meRequest.current === request) setMe(null);
      })
      .finally(() => setReady(true));
  }, []);
  useEffect(() => {
    api("/status")
      .then((s) => setInitialized(s.initialized))
      .catch((e) => setError(e.message));
    refreshMe();
  }, [refreshMe]);
  const userId = me?.user.id;
  useEffect(() => {
    const path = route.split("?")[0];
    const query = new URLSearchParams(route.split("?")[1] || "").get("q");
    const name = !userId
      ? ""
      : path.startsWith("/search")
        ? query
          ? `搜索“${query}”`
          : "搜索"
        : path === "/favorites"
          ? "收藏"
          : path === "/history"
            ? "最近观看"
            : path === "/logs"
              ? "播放日志"
              : path.startsWith("/settings")
                ? "设置"
                : "";
    // 影片页在读取到片名后自行更新标题。
    if (!path.startsWith("/film/") || !userId)
      document.title = name ? `${name} · TV Rust` : "TV Rust · 你的私人放映室";
  }, [route, userId]);
  useEffect(() => {
    if (!userId) return;
    const sync = () => {
      if (document.visibilityState === "hidden") return;
      const request = ++meRequest.current;
      api("/me")
        .then((value) => {
          if (request === meRequest.current)
            setMe((current) => (current?.user.id === userId ? value : current));
        })
        .catch((error) => {
          if (error.status === 401 && request === meRequest.current)
            setMe(null);
        });
    };
    document.addEventListener("visibilitychange", sync);
    window.addEventListener("pageshow", sync);
    return () => {
      document.removeEventListener("visibilitychange", sync);
      window.removeEventListener("pageshow", sync);
    };
  }, [userId]);
  if (error)
    return (
      <main className="page">
        <Notice kind="error">无法连接服务：{error}</Notice>
      </main>
    );
  if (initialized === null || !ready) return <Busy />;
  if (!initialized || !me)
    return (
      <Login
        setup={!initialized}
        onLogin={() => {
          setInitialized(true);
          refreshMe();
        }}
      />
    );
  // 手机底栏用搜索替换播放日志；日志仍可从账号菜单进入。
  const nav = [
    ["/", "发现", Clapperboard, ""],
    ["/search", "搜索", Search, "mobile-only"],
    ["/favorites", "收藏", Heart, ""],
    ["/history", "最近观看", Clock, ""],
    ["/logs", "播放日志", Activity, "desktop-only"],
    ["/settings", "设置", Settings, ""],
  ] as const;
  const path = route.split("?")[0];
  const logout = () => {
    void Promise.resolve(finishActivePlayback?.())
      .then(() => api("/logout", "POST", {}))
      .then(() => {
        ++meRequest.current;
        setMe(null);
      })
      .catch(() => toast("退出失败，请检查连接后重试", "error"));
  };
  let page: React.ReactNode;
  if (route.startsWith("/search"))
    page = <SearchPage route={route} userId={me.user.id} />;
  else if (route.startsWith("/film/")) {
    const [id, query] = route.slice(6).split("?");
    const params = new URLSearchParams(query);
    page = (
      <FilmPage
        key={routeKey}
        id={id}
        switchFrom={params.get("switch_from") || ""}
        switchEpisode={params.get("switch_episode") || ""}
        switchLine={params.get("switch_line") || ""}
        autoPlay={params.get("autoplay") === "1"}
        playEpisode={params.get("play_episode") || ""}
      />
    );
  } else if (route === "/favorites" || route === "/history")
    page = <Collection kind={route.slice(1) as "favorites" | "history"} />;
  else if (route === "/logs") page = <Logs />;
  else if (path === "/settings" || path.startsWith("/settings/"))
    page = (
      <SettingsPage
        key={me.user.id}
        routeTab={path.split("/")[2] || ""}
        me={me}
        refreshMe={refreshMe}
        updatePreferences={(preferences) => {
          ++meRequest.current;
          setMe((current) =>
            current?.user.id === me.user.id
              ? { ...current, preferences }
              : current,
          );
        }}
      />
    );
  else page = <Home me={me} />;
  return (
    <PreferencesContext.Provider value={me.preferences || defaultPreferences}>
      <header>
        <div className="header-inner">
          <Brand />
          <nav aria-label="主导航">
            {nav.map(([href, label, Icon, scope]) => {
              const active =
                href === "/"
                  ? path === "/"
                  : path === href || path.startsWith(href + "/");
              return (
                <a
                  href={"#" + href}
                  className={`${active ? "active" : ""} ${scope}`.trim()}
                  aria-current={active ? "page" : undefined}
                  key={href}
                >
                  <Icon size={17} />
                  <span>{label}</span>
                </a>
              );
            })}
          </nav>
          <div className="header-actions">
            <HeaderSearch hidden={path === "/" || path.startsWith("/search")} />
            <UserMenu me={me} onLogout={logout} />
          </div>
        </div>
      </header>
      <main className="page">{page}</main>
      <footer>
        <Brand />
        <span>好故事，总有下一幕。</span>
        <small>TV RUST · {me.version}</small>
      </footer>
      <Toasts />
    </PreferencesContext.Provider>
  );
}
class Boundary extends React.Component<
  { children: React.ReactNode },
  { error: boolean }
> {
  state = { error: false };
  static getDerivedStateFromError() {
    return { error: true };
  }
  render() {
    return this.state.error ? (
      <main className="page">
        <Notice kind="error">页面遇到问题，请刷新后重试。</Notice>
        <button className="outline" onClick={() => location.reload()}>
          刷新
        </button>
      </main>
    ) : (
      this.props.children
    );
  }
}
createRoot(document.getElementById("root")!).render(
  <Boundary>
    <App />
  </Boundary>,
);
