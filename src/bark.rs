//! Bark 推送客户端：构造美观的通知 payload 并 POST 到 Bark 服务器。

use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};

use crate::message::MailSummary;

const SUBJECT_MAX_CHARS: usize = 200;
const SENDER_MAX_CHARS: usize = 100;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// 推送的可配置项（全局层），账户级 icon/url 在推送时单独传入。
#[derive(Debug, Clone)]
pub struct BarkSettings {
    /// 全局默认图标；未配置时自动取发件人域名 favicon
    pub icon: Option<String>,
    pub sound: Option<String>,
    /// active / passive / timeSensitive / critical
    pub level: Option<String>,
    /// favicon URL 模板，须含 `{domain}` 占位符
    pub favicon_template: String,
}

#[derive(Clone)]
pub struct BarkClient {
    http: reqwest::blocking::Client,
    url: String,
    settings: BarkSettings,
}

impl BarkClient {
    pub fn new(url: &str, settings: BarkSettings) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            url: url.to_string(),
            settings,
        })
    }

    /// 新邮件到达时推送。
    pub fn push_new_mail(
        &self,
        alias: &str,
        account_icon: Option<&str>,
        account_url: Option<&str>,
        mail: &MailSummary,
    ) -> Result<()> {
        let payload = new_mail_payload(&self.settings, alias, account_icon, account_url, mail);
        self.send(&payload)
    }

    /// `--test-bark`：验证 Key 与图标策略，不连接邮箱。
    pub fn push_test(&self) -> Result<()> {
        let host = reqwest::Url::parse(&self.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        let icon = self
            .settings
            .icon
            .clone()
            .or_else(|| host.map(|h| favicon_url(&self.settings.favicon_template, &h)));
        let mut payload = json!({
            "title": "📬 IMAP-Notify 测试通知",
            "body": "收到这条推送，说明 Bark 配置正确 ✅",
            "group": "IMAP-Notify",
        });
        if let Some(icon) = icon {
            payload["icon"] = json!(icon);
        }
        if let Some(sound) = &self.settings.sound {
            payload["sound"] = json!(sound);
        }
        if let Some(level) = &self.settings.level {
            payload["level"] = json!(level);
        }
        self.send(&payload)
    }

    /// 发送并重试一次；失败返回 Err（由调用方决定是否中断监听）。
    fn send(&self, payload: &Value) -> Result<()> {
        let mut last = String::from("未知错误");
        for attempt in 1..=2 {
            if attempt > 1 {
                thread::sleep(RETRY_DELAY);
            }
            match self.http.post(&self.url).json(payload).send() {
                Ok(resp) => {
                    let status = resp.status();
                    let body: Value = resp.json().unwrap_or(Value::Null);
                    let code = body.get("code").and_then(Value::as_i64);
                    if status.is_success() && code == Some(200) {
                        debug!("Bark 推送成功");
                        return Ok(());
                    }
                    last = format!("HTTP {status}, code={code:?}");
                    warn!(attempt, error = %last, "Bark 返回异常");
                }
                Err(e) => {
                    last = e.without_url().to_string();
                    warn!(attempt, error = %last, "Bark 请求失败");
                }
            }
        }
        bail!("Bark 推送失败: {last}")
    }
}

/// 新邮件通知 payload：
/// - 标题：📬 新邮件 · 账户别名
/// - 副标题：发件人（显示名 + 地址）
/// - 正文：邮件主题
/// - group 按账户分组，锁屏多封邮件按账户折叠计数
/// - 图标三级策略：账户 icon > 全局 icon > 发件人域名 favicon
pub(crate) fn new_mail_payload(
    settings: &BarkSettings,
    alias: &str,
    account_icon: Option<&str>,
    account_url: Option<&str>,
    mail: &MailSummary,
) -> Value {
    let subject = truncate(mail.subject_or_default(), SUBJECT_MAX_CHARS);
    let sender = truncate(&sender_display(mail), SENDER_MAX_CHARS);
    let mut payload = json!({
        "title": format!("📬 新邮件 · {alias}"),
        "subtitle": sender,
        "body": subject,
        "group": format!("IMAP/{alias}"),
    });

    let icon = account_icon
        .or(settings.icon.as_deref())
        .map(str::to_owned)
        .or_else(|| {
            mail.from_addr
                .as_deref()
                .and_then(email_domain)
                .map(|d| favicon_url(&settings.favicon_template, d))
        });
    if let Some(icon) = icon {
        payload["icon"] = json!(icon);
    }
    if let Some(sound) = &settings.sound {
        payload["sound"] = json!(sound);
    }
    if let Some(level) = &settings.level {
        payload["level"] = json!(level);
    }
    if let Some(url) = account_url {
        payload["url"] = json!(url);
    }
    payload
}

fn sender_display(mail: &MailSummary) -> String {
    let name = mail
        .from_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let addr = mail
        .from_addr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (name, addr) {
        (Some(n), Some(a)) => format!("{n} <{a}>"),
        (Some(n), None) => n.to_string(),
        (None, Some(a)) => a.to_string(),
        (None, None) => "未知发件人".to_string(),
    }
}

/// 按字符（而非字节）安全截断，附加省略号。
pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let head: String = s.chars().take(max_chars).collect();
        format!("{head}…")
    }
}

/// 发件人邮箱的域名部分。
pub(crate) fn email_domain(addr: &str) -> Option<&str> {
    let (local, domain) = addr.rsplit_once('@')?;
    if local.is_empty()
        || domain.is_empty()
        || !domain.is_ascii()
        || !domain.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return None;
    }
    Some(domain)
}

/// 图标由 iPhone 端拉取（需为 PNG/JPG）；取不到时 Bark 自动回退应用图标。
pub(crate) fn favicon_url(template: &str, domain: &str) -> String {
    template.replace("{domain}", domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> BarkSettings {
        BarkSettings {
            icon: None,
            sound: None,
            level: None,
            favicon_template: "https://icon.horse/icon/{domain}".to_string(),
        }
    }

    fn mail(subject: &str, name: Option<&str>, addr: Option<&str>) -> MailSummary {
        MailSummary {
            subject: subject.to_string(),
            from_name: name.map(str::to_string),
            from_addr: addr.map(str::to_string),
        }
    }

    #[test]
    fn payload_with_name_and_favicon_icon() {
        let m = mail("季度报告", Some("张三"), Some("zhangsan@github.com"));
        let p = new_mail_payload(&settings(), "Gmail", None, None, &m);
        assert_eq!(p["title"], "📬 新邮件 · Gmail");
        assert_eq!(p["subtitle"], "张三 <zhangsan@github.com>");
        assert_eq!(p["body"], "季度报告");
        assert_eq!(p["group"], "IMAP/Gmail");
        assert_eq!(p["icon"], "https://icon.horse/icon/github.com");
        assert!(p.get("sound").is_none());
        assert!(p.get("url").is_none());
    }

    #[test]
    fn icon_priority_account_over_global_over_favicon() {
        let m = mail("s", Some("a"), Some("a@x.com"));
        let p = new_mail_payload(&settings(), "A", Some("https://acct/icon.png"), None, &m);
        assert_eq!(p["icon"], "https://acct/icon.png");

        let mut s = settings();
        s.icon = Some("https://global/icon.png".to_string());
        let p = new_mail_payload(&s, "A", None, None, &m);
        assert_eq!(p["icon"], "https://global/icon.png");
    }

    #[test]
    fn custom_favicon_template() {
        let mut s = settings();
        s.favicon_template = "https://www.google.com/s2/favicons?domain={domain}&sz=64".to_string();
        let m = mail("s", None, Some("a@qq.com"));
        let p = new_mail_payload(&s, "A", None, None, &m);
        assert_eq!(
            p["icon"],
            "https://www.google.com/s2/favicons?domain=qq.com&sz=64"
        );
    }

    #[test]
    fn optional_fields_passed_through() {
        let mut s = settings();
        s.sound = Some("bell".to_string());
        s.level = Some("timeSensitive".to_string());
        let m = mail("s", None, Some("a@b.c"));
        let p = new_mail_payload(&s, "A", None, Some("https://mail.example.com"), &m);
        assert_eq!(p["sound"], "bell");
        assert_eq!(p["level"], "timeSensitive");
        assert_eq!(p["url"], "https://mail.example.com");
        // 无显示名时副标题直接用地址
        assert_eq!(p["subtitle"], "a@b.c");
    }

    #[test]
    fn unknown_sender_and_long_subject() {
        let m = mail("", None, None);
        let p = new_mail_payload(&settings(), "A", None, None, &m);
        assert_eq!(p["subtitle"], "未知发件人");
        assert_eq!(p["body"], "(无主题)");
        assert!(p.get("icon").is_none());

        let long: String = "测".repeat(300);
        let m = mail(&long, None, Some("a@b.c"));
        let p = new_mail_payload(&settings(), "A", None, None, &m);
        assert_eq!(p["body"].as_str().unwrap().chars().count(), 201);
        assert!(p["body"].as_str().unwrap().ends_with('…'));
    }

    #[test]
    fn http_failure_retries_and_requires_explicit_success_code() {
        use crate::test_support::HttpMock;
        let mock = HttpMock::new(vec![(500, r#"{"code":500}"#), (200, r#"{"code":200}"#)]);
        BarkClient::new(&mock.url, settings())
            .unwrap()
            .push_test()
            .unwrap();
        let requests = mock.finish();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
        for body in ["not JSON", "{}", r#"{"code":400,"message":"secret-key"}"#] {
            let mock = HttpMock::new(vec![(200, body), (200, body)]);
            let error = BarkClient::new(&mock.url, settings())
                .unwrap()
                .push_test()
                .unwrap_err();
            assert!(!format!("{error:#}").contains("secret-key"));
            assert_eq!(mock.finish().len(), 2);
        }
    }

    #[test]
    fn transport_error_does_not_expose_device_key() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let client =
            BarkClient::new(&format!("http://{addr}/private-device-key"), settings()).unwrap();
        let error = client.push_test().unwrap_err();
        assert!(!format!("{error:#}").contains("private-device-key"));
    }

    #[test]
    fn untrusted_sender_domain_cannot_modify_favicon_url() {
        for addr in [
            "a@",
            "@example.com",
            "a@example.com/x",
            "a@example.com?key=x",
            "a@../x",
            "a@-invalid.com",
        ] {
            assert_eq!(email_domain(addr), None, "{addr}");
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(email_domain("a@mail.github.com"), Some("mail.github.com"));
        assert_eq!(email_domain("no-at-sign"), None);
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("短", 10), "短");
        assert_eq!(
            favicon_url("https://x/{domain}/i.png", "b.com"),
            "https://x/b.com/i.png"
        );
    }
}
