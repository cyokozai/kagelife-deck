//! kagelife-deck: Stream Deck MK.2 から kagelife の制御口 v1 を操作する常駐アプリ。

mod api;
mod app;
mod cli;
mod deck;
mod discovery;
mod hw;
mod render;
mod runner;
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
    let path = discovery::resolve_path(args.control_file.as_deref(), |k| std::env::var_os(k));
    match &path {
        Some(p) => eprintln!("kagelife-deck: 発見ファイル {}", p.display()),
        None => eprintln!("kagelife-deck: 発見ファイルの場所が決まらない（HOME も無い）"),
    }
    let app = app::App::new(api::HttpControl::new(path));
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
