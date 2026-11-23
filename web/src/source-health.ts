export type SourceHealth = {
  probed: number;
  verified: number;
  rejected: number;
  web_pages: number;
  last_probed_at: number;
};

export function sourceHealthStatus(health?: SourceHealth) {
  if (!health?.probed) return null;
  const { probed, verified, rejected, web_pages } = health;
  if (probed >= 3 && rejected === probed) {
    return {
      kind: "unreachable",
      label: "服务器无法访问",
      title: `近 7 天探测的 ${probed} 条线路全部被上游拒绝（多为地域限制），该源在这台服务器上基本不可用。`,
    };
  }
  if (verified === probed) {
    return {
      kind: "available",
      label: "可用",
      title: `近 7 天探测的 ${probed} 条线路全部识别出真实视频。`,
    };
  }
  if (verified > 0) {
    return {
      kind: "partial",
      label: `部分可用 ${verified}/${probed}`,
      title: `近 7 天探测的 ${probed} 条线路中，${verified} 条识别出真实视频。`,
    };
  }
  if (web_pages === probed) {
    return {
      kind: "web",
      label: "网页播放器",
      title: `近 7 天探测的 ${probed} 条线路全部返回网页播放器，不能直接作为媒体播放。`,
    };
  }
  return {
    kind: "unknown",
    label: "待确认",
    title: `近 7 天已探测 ${probed} 条线路，暂未识别出真实视频，尚不能确认该视频源是否可用。`,
  };
}
