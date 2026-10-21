//! IMAP-Notify：多账户 IMAP IDLE 新邮件监听 + Bark 推送。

mod account;
mod bark;
mod config;
mod message;

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::thread;
use tracing::info;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "IMAP-Notify - IMAP IDLE 新邮件监听，Bark 推送通知

用法:
  imap-notify [-c <config.toml>] [--test-bark | --check] [-v]

参数:
  -c, --config <PATH>   配置文件路径（默认依次查找 ./config.toml、~/.config/imap-notify/config.toml）
      --test-bark       仅向 Bark 发送一条测试通知，用于验证 Key 与图标
      --check           仅校验配置，不执行密码命令或连接网络
      --version         显示版本
  -v, --verbose         输出 debug 级别日志（默认 info，可用 RUST_LOG 覆盖）
  -h, --help            显示本帮助";

struct Args {
    config: Option<PathBuf>,
    test_bark: bool,
    check: bool,
    verbose: bool,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let level = if args.verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config_path = find_config(args.config.as_deref())?;
    info!(config = %config_path.display(), "加载配置");
    let mode = if args.test_bark {
        config::LoadMode::TestBark
    } else if args.check {
        config::LoadMode::Check
    } else {
        config::LoadMode::Watch
    };
    let app_cfg = config::load(&config_path, mode)?;
    if args.check {
        info!("配置校验通过（尚未验证凭据与网络）");
        return Ok(());
    }

    let bark = bark::BarkClient::new(
        &app_cfg.bark.url,
        bark::BarkSettings {
            icon: app_cfg.bark.icon.clone(),
            sound: app_cfg.bark.sound.clone(),
            level: app_cfg.bark.level.clone(),
            favicon_template: app_cfg.bark.favicon_template.clone(),
        },
    )
    .context("初始化 Bark 客户端失败")?;

    if args.test_bark {
        bark.push_test()?;
        info!("测试通知已发送，请在 iPhone 上查收");
        return Ok(());
    }

    ctrlc::set_handler(|| {
        info!("收到退出信号");
        std::process::exit(0);
    })
    .context("安装退出信号处理器失败")?;

    info!(
        accounts = app_cfg
            .accounts
            .iter()
            .map(|a| a.alias.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        idle_timeout_secs = app_cfg.bark.idle_timeout.as_secs(),
        "开始监听新邮件"
    );

    let mut handles = Vec::new();
    for acct in app_cfg.accounts {
        let bark = bark.clone();
        let idle_timeout = app_cfg.bark.idle_timeout;
        let alias = acct.alias.clone();
        handles.push(
            thread::Builder::new()
                .name(format!("acct-{alias}"))
                .spawn(move || account::run_account(acct, bark, idle_timeout))
                .with_context(|| format!("启动账户线程失败: {alias}"))?,
        );
    }
    for h in handles {
        h.join().map_err(|_| anyhow::anyhow!("账户线程异常退出"))?;
    }
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        config: None,
        test_bark: false,
        check: false,
        verbose: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-c" | "--config" => {
                let Some(path) = it.next() else {
                    bail!("-c/--config 需要一个路径参数\n{USAGE}");
                };
                args.config = Some(PathBuf::from(path));
            }
            "--check" => args.check = true,
            "--version" => {
                println!("imap-notify {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--test-bark" => args.test_bark = true,
            "-v" | "--verbose" => args.verbose = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => bail!("未知参数: {other}\n\n{USAGE}"),
        }
    }
    if args.test_bark && args.check {
        bail!("--check 与 --test-bark 不能同时使用");
    }
    Ok(args)
}

fn find_config(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        bail!("配置文件不存在: {}", path.display());
    }
    let cwd_config = Path::new("config.toml");
    if cwd_config.is_file() {
        return Ok(cwd_config.to_path_buf());
    }
    if let Ok(home) = std::env::var("HOME") {
        let home_config = Path::new(&home).join(".config/imap-notify/config.toml");
        if home_config.is_file() {
            return Ok(home_config);
        }
    }
    bail!(
        "未找到配置文件，请使用 -c 指定，或将其放在 ./config.toml 或 ~/.config/imap-notify/config.toml"
    );
}

#[cfg(test)]
mod test_support;
