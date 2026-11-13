# 加密 HLS 元数据回归素材

这是一秒钟的 64×64 蓝色画面及 440 Hz 合成音，使用 H.264、AAC 和 AES-128 HLS。`key.bin` 是随素材发布的测试密钥，不包含用户信息。`media.m3u8` 及 `segment-0.ts` 由本机 FFmpeg 生成，用于验证有界本地探测确实识别视频和音频。

生成时使用 `color=c=blue:size=64x64:rate=12`、`sine=frequency=440:sample_rate=48000`、时长 1 秒、`libx264 -preset ultrafast -pix_fmt yuv420p`、`aac -b:a 32k`，IV 为 16 字节零值。密钥内容为 ASCII `0123456789abcdef`。这些素材只证明元数据解析，不证明客户端声音或实际播放。
