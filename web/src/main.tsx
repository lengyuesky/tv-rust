import React, {
  useState,
  useEffect,
  useRef,
  useCallback,
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
} from "lucide-react";
import "./style.css";

type Obj = Record<string, any>;
async function api(
  path: string,
  method = "GET",
  data?: unknown,
  keepalive = false,
): Promise<any> {
  const r = await fetch("/api" + path, {
    method,
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", "X-TV-Request": "1" },
    body: data === undefined ? undefined : JSON.stringify(data),
    keepalive,
  });
  const value = await r.json();
  if (!r.ok) throw new Error(value.error || `请求失败（${r.status}）`);
  return value;
}
const time = (s: number) =>
  `${Math.floor((s || 0) / 60)}:${String(Math.floor((s || 0) % 60)).padStart(2, "0")}`;
const date = (s: number) =>
  new Date(s * 1000).toLocaleString("zh-CN", { hour12: false });
let finishActivePlayback: (() => Promise<void>) | null = null;
const go = (path: string) => {
  void Promise.resolve(finishActivePlayback?.())
    .then(() => {
      location.hash = path;
    })
    .catch(() => {});
};
function useRoute() {
  const [route, set] = useState(location.hash.slice(1) || "/");
  useEffect(() => {
    const fn = async () => {
      try {
        await finishActivePlayback?.();
        set(location.hash.slice(1) || "/");
        window.scrollTo(0, 0);
      } catch {
        // 最终快照未保存时保留播放器，让用户看到同步错误并重试。
        history.replaceState(null, "", "#" + route);
      }
    };
    addEventListener("hashchange", fn);
    return () => removeEventListener("hashchange", fn);
  }, [route]);
  return route;
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
function Poster({ film }: { film: Obj }) {
  const [failed, setFailed] = useState(false);
  const src = film.cover || (film.poster ? `/api/posters/${film.id}` : "");
  return (
    <div className="poster">
      {src && !failed ? (
        <img
          src={src}
          loading="lazy"
          referrerPolicy="no-referrer"
          onError={() => setFailed(true)}
          alt={film.title}
        />
      ) : (
        <div className="poster-fallback">
          <Film size={30} />
          <span>{film.title}</span>
        </div>
      )}
      <span className="poster-play">
        <Play size={23} fill="currentColor" />
      </span>
      {film.rate && <span className="score">{film.rate}</span>}
      {film.available === false && <span className="tag-off">来源失效</span>}
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
function Cards({ items, onOpen }: { items: Obj[]; onOpen?: (f: Obj) => void }) {
  return (
    <div className="cards">
      {items.map((f, i) => (
        <button
          key={`${f.id}-${i}`}
          className="card"
          onClick={() => (onOpen ? onOpen(f) : go("/film/" + f.id))}
        >
          <Poster film={f} />
          <strong>{f.title}</strong>
          <div className="card-meta">
            <span>{f.year || f.remarks || "等待探索"}</span>
            <span>
              {f.progress ? time(f.progress.position) : f.source_name || ""}
            </span>
          </div>
        </button>
      ))}
    </div>
  );
}
function Home({ me }: { me: Obj }) {
  const [sources, setSources] = useState<Obj[]>([]),
    [favorites, setFavorites] = useState<Obj[]>([]),
    [history, setHistory] = useState<Obj[]>([]),
    [rec, setRec] = useState<Obj[]>([]),
    [kind, setKind] = useState("movie"),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(true),
    [q, setQ] = useState("");
  useEffect(() => {
    Promise.all([api("/sources"), api("/favorites"), api("/history")])
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
  }, [kind]);
  const search = (e: React.FormEvent) => {
    e.preventDefault();
    if (q.trim()) go("/search?q=" + encodeURIComponent(q));
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
                : "请管理员导入订阅，之后即可开始搜索和观看。"}
            </p>
          </div>
          {me.user.admin && (
            <button className="outline" onClick={() => go("/settings")}>
              <Plus size={16} />
              导入订阅
            </button>
          )}
        </div>
      )}
      {history.length > 0 && (
        <section>
          <div className="section-title">
            <h2>
              继续观看 <span>CONTINUE WATCHING</span>
            </h2>
            <a href="#/history">
              全部记录 <ArrowRight size={16} />
            </a>
          </div>
          <Cards items={history.slice(0, 6)} />
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
          <Busy />
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
              onClick={() => setKind(kind === "movie" ? "tv" : "movie")}
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
function SearchPage({ route }: { route: string }) {
  const params = new URLSearchParams(route.split("?")[1] || "");
  const query = params.get("q") || "";
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
    [expanded, setExpanded] = useState<string | null>(null);
  const load = useCallback(
    async (pg: number) => {
      setBusy(true);
      setError("");
      try {
        const r = await api(
          "/search?q=" + encodeURIComponent(query) + "&page=" + pg,
        );
        setResult((old) =>
          pg > 1 && old ? { ...r, items: [...old.items, ...r.items] } : r,
        );
        setPage(pg);
      } catch (e) {
        setError((e as Error).message);
      } finally {
        setBusy(false);
      }
    },
    [query],
  );
  useEffect(() => {
    setQ(query);
    setResult(null);
    if (query) void load(1);
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
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="电影、电视剧、动漫…"
          />
          <button className="primary" disabled={busy}>
            搜索
          </button>
        </form>
      </div>
      {error && <Notice kind="error">{error}</Notice>}
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
                    <small>{s.ok ? `${s.count} 条` : s.error}</small>
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
      {busy && <Busy />}
      {expanded && (
        <div className="modal-backdrop" onClick={() => setExpanded(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <button
              className="close"
              onClick={() => setExpanded(null)}
              aria-label="关闭"
            >
              <X />
            </button>
            <span className="eyebrow">CHOOSE A SOURCE</span>
            <h2>{grouped.get(expanded)?.[0].title}</h2>
            <p>选择来源。各版本的集数、画质可能不同。</p>
            <div className="choose-sources">
              {grouped.get(expanded)?.map((f) => (
                <button key={f.id} onClick={() => openFilm(f.id)}>
                  <span>
                    <strong>{f.source_name}</strong>
                    <small>
                      {f.year} · {f.remarks}
                    </small>
                  </span>
                  <ArrowRight size={18} />
                </button>
              ))}
            </div>
          </div>
        </div>
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
};
type PlayerHandle = { finish: () => Promise<void> };
type PlayerProps = {
  session: PlayerState;
  onClose: () => void;
  onNext?: () => void;
};
const Player = forwardRef<PlayerHandle, PlayerProps>(function Player(
  { session, onClose, onNext },
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
    [speed, setSpeed] = useState("1");
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
    const loaded = () => {
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
  return (
    <div className="player-shell">
      <div className="player-top">
        <span className={`mode-pill ${state.mode}`}>
          <span />
          {state.mode === "direct" ? "直链播放" : "转发播放"}
        </span>
        <span>{switching ? "正在切换转发…" : "原始画质"}</span>
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
              if (video.current)
                video.current.playbackRate = Number(e.target.value);
            }}
          >
            {["0.75", "1", "1.25", "1.5", "2"].map((v) => (
              <option key={v}>{v}</option>
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
          onClick={() => fallback()}
        >
          转发重试
        </button>
        {!state.allow_proxy && (
          <span className="muted small">管理员已禁止转发</span>
        )}
        {onNext && (
          <button className="outline" onClick={next}>
            下一集 <ArrowRight size={16} />
          </button>
        )}
        <a href="#/logs" className="muted small">
          查看播放日志
        </a>
      </div>
    </div>
  );
});
function FilmPage({ id, switchFrom }: { id: string; switchFrom: string }) {
  const player = useRef<PlayerHandle>(null);
  const [film, setFilm] = useState<Obj | null>(null),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [line, setLine] = useState(""),
    [session, setSession] = useState<PlayerState | null>(null),
    [playingId, setPlayingId] = useState(""),
    [selecting, setSelecting] = useState(false);
  useEffect(() => {
    let active = true;
    setSession(null);
    setFilm(null);
    setError("");
    api(
      "/films/" +
        id +
        (switchFrom ? "?switch_from=" + encodeURIComponent(switchFrom) : ""),
    )
      .then((f) => {
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
      })
      .catch((e) => {
        if (active) setError(e.message);
      });
    return () => {
      active = false;
    };
  }, [id, switchFrom]);
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
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await player.current?.finish();
      setSession(null);
      const s = await api("/plays", "POST", { episode_id: eid });
      setSession(s);
      setPlayingId(eid);
      setFilm((f) => f && { ...f, switch_hint: null });
      setSelecting(false);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  if (!film) return error ? <Notice kind="error">{error}</Notice> : <Busy />;
  const lines = [
    ...new Set<string>(film.episodes.map((e: Obj) => e.line_name)),
  ];
  const episodes: Obj[] = film.episodes.filter(
    (e: Obj) => e.line_name === line,
  );
  const index = episodes.findIndex((e) => e.id === playingId);
  const hint = film.switch_hint;
  const current = hint
    ? film.episodes.find((e: Obj) => e.id === hint.episode_id)
    : film.episodes.find((e: Obj) => e.id === film.progress?.episode_id) ||
      episodes[0];
  const favorite = async () => {
    try {
      await api(
        "/films/" + id + "/favorite",
        film.favorite ? "DELETE" : "PUT",
        {},
      );
      setFilm({ ...film, favorite: !film.favorite });
    } catch (e) {
      setError((e as Error).message);
    }
  };
  return (
    <>
      <button className="back" onClick={() => history.back()}>
        <ArrowLeft size={16} />
        返回
      </button>
      {session && (
        <Player
          ref={player}
          key={session.id}
          session={session}
          onClose={() => {
            setSession(null);
            void refreshProgress();
          }}
          onNext={
            index >= 0 && index < episodes.length - 1
              ? () => play(episodes[index + 1].id)
              : undefined
          }
        />
      )}
      <div className="film-detail">
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
              onClick={() => play(current.id)}
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
                : film.progress?.position > 0
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
          来源已被移除或禁用，收藏和观看记录仍会保留。请搜索其他视频源。
        </Notice>
      )}
      <section>
        <div className="section-title">
          <h2>
            选集播放 <span>EPISODES</span>
          </h2>
          <button
            className="ghost"
            onClick={() => {
              setSelecting(!selecting);
            }}
          >
            {episodes.length} 个视频 <ChevronDown size={15} />
          </button>
        </div>
        <div className="line-tabs">
          {lines.map((l) => (
            <button
              className={line === l ? "active" : ""}
              key={l}
              onClick={() => setLine(l)}
            >
              {l}
            </button>
          ))}
        </div>
        {episodes.length ? (
          <div className={`episode-grid ${selecting ? "expanded" : ""}`}>
            {episodes.map((e) => (
              <button
                key={e.id}
                disabled={busy || !film.available}
                className={playingId === e.id ? "active" : ""}
                onClick={() => play(e.id)}
              >
                {e.title}
                {playingId === e.id && <Play size={13} />}
              </button>
            ))}
          </div>
        ) : (
          <Empty title="没有找到播放地址">
            这个条目可能需要网页解析，尝试搜索其他视频源。
          </Empty>
        )}
      </section>
    </>
  );
}
function Collection({ kind }: { kind: "favorites" | "history" }) {
  const [items, setItems] = useState<Obj[] | null>(null),
    [error, setError] = useState("");
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
        <Cards items={items} />
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
function SettingsPage({ me, refreshMe }: { me: Obj; refreshMe: () => void }) {
  const [tab, setTab] = useState(me.user.admin ? "sources" : "playback"),
    [data, setData] = useState<Obj | null>(null),
    [users, setUsers] = useState<Obj[]>([]),
    [error, setError] = useState(""),
    [message, setMessage] = useState(""),
    [busy, setBusy] = useState(false),
    [importing, setImporting] = useState(false),
    [mode, setMode] = useState("url");
  const [name, setName] = useState(""),
    [url, setUrl] = useState(""),
    [content, setContent] = useState(""),
    [username, setUsername] = useState(""),
    [password, setPassword] = useState(""),
    [oldPassword, setOldPassword] = useState("");
  const load = () => {
    if (me.user.admin) {
      api("/subscriptions")
        .then(setData)
        .catch((e) => setError(e.message));
      api("/users")
        .then(setUsers)
        .catch((e) => setError(e.message));
    }
  };
  useEffect(load, [me.user.admin]);
  const action = async (fn: () => Promise<unknown>, success = "已保存") => {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await fn();
      setMessage(success);
      load();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const changePolicy = () =>
    action(async () => {
      await api("/settings", "PATCH", { forbid_proxy: !me.forbid_proxy });
      refreshMe();
    });
  const connection = me.public_url || location.origin;
  return (
    <>
      <div className="page-heading">
        <span className="eyebrow">MAKE IT YOURS</span>
        <h1>放映室设置</h1>
        <p>管理片源、播放方式与家庭账号。</p>
      </div>
      <div className="settings-tabs">
        {(me.user.admin
          ? [
              ["sources", "订阅与视频源"],
              ["playback", "播放设置"],
              ["users", "家庭账号"],
              ["connection", "飞牛与数据"],
              ["account", "我的账号"],
            ]
          : [
              ["playback", "播放设置"],
              ["connection", "飞牛连接"],
              ["account", "我的账号"],
            ]
        ).map(([id, label]) => (
          <button
            key={id}
            onClick={() => {
              setTab(id);
              setError("");
              setMessage("");
            }}
            className={tab === id ? "active" : ""}
          >
            {label}
          </button>
        ))}
      </div>
      {error && <Notice kind="error">{error}</Notice>}
      {message && (
        <div className="success">
          <Check size={16} />
          {message}
        </div>
      )}
      {tab === "sources" && me.user.admin && (
        <>
          <div className="section-title">
            <h2>配置订阅</h2>
            <button className="primary" onClick={() => setImporting(true)}>
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
                <button className="outline" onClick={() => setImporting(true)}>
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
                    {s.error && (
                      <p className="text-error">
                        {s.error} · 继续使用上次有效配置
                      </p>
                    )}
                  </div>
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
                    onClick={() =>
                      action(
                        () => api("/subscriptions/" + s.id, "DELETE", {}),
                        "订阅已停用，收藏和历史仍保留",
                      )
                    }
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
                  视频源 <span>{data.sources.length} SOURCES</span>
                </h2>
              </div>
              <div className="source-table">
                {data.sources.map((s: Obj) => (
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
                    .then(() => setMessage("连接地址已复制"))
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
        <div
          className="modal-backdrop"
          onClick={() => !busy && setImporting(false)}
        >
          <form
            className="modal"
            onClick={(e) => e.stopPropagation()}
            onSubmit={(e) => {
              e.preventDefault();
              action(async () => {
                await api("/subscriptions", "POST", {
                  name,
                  url: mode === "url" ? url : "",
                  content: mode === "text" ? content : "",
                });
                setImporting(false);
                setName("");
                setUrl("");
                setContent("");
              }, "订阅导入成功");
            }}
          >
            <button
              className="close"
              type="button"
              onClick={() => setImporting(false)}
              aria-label="关闭"
            >
              <X />
            </button>
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
                    if (f && f.size <= 4 * 1024 * 1024)
                      void f.text().then(setContent);
                    else setError("文件不能超过 4 MiB");
                  }}
                />
              </label>
            )}
            {error && <Notice kind="error">{error}</Notice>}
            <button className="primary wide" disabled={busy}>
              {busy ? (
                <Loader2 className="spin" size={16} />
              ) : (
                <Plus size={16} />
              )}
              导入配置
            </button>
          </form>
        </div>
      )}
    </>
  );
}
function App() {
  const route = useRoute();
  const [initialized, setInitialized] = useState<boolean | null>(null),
    [me, setMe] = useState<Obj | null>(null),
    [ready, setReady] = useState(false),
    [error, setError] = useState("");
  const refreshMe = useCallback(() => {
    api("/me")
      .then(setMe)
      .catch(() => setMe(null))
      .finally(() => setReady(true));
  }, []);
  useEffect(() => {
    api("/status")
      .then((s) => setInitialized(s.initialized))
      .catch((e) => setError(e.message));
    refreshMe();
  }, [refreshMe]);
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
  const nav = [
    ["/", "发现", Clapperboard],
    ["/favorites", "收藏", Heart],
    ["/history", "最近观看", Clock],
    ["/logs", "播放日志", Activity],
    ["/settings", "设置", Settings],
  ] as const;
  let page: React.ReactNode;
  if (route.startsWith("/search")) page = <SearchPage route={route} />;
  else if (route.startsWith("/film/")) {
    const [id, query] = route.slice(6).split("?");
    page = (
      <FilmPage
        key={route}
        id={id}
        switchFrom={new URLSearchParams(query).get("switch_from") || ""}
      />
    );
  } else if (route === "/favorites" || route === "/history")
    page = <Collection kind={route.slice(1) as "favorites" | "history"} />;
  else if (route === "/logs") page = <Logs />;
  else if (route === "/settings")
    page = <SettingsPage me={me} refreshMe={refreshMe} />;
  else page = <Home me={me} />;
  return (
    <>
      <header>
        <div className="header-inner">
          <Brand />
          <nav>
            {nav.map(([href, label, Icon]) => (
              <a
                href={"#" + href}
                className={route === href ? "active" : ""}
                key={href}
              >
                <Icon size={17} />
                <span>{label}</span>
              </a>
            ))}
          </nav>
          <div className="header-actions">
            <a className="icon-button" href="#/search" aria-label="搜索">
              <Search size={19} />
            </a>
            <span className="avatar" title={me.user.username}>
              {me.user.username[0].toUpperCase()}
            </span>
            <button
              className="icon-button"
              title="退出登录"
              onClick={() => {
                void Promise.resolve(finishActivePlayback?.())
                  .then(() => api("/logout", "POST", {}))
                  .then(() => setMe(null))
                  .catch(() => {});
              }}
            >
              <LogOut size={17} />
            </button>
          </div>
        </div>
      </header>
      <main className="page">{page}</main>
      <footer>
        <Brand />
        <span>好故事，总有下一幕。</span>
        <small>TV RUST · {me.version}</small>
      </footer>
    </>
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
