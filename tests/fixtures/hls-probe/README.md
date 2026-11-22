# 加密 HLS 元数据回归素材

这是一秒钟的 64×64 蓝色画面及 440 Hz 合成音，使用 H.264、AAC 和 AES-128 HLS。`key.bin` 是随素材发布的测试密钥，不包含用户信息。`media.m3u8` 及 `segment-0.ts` 由本机 FFmpeg 生成，用于验证有界本地探测确实识别视频和音频。

生成时使用 `color=c=blue:size=64x64:rate=12`、`sine=frequency=440:sample_rate=48000`、时长 1 秒、`libx264 -preset ultrafast -pix_fmt yuv420p`、`aac -b:a 32k`，IV 为 16 字节零值。密钥内容为 ASCII `0123456789abcdef`。这些素材只证明元数据解析，不证明客户端声音或实际播放。

`image-prefixed.ts` 用于回归图片头被误识别成视频的情况。先用 FFmpeg 将上述素材解密并原编码封装为 MPEG-TS，再在前面追加一张 28×28 黑色 PNG。图片头为 112 字节，后面的 10340 字节是原始 H.264／AAC 媒体；测试同时用该图片头覆盖真实返回图片的地址和分片。它验证 MPEG-TS 识别、旧 PNG 探测缓存刷新及图片错误提示，不代表外部 CDN 或客户端均支持这类分片。

`compressed-media.m3u8.zst` 和 `compressed-master.m3u8.zst` 由 Node.js 的 `zstdCompressSync` 生成，解压后分别指向上述合成分片及该媒体清单。`compressed-media.m3u8.gz` 用 `gzipSync` 压缩相同的媒体清单。它们回归上游返回 `Content-Encoding: zstd` 或 `gzip` 时的格式识别、错误文件缓存刷新、主／子清单解压与按解压后大小限制响应；没有使用外部影片内容或账号凭据。
