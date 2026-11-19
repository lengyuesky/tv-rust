export async function api(
  path: string,
  method = "GET",
  data?: unknown,
  keepalive = false,
  signal?: AbortSignal,
): Promise<any> {
  const r = await fetch("/api" + path, {
    method,
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", "X-TV-Request": "1" },
    body: data === undefined ? undefined : JSON.stringify(data),
    keepalive,
    signal,
  });
  const value = await r.json();
  if (!r.ok)
    throw Object.assign(new Error(value.error || `请求失败（${r.status}）`), {
      status: r.status,
    });
  return value;
}
