//! 偽の kagelife（制御口 v1 ＋ previews・keys）。実機の kagelife なしで kagelife-deck を動かす・測るために使う。
//!
//! 使い方: cargo run --example fake_kagelife -- <発見ファイルの path>
//! 127.0.0.1 の空きポートで待ち、発見ファイルを書く。previews は 10 fps で frame を進め、
//! 9 スロットそれぞれ 16 通りの単色 JPEG を順に返す（JPEG は起動時に作っておく）。

use base64::Engine;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Instant;

const TOKEN: &str = "fake-token";
const VARIANTS: usize = 16;
const STATE: &str = r#"{"active":"s1","shaders":["s1","s2","s3","s4","s5","s6","s7","s8","s9"],"bpm":120.0,"bpm_measured":false,"fade":{"fading":false,"target":"","mix":0.0,"beats":4.0},"fps":60.0,"resolution":[1280,720],"last_error":null,"features":["keys","previews"]}"#;

fn jpeg(rgb: [u8; 3]) -> String {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
        .encode(&rgb.repeat(72 * 72), 72, 72, image::ExtendedColorType::Rgb8)
        .unwrap();
    base64::engine::general_purpose::STANDARD.encode(out)
}

fn main() {
    let path = std::env::args().nth(1).expect("発見ファイルの path を渡す");
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let disc = format!(
        r#"{{"addr":"{addr}","token":"{TOKEN}","pid":{},"shader_dir":"/fake","version":1}}"#,
        std::process::id()
    );
    std::fs::write(&path, disc).unwrap();
    eprintln!("fake_kagelife: {addr} で待つ（発見ファイル {path}）");

    // jpegs[slot][variant]
    let jpegs: Arc<Vec<Vec<String>>> = Arc::new(
        (0..9)
            .map(|s| {
                (0..VARIANTS)
                    .map(|v| jpeg([(s * 28) as u8, (v * 16) as u8, 128]))
                    .collect()
            })
            .collect(),
    );
    let start = Instant::now();
    for s in l.incoming().flatten() {
        let jpegs = jpegs.clone();
        std::thread::spawn(move || serve(s, start, &jpegs));
    }
}

fn serve(s: TcpStream, start: Instant, jpegs: &[Vec<String>]) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut w = s;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        let (mut len, mut auth) = (0usize, String::new());
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                match k.to_ascii_lowercase().as_str() {
                    "content-length" => len = v.trim().parse().unwrap_or(0),
                    "authorization" => auth = v.trim().to_string(),
                    _ => {}
                }
            }
        }
        let mut body = vec![0; len];
        if r.read_exact(&mut body).is_err() {
            return;
        }
        let frame = (start.elapsed().as_millis() / 100) as u64;
        let (code, out) = if auth != format!("Bearer {TOKEN}") {
            (401, r#"{"error":"unauthorized","message":""}"#.to_string())
        } else {
            match (method.as_str(), path.as_str()) {
                ("GET", "/v1/state") => (200, STATE.to_string()),
                ("GET", "/v1/previews") => {
                    let keys: Vec<String> = (0..9)
                        .map(|i| {
                            format!(
                                r#"{{"key":"{}","name":"s{}","jpeg":"{}","compile_error":false}}"#,
                                i + 1,
                                i + 1,
                                jpegs[i][frame as usize % VARIANTS]
                            )
                        })
                        .collect();
                    (
                        200,
                        format!(
                            r#"{{"size":72,"fps":10,"frame":{frame},"keys":[{}]}}"#,
                            keys.join(",")
                        ),
                    )
                }
                ("POST", _) => (200, String::from_utf8_lossy(&body).into_owned()),
                _ => (404, r#"{"error":"not_found","message":""}"#.to_string()),
            }
        };
        let resp = format!(
            "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{out}",
            out.len()
        );
        if w.write_all(resp.as_bytes()).is_err() {
            return;
        }
    }
}
