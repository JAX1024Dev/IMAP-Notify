# 更新记录

本项目采用语义化版本号，发布内容以 Git 标签和 GitHub Release 为准。

## [0.1.0] - 2026-10-21

首个公开版本。

### 功能

- Rust 多账户 IMAP IDLE 监听，单一 Bark 地址推送。
- 中文主题、发件人、账户分组、固定或自动图标、声音和通知跳转。
- 应用专用密码、授权码和可信 `password_cmd`。
- `--help`、`--version`、`--check`、`--test-bark` 和详细日志。
- 专用非 root 用户的 systemd 服务。

### 可靠性与隐私

- 同一进程内按 UIDVALIDITY 保留重连进度，补查断线窗口。
- 修复 IDLE 超时不返回业务层、批量 FETCH 部分失败重复通知、退避不重置。
- 检查 IDLE 能力、目标 FETCH UID 与 UID 上界，保留邮箱未读状态。
- Bark 严格确认 code=200，失败重试一次，不跟随重定向。
- 配置严格校验；错误日志隐藏 Key、密码命令和配置源码，默认日志不含邮件内容。

### 工程与文档

- Linux/macOS CI、Rust 1.88 兼容性检查。
- 四平台发布包、SHA-256 校验清单与标签触发的 GitHub Release。
- 中文 README、需求验收标准、审查修复记录和维护者交接指南。
- 移除容器构建文件与部署说明，补齐 MIT LICENSE。

### 已知限制

- 进程重启跳过停机邮件，无持久队列或严格一次投递保证。
- 不支持 OAuth2；真实邮箱到 iPhone 的验收需使用用户自己的账户和设备。
- IMAP 连接和普通命令、密码命令仍无应用级统一超时。

[0.1.0]: https://github.com/JAX1024Dev/IMAP-Notify/releases/tag/v0.1.0
