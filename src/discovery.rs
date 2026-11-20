//! 発見ファイル（ADR-006「接続」）の場所の決定と解析。

use serde::Deserialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 発見ファイルの場所を上書きする環境変数。
pub const ENV_CONTROL_FILE: &str = "KAGELIFE_CONTROL_FILE";

/// 受け付ける制御口の版。
pub const PROTOCOL_VERSION: u32 = 1;

/// 発見ファイルの中身。知らないフィールドは無視する。
/// Debug の表示ではトークンを伏せる（ログに出さないため）。
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Discovery {
    pub addr: String,
    pub token: String,
    #[serde(default)]
    pub pid: i64,
    #[serde(default)]
    pub shader_dir: String,
    pub version: u32,
}

impl std::fmt::Debug for Discovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Discovery")
            .field("addr", &self.addr)
            .field("token", &"<伏せた>")
            .field("pid", &self.pid)
            .field("shader_dir", &self.shader_dir)
            .field("version", &self.version)
            .finish()
    }
}

/// 発見ファイルの既定の場所の規則を選ぶ OS。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Go の os.UserCacheDir（Linux など）: `$XDG_CACHE_HOME`、無ければ `$HOME/.cache`
    Linux,
    /// Go の os.UserCacheDir（darwin）: `$HOME/Library/Caches`
    MacOs,
}

impl Os {
    /// このプログラムが動いている OS の規則。
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// 発見ファイルを読めなかった理由。
#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryError {
    /// ファイルが無い・読めない
    Io(String),
    /// JSON として読めない、または必須項目が無い
    Parse(String),
    /// version が 1 以外
    Version(u32),
    /// addr か token が空
    Empty,
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "発見ファイルを読めない: {e}"),
            Self::Parse(e) => write!(f, "発見ファイルを解析できない: {e}"),
            Self::Version(v) => write!(f, "制御口の版 {v} は扱えない（1 だけ）"),
            Self::Empty => write!(f, "発見ファイルの addr か token が空"),
        }
    }
}

/// 発見ファイルの場所を決める。
///
/// 1. `--control-file` の値
/// 2. 環境変数 `KAGELIFE_CONTROL_FILE`（空でなければ）
/// 3. Go の os.UserCacheDir と同じ規則の下の `kagelife/control.json`
///    - Linux: `$XDG_CACHE_HOME`（空でなければ）、無ければ `$HOME/.cache`
///    - macOS: `$HOME/Library/Caches`
///
/// どれも決まらなければ None。env は環境変数の読み出し（テストで差し替える）。
pub fn resolve_path(arg: Option<&Path>, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    resolve_path_for(Os::current(), arg, env)
}

/// resolve_path の OS を選べる版（OS に依らずに両方の規則をテストするため）。
pub fn resolve_path_for(
    os: Os,
    arg: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    if let Some(p) = arg {
        return Some(p.to_path_buf());
    }
    let non_empty = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(p) = non_empty(ENV_CONTROL_FILE) {
        return Some(PathBuf::from(p));
    }
    let cache = match os {
        Os::Linux => match non_empty("XDG_CACHE_HOME") {
            Some(x) => PathBuf::from(x),
            None => PathBuf::from(non_empty("HOME")?).join(".cache"),
        },
        Os::MacOs => PathBuf::from(non_empty("HOME")?)
            .join("Library")
            .join("Caches"),
    };
    Some(cache.join("kagelife").join("control.json"))
}

/// 発見ファイルの中身を解析し、版と必須項目を確かめる。
pub fn parse(bytes: &[u8]) -> Result<Discovery, DiscoveryError> {
    let d: Discovery =
        serde_json::from_slice(bytes).map_err(|e| DiscoveryError::Parse(e.to_string()))?;
    if d.version != PROTOCOL_VERSION {
        return Err(DiscoveryError::Version(d.version));
    }
    if d.addr.is_empty() || d.token.is_empty() {
        return Err(DiscoveryError::Empty);
    }
    Ok(d)
}

/// path の発見ファイルを読んで解析する。
pub fn load(path: &Path) -> Result<Discovery, DiscoveryError> {
    let b = std::fs::read(path).map_err(|e| DiscoveryError::Io(e.to_string()))?;
    parse(&b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).map(OsString::from)
    }

    #[test]
    fn 引数が最優先() {
        let p = resolve_path(
            Some(Path::new("/x/c.json")),
            env_of(&[(ENV_CONTROL_FILE, "/e.json")]),
        );
        assert_eq!(p, Some(PathBuf::from("/x/c.json")));
    }

    #[test]
    fn 次に環境変数() {
        let p = resolve_path(
            None,
            env_of(&[(ENV_CONTROL_FILE, "/e.json"), ("XDG_CACHE_HOME", "/xdg")]),
        );
        assert_eq!(p, Some(PathBuf::from("/e.json")));
    }

    #[test]
    fn 空の環境変数は無いものとする() {
        let p = resolve_path_for(
            Os::Linux,
            None,
            env_of(&[
                (ENV_CONTROL_FILE, ""),
                ("XDG_CACHE_HOME", ""),
                ("HOME", "/home/u"),
            ]),
        );
        assert_eq!(
            p,
            Some(PathBuf::from("/home/u/.cache/kagelife/control.json"))
        );
    }

    #[test]
    fn xdg_cache_home_を使う() {
        let p = resolve_path_for(
            Os::Linux,
            None,
            env_of(&[("XDG_CACHE_HOME", "/xdg"), ("HOME", "/home/u")]),
        );
        assert_eq!(p, Some(PathBuf::from("/xdg/kagelife/control.json")));
    }

    #[test]
    fn どれも無ければ_none() {
        assert_eq!(resolve_path(None, env_of(&[])), None);
    }

    #[test]
    fn linux_の規則は_xdg_と_dot_cache() {
        let p = resolve_path_for(
            Os::Linux,
            None,
            env_of(&[("XDG_CACHE_HOME", "/xdg"), ("HOME", "/home/u")]),
        );
        assert_eq!(p, Some(PathBuf::from("/xdg/kagelife/control.json")));
        let p = resolve_path_for(Os::Linux, None, env_of(&[("HOME", "/home/u")]));
        assert_eq!(
            p,
            Some(PathBuf::from("/home/u/.cache/kagelife/control.json"))
        );
    }

    #[test]
    fn macos_の既定は_library_caches_で_xdg_を見ない() {
        // Go の os.UserCacheDir（darwin）は $HOME/Library/Caches。XDG_CACHE_HOME は使わない
        let p = resolve_path_for(
            Os::MacOs,
            None,
            env_of(&[("XDG_CACHE_HOME", "/xdg"), ("HOME", "/Users/u")]),
        );
        assert_eq!(
            p,
            Some(PathBuf::from(
                "/Users/u/Library/Caches/kagelife/control.json"
            ))
        );
    }

    #[test]
    fn macos_でも引数と環境変数が先() {
        let env = env_of(&[(ENV_CONTROL_FILE, "/e.json"), ("HOME", "/Users/u")]);
        assert_eq!(
            resolve_path_for(Os::MacOs, Some(Path::new("/a.json")), &env),
            Some(PathBuf::from("/a.json"))
        );
        assert_eq!(
            resolve_path_for(Os::MacOs, None, &env),
            Some(PathBuf::from("/e.json"))
        );
    }

    #[test]
    fn macos_で_home_が無ければ_none() {
        assert_eq!(
            resolve_path_for(Os::MacOs, None, env_of(&[("HOME", "")])),
            None
        );
    }

    #[test]
    fn 今の_os_の規則を使う() {
        let want = if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        };
        assert_eq!(Os::current(), want);
    }

    #[test]
    fn debug_の表示にトークンを出さない() {
        let d = parse(br#"{"addr":"127.0.0.1:1","token":"SECRET-tok-42","version":1}"#).unwrap();
        let s = format!("{d:?}");
        assert!(!s.contains("SECRET-tok-42"), "{s}");
        assert!(s.contains("127.0.0.1:1"), "{s}");
    }

    #[test]
    fn 正しい発見ファイルを読む_知らないフィールドは無視() {
        let d = parse(
            br#"{"addr":"127.0.0.1:4000","token":"ab","pid":7,"shader_dir":"/s","version":1,"extra":true}"#,
        )
        .unwrap();
        assert_eq!(d.addr, "127.0.0.1:4000");
        assert_eq!(d.token, "ab");
        assert_eq!(d.pid, 7);
    }

    #[test]
    fn 版が1以外は受け付けない() {
        let e = parse(br#"{"addr":"127.0.0.1:1","token":"t","version":2}"#).unwrap_err();
        assert_eq!(e, DiscoveryError::Version(2));
    }

    #[test]
    fn 必須項目が無い_または空() {
        assert!(matches!(
            parse(br#"{"addr":"127.0.0.1:1","version":1}"#),
            Err(DiscoveryError::Parse(_))
        ));
        assert_eq!(
            parse(br#"{"addr":"","token":"t","version":1}"#),
            Err(DiscoveryError::Empty)
        );
        assert!(matches!(parse(b"not json"), Err(DiscoveryError::Parse(_))));
    }

    #[test]
    fn ファイルが無ければ_io() {
        assert!(matches!(
            load(Path::new("/nonexistent/kagelife-deck-test/control.json")),
            Err(DiscoveryError::Io(_))
        ));
    }
}
