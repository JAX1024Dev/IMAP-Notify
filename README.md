# IMAP-Notify

[![CI](https://github.com/JAX1024Dev/IMAP-Notify/actions/workflows/ci.yml/badge.svg)](https://github.com/JAX1024Dev/IMAP-Notify/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/JAX1024Dev/IMAP-Notify)](https://github.com/JAX1024Dev/IMAP-Notify/releases/latest)
[![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**多个邮箱，一处推送。** 使用 Rust 编写的 IMAP IDLE 监听服务，将新邮件主题和发件人通过 [Bark](https://github.com/Finb/Bark) 推送到 iPhone。

- **实时监听**：每个账户独立长连接，支持 IMAPS 和强制加密的 STARTTLS，校验服务器证书。
- **通知清晰**：中文主题解码、发件人副标题、按账户分组、图标和点击跳转。
- **自动恢复**：断线指数退避重连，同一进程内保留进度，补查断线期间的新邮件。
- **轻量部署**：单个可执行文件，支持 Linux systemd 与 macOS 命令行运行。
- **保护凭据**：支持密码命令，本地配置不入库，错误日志隐藏 Bark Key 和密码命令。

```text
📬 新邮件 · 工作邮箱
张三 <zhangsan@example.com>
【季度报告】请查收 10 月数据
```

## 快速开始

### 1. 安装

从 [Releases](https://github.com/JAX1024Dev/IMAP-Notify/releases/latest) 下载对应压缩包，解压后进入目录。每个包包含程序、配置模板、许可证及 systemd 服务。

| 系统 | 架构 | 文件名后缀 |
| --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu.tar.gz` |
| Linux | ARM64 | `aarch64-unknown-linux-gnu.tar.gz` |
| macOS | Apple Silicon | `aarch64-apple-darwin.tar.gz` |
| macOS | Intel | `x86_64-apple-darwin.tar.gz` |

Linux 预编译包基于 Ubuntu 24.04 构建，需要 glibc 2.39 或更新版本、OpenSSL 3 运行库和系统 CA 证书。旧系统请在目标系统从源码编译。macOS 包未经 Apple 签名或公证，首次运行可能需要在系统安全设置中允许。

下载 `SHA256SUMS` 与压缩包放在同一目录，可使用 `sha256sum --ignore-missing -c SHA256SUMS` 校验；macOS 使用 `shasum -a 256 <压缩包>` 与清单中的值比较。

也可以用 Rust **1.88+** 从源码构建。Linux 需安装 C 编译工具、`pkg-config`、OpenSSL 开发包和 CA 证书（Debian/Ubuntu：`sudo apt-get install build-essential pkg-config libssl-dev ca-certificates`）；macOS 需安装 Xcode Command Line Tools：

```bash
git clone https://github.com/JAX1024Dev/IMAP-Notify.git
cd IMAP-Notify
cargo build --release --locked
# 可执行文件：target/release/imap-notify
```

### 2. 准备配置

安装 [Bark iOS App](https://apps.apple.com/app/id1403753865)，复制含设备 Key 的推送地址，例如 `https://api.day.app/你的Key`。在邮箱提供方启用 IMAP，并获取应用专用密码或授权码。

```bash
cp config.example.toml config.toml
chmod 600 config.toml
# 编辑 config.toml，填入 Bark 地址和邮箱凭据
```

最小配置：

```toml
[bark]
url = "https://api.day.app/你的Key"

[[accounts]]
alias = "工作邮箱"
host = "imap.example.com"
username = "me@example.com"
password = "应用专用密码或授权码"
```

完整多账户示例见 [`config.example.toml`](config.example.toml)。Bark 只支持一个全局推送地址。

- [Gmail 应用专用密码说明](https://support.google.com/accounts/answer/185833)：需要启用两步验证，部分组织账户或安全策略可能不允许。
- QQ、163、126 等邮箱：在提供方的账户设置中开启 IMAP，并使用其授权码；以当前官方说明为准。
- 本项目使用密码/授权码登录，**不支持 XOAUTH2**。要求 OAuth 的邮箱（包括 Outlook.com）无法直接使用。

### 3. 校验与启动

以下命令使用压缩包中的程序；源码构建时将 `./imap-notify` 替换为 `./target/release/imap-notify`。

```bash
# 离线校验，不执行 password_cmd，不连接邮箱或 Bark
./imap-notify -c config.toml --check

# 发一条测试通知，不连接邮箱、不执行 password_cmd
./imap-notify -c config.toml --test-bark

# 启动监听
./imap-notify -c config.toml
```

首次连接跳过存量邮件。启动后向该邮箱发送一封测试邮件，核对主题、发件人和图标。`--test-bark` 也支持只有 `[bark]` 的配置文件。

## 配置参考

### `[bark]`

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `url` | 必填 | 含设备 Key 的完整 HTTP(S) URL；推荐 HTTPS，不接受 query、fragment 或 URL 内用户名密码 |
| `icon` | 自动 favicon | 全局图标 URL，建议 PNG/JPG |
| `favicon_template` | `https://icon.horse/icon/{domain}` | 必须包含 `{domain}`；由 iPhone 拉取 |
| `sound` | Bark 默认 | Bark App 中支持的声音名称 |
| `level` | Bark 默认 | `active`、`passive`、`timeSensitive`、`critical` |
| `idle_timeout_secs` | `600` | IDLE 等待秒数，范围 **60–1740**；越界直接报错 |

### `[[accounts]]`

重复此配置块以添加账户。每个账户监听一个文件夹，各账户的有效别名必须唯一。

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `alias` | `username` | 通知标题和分组名称 |
| `host` | 必填 | IMAP 主机名 |
| `port` | `993` | 993 使用直接 TLS；其他端口使用 STARTTLS，不允许明文降级 |
| `username` | 必填 | 登录用户名，通常为邮箱地址 |
| `password` | 与 `password_cmd` 至少选一 | 应用专用密码或授权码 |
| `password_cmd` | 无 | 执行 `sh -c` 并读取 UTF-8 stdout，优先于 `password` |
| `folder` | `INBOX` | 监听文件夹，不能为空 |
| `icon` | 继承全局 | 账户固定图标，优先于全局和自动 favicon |
| `url` | 无 | 点击通知跳转的 HTTP(S) 链接 |

例如使用密码管理器：

```toml
password_cmd = "pass show email/work"
```

命令必须只输出密码，程序会去掉首尾空白；非零退出、空输出或控制字符会阻止启动。仅执行可信配置文件中的命令。命令在启动监听时运行一次，失败不会回退到 `password`。`--check` 不验证命令是否可用，也不验证邮箱认证。

未知字段、无效 URL、重复别名、缺少凭据等配置问题会直接报错。配置变更后需要重启。

## Linux systemd 部署

以下命令适用于常见 Debian/Ubuntu 系统，以解压后的发布包为当前目录；源码构建时修改二进制来源路径。

```bash
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin imap-notify
sudo install -m 755 imap-notify /usr/local/bin/imap-notify
sudo install -d -m 750 -o root -g imap-notify /etc/imap-notify
sudo install -m 640 -o root -g imap-notify config.toml /etc/imap-notify/config.toml
sudo install -m 644 deploy/imap-notify.service /etc/systemd/system/imap-notify.service
sudo systemctl daemon-reload
sudo systemctl enable --now imap-notify
journalctl -u imap-notify -f
```

如果用户已存在，跳过 `useradd`。服务以专用非 root 用户运行，并限制写入文件系统和访问用户目录。使用 `password_cmd` 时，确保命令能以 `imap-notify` 用户执行；示例服务的 `ProtectHome=true` 会屏蔽 `/home`、`/root`、`/run/user`，个人密码管理器可能需要专门的服务凭据方案。

更新程序后执行 `sudo systemctl restart imap-notify`。状态查询用 `systemctl status imap-notify`，日志级别可通过 service override 中的 `Environment=RUST_LOG=debug` 调整。

## 行为与边界

- **新邮件判定**：以首次 SELECT 的 `UIDNEXT` 为基线，使用 UID 查询和 `BODY.PEEK` 读取主题、发件人，保留未读状态。
- **断线恢复**：重连间隔为 5、10、20 秒直至 300 秒；一次 IDLE 正常返回后重置退避。同一进程、同一 `UIDVALIDITY` 下保留每封进度，先补查再 IDLE。
- **重启与邮箱重建**：进度仅保存在内存。进程重启或 `UIDVALIDITY` 变化时重新建基线，停机期间的邮件不会补发。
- **推送失败**：每封最多尝试两次，失败告警后继续推进进度；没有持久队列。网络超时重试可能造成重复，因此不保证严格一次送达。
- **兼容性**：服务器必须支持 IDLE 并返回 `UIDVALIDITY`。不支持 OAuth、热加载、多文件夹合并和正文预览。
- **隐私**：主题和发件人会发送到你配置的 Bark 服务。自动 favicon 会使图标服务接触到发件人域名；可配置固定 `icon` 避免这一请求。默认日志仅记录账户别名和 UID，不记录邮件主题、发件人或密码。
- **退出与超时**：Ctrl-C 直接结束进程，不保证 IMAP LOGOUT。底层库的连接、登录及普通命令尚无应用级统一超时；长时间卡住可重启服务。

图标加载依赖 iPhone 网络与图标服务，不影响邮件推送请求。未收到推送时，先运行 `--test-bark`；邮箱认证失败则核对 IMAP 开关、授权码和服务提供方的认证要求。

## 开发与发布

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

CI 在 Linux 和 macOS 上运行格式检查、Clippy、测试和发布构建，并单独检查 Rust 1.88 兼容性与 RustSec 依赖漏洞。测试使用本地 HTTP 服务和脚本化 IMAP 传输，不依赖真实邮箱。真实邮箱到 iPhone 的端到端验证仍需手动完成。

发布时更新 `Cargo.toml`、`Cargo.lock`、`CHANGELOG.md` 和 `docs/releases/vX.Y.Z.md`，提交后推送匹配的 `vX.Y.Z` 标签。[Release 工作流](.github/workflows/release.yml) 会完成检查、四个平台构建、打包和 SHA-256 校验文件，再发布 GitHub Release。也可在 Actions 中选择已有版本标签手动重跑。仅发布任务拥有 `contents: write` 权限，无需额外配置仓库 Secret。

- [需求与验收标准](docs/REQUIREMENTS.md)
- [审查结果与修复记录](docs/REVIEW.md)
- [维护者交接文档](HANDOFF.md)
- [更新记录](CHANGELOG.md)

## 许可证

[MIT](LICENSE) © JAX1024Dev
