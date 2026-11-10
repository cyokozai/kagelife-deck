//! キーの描画内容（文字・色・プレビュー画像）の決定。描画そのもの（ピクセル）は render.rs。
//!
//! 配置（5 列 × 3 行。キー番号 idx = 行 * 5 + 列）。左 3 列は kagelife の数字キー 1〜9 と同じスロット:
//!
//! ```text
//!  idx:  0  1  2 |  3     4
//!        5  6  7 |  8     9
//!       10 11 12 | 13    14
//! 役割:  1  2  3 | TAP   状態
//!        4  5  6 | 方式  0（戻る）
//!        7  8  9 | [     ]
//! ```

use crate::api::State;
use std::rc::Rc;

/// キーの数（MK.2）。
pub const KEYS: usize = 15;
/// スロット 1〜9 のキー番号（読む順）。
pub const SLOT_KEYS: [usize; 9] = [0, 1, 2, 5, 6, 7, 10, 11, 12];
pub const KEY_TAP: usize = 3;
pub const KEY_STATUS: usize = 4;
pub const KEY_MODE: usize = 8;
pub const KEY_BACK: usize = 9;
pub const KEY_BEAT_DOWN: usize = 13;
pub const KEY_BEAT_UP: usize = 14;

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
    pub const ACTIVE: Rgb = Rgb(0, 200, 80);
    pub const TARGET: Rgb = Rgb(255, 150, 0);
    pub const ERROR: Rgb = Rgb(220, 0, 0);
    pub const NAV: Rgb = Rgb(20, 20, 60);
    pub const DISABLED: Rgb = Rgb(25, 25, 25);
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
    /// 縁取り（active・フェード先・エラーの印）
    pub frame: Option<Rgb>,
    pub lines: Vec<String>,
    /// 左上に小さく出す数字キーの番号
    pub badge: Option<String>,
    /// プレビュー（72×72 の RGB）。あれば背景の代わりに敷く
    pub image: Option<Rc<[u8]>>,
}

impl Face {
    fn new(bg: Rgb, fg: Rgb, lines: &[&str]) -> Self {
        Self {
            bg,
            fg,
            frame: None,
            lines: lines.iter().map(|s| s.to_string()).collect(),
            badge: None,
            image: None,
        }
    }
}

/// シェーダのキーを押したときの切替の方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Fade,
    Cut,
}

/// 数字キーに割り当たったシェーダ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub name: String,
    pub image: Option<Rc<[u8]>>,
    pub compile_error: bool,
}

/// キー番号 idx がスロットなら 0 始まりの番号（数字キーは +1）。
pub fn slot_of_key(idx: usize) -> Option<usize> {
    SLOT_KEYS.iter().position(|&k| k == idx)
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

fn slot_face(s: &State, digit: usize, slot: Option<&Slot>) -> Face {
    let badge = Some(digit.to_string());
    let Some(slot) = slot else {
        let mut f = Face::new(color::BLACK, color::DIM, &[]);
        f.badge = badge;
        return f;
    };
    let target = s.fade.fading && s.fade.target == slot.name;
    let active = s.active == slot.name;
    let error = slot.compile_error || s.last_error.as_ref().is_some_and(|e| e.shader == slot.name);
    let role = if target {
        Some(color::TARGET)
    } else if active {
        Some(color::ACTIVE)
    } else {
        None
    };
    let frame = if error { Some(color::ERROR) } else { role };
    match &slot.image {
        Some(img) => Face {
            bg: color::BLACK,
            fg: color::WHITE,
            frame,
            lines: if error { vec!["ERR".into()] } else { vec![] },
            badge,
            image: Some(img.clone()),
        },
        None => {
            let bg = match (target, active) {
                (true, _) => Rgb(120, 70, 0),
                (false, true) => Rgb(0, 100, 40),
                _ => color::SHADER,
            };
            let mut lines = wrap_name(&slot.name);
            if error {
                lines.push("ERR".into());
            }
            Face {
                bg,
                fg: color::WHITE,
                frame,
                lines,
                badge,
                image: None,
            }
        }
    }
}

/// 全キーの描画内容を決める。state が None なら offline の表示。
/// keys は feature `keys` の有無（無ければ 0・`[`・`]` を灰色で無効にする）。
pub fn faces(state: Option<&State>, slots: &[Option<Slot>], mode: Mode, keys: bool) -> Vec<Face> {
    let Some(s) = state else {
        return (0..KEYS)
            .map(|_| Face::new(color::OFFLINE, color::WHITE, &["offline"]))
            .collect();
    };
    let mut v: Vec<Face> = (0..KEYS)
        .map(|_| Face::new(color::BLACK, color::WHITE, &[]))
        .collect();
    for (i, &k) in SLOT_KEYS.iter().enumerate() {
        v[k] = slot_face(s, i + 1, slots.get(i).and_then(Option::as_ref));
    }

    let tap_bg = if s.bpm_measured {
        color::TAP_MEASURED
    } else {
        color::TAP_DEFAULT
    };
    v[KEY_TAP] = Face::new(tap_bg, color::WHITE, &["TAP", &format!("{:.1}", s.bpm)]);

    let fps = format!("{:.0}fps", s.fps);
    v[KEY_STATUS] = if s.last_error.is_some() {
        Face::new(color::STATUS_ERR, color::WHITE, &["ON", &fps, "ERR"])
    } else {
        Face::new(color::STATUS_OK, color::WHITE, &["ON", &fps])
    };

    v[KEY_MODE] = match mode {
        Mode::Fade => Face::new(
            color::MODE_FADE,
            color::WHITE,
            &["FADE", &format!("{} beat", fmt_beats(s.fade.beats))],
        ),
        Mode::Cut => Face::new(color::MODE_CUT, color::WHITE, &["CUT"]),
    };

    let (bg, fg) = if keys {
        (color::NAV, color::WHITE)
    } else {
        (color::DISABLED, color::DIM)
    };
    v[KEY_BACK] = Face::new(bg, fg, &["0", "BACK"]);
    v[KEY_BEAT_DOWN] = Face::new(bg, fg, &["[", "BEAT-"]);
    v[KEY_BEAT_UP] = Face::new(bg, fg, &["]", "BEAT+"]);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Fade, LastError};

    fn state(n: usize) -> State {
        State {
            active: "s01".into(),
            shaders: (1..=n).map(|i| format!("s{i:02}")).collect(),
            bpm: 120.0,
            fade: Fade {
                beats: 4.0,
                ..Default::default()
            },
            fps: 59.9,
            ..Default::default()
        }
    }

    fn slot(name: &str) -> Option<Slot> {
        Some(Slot {
            name: name.into(),
            image: None,
            compile_error: false,
        })
    }

    fn img() -> Rc<[u8]> {
        vec![7u8; 72 * 72 * 3].into()
    }

    #[test]
    fn スロットのキー番号() {
        assert_eq!(slot_of_key(0), Some(0));
        assert_eq!(slot_of_key(2), Some(2));
        assert_eq!(slot_of_key(5), Some(3));
        assert_eq!(slot_of_key(12), Some(8));
        for k in [3, 4, 8, 9, 13, 14] {
            assert_eq!(slot_of_key(k), None);
        }
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
        let f = faces(None, &[], Mode::Fade, true);
        assert_eq!(f.len(), KEYS);
        assert!(
            f.iter()
                .all(|f| f.lines == vec!["offline"] && f.bg == color::OFFLINE)
        );
    }

    #[test]
    fn 文字の表示_active_とフェード先とエラー() {
        let mut s = state(3);
        s.fade.fading = true;
        s.fade.target = "s02".into();
        s.last_error = Some(LastError {
            shader: "s03".into(),
            message: "x".into(),
        });
        let slots = [slot("s01"), slot("s02"), slot("s03")];
        let f = faces(Some(&s), &slots, Mode::Fade, false);
        assert_eq!(f[0].frame, Some(color::ACTIVE));
        assert_eq!(f[0].lines, vec!["s01"]);
        assert_eq!(f[0].badge.as_deref(), Some("1"));
        assert_eq!(f[1].frame, Some(color::TARGET));
        assert_eq!(f[2].frame, Some(color::ERROR));
        assert_eq!(f[2].lines, vec!["s03", "ERR"]);
        // スロット 4（idx 5）は空
        assert!(f[5].lines.is_empty());
        assert_eq!(f[5].badge.as_deref(), Some("4"));
        assert_eq!(f[5].bg, color::BLACK);
    }

    #[test]
    fn フェードしていなければ_target_は印を付けない() {
        let mut s = state(2);
        s.fade.target = "s02".into();
        let f = faces(Some(&s), &[slot("s01"), slot("s02")], Mode::Fade, false);
        assert_eq!(f[1].frame, None);
    }

    #[test]
    fn プレビューは画像に番号と枠を重ねる() {
        let s = state(2);
        let slots = [
            Some(Slot {
                name: "s01".into(),
                image: Some(img()),
                compile_error: false,
            }),
            Some(Slot {
                name: "s02".into(),
                image: None,
                compile_error: true,
            }),
        ];
        let f = faces(Some(&s), &slots, Mode::Fade, true);
        assert!(f[0].image.is_some());
        assert_eq!(f[0].frame, Some(color::ACTIVE));
        assert!(f[0].lines.is_empty());
        assert_eq!(f[0].badge.as_deref(), Some("1"));
        // コンパイル失敗（jpeg が null）は名前と ERR
        assert!(f[1].image.is_none());
        assert_eq!(f[1].frame, Some(color::ERROR));
        assert_eq!(f[1].lines, vec!["s02", "ERR"]);
    }

    #[test]
    fn 同じ画像なら同じ_face() {
        let s = state(1);
        let a = faces(
            Some(&s),
            &[Some(Slot {
                name: "s01".into(),
                image: Some(img()),
                compile_error: false,
            })],
            Mode::Fade,
            true,
        );
        let b = faces(
            Some(&s),
            &[Some(Slot {
                name: "s01".into(),
                image: Some(img()),
                compile_error: false,
            })],
            Mode::Fade,
            true,
        );
        assert_eq!(a[0], b[0]);
    }

    #[test]
    fn keys_が無ければ_0_と括弧は灰色() {
        let s = state(1);
        let off = faces(Some(&s), &[], Mode::Fade, false);
        let on = faces(Some(&s), &[], Mode::Fade, true);
        for k in [KEY_BACK, KEY_BEAT_DOWN, KEY_BEAT_UP] {
            assert_eq!(off[k].bg, color::DISABLED);
            assert_eq!(off[k].fg, color::DIM);
            assert_eq!(on[k].bg, color::NAV);
        }
        assert_eq!(on[KEY_BEAT_DOWN].lines, vec!["[", "BEAT-"]);
        assert_eq!(on[KEY_BEAT_UP].lines, vec!["]", "BEAT+"]);
        assert_eq!(on[KEY_BACK].lines, vec!["0", "BACK"]);
    }

    #[test]
    fn タップは_bpm_と測定の有無() {
        let mut s = state(1);
        let f = faces(Some(&s), &[], Mode::Fade, true);
        assert_eq!(f[KEY_TAP].lines, vec!["TAP", "120.0"]);
        assert_eq!(f[KEY_TAP].bg, color::TAP_DEFAULT);
        s.bpm = 128.25;
        s.bpm_measured = true;
        let f = faces(Some(&s), &[], Mode::Fade, true);
        assert_eq!(f[KEY_TAP].lines, vec!["TAP", "128.2"]);
        assert_eq!(f[KEY_TAP].bg, color::TAP_MEASURED);
    }

    #[test]
    fn 方式のキー() {
        let mut s = state(1);
        assert_eq!(
            faces(Some(&s), &[], Mode::Fade, true)[KEY_MODE].lines,
            vec!["FADE", "4 beat"]
        );
        s.fade.beats = 0.5;
        assert_eq!(
            faces(Some(&s), &[], Mode::Fade, true)[KEY_MODE].lines,
            vec!["FADE", "0.5 beat"]
        );
        let f = faces(Some(&s), &[], Mode::Cut, true);
        assert_eq!(f[KEY_MODE].lines, vec!["CUT"]);
        assert_eq!(f[KEY_MODE].bg, color::MODE_CUT);
    }

    #[test]
    fn 状態のキー() {
        let mut s = state(1);
        let f = faces(Some(&s), &[], Mode::Fade, true);
        assert_eq!(f[KEY_STATUS].lines, vec!["ON", "60fps"]);
        assert_eq!(f[KEY_STATUS].bg, color::STATUS_OK);
        s.last_error = Some(LastError::default());
        let f = faces(Some(&s), &[], Mode::Fade, true);
        assert_eq!(f[KEY_STATUS].lines, vec!["ON", "60fps", "ERR"]);
        assert_eq!(f[KEY_STATUS].bg, color::STATUS_ERR);
    }
}
