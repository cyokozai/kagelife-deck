//! kagelife-deck: Stream Deck MK.2 から kagelife の制御口 v1 を操作する常駐アプリ。

mod api;
mod app;
mod cli;
mod deck;
mod discovery;
mod hw;
mod render;
mod runner;
mod ssh;
mod tap;
mod view;

use std::time::Duration;

/// Stream Deck が無いときに探し直す間隔。
const SCAN_EVERY: Duration = Duration::from_secs(2);

fn main() {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Parsed::Run(a)) => a,
        Ok(cli::Parsed::Help) => {
            println!("{}", cli::USAGE);
            return;
        }
        Err(e) => {
            eprintln!("kagelife-deck: {e}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };
    let control = match &args.ssh {
        Some(host) => {
            install_cleanup();
            eprintln!(
                "kagelife-deck: ssh {host} 越しに繋ぐ（相手の発見ファイル {}）",
                args.remote_file()
            );
            let t = ssh::Tunneler::new(
                ssh::SystemSsh::new(),
                host,
                args.remote_file(),
                Box::new(|m| eprintln!("kagelife-deck: {m}")),
            );
            let backoff = ssh::Backoff::new(ssh::BACKOFF_MIN, ssh::BACKOFF_MAX);
            api::HttpControl::with_locator(Box::new(ssh::SshLocator::start(
                t,
                backoff,
                ssh::CHECK_EVERY,
            )))
        }
        None => {
            let path =
                discovery::resolve_path(args.control_file.as_deref(), |k| std::env::var_os(k));
            match &path {
                Some(p) => eprintln!("kagelife-deck: 発見ファイル {}", p.display()),
                None => eprintln!("kagelife-deck: 発見ファイルの場所が決まらない（HOME も無い）"),
            }
            api::HttpControl::new(path)
        }
    };
    let app = app::App::new(control);
    let poll_every = Duration::from_secs_f64(1.0 / args.poll_hz);
    let mut r = runner::Runner::new(
        app,
        hw::HwOpener::new(args.brightness),
        poll_every,
        SCAN_EVERY,
    );
    loop {
        r.turn();
    }
}

/// 終わるときにトンネルの ssh を止める（孤児の ssh を残さない）。
/// - SIGINT・SIGTERM・SIGHUP: 止めてから終了コード 130 で終わる
/// - panic（release では panic=abort）: abort の前に panic のフックで止める
///
/// SIGKILL では止められない。
fn install_cleanup() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ssh::CHILDREN.kill_all(false);
        prev(info);
    }));
    let set = ctrlc::set_handler(|| {
        eprintln!("kagelife-deck: 終了の合図を受けた。ssh を止めて終わる");
        ssh::CHILDREN.kill_all(true);
        std::process::exit(130);
    });
    if let Err(e) = set {
        eprintln!("kagelife-deck: 終了の合図を受け取れない（ssh が残るかもしれない）: {e}");
    }
}
