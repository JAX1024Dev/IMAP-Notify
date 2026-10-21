//! 新邮件头解析：从 IMAP FETCH 回传的原始头字节中提取主题与发件人。
//!
//! mail-parser 负责处理 MIME/RFC 2047 编码字（如 `=?UTF-8?B?...?=` 的中文主题）。

use mail_parser::{Address, MessageParser};

#[derive(Debug, Clone, PartialEq)]
pub struct MailSummary {
    /// 已解码的主题，可能为空字符串
    pub subject: String,
    pub from_name: Option<String>,
    pub from_addr: Option<String>,
}

impl MailSummary {
    pub fn subject_or_default(&self) -> &str {
        if self.subject.trim().is_empty() {
            "(无主题)"
        } else {
            &self.subject
        }
    }
}

pub fn parse_summary(header_bytes: &[u8]) -> MailSummary {
    let parsed = MessageParser::default().parse(header_bytes);
    let subject = parsed
        .as_ref()
        .and_then(|m| m.subject())
        .map(str::to_string);
    let (from_name, from_addr) = match parsed.as_ref().and_then(|m| m.from()) {
        Some(Address::List(list)) => list.first().map(addr_parts).unwrap_or((None, None)),
        Some(Address::Group(groups)) => groups
            .first()
            .and_then(|g| g.addresses.first())
            .map(addr_parts)
            .unwrap_or((None, None)),
        None => (None, None),
    };
    MailSummary {
        subject: subject.unwrap_or_default(),
        from_name,
        from_addr,
    }
}

fn addr_parts(addr: &mail_parser::Addr) -> (Option<String>, Option<String>) {
    (
        addr.name.as_deref().map(str::to_string),
        addr.address.as_deref().map(str::to_string),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_rfc2047_base64_chinese() {
        // Subject: 你好，世界 ; From: 张三 <zhangsan@example.com>
        let raw = concat!(
            "Subject: =?UTF-8?B?5L2g5aW977yM5LiW55WM?=\r\n",
            "From: =?UTF-8?B?5byg5LiJ?= <zhangsan@example.com>\r\n",
            "Date: Tue, 21 Oct 2026 10:00:00 +0800\r\n",
            "\r\n"
        );
        let m = parse_summary(raw.as_bytes());
        assert_eq!(m.subject, "你好，世界");
        assert_eq!(m.from_name.as_deref(), Some("张三"));
        assert_eq!(m.from_addr.as_deref(), Some("zhangsan@example.com"));
    }

    #[test]
    fn decodes_rfc2047_quoted_printable() {
        let raw = "Subject: =?UTF-8?Q?Hello_World?=\r\nFrom: foo@bar.com\r\n\r\n";
        let m = parse_summary(raw.as_bytes());
        assert_eq!(m.subject, "Hello World");
        assert_eq!(m.from_addr.as_deref(), Some("foo@bar.com"));
        assert_eq!(m.from_name, None);
    }

    #[test]
    fn plain_headers() {
        let raw = "Subject: Plain subject\r\nFrom: Alice <alice@example.org>\r\n\r\n";
        let m = parse_summary(raw.as_bytes());
        assert_eq!(m.subject, "Plain subject");
        assert_eq!(m.from_name.as_deref(), Some("Alice"));
        assert_eq!(m.from_addr.as_deref(), Some("alice@example.org"));
    }

    #[test]
    fn missing_subject_falls_back() {
        let raw = "From: a@b.c\r\n\r\n";
        let m = parse_summary(raw.as_bytes());
        assert_eq!(m.subject_or_default(), "(无主题)");
    }
}
