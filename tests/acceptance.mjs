// 受控源与真实容器的 API 验收；浏览器、飞牛的画面观察另行记录。
// prepare 仅允许空数据库，避免把验收账号与片源导入已有部署。
import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';

const mode = process.argv[2] || 'prepare';
const base = (process.env.TV_TEST_URL || 'http://127.0.0.1:55320').replace(/\/$/, '');
const fixture = process.env.TV_TEST_FIXTURE || 'http://127.0.0.1:55321/config.json';
const out = resolve(process.env.TV_TEST_OUTPUT || 'tmp/continuation-20261112');
const savedSession = mode === 'prepare' ? null : JSON.parse(await readFile(join(out, 'test-session.json'), 'utf8'));
if (savedSession) assert.equal(savedSession.base, base, '验收地址与已保存会话不一致，请指定对应的 TV_TEST_URL 和 TV_TEST_OUTPUT');
const admin = { username: savedSession?.admin.username || 'tvcheck', password: process.env.TV_TEST_PASSWORD || savedSession?.admin.password || randomBytes(24).toString('base64url') };
const member = { username: savedSession?.member.username || 'familycheck', password: process.env.TV_TEST_MEMBER_PASSWORD || savedSession?.member.password || randomBytes(24).toString('base64url') };
await mkdir(out, { recursive: true });
const checks = [];
let cookie = '';

async function api(path, method = 'GET', body, auth = cookie, expected = 200) {
  const r = await fetch(base + '/api' + path, {
    method,
    headers: { 'content-type': 'application/json', 'x-tv-request': '1', ...(auth ? { cookie: auth } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(45000),
  });
  const v = await r.json();
  assert.equal(r.status, expected, `${method} ${path}: ${v.error || r.status}`);
  if (path === '/login' && expected === 200) cookie = r.headers.get('set-cookie').split(';')[0];
  return v;
}
async function configState(state) {
  const r = await fetch(new URL('/__config', fixture), { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ state }) });
  assert.equal(r.status, 200);
}
function base58(input) {
  const alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
  const bytes = Buffer.from(input);
  let n = BigInt('0x' + bytes.toString('hex')), result = '';
  while (n > 0n) { result = alphabet[Number(n % 58n)] + result; n /= 58n; }
  for (const byte of bytes) { if (byte !== 0) break; result = '1' + result; }
  return result;
}
function pass(name) { checks.push(name); console.log(`通过：${name}`); }
async function native(path, body, token) {
  const r = await fetch(base + '/v/api/v1' + path, {
    method: 'POST', headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` },
    body: JSON.stringify(body), signal: AbortSignal.timeout(45000),
  });
  assert.equal(r.status, 200);
  const v = await r.json(); assert.equal(v.code, 0); return v.data;
}
async function snapshot() {
  const settings = await api('/settings');
  const users = await api('/users');
  const subscriptions = await api('/subscriptions');
  const favorites = await api('/favorites');
  const history = await api('/history');
  return {
    settings: { forbid_proxy: settings.forbid_proxy },
    users: users.map(({ id, username, admin, disabled }) => ({ id, username, admin, disabled })).sort((a, b) => a.id.localeCompare(b.id)),
    subscriptions: subscriptions.subscriptions.map(({ id, name, interval_seconds, error }) => ({ id, name, interval_seconds, error })).sort((a, b) => a.id.localeCompare(b.id)),
    sources: subscriptions.sources.map(({ id, subscription_id, source_key, enabled, present, upstream_enabled }) => ({ id, subscription_id, source_key, enabled, present, upstream_enabled })).sort((a, b) => a.id.localeCompare(b.id)),
    favorites: favorites.items.map(f => f.id).sort(),
    progress: history.items.map(f => ({ film_id: f.id, episode_id: f.progress.episode_id, position: f.progress.position, duration: f.progress.duration })).sort((a, b) => a.film_id.localeCompare(b.film_id)),
  };
}

if (mode === 'prepare') {
  const status = await api('/status');
  assert.equal(status.initialized, false, 'prepare 仅用于空的验收数据库；已有部署请使用 snapshot/check');
  await api('/setup', 'POST', admin);
  await api('/login', 'POST', admin);
  assert.equal((await api('/sources')).sources.length, 0);
  pass('首次启动没有预置源');
  await api('/users', 'POST', member);
  await configState('valid');
  const config = await (await fetch(fixture)).json();
  const imported = await api('/subscriptions', 'POST', { name: '远程验收订阅', url: fixture });
  await api('/subscriptions', 'POST', { name: 'JSON 验收订阅', content: JSON.stringify(config) });
  await api('/subscriptions', 'POST', { name: 'Base58 验收订阅', content: base58(JSON.stringify(config)) });
  let data = await api('/subscriptions');
  assert.equal(data.sources.length, 3);
  assert.equal(new Set(data.sources.map(s => s.id)).size, 3);
  assert.equal(new Set(data.sources.map(s => s.source_key)).size, 1);
  const primary = data.sources.find(s => s.subscription_id === imported.id);
  for (const s of data.sources) if (s.id !== primary.id) await api('/sources/' + s.id, 'PATCH', { enabled: false });
  pass('JSON、Base58、URL 导入与多订阅源归属');

  await api('/sources/' + primary.id, 'PATCH', { enabled: false });
  await configState('invalid');
  await api('/subscriptions/' + imported.id + '/refresh', 'POST', {}, cookie, 400);
  data = await api('/subscriptions');
  assert.ok(data.subscriptions.find(s => s.id === imported.id).error);
  assert.equal(data.sources.find(s => s.id === primary.id).present, true);
  assert.equal(data.sources.find(s => s.id === primary.id).enabled, false);
  await configState('valid');
  await api('/subscriptions/' + imported.id + '/refresh', 'POST', {});
  data = await api('/subscriptions');
  assert.equal(data.sources.find(s => s.id === primary.id).enabled, false);
  assert.equal(data.subscriptions.find(s => s.id === imported.id).error, '');
  await api('/sources/' + primary.id, 'PATCH', { enabled: true });
  pass('无效更新保留有效配置、源身份与启停选择');

  const failedConfig = { api_site: { failure: { name: '受控失败源', api: new URL('/failed-api', fixture).href } } };
  await api('/subscriptions', 'POST', { name: '失败源验收订阅', content: JSON.stringify(failedConfig) });
  const alternateConfig = { api_site: { alternate: { name: '换源验收源', api: new URL('/alternate-api', fixture).href } } };
  await api('/subscriptions', 'POST', { name: '换源验收订阅', content: JSON.stringify(alternateConfig) });
  const search = await api('/search?q=' + encodeURIComponent('验收'));
  assert.ok(search.sources.some(s => !s.ok));
  assert.ok(search.items.some(f => f.source_id === primary.id));
  pass('部分源失败时聚合搜索保留可用结果');
  const movie = search.items.find(f => f.title === 'TV Rust 验收电影' && f.source_id === primary.id);
  const tv = search.items.find(f => f.title === 'TV Rust 验收剧集' && f.source_id === primary.id);
  const alternate = search.items.find(f => f.title === tv.title && f.source_id !== primary.id);
  const movieView = await api('/films/' + movie.id);
  const tvView = await api('/films/' + tv.id);
  assert.equal(tvView.episodes.length, 2);
  await api('/films/' + movie.id + '/favorite', 'PUT', {});
  await api('/films/' + tv.id + '/favorite', 'PUT', {});
  const p = await api('/plays', 'POST', { episode_id: movieView.episodes[0].id });
  assert.equal(p.mode, 'direct');
  await api('/plays/' + p.id + '/progress', 'POST', { position: 36, duration: 120 });
  await api('/plays/' + p.id + '/end', 'POST', { position: 37.5, duration: 120 });
  const episode2 = tvView.episodes.find(e => e.title === '第02集');
  const hls = await api('/plays', 'POST', { episode_id: episode2.id });
  assert.equal(hls.kind, 'hls');
  await api('/plays/' + hls.id + '/end', 'POST', { position: 34, duration: hls.duration });
  const switched = await api(`/films/${alternate.id}?switch_from=${tv.id}`);
  assert.equal(switched.switch_hint.matched, true);
  assert.equal(switched.switch_hint.episode_title, '第二集');
  assert.equal(switched.progress, null);
  pass('电影/剧集详情、HLS 识别、最终进度及换源选集提示');

  const nativeLogin = await fetch(base + '/v/api/v2/user/loginByPassword', {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ username: admin.username, password: createHash('sha256').update(admin.password).digest('hex') }),
  });
  assert.equal(nativeLogin.status, 200);
  const token = (await nativeLogin.json()).data.token;
  assert.equal((await native('/favorite/list', { page: 1, page_size: 50 }, token)).list.length, 2);
  assert.equal((await native('/play/info', { media_guid: episode2.id }, token)).ts, 34);
  const originalCookie = cookie;
  await api('/login', 'POST', member);
  const memberCookie = cookie;
  assert.equal((await api('/favorites')).items.length, 0);
  assert.equal((await api('/history')).items.length, 0);
  assert.equal((await api('/logs')).items.length, 0);
  await api('/plays/' + p.id + '/progress', 'POST', { position: 10, duration: 120 }, memberCookie, 403);
  await api('/subscriptions', 'GET', undefined, memberCookie, 403);
  await api('/settings', 'PATCH', { forbid_proxy: true }, memberCookie, 403);
  cookie = originalCookie;
  pass('同一用户网页/飞牛接口共享收藏进度，家庭账号隔离');

  const retry = await api('/plays', 'POST', { episode_id: movieView.episodes[0].id });
  const proxy = await api('/plays/' + retry.id + '/fallback', 'POST', { reason: 'manual' });
  assert.equal(proxy.mode, 'proxy');
  let r = await fetch(base + proxy.url, { headers: { range: 'bytes=0-31' } });
  assert.equal(r.status, 206);
  assert.equal((await r.arrayBuffer()).byteLength, 32);
  await api('/settings', 'PATCH', { forbid_proxy: true });
  r = await fetch(base + proxy.url); assert.equal(r.status, 403); await r.text();
  await api('/plays/' + retry.id + '/fallback', 'POST', { reason: 'manual' }, cookie, 403);
  const directOnly = await api('/plays', 'POST', { episode_id: movieView.episodes[0].id });
  assert.equal(directOnly.mode, 'direct'); assert.equal(directOnly.allow_proxy, false);
  await api('/plays/' + directOnly.id + '/end', 'POST', {});
  await api('/settings', 'PATCH', { forbid_proxy: false });
  await api('/plays/' + retry.id + '/end', 'POST', {});
  const log = (await api('/logs')).items.find(p => p.id === retry.id);
  assert.ok(log.events.some(e => e.event === 'proxy_selected'));
  assert.ok(log.events.some(e => e.event === 'proxy_blocked'));
  assert.ok(log.events.every(e => !e.message.includes('/media/') && !e.message.includes('token=')));
  pass('直链优先、原字节 Range 转发、禁止转发与过程日志');

  const pageSearch = await api('/search?q=' + encodeURIComponent('网页地址'));
  const pageFilm = pageSearch.items.find(f => f.source_id === primary.id);
  const page = await api('/films/' + pageFilm.id);
  const rejected = await api('/plays', 'POST', { episode_id: page.episodes[0].id }, cookie, 400);
  assert.match(rejected.error, /网页播放器/);
  pass('网页式播放地址显示解析限制与换源提示');
  await writeFile(join(out, 'test-session.json'), JSON.stringify({ base, admin, member, cookie, movie: movie.id, tv: tv.id, alternate: alternate.id }, null, 2), { mode: 0o600 });
  await writeFile(join(out, 'api-acceptance.json'), JSON.stringify({ at: new Date().toISOString(), scope: '真实容器的受控 API 验收，不代表客户端画面验收', checks }, null, 2));
} else {
  await api('/login', 'POST', admin);
}

if (mode === 'prepare' || mode === 'snapshot' || mode === 'backup') {
  const state = await snapshot();
  await writeFile(join(out, 'persistent-state.json'), JSON.stringify(state, null, 2));
  if (mode === 'backup') {
    const r = await fetch(base + '/api/backup', { headers: { cookie } });
    assert.equal(r.status, 200);
    await writeFile(join(out, 'consistent.sqlite'), Buffer.from(await r.arrayBuffer()));
    pass('运行中导出一致性 SQLite 备份');
  }
} else if (mode === 'check') {
  const expected = JSON.parse(await readFile(join(out, 'persistent-state.json'), 'utf8'));
  assert.deepEqual(await snapshot(), expected);
  pass('账号、订阅归属、启停设置、收藏与进度和保存快照一致');
} else if (mode === 'native-session') {
  const session = JSON.parse(await readFile(join(out, 'test-session.json'), 'utf8'));
  const film = await api('/films/' + session.movie);
  const episode = film.episodes[0];
  const login = await fetch(base + '/v/api/v2/user/loginByPassword', {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ username: admin.username, password: createHash('sha256').update(admin.password).digest('hex') }),
  });
  assert.equal(login.status, 200);
  const token = (await login.json()).data.token;
  const first = await native('/stream', { media_guid: episode.id }, token);
  const firstLink = first.direct_link_qualities[0].url;
  await native('/play/record', { media_guid: episode.id, item_guid: film.id, play_link: firstLink, ts: 42 }, token);
  assert.equal((await native('/play/info', { media_guid: episode.id }, token)).ts, 42);
  const second = await native('/stream', { media_guid: episode.id }, token);
  const secondLink = second.direct_link_qualities[0].url;
  assert.notEqual(firstLink, secondLink);
  const target1 = new URL(firstLink), target2 = new URL(secondLink);
  assert.match(target1.hash, /tv-rust-play=/);
  target1.hash = ''; target2.hash = '';
  assert.equal(target1.href, target2.href);
  await native('/play/record', { media_guid: episode.id, item_guid: film.id, play_link: secondLink, ts: 19 }, token);
  await native('/play/record', { media_guid: episode.id, item_guid: film.id, play_link: firstLink, ts: 90 }, token);
  assert.equal((await api('/films/' + film.id)).progress.position, 19);
  const waiting = (await api('/logs')).items.find(p => p.film_id === film.id);
  await api('/plays/' + waiting.id + '/fallback', 'POST', { reason: 'manual' });
  await native('/play/record', { media_guid: episode.id, item_guid: film.id, play_link: secondLink, ts: 20 }, token);
  assert.equal((await api('/logs')).items.find(p => p.id === waiting.id).state, 'fallback');
  const transferred = await native('/stream', { media_guid: episode.id }, token);
  const transferLink = transferred.direct_link_qualities[0].url;
  assert.match(new URL(transferLink).pathname, /^\/media\//);
  await native('/play/record', { media_guid: episode.id, item_guid: film.id, play_link: secondLink, ts: 90 }, token);
  await native('/play/record', { media_guid: episode.id, item_guid: film.id, play_link: transferLink, ts: 19 }, token);
  assert.equal((await api('/films/' + film.id)).progress.position, 19);
  await native('/play/quit', { play_link: transferLink }, token);
  const newest = (await api('/logs')).items.find(p => p.film_id === film.id);
  assert.equal(newest.ended, true);
  pass('真实容器的 STRM 会话身份、重复播放进度、手动转发延迟重试、旧 URL 回传拦截与退出协议');
  await writeFile(join(out, 'native-protocol.json'), JSON.stringify({ at: new Date().toISOString(), scope: 'HTTP 协议检查，不代表原生画面验收', checks }, null, 2));
} else {
  throw new Error(`未知模式：${mode}`);
}
