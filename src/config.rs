//! 配置文件加载与校验。
//!
//! 最终产出的 [`AppConfig`] 已合并默认值，并把可选的 `password_cmd`
//! 解析为明文密码（进程内使用，不打日志）。

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// 单次 IDLE 等待时长上限（RFC 2177 建议不超过 29 分钟）。
pub const MAX_IDLE_TIMEOUT_SECS: u64 = 1740;

/// 默认 favicon URL 模板（icon.horse 返回 PNG，实测可用）。
/// 图标最终由 iPhone 在展示通知时拉取，需为 PNG/JPG 格式。
pub const DEFAULT_FAVICON_TEMPLATE: &str = "https://icon.horse/icon/{domain}";

#[derive(Debug)]
pub struct AppConfig {
    pub bark: BarkConfig,
    pub accounts: Vec<AccountConfig>,
}

#[derive(Clone)]
pub struct BarkConfig {
    /// Bark 推送地址（含设备 Key），如 `https://api.day.app/AbCdEfGh`
    pub url: String,
    /// 全局默认图标，未配置时自动取发件人域名 favicon
    pub icon: Option<String>,
    pub sound: Option<String>,
    /// 通知级别：active / passive / timeSensitive / critical
    pub level: Option<String>,
    /// favicon URL 模板，须含 `{domain}` 占位符
    pub favicon_template: String,
    pub idle_timeout: Duration,
}

pub struct AccountConfig {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub folder: String,
    /// 账户级图标，优先于全局
    pub icon: Option<String>,
    /// 点击通知跳转的链接
    pub url: Option<String>,
}

// 密码不能随 Debug 输出泄漏
impl std::fmt::Debug for AccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountConfig")
            .field("alias", &self.alias)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"***")
            .field("folder", &self.folder)
            .field("icon", &self.icon)
            .field("url", &self.url)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    bark: RawBark,
    #[serde(default)]
    accounts: Vec<RawAccount>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBark {
    url: String,
    icon: Option<String>,
    sound: Option<String>,
    level: Option<String>,
    favicon_template: Option<String>,
    idle_timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAccount {
    alias: Option<String>,
    host: String,
    port: Option<u16>,
    username: String,
    password: Option<String>,
    /// 从命令输出读取密码（如 `pass email/gmail`），优先于 password
    password_cmd: Option<String>,
    folder: Option<String>,
    icon: Option<String>,
    url: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum LoadMode {
    Watch,
    Check,
    TestBark,
}

impl std::fmt::Debug for BarkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BarkConfig")
            .field("url", &"***")
            .finish_non_exhaustive()
    }
}

pub fn load(path: &Path, mode: LoadMode) -> Result<AppConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
    // toml::Error 的 Display 会输出源代码行，可能包含密码或 Bark Key。
    let raw: RawConfig = toml::from_str(&text).map_err(|_| {
        anyhow::anyhow!(
            "配置 TOML 解析失败，请检查字段名、类型和语法: {}（为保护凭据不显示源代码）",
            path.display()
        )
    })?;
    build_config(raw, mode)
}

#[cfg(test)]
fn from_raw(raw: RawConfig) -> Result<AppConfig> {
    build_config(raw, LoadMode::Watch)
}

fn build_config(raw: RawConfig, mode: LoadMode) -> Result<AppConfig> {
    let bark_url = raw.bark.url.trim().trim_end_matches('/').to_string();
    let parsed_url = validate_http_url(&bark_url, "[bark] url")?;
    if !parsed_url
        .path_segments()
        .is_some_and(|mut parts| parts.any(|p| !p.is_empty()))
    {
        bail!("[bark] url 必须是包含设备 Key 的完整地址，例如 https://api.day.app/你的Key");
    }

    if let Some(level) = &raw.bark.level
        && !matches!(
            level.as_str(),
            "active" | "passive" | "timeSensitive" | "critical"
        )
    {
        bail!("[bark] level 仅支持 active / passive / timeSensitive / critical（当前: {level}）");
    }

    if raw.accounts.is_empty() && mode != LoadMode::TestBark {
        bail!("配置文件中未定义任何 [[accounts]]，至少需要一个邮箱账户");
    }

    let favicon_template = raw
        .bark
        .favicon_template
        .unwrap_or_else(|| DEFAULT_FAVICON_TEMPLATE.to_string());
    if !favicon_template.contains("{domain}") {
        bail!("[bark] favicon_template 必须包含 {{domain}} 占位符（当前: {favicon_template}）");
    }

    validate_http_url(
        &favicon_template.replace("{domain}", "example.com"),
        "[bark] favicon_template",
    )?;
    validate_optional_url(raw.bark.icon.as_deref(), "[bark] icon")?;
    let timeout_secs = raw.bark.idle_timeout_secs.unwrap_or(600);
    if !(60..=MAX_IDLE_TIMEOUT_SECS).contains(&timeout_secs) {
        bail!("[bark] idle_timeout_secs 必须在 60–1740 之间");
    }
    let idle_timeout = Duration::from_secs(timeout_secs);
    let mut aliases = HashSet::new();

    let accounts = raw
        .accounts
        .into_iter()
        .map(|a| {
            let alias = a
                .alias
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| a.username.clone());
            if a.host.trim().is_empty() {
                bail!("[{alias}] host 不能为空");
            }
            if !aliases.insert(alias.clone()) {
                bail!("账户 alias 必须唯一: {alias}");
            }
            if a.username.chars().any(char::is_control)
                || a.folder
                    .as_deref()
                    .is_some_and(|s| s.chars().any(char::is_control))
                || a.username.trim().is_empty()
                || a.folder.as_deref().is_some_and(|s| s.trim().is_empty())
                || a.port == Some(0)
            {
                bail!("[{alias}] username、folder 不能为空，port 不能为 0");
            }
            validate_optional_url(a.icon.as_deref(), "账户 icon")?;
            validate_optional_url(a.url.as_deref(), "账户 url")?;
            let has_password = a.password.as_deref().is_some_and(|s| !s.trim().is_empty());
            let has_command = a
                .password_cmd
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty());
            if !has_command
                && a.password
                    .as_deref()
                    .is_some_and(|p| p.trim().chars().any(char::is_control))
            {
                bail!("[{alias}] 密码含控制字符");
            }
            if mode != LoadMode::TestBark && !has_password && !has_command {
                bail!("[{alias}] 未配置 password 或 password_cmd");
            }
            let password = if mode == LoadMode::Watch {
                let password = resolve_password(a.password_cmd.as_deref(), a.password.as_deref())?;
                if password.is_empty() || password.chars().any(char::is_control) {
                    bail!("[{alias}] 密码为空或含控制字符");
                }
                password
            } else {
                String::new()
            };
            Ok(AccountConfig {
                alias,
                host: a.host.trim().to_string(),
                port: a.port.unwrap_or(993),
                username: a.username.trim().to_string(),
                password,
                folder: a.folder.unwrap_or_else(|| "INBOX".to_string()),
                icon: a.icon,
                url: a.url,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(AppConfig {
        bark: BarkConfig {
            url: bark_url,
            icon: raw.bark.icon,
            sound: raw.bark.sound,
            level: raw.bark.level,
            favicon_template,
            idle_timeout,
        },
        accounts,
    })
}

fn validate_http_url(value: &str, field: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| anyhow::anyhow!("{field} 必须是完整的 HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some() && field == "[bark] url"
    {
        bail!("{field} URL 无效（需 HTTP(S)、主机，禁止用户凭据和 fragment；Bark 地址禁止 query）");
    }
    Ok(url)
}

fn validate_optional_url(value: Option<&str>, field: &str) -> Result<()> {
    if let Some(value) = value {
        validate_http_url(value, field)?;
    }
    Ok(())
}

/// `password_cmd` 优先；命令失败时直接报错终止，避免带着空密码反复重试。
fn resolve_password(cmd: Option<&str>, password: Option<&str>) -> Result<String> {
    match cmd.map(str::trim).filter(|s| !s.is_empty()) {
        Some(cmd) => {
            let out = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .context("执行 password_cmd 失败")?;
            if !out.status.success() {
                bail!("password_cmd 执行失败，退出码 {:?}", out.status.code());
            }
            Ok(String::from_utf8(out.stdout)
                .context("password_cmd 输出不是 UTF-8")?
                .trim()
                .to_string())
        }
        None => Ok(password.unwrap_or_default().trim().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_toml() -> String {
        r#"
[bark]
url = "https://api.day.app/TestKey123"

[[accounts]]
alias = "Gmail"
host = "imap.gmail.com"
username = "me@gmail.com"
password = "app-password"

[[accounts]]
host = "imap.qq.com"
port = 143
username = "12345@qq.com"
password_cmd = "echo qq-auth-code"
folder = "INBOX"
"#
        .to_string()
    }

    #[test]
    fn rejects_malformed_urls_without_exposing_keys() {
        for url in [
            "bad/secret-key",
            "ftp://host/secret-key",
            "https://user:secret-key@host/k",
            "https://host/k?secret-key=x",
            "https://host/k#secret-key",
        ] {
            let mut raw: RawConfig = toml::from_str(&sample_toml()).unwrap();
            raw.bark.url = url.into();
            let error = from_raw(raw).unwrap_err();
            assert!(!format!("{error:#}").contains("secret-key"));
        }
    }

    #[test]
    fn rejects_empty_fields_duplicate_aliases_and_unknown_keys() {
        for field in ["host", "username", "folder"] {
            let mut raw: RawConfig = toml::from_str(&sample_toml()).unwrap();
            match field {
                "host" => raw.accounts[0].host = " ".into(),
                "username" => raw.accounts[0].username = " ".into(),
                _ => raw.accounts[0].folder = Some(" ".into()),
            }
            assert!(from_raw(raw).is_err());
        }
        let mut raw: RawConfig = toml::from_str(&sample_toml()).unwrap();
        raw.accounts[1].alias = raw.accounts[0].alias.clone();
        assert!(from_raw(raw).is_err());
        let mut raw: RawConfig = toml::from_str(&sample_toml()).unwrap();
        raw.accounts[0].port = Some(0);
        assert!(from_raw(raw).is_err());
        assert!(
            toml::from_str::<RawConfig>(&sample_toml().replace("password =", "pasword =")).is_err()
        );
    }

    #[test]
    fn check_and_bark_test_do_not_execute_password_commands() {
        let mut raw: RawConfig = toml::from_str(&sample_toml()).unwrap();
        raw.accounts[0].password_cmd = Some("exit 99".into());
        let cfg = build_config(raw, LoadMode::Check).unwrap();
        assert!(cfg.accounts[0].password.is_empty());
        let raw = toml::from_str(
            r#"[bark]
url = "https://api.day.app/private-device-key"
"#,
        )
        .unwrap();
        let cfg = build_config(raw, LoadMode::TestBark).unwrap();
        assert!(!format!("{cfg:?}").contains("private-device-key"));
        let mut raw: RawConfig = toml::from_str(&sample_toml()).unwrap();
        raw.accounts[0].password_cmd = Some("printf ''".into());
        assert!(from_raw(raw).is_err());
    }

    #[test]
    fn command_errors_do_not_echo_command_secrets() {
        let error = resolve_password(Some("exit 3 # private-password"), None).unwrap_err();
        assert!(!format!("{error:#}").contains("private-password"));
    }

    #[test]
    fn parses_multiple_accounts_with_defaults() {
        let cfg = from_raw(toml::from_str(&sample_toml()).unwrap()).unwrap();
        assert_eq!(cfg.accounts.len(), 2);
        assert_eq!(cfg.accounts[0].port, 993);
        assert_eq!(cfg.accounts[0].folder, "INBOX");
        assert_eq!(cfg.accounts[0].password, "app-password");
        // 无 alias 时回退到用户名；password_cmd 优先且去除尾部换行
        assert_eq!(cfg.accounts[1].alias, "12345@qq.com");
        assert_eq!(cfg.accounts[1].password, "qq-auth-code");
        assert_eq!(cfg.accounts[1].port, 143);
        assert_eq!(cfg.bark.idle_timeout, Duration::from_secs(600));
    }

    #[test]
    fn bark_url_must_contain_key() {
        let raw: RawConfig = toml::from_str(
            r#"
[bark]
url = "https://api.day.app"
[[accounts]]
host = "h"
username = "u"
password = "p"
"#,
        )
        .unwrap();
        assert!(from_raw(raw).unwrap_err().to_string().contains("Key"));
    }

    #[test]
    fn invalid_level_rejected() {
        let raw: RawConfig = toml::from_str(
            r#"
[bark]
url = "https://api.day.app/k"
level = "loud"
[[accounts]]
host = "h"
username = "u"
password = "p"
"#,
        )
        .unwrap();
        assert!(from_raw(raw).is_err());
    }

    #[test]
    fn empty_accounts_rejected() {
        let raw: RawConfig = toml::from_str("[bark]\nurl = \"https://api.day.app/k\"\n").unwrap();
        assert!(from_raw(raw).is_err());
    }

    #[test]
    fn idle_timeout_out_of_range_rejected() {
        let raw: RawConfig = toml::from_str(
            r#"
[bark]
url = "https://api.day.app/k"
idle_timeout_secs = 99999
[[accounts]]
host = "h"
username = "u"
password = "p"
"#,
        )
        .unwrap();
        assert!(from_raw(raw).is_err());
    }

    #[test]
    fn password_cmd_takes_priority_and_trims() {
        let p = resolve_password(Some("echo  secret \n"), Some("fallback")).unwrap();
        assert_eq!(p, "secret");
        let p = resolve_password(None, Some(" pw ")).unwrap();
        assert_eq!(p, "pw");
        assert!(resolve_password(Some("exit 3"), None).is_err());
    }

    #[test]
    fn favicon_template_defaulted_and_validated() {
        let cfg = from_raw(toml::from_str(&sample_toml()).unwrap()).unwrap();
        assert_eq!(cfg.bark.favicon_template, DEFAULT_FAVICON_TEMPLATE);

        let mut raw: RawConfig = toml::from_str(&sample_toml()).unwrap();
        raw.bark.favicon_template = Some("https://fav.example.com/get?d={domain}&s=64".into());
        let cfg = from_raw(raw).unwrap();
        assert_eq!(
            cfg.bark.favicon_template,
            "https://fav.example.com/get?d={domain}&s=64"
        );

        let mut raw: RawConfig = toml::from_str(&sample_toml()).unwrap();
        raw.bark.favicon_template = Some("https://fav.example.com/get".into());
        assert!(from_raw(raw).is_err());
    }
}
