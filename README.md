# TV Rust

[![检查与镜像发布](https://github.com/lengyuesky/tv-rust/actions/workflows/check.yml/badge.svg)](https://github.com/lengyuesky/tv-rust/actions/workflows/check.yml)

Rust 影视聚合服务与中文 React/TypeScript 网页，兼容 LunaTV 配置和飞牛播放器。网页与飞牛使用同一套家庭账号，同一用户共享收藏、历史与进度。

支持 JSON、Base58 文本、订阅 URL、手动及定时刷新、多订阅源归属和按源启停；首次启动的视频源为空，由管理员导入。电影、剧集可聚合搜索、查看详情、选集、播放 MP4/HLS、换源与续播。豆瓣推荐不可用时仍可使用搜索与个人数据。

移除订阅会同步移出当前源管理列表，保留个人收藏与观看记录。搜索按片名相关性排序并过滤上游夹带的无关条目。网页封面由浏览器直接获取，在“设置 → 封面设置”选择 LunaTV 使用的腾讯云 CDN、阿里云 CDN 或自定义图片源，默认腾讯云，按账号保存并跨设备同步。加载失败时自动重试并切换 CDN，再尝试同作品备用封面；全部失败后显示片名占位。

管理员导入订阅时可选择“所有用户／指定用户／仅管理员”，已有订阅通过“使用用户”修改范围。“仅管理员”无需选择成员，所有普通成员均不可用；订阅刷新保留范围，管理员始终可用。网页和飞牛共用使用范围，撤权后保留个人记录并标记无权限，停止本站转发，重新授权后恢复可用。回归范围见 [账号图片设置与订阅授权验收](docs/verification-user-settings-20261119.md) 和 [此前订阅、封面与搜索回归](docs/verification-20261119.md)。

## Docker 部署

默认使用 GitHub Actions 构建并发布的 `ghcr.io/lengyuesky/tv-rust:latest` 镜像。如需固定版本，修改 `compose.yaml` 中的 `image` 标签。

```sh
cp .env.example .env
docker compose pull
docker compose up -d
```

打开 `http://服务器地址:3000`，设置管理员，再到“设置 → 订阅与视频源”导入配置。其他家庭成员在“家庭账号”中创建。

`.env` 中的设置：

| 变量 | 用途 |
| --- | --- |
| `TV_PORT` | 对外端口，默认 `3000` |
| `TV_PUBLIC_URL` | 其他设备或反向代理使用的完整服务根地址，例如 `https://tv.example.com`；留空使用当前访问地址 |
| `TV_ALLOW_PRIVATE_UPSTREAM` | 连接局域网订阅或视频源时设为 `1`；默认 `0` |

单容器包含 Rust、网页与媒体元数据探测工具，使用 SQLite 命名卷 `tv-data` 持久化。容器以普通用户运行，临时探测使用 `/tmp`。支持 `linux/amd64` 和 `linux/arm64` 构建：

```sh
docker buildx build --platform linux/amd64 --load -t tv-rust:amd64 .
docker buildx build --platform linux/arm64 --load -t tv-rust:arm64 .
```

本地构建后，将 `compose.yaml` 中的 `image` 改为对应的本地镜像标签，再执行 `docker compose up -d`。当前本机运行情况和实际播放范围见 [验收记录](docs/verification-20261112.md)。

## GitHub Actions 镜像发布

[检查与镜像发布工作流](.github/workflows/check.yml) 先运行格式、Clippy、Rust 测试和网页构建，再让 AMD64 与 ARM64 各使用一台原生 Ubuntu runner 并行构建。公开仓库的 `ubuntu-24.04` 和 `ubuntu-24.04-arm` 均为 [4 核、16 GB 内存的免费标准 runner](https://docs.github.com/en/actions/reference/runners/github-hosted-runners#standard-github-hosted-runners-for-public-repositories)，并行构建阶段合计可用 8 核、32 GB。Rust 使用每台 runner 的全部可用 CPU，依赖单独分层并按架构保存构建缓存，保留 release 优化。

检查使用 runner 现有编译工具，固定并缓存通过 AMD64 受控素材验证的 FFprobe 测试工具，避免重复运行 Ubuntu 系统包安装。网页只构建一次，再将同一产物交给两种架构。每个架构只执行一次 Docker 构建，同时导出本地镜像和 GHCR 摘要；本地镜像通过普通用户、只读文件系统下的空启动、健康接口、网页和 FFprobe 检查后，才合并清单并更新发布标签。缓存上传失败不阻断镜像发布。普通本地 `docker build` 仍可直接从源码构建完整镜像。

工作流使用 `GITHUB_TOKEN` 授权 GHCR。构建故障原因和本轮验证见 [构建优化记录](docs/build-20261119.md)。

| 触发方式 | 发布标签 |
| --- | --- |
| 推送 `main` 或在 `main` 手动运行 | `latest`、`sha-<完整提交 SHA>` |
| 推送版本标签，如 `v0.1.0` | `0.1.0`、`0.1`、`sha-<完整提交 SHA>` |
| Pull Request 或在其他分支手动运行 | 检查与原生构建 |

版本标签必须与 Rust 和网页的版本一致。公开镜像地址为 `ghcr.io/lengyuesky/tv-rust`，Docker 会按主机架构选择 AMD64 或 ARM64。

## 飞牛连接和播放

在飞牛播放器中添加同一个服务根地址，使用网页账号登录。首页的电影、剧集目录来自当前账号的收藏与观看记录；原生搜索查询当前账号获准使用且已启用的视频源。测试使用本机飞牛播放器 `1.12.3 / 1123003`。受控 MP4、非加密 HLS 和跨端续播已有实际画面验证；当前所测 AES-128 HLS 在该客户端仍报 `-99985`，直链及手动转发均未通过，部署前请按所用客户端和片源核对兼容性。

默认向客户端交付媒体直链。网页只在明确的网络或跨域错误时自动回退一次，也提供“转发重试”。确认为播放器网页的地址会提示需要额外解析，可以换源。

飞牛使用 STRM 的原画直链协商。每次交付附带独立 URL 片段作为播放关联；片段不随 HTTP 请求发送给视频源，源的地址和查询签名保持原样。飞牛没有已验证的直链失败回报，因此手动转发入口在网页“播放日志”：选择当前记录的“为飞牛准备转发重试”，再回飞牛重试该条目。只影响当前会话。

管理员可在“播放设置”开启全局“禁止转发”。该设置覆盖自动回退、手动重试、飞牛请求和已签发的转发地址，开启时停止现有转发。客户端已缓冲的画面可能短暂继续。

播放日志区分地址交付、播放器事件、进度回传与资源转发，记录方式变化及原因；普通用户只能看自己的日志，管理员可看全部，默认保留 30 天。地址交付、非零进度或收到转发字节都不能单独证明客户端实际解码成功。

## 换源与观看状态

播放页在播放器下方显示当前播放源，点击“切换播放源”可选择当前账号获准使用的同作品来源；同一来源有多条播放线路时，可直接用下拉框切换。首次展开查找其他来源，“刷新来源”会重新查询上游。候选需标题、明确年份、类别和版本一致；某个源暂未响应时，其余结果仍可使用。

正在播放时按实际集号匹配目标单集，能确认时保存最终进度并自动开始新播放；合辑或重复集号等无法确认的情况先让用户选集，原播放在确认前继续。切换同一影片的线路承接当前播放位置，跨来源使用各自的观看进度，不自动复制旧来源的位置或收藏。也可继续使用详情中的“搜索其他源”入口。

播放器上方显示具体画质，例如 `720p · 1280 × 720 · H.264 · 24 fps`。播放前使用真实媒体探测信息，播放后以浏览器实际解码尺寸为准，多档 HLS 的编码及帧率跟随对应档位；未知信息显示“画质待识别”。桌面及手机均支持显示，回归范围见 [播放页换源与画质验收](docs/verification-playback-20261120.md)。

进度保存按账号和来源条目隔离。网页关闭、切集、换源和退出登录会先保存最终快照，快照和会话结束在同一数据库事务中完成；旧会话的迟到回传不能覆盖新会话。

## 升级、备份和恢复

升级或重建继续使用命名卷：

```sh
docker compose pull
docker compose up -d
```

可在网页“设置 → 飞牛与数据”下载一致性数据库备份，也可以在运行中执行：

```sh
mkdir -p backups
docker compose exec tv-rust tv-rust backup /data/manual-backup.sqlite
docker compose cp tv-rust:/data/manual-backup.sqlite ./backups/manual-backup.sqlite
```

备份路径必须是新文件。当前账号、订阅、目录、个人数据及日志都存储在 `tv.sqlite`；临时媒体探测文件无需备份。运行中的 SQLite 数据库应通过上述备份入口导出，以包含 WAL 中已提交的数据。

恢复会用备份替换当前数据库，先保留一份当前备份，再停止服务并导入：

```sh
docker compose stop
docker compose run --rm --no-deps --user root \
  --entrypoint /bin/sh \
  -v "$PWD/backups/manual-backup.sqlite:/backup.sqlite:ro" \
  tv-rust -c 'rm -f /data/tv.sqlite-wal /data/tv.sqlite-shm; cp /backup.sqlite /data/tv.sqlite; chown 10001:10001 /data/tv.sqlite'
docker compose up -d
```

忘记管理员密码时，可在本机运行恢复命令，新密码从标准输入读取：

```sh
docker compose exec tv-rust tv-rust reset-password 管理员用户名
```

修改密码会撤销该账号全部设备的旧登录和播放授权，需要重新登录。

## 本地开发与重复验收

需要 Rust stable、Node.js 22 或更新版本，以及 `ffprobe`。默认服务地址为 `0.0.0.0:3000`，数据目录为 `data/`，网页目录为 `web/dist/`：

```sh
cd web
npm ci
npm run build
cd ..
cargo run --locked
```

检查命令：

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
npm --prefix web test
npm --prefix web run build
```

生成和运行受控 MP4/HLS 素材：

```sh
node tests/fixtures/media.mjs
FIXTURE_BIND=0.0.0.0 FIXTURE_PUBLIC_URL=http://本机局域网地址:55321 node tests/fixtures/server.mjs
```

对独立空数据库运行 API 验收，服务应允许访问受控的局域网视频源：

```sh
TV_TEST_URL=http://127.0.0.1:55320 \
TV_TEST_FIXTURE=http://本机局域网地址:55321/config.json \
node tests/acceptance.mjs prepare
node tests/acceptance.mjs snapshot
node tests/acceptance.mjs check
node tests/acceptance.mjs backup
```

`prepare` 只允许未初始化的数据库；输出写入 `tmp/continuation-20261112`，可通过 `TV_TEST_OUTPUT` 指定独立目录。API 验收不替代浏览器或飞牛的实际画面观察。

验收账号密码每次 `prepare` 随机生成，保存在该输出目录的权限 `600` 会话文件中。后续 `snapshot`、`check`、`backup` 和 `native-session` 使用同一会话文件。需要指定密码时可设置 `TV_TEST_PASSWORD` 与 `TV_TEST_MEMBER_PASSWORD`。

需求和设计决策见 [设计记录](docs/design.md)、[调查记录](docs/research-20261112.md)、[术语表](GLOSSARY.md) 与 `docs/adr/`。

## 许可证

本项目使用 [MIT 许可证](LICENSE)。第三方依赖和媒体工具适用各自的许可证。
