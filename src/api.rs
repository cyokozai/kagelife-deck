//! kagelife の制御口 v1（ADR-006）のクライアント。
//!
//! 使うのは GET /v1/state・POST /v1/switch・POST /v1/crossfade・POST /v1/bpm と、
//! 任意機能の GET /v1/previews（`previews`）・POST /v1/keys（`keys`）。
//! 要求本文には契約のフィールドしか入れない（補足 19: 知らないフィールドは 400 になる）。
//! 応答の知らないフィールドは無視する（deny_unknown_fields を付けない）。

use crate::discovery::{self, Discovery};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

/// GET /v1/state の応答（使う項目だけ）。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct State {
    #[serde(default)]
    pub active: String,
    #[serde(default)]
    pub shaders: Vec<String>,
    #[serde(default = "default_bpm")]
    pub bpm: f64,
    #[serde(default)]
    pub bpm_measured: bool,
    #[serde(default)]
    pub fade: Fade,
    #[serde(default)]
    pub fps: f64,
    #[serde(default)]
    pub last_error: Option<LastError>,
    /// 任意機能の名前（補足 20）。`previews`・`keys` があれば使う
    #[serde(default)]
    pub features: Vec<String>,
}

impl State {
    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

fn default_bpm() -> f64 {
    120.0
}

/// 任意機能 `previews` の名前。
pub const FEATURE_PREVIEWS: &str = "previews";
/// 任意機能 `keys` の名前。
pub const FEATURE_KEYS: &str = "keys";

/// GET /v1/previews の応答。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Previews {
    #[serde(default)]
    pub size: u32,
    #[serde(default)]
    pub fps: f64,
    #[serde(default)]
    pub frame: u64,
    #[serde(default)]
    pub keys: Vec<PreviewKey>,
}

/// previews の keys の 1 つ。key は "1"〜"9"。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct PreviewKey {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub name: String,
    /// base64 の JPEG。まだ描いていない・コンパイルに失敗したスロットでは null
    #[serde(default)]
    pub jpeg: Option<String>,
    #[serde(default)]
    pub compile_error: bool,
}

/// state の fade。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Fade {
    #[serde(default)]
    pub fading: bool,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub mix: f64,
    #[serde(default)]
    pub beats: f64,
}

/// state の last_error。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct LastError {
    #[serde(default)]
    pub shader: String,
    #[serde(default)]
    pub message: String,
}

/// 制御口の呼び出しの失敗。
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// 発見ファイルが無い・繋がらない・認証が通らない（kagelife が動いていないとみなす）
    Offline(String),
    /// 2xx 以外で、401 でないもの（404 not_found など）
    Status {
        code: u16,
        error: String,
        message: String,
    },
    /// 応答を解析できない
    Decode(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline(m) => write!(f, "offline: {m}"),
            Self::Status {
                code,
                error,
                message,
            } => write!(f, "{code} {error}: {message}"),
            Self::Decode(m) => write!(f, "応答を解析できない: {m}"),
        }
    }
}

/// 制御口の操作。テストでは偽物に差し替える。
pub trait Control {
    fn state(&mut self) -> Result<State, ApiError>;
    fn switch(&mut self, name: &str) -> Result<(), ApiError>;
    /// beats は送らない（kagelife の現在のプリセットを使う）
    fn crossfade(&mut self, name: &str) -> Result<(), ApiError>;
    fn set_bpm(&mut self, bpm: f64) -> Result<(), ApiError>;
    /// GET /v1/previews（feature `previews`）
    fn previews(&mut self) -> Result<Previews, ApiError>;
    /// POST /v1/keys（feature `keys`）。shift が None なら送らない
    fn key(&mut self, key: &str, shift: Option<bool>) -> Result<(), ApiError>;
}

/// POST の待ち時間の上限。kagelife の loop_timeout（2 秒）より少し長くする。
const POST_TIMEOUT: Duration = Duration::from_millis(2500);
/// GET（state・previews）の待ち時間の上限。取り直しはすぐ次が来るので、入力を止める時間を短くする。
const GET_TIMEOUT: Duration = Duration::from_secs(1);

/// HTTP で制御口を呼ぶ。接続の失敗と 401 のときは発見ファイルを読み直す。
pub struct HttpControl {
    path: Option<PathBuf>,
    disc: Option<Discovery>,
    get_agent: ureq::Agent,
    post_agent: ureq::Agent,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        // 127.0.0.1 へ直接繋ぐ。環境変数のプロキシを使わない
        .proxy(None)
        .user_agent("kagelife-deck")
        .build()
        .into()
}

/// 送受信の結果（ステータスと本文）。Err は接続の失敗。
type Exchange = Result<(u16, Vec<u8>), String>;

impl HttpControl {
    /// path は発見ファイルの場所。None なら常に offline。
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            disc: None,
            get_agent: agent(GET_TIMEOUT),
            post_agent: agent(POST_TIMEOUT),
        }
    }

    fn exchange(&self, d: &Discovery, method: &str, path: &str, body: Option<&[u8]>) -> Exchange {
        let url = format!("http://{}{}", d.addr, path);
        let auth = format!("Bearer {}", d.token);
        let res = match body {
            None => self
                .get_agent
                .get(&url)
                .header("Authorization", &auth)
                .header("Accept", "application/json")
                .call(),
            Some(b) => {
                debug_assert_eq!(method, "POST");
                self.post_agent
                    .post(&url)
                    .header("Authorization", &auth)
                    .header("Accept", "application/json")
                    .content_type("application/json")
                    .send(b)
            }
        };
        let mut res = res.map_err(|e| e.to_string())?;
        let code = res.status().as_u16();
        let bytes = res.body_mut().read_to_vec().map_err(|e| e.to_string())?;
        Ok((code, bytes))
    }

    fn load(&self) -> Result<Discovery, ApiError> {
        let p = self
            .path
            .as_ref()
            .ok_or_else(|| ApiError::Offline("発見ファイルの場所が決まらない".into()))?;
        discovery::load(p).map_err(|e| ApiError::Offline(e.to_string()))
    }

    /// 要求を送る。接続の失敗・401 のときは発見ファイルを読み直し、中身が変わっていれば 1 回だけ送り直す。
    fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<Vec<u8>, ApiError> {
        let mut d = match self.disc.take() {
            Some(d) => d,
            None => self.load()?,
        };
        for attempt in 0..2 {
            let why = match self.exchange(&d, method, path, body) {
                Ok((code, bytes)) if (200..300).contains(&code) => {
                    self.disc = Some(d);
                    return Ok(bytes);
                }
                Ok((401, _)) => "401 unauthorized".to_string(),
                Ok((code, bytes)) => {
                    self.disc = Some(d);
                    return Err(status_error(code, &bytes));
                }
                Err(e) => e,
            };
            // kagelife が再起動してポートとトークンが変わったかもしれない
            if attempt == 0
                && let Ok(fresh) = self.load()
                && fresh != d
            {
                d = fresh;
                continue;
            }
            return Err(ApiError::Offline(why));
        }
        unreachable!("2 回目は必ず return する")
    }
}

fn status_error(code: u16, bytes: &[u8]) -> ApiError {
    #[derive(Deserialize, Default)]
    struct Body {
        #[serde(default)]
        error: String,
        #[serde(default)]
        message: String,
    }
    let b: Body = serde_json::from_slice(bytes).unwrap_or_default();
    ApiError::Status {
        code,
        error: b.error,
        message: b.message,
    }
}

impl Control for HttpControl {
    fn state(&mut self) -> Result<State, ApiError> {
        let b = self.request("GET", "/v1/state", None)?;
        serde_json::from_slice(&b).map_err(|e| ApiError::Decode(e.to_string()))
    }

    fn switch(&mut self, name: &str) -> Result<(), ApiError> {
        let body = serde_json::json!({ "name": name }).to_string();
        self.request("POST", "/v1/switch", Some(body.as_bytes()))
            .map(|_| ())
    }

    fn crossfade(&mut self, name: &str) -> Result<(), ApiError> {
        let body = serde_json::json!({ "name": name }).to_string();
        self.request("POST", "/v1/crossfade", Some(body.as_bytes()))
            .map(|_| ())
    }

    fn set_bpm(&mut self, bpm: f64) -> Result<(), ApiError> {
        let body = serde_json::json!({ "bpm": bpm }).to_string();
        self.request("POST", "/v1/bpm", Some(body.as_bytes()))
            .map(|_| ())
    }

    fn previews(&mut self) -> Result<Previews, ApiError> {
        let b = self.request("GET", "/v1/previews", None)?;
        serde_json::from_slice(&b).map_err(|e| ApiError::Decode(e.to_string()))
    }

    fn key(&mut self, key: &str, shift: Option<bool>) -> Result<(), ApiError> {
        let body = match shift {
            Some(s) => serde_json::json!({ "key": key, "shift": s }),
            None => serde_json::json!({ "key": key }),
        }
        .to_string();
        self.request("POST", "/v1/keys", Some(body.as_bytes()))
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// 偽サーバが受けた要求。
    #[derive(Debug)]
    struct Got {
        method: String,
        path: String,
        auth: Option<String>,
        body: String,
    }

    /// 127.0.0.1 の空きポートで、決まった応答を順に返す偽サーバを立てる。
    fn fake_server(replies: Vec<(u16, &'static str)>) -> (String, mpsc::Receiver<Got>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for (code, body) in replies {
                let (s, _) = l.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let mut auth = None;
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    match k.to_ascii_lowercase().as_str() {
                        "authorization" => auth = Some(v.trim().to_string()),
                        "content-length" => len = v.trim().parse().unwrap(),
                        _ => {}
                    }
                }
                let mut b = vec![0; len];
                r.read_exact(&mut b).unwrap();
                tx.send(Got {
                    method,
                    path,
                    auth,
                    body: String::from_utf8(b).unwrap(),
                })
                .unwrap();
                let mut s = s;
                write!(
                    s,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (addr, rx)
    }

    /// テストごとに別の発見ファイルを作る。
    struct TempFile(PathBuf);
    impl TempFile {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "kagelife-deck-test-{}-{tag}.json",
                std::process::id()
            ));
            Self(p)
        }
        fn write(&self, addr: &str, token: &str) {
            let j = format!(
                r#"{{"addr":"{addr}","token":"{token}","pid":1,"shader_dir":"/s","version":1}}"#
            );
            std::fs::write(&self.0, j).unwrap();
        }
    }
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn body_json(g: &Got) -> serde_json::Value {
        serde_json::from_str(&g.body).unwrap()
    }

    #[test]
    fn state_は_bearer_付きで取り_知らないフィールドは無視() {
        let (addr, rx) = fake_server(vec![(
            200,
            r#"{"active":"01_uv","shaders":["01_uv","02_x"],"bpm":128.5,"bpm_measured":true,
               "fade":{"fading":true,"target":"02_x","mix":0.3,"beats":4.0},
               "fps":59.9,"resolution":[1280,720],"last_error":{"shader":"02_x","message":"m"},
               "features":[],"unknown":{"a":1}}"#,
        )]);
        let f = TempFile::new("state");
        f.write(&addr, "tok1");
        let mut c = HttpControl::new(Some(f.0.clone()));
        let s = c.state().unwrap();
        assert_eq!(s.active, "01_uv");
        assert_eq!(s.shaders, vec!["01_uv", "02_x"]);
        assert_eq!(s.bpm, 128.5);
        assert!(s.bpm_measured);
        assert!(s.fade.fading);
        assert_eq!(s.fade.target, "02_x");
        assert_eq!(s.fps, 59.9);
        assert_eq!(s.last_error.unwrap().shader, "02_x");
        let g = rx.recv().unwrap();
        assert_eq!(g.method, "GET");
        assert_eq!(g.path, "/v1/state");
        assert_eq!(g.auth.as_deref(), Some("Bearer tok1"));
    }

    #[test]
    fn last_error_が_null() {
        let (addr, _rx) = fake_server(vec![(
            200,
            r#"{"active":"","shaders":[],"bpm":120,"bpm_measured":false,"fade":{"fading":false,"target":"","mix":0,"beats":4},"fps":60,"last_error":null,"features":[]}"#,
        )]);
        let f = TempFile::new("null");
        f.write(&addr, "t");
        let s = HttpControl::new(Some(f.0.clone())).state().unwrap();
        assert_eq!(s.last_error, None);
        assert_eq!(s.bpm, 120.0);
    }

    #[test]
    fn crossfade_は_name_だけを送る() {
        let (addr, rx) = fake_server(vec![(200, r#"{"target":"x","beats":4.0}"#)]);
        let f = TempFile::new("xfade");
        f.write(&addr, "tok");
        HttpControl::new(Some(f.0.clone())).crossfade("x").unwrap();
        let g = rx.recv().unwrap();
        assert_eq!(g.method, "POST");
        assert_eq!(g.path, "/v1/crossfade");
        assert_eq!(g.auth.as_deref(), Some("Bearer tok"));
        assert_eq!(body_json(&g), serde_json::json!({"name":"x"}));
    }

    #[test]
    fn switch_と_bpm_の本文() {
        let (addr, rx) = fake_server(vec![(200, r#"{"active":"y"}"#), (200, r#"{"bpm":128.0}"#)]);
        let f = TempFile::new("switch");
        f.write(&addr, "tok");
        let mut c = HttpControl::new(Some(f.0.clone()));
        c.switch("y").unwrap();
        c.set_bpm(128.0).unwrap();
        let g = rx.recv().unwrap();
        assert_eq!(g.path, "/v1/switch");
        assert_eq!(body_json(&g), serde_json::json!({"name":"y"}));
        let g = rx.recv().unwrap();
        assert_eq!(g.path, "/v1/bpm");
        assert_eq!(g.auth.as_deref(), Some("Bearer tok"));
        assert_eq!(body_json(&g), serde_json::json!({"bpm":128.0}));
    }

    #[test]
    fn 二百番台以外はエラーの中身を返す() {
        let (addr, _rx) = fake_server(vec![(
            404,
            r#"{"error":"not_found","message":"shader x is not loaded"}"#,
        )]);
        let f = TempFile::new("404");
        f.write(&addr, "tok");
        let e = HttpControl::new(Some(f.0.clone())).switch("x").unwrap_err();
        assert_eq!(
            e,
            ApiError::Status {
                code: 404,
                error: "not_found".into(),
                message: "shader x is not loaded".into()
            }
        );
    }

    #[test]
    fn 発見ファイルが無ければ_offline() {
        let f = TempFile::new("missing");
        let e = HttpControl::new(Some(f.0.clone())).state().unwrap_err();
        assert!(matches!(e, ApiError::Offline(_)), "{e:?}");
        let e = HttpControl::new(None).state().unwrap_err();
        assert!(matches!(e, ApiError::Offline(_)), "{e:?}");
    }

    #[test]
    fn 認証が通らなければ発見ファイルを読み直して送り直す() {
        // 古い kagelife: 401 を返す。新しい kagelife: 200
        let (old, _rx_old) = fake_server(vec![(401, r#"{"error":"unauthorized","message":""}"#)]);
        let (new, rx_new) = fake_server(vec![(200, r#"{"active":"a","shaders":["a"],"bpm":120}"#)]);
        let f = TempFile::new("reauth");
        f.write(&old, "old");
        let mut c = HttpControl::new(Some(f.0.clone()));
        // 1 回目の読み込みの後で、kagelife が再起動して発見ファイルを書き換えた
        c.disc = Some(discovery::load(&f.0).unwrap());
        f.write(&new, "new");
        let s = c.state().unwrap();
        assert_eq!(s.active, "a");
        assert_eq!(rx_new.recv().unwrap().auth.as_deref(), Some("Bearer new"));
    }

    #[test]
    fn 繋がらなければ_offline_で_発見ファイルが直れば戻る() {
        // 閉じたポート（bind して即座に離す）
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().to_string()
        };
        let f = TempFile::new("dead");
        f.write(&dead, "t");
        let mut c = HttpControl::new(Some(f.0.clone()));
        assert!(matches!(c.state(), Err(ApiError::Offline(_))));
        let (live, _rx) = fake_server(vec![(200, r#"{"active":"z","shaders":["z"],"bpm":120}"#)]);
        f.write(&live, "t2");
        assert_eq!(c.state().unwrap().active, "z");
    }

    #[test]
    fn state_の_features() {
        let (addr, _rx) = fake_server(vec![(200, r#"{"features":["keys","previews"]}"#)]);
        let f = TempFile::new("features");
        f.write(&addr, "t");
        let s = HttpControl::new(Some(f.0.clone())).state().unwrap();
        assert!(s.has(FEATURE_PREVIEWS) && s.has(FEATURE_KEYS));
        assert!(!State::default().has(FEATURE_KEYS));
    }

    #[test]
    fn previews_を取る_jpeg_の_null_と知らないフィールド() {
        let (addr, rx) = fake_server(vec![(
            200,
            r#"{"size":72,"fps":10,"frame":123,"extra":1,
               "keys":[{"key":"1","name":"01_uv","jpeg":"QUJD","compile_error":false},
                       {"key":"3","name":"03_bad","jpeg":null,"compile_error":true,"x":0}]}"#,
        )]);
        let f = TempFile::new("previews");
        f.write(&addr, "tp");
        let p = HttpControl::new(Some(f.0.clone())).previews().unwrap();
        assert_eq!(p.frame, 123);
        assert_eq!(p.keys.len(), 2);
        assert_eq!(p.keys[0].jpeg.as_deref(), Some("QUJD"));
        assert_eq!(p.keys[1].key, "3");
        assert_eq!(p.keys[1].jpeg, None);
        assert!(p.keys[1].compile_error);
        let g = rx.recv().unwrap();
        assert_eq!(
            (g.method.as_str(), g.path.as_str()),
            ("GET", "/v1/previews")
        );
        assert_eq!(g.auth.as_deref(), Some("Bearer tp"));
    }

    #[test]
    fn keys_の本文_shift_は指定したときだけ() {
        let (addr, rx) = fake_server(vec![(200, r#"{"key":"1"}"#), (200, r#"{"key":"["}"#)]);
        let f = TempFile::new("keys");
        f.write(&addr, "tk");
        let mut c = HttpControl::new(Some(f.0.clone()));
        c.key("1", Some(true)).unwrap();
        c.key("[", None).unwrap();
        let g = rx.recv().unwrap();
        assert_eq!((g.method.as_str(), g.path.as_str()), ("POST", "/v1/keys"));
        assert_eq!(g.auth.as_deref(), Some("Bearer tk"));
        assert_eq!(body_json(&g), serde_json::json!({"key":"1","shift":true}));
        assert_eq!(
            body_json(&rx.recv().unwrap()),
            serde_json::json!({"key":"["})
        );
    }

    #[test]
    fn keys_の_404_と_400_はエラーの中身を返し_offline_にしない() {
        let (addr, _rx) = fake_server(vec![
            (
                404,
                r#"{"error":"not_found","message":"no shader on key 7"}"#,
            ),
            (400, r#"{"error":"invalid_key","message":"bad"}"#),
        ]);
        let f = TempFile::new("keys-err");
        f.write(&addr, "t");
        let mut c = HttpControl::new(Some(f.0.clone()));
        assert!(matches!(
            c.key("7", Some(false)),
            Err(ApiError::Status { code: 404, ref error, .. }) if error == "not_found"
        ));
        assert!(matches!(
            c.key("x", None),
            Err(ApiError::Status { code: 400, ref error, .. }) if error == "invalid_key"
        ));
    }

    /// 要求を読んでから delay 待って 200 を返す偽サーバ（1 回だけ）。
    fn slow_server(delay: Duration) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                let h = h.trim_end().to_ascii_lowercase();
                if h.is_empty() {
                    break;
                }
                if let Some(v) = h.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut b = vec![0; len];
            r.read_exact(&mut b).unwrap();
            thread::sleep(delay);
            let body = r#"{"bpm":120.0}"#;
            let mut s = s;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        addr
    }

    #[test]
    fn get_は1秒で諦める() {
        let addr = slow_server(Duration::from_secs(3));
        let f = TempFile::new("get-timeout");
        f.write(&addr, "t");
        let t = std::time::Instant::now();
        assert!(matches!(
            HttpControl::new(Some(f.0.clone())).state(),
            Err(ApiError::Offline(_))
        ));
        let took = t.elapsed();
        assert!(
            took >= Duration::from_millis(900) && took < Duration::from_millis(1800),
            "{took:?}"
        );
    }

    #[test]
    fn post_は1秒を超えても待つ() {
        let addr = slow_server(Duration::from_millis(1500));
        let f = TempFile::new("post-wait");
        f.write(&addr, "t");
        HttpControl::new(Some(f.0.clone())).set_bpm(120.0).unwrap();
    }

    #[test]
    fn 読み直しても同じなら送り直さない() {
        let (addr, rx) = fake_server(vec![
            (401, r#"{"error":"unauthorized","message":""}"#),
            (200, r#"{}"#),
        ]);
        let f = TempFile::new("same");
        f.write(&addr, "t");
        let mut c = HttpControl::new(Some(f.0.clone()));
        assert!(matches!(c.state(), Err(ApiError::Offline(_))));
        // 1 回しか届いていない
        rx.recv().unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
