//! 单账户监视循环：连接 → 记录 UIDNEXT 基线 → IDLE 等待新邮件 → 推送 → 断线退避重连。
//!
//! 每个账户在独立线程中运行 [`run_account`]，任何网络/协议错误都不会终止进程，
//! 而是指数退避后重新连接（5s → 10s → … → 上限 300s，连接成功后重置）。

use anyhow::{Context, Result, bail};
use imap::extensions::idle;
use std::thread;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::bark::BarkClient;
use crate::config::AccountConfig;
use crate::message;

/// FETCH 头部字段：PEEK 确保不把新邮件标记为已读。
const FETCH_QUERY: &str = "(UID BODY.PEEK[HEADER.FIELDS (SUBJECT FROM DATE)])";

const RECONNECT_MIN: Duration = Duration::from_secs(5);
const RECONNECT_MAX: Duration = Duration::from_secs(300);

pub fn run_account(cfg: AccountConfig, bark: BarkClient, idle_timeout: Duration) {
    let mut backoff = RECONNECT_MIN;
    let mut cursor = MailboxCursor::default();
    loop {
        if let Err(e) = watch_once(&cfg, &bark, idle_timeout, &mut cursor, &mut backoff) {
            warn!(alias = %cfg.alias, error = %format!("{e:#}"), "连接中断");
        }
        info!(alias = %cfg.alias, secs = backoff.as_secs(), "等待后重连");
        thread::sleep(backoff);
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

fn watch_once(
    cfg: &AccountConfig,
    bark: &BarkClient,
    idle_timeout: Duration,
    cursor: &mut MailboxCursor,
    backoff: &mut Duration,
) -> Result<()> {
    let client = imap::ClientBuilder::new(&cfg.host, cfg.port)
        .connect()
        .with_context(|| format!("[{}] 连接 {}:{} 失败", cfg.alias, cfg.host, cfg.port))?;
    let session = client
        .login(&cfg.username, &cfg.password)
        .map_err(|(e, _)| e)
        .with_context(|| format!("[{}] 登录失败（检查用户名/密码或授权码）", cfg.alias))?;

    watch_session(session, cfg, bark, idle_timeout, cursor, backoff)
}

#[derive(Default, Debug)]
struct MailboxCursor {
    uid_validity: Option<u32>,
    // u64 allows the exhausted u32 UID space to be represented without wrapping.
    next_uid: u64,
}

impl MailboxCursor {
    fn select(&mut self, uid_validity: u32, next_uid: u64) {
        if self.uid_validity != Some(uid_validity) {
            self.uid_validity = Some(uid_validity);
            self.next_uid = next_uid;
        }
    }
}

fn watch_session<T: std::io::Read + std::io::Write + idle::SetReadTimeout>(
    mut session: imap::Session<T>,
    cfg: &AccountConfig,
    bark: &BarkClient,
    idle_timeout: Duration,
    cursor: &mut MailboxCursor,
    backoff: &mut Duration,
) -> Result<()> {
    if !session.capabilities()?.has_str("IDLE") {
        bail!("服务器不支持 IDLE，无法实时监听");
    }
    let mailbox = session
        .select(&cfg.folder)
        .with_context(|| format!("[{}] 打开文件夹 {} 失败", cfg.alias, cfg.folder))?;
    // 基线 = UIDNEXT：仅在基线之后到达的邮件才算新邮件，启动时不回放存量邮件
    let baseline = match mailbox.uid_next {
        Some(n) => u64::from(n),
        None => session
            .uid_search("UID 1:*")
            .with_context(|| format!("[{}] 查询已有邮件 UID 失败", cfg.alias))?
            .into_iter()
            .max()
            .map_or(1, |max| u64::from(max) + 1),
    };
    let validity = mailbox
        .uid_validity
        .context("服务器未返回 UIDVALIDITY，无法安全去重")?;
    if validity == 0 || baseline == 0 {
        bail!("服务器返回无效的 UIDVALIDITY 或 UIDNEXT");
    }
    cursor.select(validity, baseline);
    info!(
        alias = %cfg.alias,
        uid_next = cursor.next_uid,
        exists = mailbox.exists,
        "已连接并登录，开始监听"
    );

    loop {
        // 重连后先补查，再进入 IDLE；进程首次连接时基线已跳过存量邮件。
        check_new_mail(&mut session, cfg, bark, &mut cursor.next_uid)?;
        // 命令执行期间收到的 EXISTS 可能已被库缓存，进入 IDLE 前补查。
        if session
            .take_all_unsolicited()
            .any(|r| matches!(r, imap::types::UnsolicitedResponse::Exists(_)))
        {
            continue;
        }
        // 默认 keepalive 会无限在库内部续接，必须关闭才能在超时后回到 SEARCH。
        let outcome = session
            .idle()
            .timeout(idle_timeout)
            .keepalive(false)
            .wait_while(idle::stop_on_any)
            .with_context(|| format!("[{}] IDLE 会话中断", cfg.alias))?;
        debug!(alias = %cfg.alias, ?outcome, "IDLE 结束，检查新邮件");
        *backoff = RECONNECT_MIN;
    }
}

/// 查找 UID >= baseline 的新邮件，逐封推送并推进基线。
fn check_new_mail<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    cfg: &AccountConfig,
    bark: &BarkClient,
    baseline: &mut u64,
) -> Result<()> {
    if *baseline > u64::from(u32::MAX) {
        return Ok(());
    }
    // UID n:* 在 n 不存在时可能返回最大的旧 UID，过滤掉
    let query = format!("UID {baseline}:*");
    let uids = session
        .uid_search(&query)
        .with_context(|| format!("[{}] UID SEARCH 失败", cfg.alias))?;
    let mut new_uids: Vec<u32> = uids
        .into_iter()
        .filter(|u| u64::from(*u) >= *baseline)
        .collect();
    if new_uids.is_empty() {
        return Ok(());
    }
    new_uids.sort_unstable();

    for uid in new_uids.iter() {
        let fetches = session
            .uid_fetch(uid.to_string(), FETCH_QUERY)
            .with_context(|| format!("[{}] UID FETCH {uid} 失败", cfg.alias))?;
        for fetch in fetches.iter() {
            // FETCH 可能夹带其他邮件的非请求响应，不能把它们当作当前 UID。
            if fetch.uid != Some(*uid) {
                continue;
            }
            let Some(headers) = fetch.header() else {
                continue;
            };
            let mail = message::parse_summary(headers);
            info!(
                alias = %cfg.alias,
                uid,
                "收到新邮件"
            );
            if let Err(e) =
                bark.push_new_mail(&cfg.alias, cfg.icon.as_deref(), cfg.url.as_deref(), &mail)
            {
                // 推送失败只告警，不中断 IMAP 会话（基线照常推进，避免恢复后重复通知）
                warn!(alias = %cfg.alias, uid, error = %format!("{e:#}"), "Bark 推送失败");
            }
        }
        // 每封处理后提交进度，后续 FETCH 失败不会导致已处理邮件重发。
        *baseline = u64::from(*uid) + 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bark::BarkSettings, test_support::HttpMock};
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};
    use std::sync::{Arc, Mutex};

    struct Script {
        reads: VecDeque<io::Result<Vec<u8>>>,
        written: Arc<Mutex<Vec<u8>>>,
    }
    impl Read for Script {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            match self.reads.pop_front() {
                Some(Ok(bytes)) => {
                    let n = bytes.len().min(out.len());
                    out[..n].copy_from_slice(&bytes[..n]);
                    if n < bytes.len() {
                        self.reads.push_front(Ok(bytes[n..].to_vec()));
                    }
                    Ok(n)
                }
                Some(Err(e)) => Err(e),
                None => Ok(0),
            }
        }
    }
    impl Write for Script {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.written.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl idle::SetReadTimeout for Script {
        fn set_read_timeout(&mut self, _: Option<Duration>) -> imap::error::Result<()> {
            Ok(())
        }
    }
    fn reply(text: &str) -> io::Result<Vec<u8>> {
        Ok(text.as_bytes().to_vec())
    }
    fn session(
        mut replies: Vec<io::Result<Vec<u8>>>,
    ) -> (imap::Session<Script>, Arc<Mutex<Vec<u8>>>) {
        let written = Arc::new(Mutex::new(Vec::new()));
        replies.insert(0, reply("a1 OK login\r\n"));
        let stream = Script {
            reads: replies.into(),
            written: written.clone(),
        };
        let session = imap::Client::new(stream)
            .login("user", "password")
            .map_err(|(e, _)| e)
            .unwrap();
        (session, written)
    }
    fn cfg() -> AccountConfig {
        AccountConfig {
            alias: "test".into(),
            host: "unused".into(),
            port: 993,
            username: "user".into(),
            password: "password".into(),
            folder: "INBOX".into(),
            icon: None,
            url: None,
        }
    }
    fn bark(url: &str) -> BarkClient {
        BarkClient::new(
            url,
            BarkSettings {
                icon: None,
                sound: None,
                level: None,
                favicon_template: "https://example.com/{domain}".into(),
            },
        )
        .unwrap()
    }
    fn fetch(tag: &str, uid: u32) -> String {
        let header = "Subject: Test\r\nFrom: sender@example.com\r\n\r\n";
        format!(
            "* 1 FETCH (UID {uid} BODY[HEADER.FIELDS (SUBJECT FROM DATE)] {{{}}}\r\n{header})\r\n{tag} OK fetch\r\n",
            header.len()
        )
    }

    #[test]
    fn idle_timeout_checks_mail_and_reconnect_catches_gap_without_replaying_history() {
        let http = HttpMock::new(vec![(200, r#"{"code":200}"#), (200, r#"{"code":200}"#)]);
        let bark = bark(&http.url);
        let mut cursor = MailboxCursor::default();
        let mut backoff = RECONNECT_MAX;
        let (first, written) = session(vec![
            reply("* CAPABILITY IMAP4rev1 IDLE\r\na2 OK capability\r\n"),
            reply(
                "* 9 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 10] next\r\na3 OK select\r\n",
            ),
            reply("* SEARCH 9\r\na4 OK search\r\n"),
            reply("+ idling\r\n"),
            Err(io::Error::new(io::ErrorKind::TimedOut, "timeout")),
            reply("a5 OK idle\r\n"),
            reply("* SEARCH 9 10\r\na6 OK search\r\n"),
            reply(&fetch("a7", 10)),
            reply("+ idling\r\n"),
        ]);
        assert!(
            watch_session(
                first,
                &cfg(),
                &bark,
                Duration::from_secs(60),
                &mut cursor,
                &mut backoff
            )
            .is_err()
        );
        assert_eq!(cursor.next_uid, 11);
        assert_eq!(backoff, RECONNECT_MIN);
        let commands = String::from_utf8(written.lock().unwrap().clone()).unwrap();
        assert!(commands.contains("UID SEARCH UID 10:*"));
        assert!(!commands.contains("UID FETCH 9 "));
        assert!(commands.contains("BODY.PEEK[HEADER.FIELDS"));
        assert!(commands.contains("DONE\r\na6 UID SEARCH"));

        let (second, written) = session(vec![
            reply("* CAPABILITY IMAP4rev1 IDLE\r\na2 OK capability\r\n"),
            reply(
                "* 11 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 12] next\r\na3 OK select\r\n",
            ),
            reply("* SEARCH 11\r\na4 OK search\r\n"),
            reply(&fetch("a5", 11)),
            reply("+ idling\r\n"),
        ]);
        assert!(
            watch_session(
                second,
                &cfg(),
                &bark,
                Duration::from_secs(60),
                &mut cursor,
                &mut backoff
            )
            .is_err()
        );
        assert_eq!(cursor.next_uid, 12);
        assert!(
            String::from_utf8(written.lock().unwrap().clone())
                .unwrap()
                .contains("UID SEARCH UID 11:*")
        );
        assert_eq!(http.finish().len(), 2);
    }

    #[test]
    fn partial_fetch_failure_preserves_progress_and_ignores_unsolicited_fetch() {
        let http = HttpMock::new(vec![(200, r#"{"code":200}"#)]);
        let bark = bark(&http.url);
        let unsolicited = fetch("a3", 99).replace("a3 OK fetch\r\n", "");
        let (mut session, _) = session(vec![
            reply("* SEARCH 11 9 10\r\na2 OK search\r\n"),
            reply(&(unsolicited + &fetch("a3", 10))),
            reply("a4 NO failed fetch\r\n"),
        ]);
        let mut baseline = 10;
        assert!(check_new_mail(&mut session, &cfg(), &bark, &mut baseline).is_err());
        assert_eq!(baseline, 11);
        assert_eq!(http.finish().len(), 1);
    }

    #[test]
    fn validity_change_resets_cursor_and_uid_exhaustion_does_not_wrap() {
        let mut cursor = MailboxCursor::default();
        cursor.select(7, 10);
        cursor.select(7, 20);
        assert_eq!(cursor.next_uid, 10);
        cursor.select(8, 3);
        assert_eq!(cursor.next_uid, 3);
        let (mut session, written) = session(vec![]);
        let mut baseline = u64::from(u32::MAX) + 1;
        check_new_mail(
            &mut session,
            &cfg(),
            &bark("http://127.0.0.1:1/k"),
            &mut baseline,
        )
        .unwrap();
        assert!(
            !String::from_utf8(written.lock().unwrap().clone())
                .unwrap()
                .contains("SEARCH")
        );
    }

    #[test]
    fn server_without_idle_is_rejected_before_select() {
        let (session, written) = session(vec![reply(
            "* CAPABILITY IMAP4rev1\r\na2 OK capability\r\n",
        )]);
        let error = watch_session(
            session,
            &cfg(),
            &bark("http://127.0.0.1:1/k"),
            Duration::from_secs(60),
            &mut MailboxCursor::default(),
            &mut RECONNECT_MIN.clone(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("IDLE"));
        assert!(
            !String::from_utf8(written.lock().unwrap().clone())
                .unwrap()
                .contains("SELECT")
        );
    }
}
