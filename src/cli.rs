//! コマンドラインの引数（最小限）。

use std::path::PathBuf;

pub const USAGE: &str = "\
使い方: kagelife-deck [--poll-hz <Hz>] [--brightness <0-100>] [--control-file <path>]

  --poll-hz       GET /v1/state を取り直す頻度（既定 2、0.1〜30）
  --brightness    Stream Deck の明るさ（既定 60、0〜100）
  --control-file  kagelife の発見ファイル（既定は $KAGELIFE_CONTROL_FILE、
                  無ければ $XDG_CACHE_HOME/kagelife/control.json か ~/.cache/kagelife/control.json）
  -h, --help      この説明を出す";

#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    pub poll_hz: f64,
    pub brightness: u8,
    pub control_file: Option<PathBuf>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            poll_hz: 2.0,
            brightness: 60,
            control_file: None,
        }
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
            _ => return Err(format!("知らない引数: {arg}")),
        }
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
    fn ヘルプ() {
        assert_eq!(p(&["--help"]), Ok(Parsed::Help));
        assert_eq!(p(&["-h"]), Ok(Parsed::Help));
    }
}
