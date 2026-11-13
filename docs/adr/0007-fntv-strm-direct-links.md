---
status: accepted
---

# 飞牛直链使用 STRM 协商

飞牛播放器 1.12.3 支持通过 `cloud_storage_type: 9001` 与 `direct_link_qualities[].url` 获取外部媒体地址。本项目采用此通道并设置空媒体请求头，受控源实测收到 `trim_player` 直接请求且没有 Authorization 或 Cookie。普通媒体入口重定向虽然能够起播，却会携带客户端认证头，因此不作为外部直链的交付方式。

`/stream/directLink` 的响应与 `/stream` 使用同一种媒体模型。播放会话从协商时建立，不依赖服务器读取媒体字节，因而直链播放也能保存对应用户的进度和日志。
