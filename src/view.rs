//! キーの描画内容（文字・色）の決定。描画そのもの（ピクセル）は render.rs。
//!
//! 配置（5 列 × 3 行。キー番号は左上から 0〜14）:
//! - 0〜9: シェーダの一覧（10 個ずつのページ）
//! - 10: 前のページ / 11: 次のページ / 12: タップ BPM / 13: 切替の方式（fade ↔ cut） / 14: 状態

use crate::api::State;

/// キーの数（MK.2）。
pub const KEYS: usize = 15;
/// 1 ページに並べるシェーダの数。
pub const PAGE_SIZE: usize = 10;
pub const KEY_PREV: usize = 10;
pub const KEY_NEXT: usize = 11;
pub const KEY_TAP: usize = 12;
pub const KEY_MODE: usize = 13;
pub const KEY_STATUS: usize = 14;

/// 1 行に入る文字数（72px ÷ 7px の字幅。FONT_7X14 を使う）。
pub const LINE_CHARS: usize = 10;
/// シェーダ名に使う行の上限（ERR の行を足しても 5 行 × 14px に収まる数）。
pub const NAME_LINES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// 色の取り決め。
pub mod color {
    use super::Rgb;
    pub const WHITE: Rgb = Rgb(240, 240, 240);
    pub const DIM: Rgb = Rgb(90, 90, 90);
    pub const BLACK: Rgb = Rgb(0, 0, 0);
    pub const SHADER: Rgb = Rgb(32, 32, 40);
    pub const ACTIVE: Rgb = Rgb(0, 110, 50);
    pub const TARGET: Rgb = Rgb(190, 100, 0);
    pub const ERROR: Rgb = Rgb(220, 0, 0);
    pub const NAV: Rgb = Rgb(20, 20, 60);
    pub const TAP_DEFAULT: Rgb = Rgb(50, 50, 50);
    pub const TAP_MEASURED: Rgb = Rgb(0, 70, 160);
    pub const MODE_FADE: Rgb = Rgb(90, 0, 120);
    pub const MODE_CUT: Rgb = Rgb(0, 100, 110);
    pub const STATUS_OK: Rgb = Rgb(0, 90, 40);
    pub const STATUS_ERR: Rgb = Rgb(150, 0, 0);
    pub const OFFLINE: Rgb = Rgb(40, 0, 0);
}

/// 1 つのキーの描画内容。これが前回と同じならキー画像を送らない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Face {
    pub bg: Rgb,
    pub fg: Rgb,
    /// 縁取り（エラーの印）
    pub frame: Option<Rgb>,
    pub lines: Vec<String>,
}

impl Face {
    fn new(bg: Rgb, fg: Rgb, lines: &[&str]) -> Self {
        Self {
            bg,
            fg,
            frame: None,
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// シェーダのキーを押したときの切替の方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Fade,
    Cut,
}

/// シェーダ n 個のページ数（0 個でも 1）。
pub fn page_count(n: usize) -> usize {
    n.div_ceil(PAGE_SIZE).max(1)
}

/// ページ送り（端で反対側へ回る）。delta は +1 か -1。
pub fn step_page(page: usize, delta: isize, n: usize) -> usize {
    let pc = page_count(n) as isize;
    (page as isize + delta).rem_euclid(pc) as usize
}

/// page の key 番目（0〜9）に並ぶシェーダの名前。
pub fn shader_at(state: &State, page: usize, key: usize) -> Option<&str> {
    if key >= PAGE_SIZE {
        return None;
    }
    state
        .shaders
        .get(page * PAGE_SIZE + key)
        .map(String::as_str)
}

/// 名前を LINE_CHARS 文字ずつに折り返す。NAME_LINES 行を超える分は最後の行の末尾を `~` にして切る。
pub fn wrap_name(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut lines: Vec<String> = chars
        .chunks(LINE_CHARS)
        .map(|c| c.iter().collect())
        .collect();
    if lines.len() > NAME_LINES {
        lines.truncate(NAME_LINES);
        let last = lines.last_mut().expect("NAME_LINES > 0");
        last.pop();
        last.push('~');
    }
    lines
}

/// 拍数の表示（整数なら小数点を付けない）。
fn fmt_beats(b: f64) -> String {
    if b.fract() == 0.0 {
        format!("{b:.0}")
    } else {
        format!("{b}")
    }
}

/// 全キーの描画内容を決める。state が None なら offline の表示。
pub fn faces(state: Option<&State>, page: usize, mode: Mode) -> Vec<Face> {
    let Some(s) = state else {
        return (0..KEYS)
            .map(|_| Face::new(color::OFFLINE, color::WHITE, &["offline"]))
            .collect();
    };
    let mut v = Vec::with_capacity(KEYS);
    let err_shader = s.last_error.as_ref().map(|e| e.shader.as_str());
    for key in 0..PAGE_SIZE {
        v.push(match shader_at(s, page, key) {
            None => Face::new(color::BLACK, color::WHITE, &[]),
            Some(name) => {
                let bg = if s.fade.fading && s.fade.target == name {
                    color::TARGET
                } else if s.active == name {
                    color::ACTIVE
                } else {
                    color::SHADER
                };
                let mut f = Face {
                    bg,
                    fg: color::WHITE,
                    frame: None,
                    lines: wrap_name(name),
                };
                if err_shader == Some(name) {
                    f.frame = Some(color::ERROR);
                    f.lines.push("ERR".into());
                }
                f
            }
        });
    }

    let pc = page_count(s.shaders.len());
    let pos = format!("{}/{}", page + 1, pc);
    let nav_fg = if pc > 1 { color::WHITE } else { color::DIM };
    v.push(Face::new(color::NAV, nav_fg, &["<", "PAGE", &pos]));
    v.push(Face::new(color::NAV, nav_fg, &[">", "PAGE", &pos]));

    let tap_bg = if s.bpm_measured {
        color::TAP_MEASURED
    } else {
        color::TAP_DEFAULT
    };
    v.push(Face::new(tap_bg, color::WHITE, &["TAP", &format!("{:.1}", s.bpm)]));

    v.push(match mode {
        Mode::Fade => Face::new(
            color::MODE_FADE,
            color::WHITE,
            &["FADE", &format!("{} beat", fmt_beats(s.fade.beats))],
        ),
        Mode::Cut => Face::new(color::MODE_CUT, color::WHITE, &["CUT"]),
    });

    let fps = format!("{:.0}fps", s.fps);
    v.push(if s.last_error.is_some() {
        Face::new(color::STATUS_ERR, color::WHITE, &["ON", &fps, "ERR"])
    } else {
        Face::new(color::STATUS_OK, color::WHITE, &["ON", &fps])
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Fade, LastError};

    fn state(n: usize) -> State {
        State {
            active: "s00".into(),
            shaders: (0..n).map(|i| format!("s{i:02}")).collect(),
            bpm: 120.0,
            fade: Fade {
                beats: 4.0,
                ..Default::default()
            },
            fps: 59.9,
            ..Default::default()
        }
    }

    #[test]
    fn ページ数() {
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(1), 1);
        assert_eq!(page_count(10), 1);
        assert_eq!(page_count(11), 2);
        assert_eq!(page_count(25), 3);
    }

    #[test]
    fn ページ送りは端で回る() {
        assert_eq!(step_page(0, 1, 25), 1);
        assert_eq!(step_page(2, 1, 25), 0);
        assert_eq!(step_page(0, -1, 25), 2);
        assert_eq!(step_page(0, 1, 3), 0);
    }

    #[test]
    fn ページの中のシェーダ() {
        let s = state(13);
        assert_eq!(shader_at(&s, 0, 0), Some("s00"));
        assert_eq!(shader_at(&s, 1, 2), Some("s12"));
        assert_eq!(shader_at(&s, 1, 3), None);
        assert_eq!(shader_at(&s, 0, 10), None);
    }

    #[test]
    fn 名前の折り返し() {
        assert_eq!(wrap_name("01_uv"), vec!["01_uv"]);
        assert_eq!(wrap_name("abcdefghijkl"), vec!["abcdefghij", "kl"]);
        let long = "a".repeat(64);
        let w = wrap_name(&long);
        assert_eq!(w.len(), NAME_LINES);
        assert!(w.last().unwrap().ends_with('~'));
        assert_eq!(w.last().unwrap().chars().count(), LINE_CHARS);
    }

    #[test]
    fn offline_は全キーに出す() {
        let f = faces(None, 0, Mode::Fade);
        assert_eq!(f.len(), KEYS);
        assert!(f.iter().all(|f| f.lines == vec!["offline"] && f.bg == color::OFFLINE));
    }

    #[test]
    fn active_と_フェード先と_エラーの色() {
        let mut s = state(3);
        s.active = "s00".into();
        s.fade.fading = true;
        s.fade.target = "s01".into();
        s.last_error = Some(LastError {
            shader: "s02".into(),
            message: "x".into(),
        });
        let f = faces(Some(&s), 0, Mode::Fade);
        assert_eq!(f[0].bg, color::ACTIVE);
        assert_eq!(f[1].bg, color::TARGET);
        assert_eq!(f[2].bg, color::SHADER);
        assert_eq!(f[2].frame, Some(color::ERROR));
        assert_eq!(f[2].lines, vec!["s02", "ERR"]);
        assert_eq!(f[3].lines, Vec::<String>::new());
        assert_eq!(f[3].bg, color::BLACK);
    }

    #[test]
    fn フェードしていなければ_target_は色を付けない() {
        let mut s = state(2);
        s.fade.target = "s01".into();
        let f = faces(Some(&s), 0, Mode::Fade);
        assert_eq!(f[1].bg, color::SHADER);
    }

    #[test]
    fn 二ページ目() {
        let s = state(12);
        let f = faces(Some(&s), 1, Mode::Fade);
        assert_eq!(f[0].lines, vec!["s10"]);
        assert_eq!(f[1].lines, vec!["s11"]);
        assert!(f[2].lines.is_empty());
        assert_eq!(f[KEY_PREV].lines, vec!["<", "PAGE", "2/2"]);
        assert_eq!(f[KEY_NEXT].fg, color::WHITE);
    }

    #[test]
    fn 一ページだけならページ送りは暗く() {
        let f = faces(Some(&state(3)), 0, Mode::Fade);
        assert_eq!(f[KEY_PREV].fg, color::DIM);
        assert_eq!(f[KEY_NEXT].lines, vec![">", "PAGE", "1/1"]);
    }

    #[test]
    fn タップは_bpm_と測定の有無() {
        let mut s = state(1);
        let f = faces(Some(&s), 0, Mode::Fade);
        assert_eq!(f[KEY_TAP].lines, vec!["TAP", "120.0"]);
        assert_eq!(f[KEY_TAP].bg, color::TAP_DEFAULT);
        s.bpm = 128.25;
        s.bpm_measured = true;
        let f = faces(Some(&s), 0, Mode::Fade);
        assert_eq!(f[KEY_TAP].lines, vec!["TAP", "128.2"]);
        assert_eq!(f[KEY_TAP].bg, color::TAP_MEASURED);
    }

    #[test]
    fn 方式のキー() {
        let mut s = state(1);
        assert_eq!(faces(Some(&s), 0, Mode::Fade)[KEY_MODE].lines, vec!["FADE", "4 beat"]);
        s.fade.beats = 0.5;
        assert_eq!(faces(Some(&s), 0, Mode::Fade)[KEY_MODE].lines, vec!["FADE", "0.5 beat"]);
        let f = faces(Some(&s), 0, Mode::Cut);
        assert_eq!(f[KEY_MODE].lines, vec!["CUT"]);
        assert_eq!(f[KEY_MODE].bg, color::MODE_CUT);
    }

    #[test]
    fn 状態のキー() {
        let mut s = state(1);
        let f = faces(Some(&s), 0, Mode::Fade);
        assert_eq!(f[KEY_STATUS].lines, vec!["ON", "60fps"]);
        assert_eq!(f[KEY_STATUS].bg, color::STATUS_OK);
        s.last_error = Some(LastError::default());
        let f = faces(Some(&s), 0, Mode::Fade);
        assert_eq!(f[KEY_STATUS].lines, vec!["ON", "60fps", "ERR"]);
        assert_eq!(f[KEY_STATUS].bg, color::STATUS_ERR);
    }
}
