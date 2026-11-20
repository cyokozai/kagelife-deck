//! コマンドラインの引数（最小限）。

use std::path::PathBuf;

pub const USAGE: &str = "\
使い方: kagelife-deck [--poll-hz <Hz>] [--brightness <0-100>]
                     [--control-file <path> | --ssh <host> [--remote-control-file <path>]]

  --poll-hz       GET /v1/state を取り直す頻度（既定 2、0.1〜30）
  --brightness    Stream Deck の明るさ（既定 60、0〜100）
  --control-file  この機械の kagelife の発見ファイル（既定は $KAGELIFE_CONTROL_FILE、無ければ
                  Linux: $XDG_CACHE_HOME/kagelife/control.json か ~/.cache/kagelife/control.json
                  macOS: ~/Library/Caches/kagelife/control.json）
  --ssh           kagelife が別の機械で動いているとき、その ssh の宛先（~/.ssh/config の別名など）。
                  鍵で入れること（BatchMode=yes）。相手の発見ファイルを ssh で読み、トンネルを張って繋ぐ
  --remote-control-file
                  相手の発見ファイルの場所（既定 ~/.cache/kagelife/control.json。相手のシェルが ~ を展開する）
  -h, --help      この説明を出す";

#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    pub poll_hz: f64,
    pub brightness: u8,
    pub control_file: Option<PathBuf>,
    /// ssh 越しのモードの宛先
    pub ssh: Option<String>,
    pub remote_control_file: Option<String>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            poll_hz: 2.0,
            brightness: 60,
            control_file: None,
            ssh: None,
            remote_control_file: None,
        }
    }
}

impl Args {
    /// ssh 越しのモードで読む、相手の発見ファイルの場所。
    pub fn remote_file(&self) -> &str {
        self.remote_control_file
            .as_deref()
            .unwrap_or(crate::ssh::DEFAULT_REMOTE_FILE)
    }
}

#[derive(Debug, PartialEq)]
pub enum Parsed {
    Run(Args),
    Help,
}

/// 引数（プログラム名を除く）を解析する。`--name value` と `--name=value` の両方を受ける。
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
    let mut a = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(Parsed::Help);
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{name} に値がない"))
        };
        match name.as_str() {
            "--poll-hz" => {
                let v = value()?;
                let hz: f64 = v
                    .parse()
                    .map_err(|_| format!("--poll-hz の値が数でない: {v}"))?;
                if !(0.1..=30.0).contains(&hz) {
                    return Err(format!("--poll-hz は 0.1〜30: {v}"));
                }
                a.poll_hz = hz;
            }
            "--brightness" => {
                let v = value()?;
                let b: u8 = v
                    .parse()
                    .ok()
                    .filter(|b| *b <= 100)
                    .ok_or_else(|| format!("--brightness は 0〜100 の整数: {v}"))?;
                a.brightness = b;
            }
            "--control-file" => {
                let v = value()?;
                if v.is_empty() {
                    return Err("--control-file が空".into());
                }
                a.control_file = Some(PathBuf::from(v));
            }
            "--ssh" => {
                let v = value()?;
                if v.is_empty() || v.starts_with('-') {
                    return Err(format!("--ssh の宛先が空か - で始まる: {v}"));
                }
                a.ssh = Some(v);
            }
            "--remote-control-file" => {
                let v = value()?;
                if v.is_empty() {
                    return Err("--remote-control-file が空".into());
                }
                a.remote_control_file = Some(v);
            }
            _ => return Err(format!("知らない引数: {arg}")),
        }
    }
    if a.ssh.is_none() && a.remote_control_file.is_some() {
        return Err("--remote-control-file は --ssh と一緒に使う".into());
    }
    if a.ssh.is_some() && a.control_file.is_some() {
        return Err("--control-file と --ssh は同時に使えない".into());
    }
    Ok(Parsed::Run(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Result<Parsed, String> {
        parse(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn 既定値() {
        assert_eq!(p(&[]), Ok(Parsed::Run(Args::default())));
        let Ok(Parsed::Run(a)) = p(&[]) else { panic!() };
        assert_eq!(a.poll_hz, 2.0);
        assert_eq!(a.brightness, 60);
    }

    #[test]
    fn 値は空白でも等号でも() {
        let Ok(Parsed::Run(a)) = p(&[
            "--poll-hz",
            "5",
            "--brightness=30",
            "--control-file",
            "/tmp/c.json",
        ]) else {
            panic!()
        };
        assert_eq!(a.poll_hz, 5.0);
        assert_eq!(a.brightness, 30);
        assert_eq!(a.control_file, Some(PathBuf::from("/tmp/c.json")));
    }

    #[test]
    fn 範囲外と知らない引数は拒否() {
        assert!(p(&["--poll-hz", "0"]).is_err());
        assert!(p(&["--poll-hz", "31"]).is_err());
        assert!(p(&["--poll-hz", "x"]).is_err());
        assert!(p(&["--brightness", "101"]).is_err());
        assert!(p(&["--brightness"]).is_err());
        assert!(p(&["--verbose"]).is_err());
    }

    #[test]
    fn ssh_の既定は無しで_相手の発見ファイルは既定の場所() {
        let Ok(Parsed::Run(a)) = p(&[]) else { panic!() };
        assert_eq!(a.ssh, None);
        let Ok(Parsed::Run(a)) = p(&["--ssh", "pc01"]) else {
            panic!()
        };
        assert_eq!(a.ssh.as_deref(), Some("pc01"));
        assert_eq!(a.remote_file(), "~/.cache/kagelife/control.json");
    }

    #[test]
    fn 相手の発見ファイルの場所を変えられる() {
        let Ok(Parsed::Run(a)) = p(&[
            "--ssh=cyokozai@pc01",
            "--remote-control-file",
            "/run/k/control.json",
        ]) else {
            panic!()
        };
        assert_eq!(a.ssh.as_deref(), Some("cyokozai@pc01"));
        assert_eq!(a.remote_file(), "/run/k/control.json");
    }

    #[test]
    fn ssh_の組み合わせの誤りは拒否() {
        // 相手の場所だけ指定しても使い道がない
        assert!(p(&["--remote-control-file", "/x.json"]).is_err());
        // この機械の発見ファイルと ssh は同時に使えない
        assert!(p(&["--ssh", "pc01", "--control-file", "/x.json"]).is_err());
        // ssh のオプションに読まれる名前・空は受け付けない
        assert!(p(&["--ssh", "-oProxyCommand=x"]).is_err());
        assert!(p(&["--ssh="]).is_err());
        assert!(p(&["--ssh", "pc01", "--remote-control-file="]).is_err());
    }

    #[test]
    fn ヘルプ() {
        assert_eq!(p(&["--help"]), Ok(Parsed::Help));
        assert_eq!(p(&["-h"]), Ok(Parsed::Help));
    }
}
