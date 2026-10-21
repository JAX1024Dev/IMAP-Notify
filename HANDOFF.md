# 维护者交接指南

使用与安装见 [README](README.md)，产品范围见 [需求与验收](docs/REQUIREMENTS.md)，原实现的缺陷与本轮修复见 [审查记录](docs/REVIEW.md)。此文档描述 v0.1.0 的实际实现。

## 项目结构

| 路径 | 职责 |
| --- | --- |
| `src/main.rs` | CLI、日志、配置查找、测试通知、每账户线程和退出信号 |
| `src/config.rs` | 严格 TOML 校验、默认值、密码命令、加载模式和凭据脱敏 |
| `src/account.rs` | 登录、能力检查、UID 游标、IDLE、抓取与断线重连；含协议回归测试 |
| `src/bark.rs` | 通知结构、图标、HTTP 发送、重试与错误脱敏 |
| `src/message.rs` | MIME/RFC 2047 主题和发件人解码 |
| `src/test_support.rs` | 仅测试编译的本地 HTTP 服务 |
| `tests/cli.rs` | 真实进程 CLI 回归测试 |
| `config.example.toml` | 所有支持字段的配置示例 |
| `deploy/imap-notify.service` | 非 root systemd 服务示例 |
| `.github/workflows/ci.yml` | Linux/macOS 检查与 Rust 1.88 兼容性 |
| `.github/workflows/release.yml` | 标签检查、复用 CI、四平台构建、打包和发布 |

## 核心流程

```text
main → 参数解析 → Watch / Check / TestBark 配置模式
  ├─ Check：离线校验后退出，不执行密码命令
  ├─ TestBark：仅发测试通知，不执行密码命令，不连接邮箱
  └─ Watch：解析密码 → 每账户独立线程
       TLS / STARTTLS → LOGIN → CAPABILITY(IDLE) → SELECT
       → 首次按 UIDNEXT 建基线；同 UIDVALIDITY 重连复用内存进度
       → SEARCH → 逐封 PEEK 头部 → Bark → 逐封推进进度
       → 消耗缓存 EXISTS，有变化则补查
       → IDLE（keepalive=false，60–1740 秒）→ 返回 SEARCH
       → 失败后退避 5 秒直至 300 秒并重连
```

### 必须保留的协议细节

1. **UID 区间反转**：`UID n:*` 在 n 大于最大 UID 时也可能返回最大的旧 UID，必须再次过滤 `uid >= next_uid`。
2. **UIDVALIDITY**：只在该值保持一致时复用游标；缺失或为零直接拒绝，变化时按新基线跳过存量。
3. **逐封提交**：Bark 成功、最终失败或邮件被删除后均推进；后续 FETCH 出错不撤回已完成的进度。
4. **PEEK**：不能换成 `BODY[...]`，以免设置 `\Seen`。FETCH 结果必须匹配目标 UID，忽略其他邮件的非请求响应。
5. **IDLE keepalive**：alpha 库默认在超时后内部无限续接，必须设置 `keepalive(false)` 使业务层得到超时返回。不要直接照搬旧版交接中的链式调用。
6. **缓存响应**：命令中的 EXISTS 可能先被库消费并入队，下一次 IDLE 不一定再次收到；进入 IDLE 前处理此队列。
7. **UID 上界**：游标采用 u64，可表示 u32 最大 UID 加一，避免饱和加法让最后一封邮件反复推送。

## 设计约束

- Rust、同步 IMAP、每账户一条线程；适用于少量个人邮箱。
- 单一全局 Bark 地址是需求约束。
- 图标优先账户、全局、发件人域名服务，由手机加载；不在服务端下载图标。
- 密码命令使用 `sh -c`，仅执行可信配置；启动监听时解析一次。
- `AccountConfig`、`BarkConfig` 的 Debug 不打印凭据；TOML 源行、HTTP URL、密码命令、服务端 message 不进入应用错误日志。
- 通知数据交给配置的 Bark 服务；默认日志仅保留账户别名和 UID。

## TLS 依赖审计决策

发布前 `cargo audit` 在原 IMAP rustls 依赖链中发现 7 项漏洞：rustls 0.22.4 的 RUSTSEC-2026-0346/0349，以及 rustls-webpki 0.102.8 的 RUSTSEC-2026-0049/0098/0099/0104/0382。上游 imap alpha 的 rustls-connector 版本约束无法仅通过锁文件升级解除。

因此 IMAP 改用该库现有的 `native-tls` 后端，继续保持 AutoTls（993 直接 TLS，其他端口必须 STARTTLS）和证书校验。macOS 使用系统 TLS，Linux 使用 OpenSSL。Bark HTTP 仍使用已更新的 rustls 0.23 系列。该调整避免自行实现 TLS/STARTTLS 传输，也移除了旧 rustls 链及其未维护的 rustls-pemfile。代价是 Linux 构建需要 OpenSSL 开发包，发布包需要 OpenSSL 3 运行库；README 已同步。不要重新开启原 `rustls-tls` feature，除非上游已更新且审计通过。

## 当前限制与运维

- 游标不持久化。网络重连可补查，进程重启和 UIDVALIDITY 变化会跳过存量。
- Bark 单条最多两次尝试、间隔 2 秒，每次总请求超时 15 秒、连接超时 10 秒；要求 HTTP 2xx 且 JSON 整数 code=200，不跟随重定向。最终失败仍推进游标，可能丢通知；超时重试可能重复。
- IMAP 连接和普通命令、password_cmd 当前没有统一应用级超时；长时间卡住需要人工重启。不是可持久重试队列。
- `ctrlc` 直接结束进程，没有 IMAP LOGOUT 清理；SIGTERM 使用系统默认行为。
- 不支持 OAuth2 和自动密码刷新。密码登录被禁止的服务提供方不能直接使用。
- systemd 配置路径 `/etc/imap-notify/config.toml`，权限 `640 root:imap-notify`；二进制 `/usr/local/bin/imap-notify`。服务显式使用专用账户，`ProtectHome=true` 会限制个人密码管理器。
- 维护命令：`systemctl status/restart imap-notify`、`journalctl -u imap-notify -f`。
- 连接中断自动退避；认证失败通常需要人工更新授权码，单账户不会阻断其他账户。

## 开发与验证

依赖安全检查：`cargo audit`（需安装 cargo-audit），CI 使用 RustSec 审计。

Rust 1.88+，edition 2024；完整锁文件需入库。提交前执行 README 开发章节中的四项检查。协议回归测试使用真实 IMAP 解析器加脚本传输，Bark 使用本地 TCP HTTP 服务，不访问真实邮箱。CLI 测试启动实际二进制检查参数和脱敏。

尚未覆盖的手工验收见需求文档：真实邮箱认证、TLS 握手与 CA 环境、手机通知显示、Linux systemd 运维。不要将模拟测试描述为真实邮箱端到端验证。

字段变化必须同步修改配置示例、README 配置表、需求和测试。依赖升级时重点检查 imap alpha 的 IDLE、响应缓存、UID 和 FETCH API；不要仅依赖编译通过。

## 发布

1. 更新 Cargo.toml 版本，用 Cargo 刷新 Cargo.lock；同步 CHANGELOG 与 `docs/releases/vX.Y.Z.md`。
2. 完成本地检查，提交并推送 main，确认 CI。
3. 创建匹配版本的 annotated tag：`git tag -a vX.Y.Z -m 'Release vX.Y.Z'`，推送标签。
4. 观察 Release 工作流，确认四个平台、SHA256SUMS、发布说明齐全；失败时修复后用新版本发布，或在相同标签重新运行可恢复的失败任务。

Release 仅 publish job 拥有写权限；第三方 Action 固定 commit SHA。版本与发布说明检查失败会阻止构建，任何平台失败会阻止发布。publish 先创建草稿，资产上传成功后再公开；重跑支持覆盖同名资产。不要移动已公开发布的版本标签。

## 后续工作优先级

1. 带 UIDVALIDITY 的跨进程持久化游标与明确投递语义。
2. IMAP/密码命令超时、取消和协作退出。
3. 提供方兼容性测试、真实 TLS 本地集成夹具。
4. 按实际需求再引入 OAuth2、多文件夹和热加载。

## 参考

- [IMAP IDLE / RFC 2177](https://www.rfc-editor.org/rfc/rfc2177)
- [rust-imap](https://github.com/jonhoo/rust-imap)
- [Bark](https://github.com/Finb/Bark) 与 [服务端 API](https://github.com/Finb/bark-server/blob/master/docs/API_V2.md)
