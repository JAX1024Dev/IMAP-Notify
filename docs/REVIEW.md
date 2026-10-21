# 首版代码审查与修复记录

审查范围：原始 Rust 源码、交接说明、配置、部署文件、Git 忽略规则和发布流程。首版版本为 v0.1.0。

| 优先级 | 原问题及影响 | 修复与验证 |
| --- | --- | --- |
| P1 | 每次网络重连重建 UIDNEXT，断线期间的新邮件被跳过 | 每账户持有内存游标，按 UIDVALIDITY 决定复用或重建；重连先补查，协议回归测试验证 |
| P1 | IMAP IDLE 默认 keepalive 在库内部续接，业务层超时检查无法执行 | 显式 `keepalive(false)`；模拟 TimedOut 验证 DONE 后执行 SEARCH |
| P1 | Bark 把成功 HTTP 下缺失 code 的空/非法 JSON 当成功 | 仅接受 HTTP 2xx 且整数 code=200；本地 HTTP 测试验证拒绝与重试 |
| P1 | HTTP 错误包含请求 URL，配置解析显示含 Key/密码的源码，命令错误回显命令 | reqwest `without_url`；TOML 错误隐藏源行；不回显命令或服务端 message；配置 Debug 隐藏 Bark URL；错误回归测试 |
| P1 | systemd 未指定 User，默认以 root 执行密码命令 | 显式 User/Group，更新安装步骤和文件权限，保留文件系统加固 |
| P2 | 整批 FETCH 完才推进 UID，部分成功后失败可能重发 | 每封处理后推进；部分失败与非请求 FETCH 回归测试 |
| P2 | 库缓存的非请求 EXISTS 可能错过及时检查，且队列积累 | SEARCH/FETCH 后消耗非请求响应，存在 EXISTS 时先补查再 IDLE |
| P2 | 重连退避从不重置，与原文档不符 | 成功完成一次 IDLE 后重置至 5 秒；模拟会话断言 |
| P2 | 未检查 IDLE 能力，可能无效重连或触发 alpha 库异常路径 | 登录后检查 CAPABILITY；缺少 IDLE 时明确拒绝；回归测试 |
| P2 | 过宽 Bark URL 校验、空账户字段、重复 alias、拼错配置静默忽略 | URL 结构校验、必填字段/端口/唯一别名校验、deny_unknown_fields；边界测试 |
| P2 | --test-bark 仍要求邮箱并执行密码命令 | 拆分 Watch/Check/TestBark 加载模式；测试通知允许纯 Bark 配置 |
| P2 | 自动图标使用不可信发件人域名直接拼 URL | 只接受 ASCII DNS 标签格式，拒绝路径、query 和空域名；单测 |
| P2 | 无 CI、发布自动化、许可证正文，配置及本地会话未忽略 | CI + 标签发布、四平台压缩包与 SHA256SUMS、MIT LICENSE、完整忽略规则 |
| P3 | 无版本查询、离线检查，工作线程 join 错误被忽略 | 添加 --version/--check，传播 join 错误，CLI 集成测试 |
| P3 | 默认日志输出邮件主题和发件人 | 仅记录别名和 UID，降低日志中的邮件内容暴露 |

## 保留的限制

- 没有持久化游标或可靠投递队列：重启跳过停机邮件，Bark 最终失败后仍推进进度；超时重试可能重复。
- IMAP 库仍为 `3.0.0-alpha.15`，连接、TLS 握手及普通命令无统一应用级超时。短期保留当前实现，运维可重启卡住的服务；后续考虑可取消且有超时的传输层。
- Ctrl-C 直接终止，没有逐连接 LOGOUT；默认 SIGTERM 由系统终止进程。
- password_cmd 是可信本地 shell 配置，当前没有执行超时，只适用于受信配置。
- 无 OAuth2、无真实邮箱/手机自动验收。单测中的 IMAP 使用脚本传输，不涵盖真实网络 TLS 握手与提供方差异。
- Linux 发布包依赖构建机的 glibc；macOS 发布包未签名、公证，README 已说明。

## 验证入口

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
./target/release/imap-notify --version
./target/release/imap-notify -c config.example.toml --check
```

本地测试包括 28 项单元/协议测试和 2 项 CLI 集成测试。GitHub CI 对 Linux/macOS 重复执行检查，并额外验证 Rust 1.88 构建兼容性。远程执行结果以仓库 Actions 为准。
