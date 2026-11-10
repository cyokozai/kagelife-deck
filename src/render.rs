//! Face を 72×72 の RGB に描く。文字は embedded-graphics 同梱の FONT_7X14（ASCII）。

use crate::view::{Face, Rgb};
use embedded_graphics::{
    mono_font::{
        MonoTextStyle,
        ascii::{FONT_6X10, FONT_7X14, FONT_10X20},
    },
    pixelcolor::Rgb888,
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, StrokeAlignment},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use std::convert::Infallible;

/// キー画像の一辺（MK.2）。
pub const SIZE: usize = 72;
/// 行の高さ（FONT_7X14 と FONT_10X20 の字の高さ）。
const LINE_H: i32 = 14;
const BIG_H: i32 = 20;
/// 押した直後に白へ寄せる割合（256 分率）。
const LIT: u16 = 100;
/// エラーの縁取りの太さ。
const FRAME: u32 = 4;
/// 番号の字幅と、黒地の高さ（FONT_6X10）。
const BADGE_CHAR_W: u32 = 6;
const BADGE_H: u32 = 12;

/// RGB の画素を並べた画面。
struct Canvas {
    buf: Vec<u8>,
}

impl OriginDimensions for Canvas {
    fn size(&self) -> Size {
        Size::new(SIZE as u32, SIZE as u32)
    }
}

impl DrawTarget for Canvas {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            if (0..SIZE as i32).contains(&p.x) && (0..SIZE as i32).contains(&p.y) {
                let i = (p.y as usize * SIZE + p.x as usize) * 3;
                self.buf[i..i + 3].copy_from_slice(&[c.r(), c.g(), c.b()]);
            }
        }
        Ok(())
    }
}

fn rgb(c: Rgb) -> Rgb888 {
    Rgb888::new(c.0, c.1, c.2)
}

/// face を描いて RGB（72×72×3 バイト、行優先・上が先）を返す。
pub fn render(face: &Face) -> Vec<u8> {
    let Rgb(r, g, b) = face.bg;
    let mut c = Canvas {
        buf: match &face.image {
            Some(img) if img.len() == SIZE * SIZE * 3 => img.to_vec(),
            _ => [r, g, b].repeat(SIZE * SIZE),
        },
    };

    if face.lit {
        for v in c.buf.iter_mut() {
            *v = (*v as u16 + (((255 - *v as u16) * LIT) >> 8)) as u8;
        }
    }

    if let Some(fc) = face.frame {
        let style = PrimitiveStyleBuilder::new()
            .stroke_color(rgb(fc))
            .stroke_width(FRAME)
            .stroke_alignment(StrokeAlignment::Inside)
            .build();
        let _ = Rectangle::new(Point::zero(), c.size())
            .into_styled(style)
            .draw(&mut c);
    }

    // 行の塊を上下の中央に置く。big の行だけ大きな字にする
    let small = MonoTextStyle::new(&FONT_7X14, rgb(face.fg));
    let large = MonoTextStyle::new(&FONT_10X20, rgb(face.fg));
    let layout = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Top)
        .build();
    let height = |i: usize| if face.big == Some(i) { BIG_H } else { LINE_H };
    let total: i32 = (0..face.lines.len()).map(height).sum();
    let mut y = (SIZE as i32 - total) / 2;
    for (i, line) in face.lines.iter().enumerate() {
        let style = if face.big == Some(i) { large } else { small };
        let p = Point::new(SIZE as i32 / 2, y);
        let _ = Text::with_text_style(line, p, style, layout).draw(&mut c);
        y += height(i);
    }

    // 数字キーの番号は左上に、黒地に小さく
    if let Some(badge) = &face.badge {
        let w = BADGE_CHAR_W * badge.chars().count() as u32 + 3;
        let _ = Rectangle::new(Point::zero(), Size::new(w, BADGE_H))
            .into_styled(PrimitiveStyle::with_fill(Rgb888::BLACK))
            .draw(&mut c);
        let style = MonoTextStyle::new(&FONT_6X10, Rgb888::WHITE);
        let _ = Text::with_baseline(badge, Point::new(2, 1), style, Baseline::Top).draw(&mut c);
    }
    c.buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::color;

    fn px(buf: &[u8], x: usize, y: usize) -> Rgb {
        let i = (y * SIZE + x) * 3;
        Rgb(buf[i], buf[i + 1], buf[i + 2])
    }

    fn face(lines: &[&str], frame: Option<Rgb>) -> Face {
        Face {
            bg: color::ACTIVE,
            fg: color::WHITE,
            frame,
            lines: lines.iter().map(|s| s.to_string()).collect(),
            badge: None,
            image: None,
            big: None,
            lit: false,
        }
    }

    #[test]
    fn 大きさと背景() {
        let b = render(&face(&[], None));
        assert_eq!(b.len(), SIZE * SIZE * 3);
        let Rgb(r, g, bl) = color::ACTIVE;
        assert!(b.chunks(3).all(|p| p == [r, g, bl]));
    }

    #[test]
    fn 文字は前景色で描く() {
        let b = render(&face(&["01_uv"], None));
        let fg = b.chunks(3).filter(|p| *p == [240, 240, 240]).count();
        assert!(fg > 20, "前景の画素が少ない: {fg}");
        // 隅は背景のまま
        assert_eq!(px(&b, 0, 0), color::ACTIVE);
    }

    #[test]
    fn 縁取りは内側に描く() {
        let b = render(&face(&[], Some(color::ERROR)));
        assert_eq!(px(&b, 0, 0), color::ERROR);
        assert_eq!(px(&b, 3, 36), color::ERROR);
        assert_eq!(px(&b, 71, 71), color::ERROR);
        assert_eq!(px(&b, 4, 36), color::ACTIVE);
    }

    #[test]
    fn 五行まで収まる() {
        let b = render(&face(&["aaaa", "bbbb", "cccc", "dddd", "ERR"], None));
        // 最上段と最下段の行（1px の余白）は背景
        assert!((0..SIZE).all(|x| px(&b, x, 0) == color::ACTIVE));
        assert!((0..SIZE).all(|x| px(&b, x, 71) == color::ACTIVE));
    }

    #[test]
    fn プレビューを敷いて番号を重ねる() {
        let mut f = face(&[], Some(color::ACTIVE));
        f.image = Some(vec![10u8; SIZE * SIZE * 3].into());
        f.badge = Some("7".into());
        let b = render(&f);
        // 中央はプレビュー、縁は枠、左上は番号の黒地（枠より後に描く）
        assert_eq!(px(&b, 36, 36), Rgb(10, 10, 10));
        assert_eq!(px(&b, 71, 36), color::ACTIVE);
        assert_eq!(px(&b, 0, 0), color::BLACK);
        let white = (0..12)
            .flat_map(|y| (0..9).map(move |x| (x, y)))
            .filter(|&(x, y)| px(&b, x, y) == Rgb(255, 255, 255))
            .count();
        assert!(white > 5, "番号が描かれていない: {white}");
    }

    #[test]
    fn 押した直後は明るい() {
        let mut f = face(&[], None);
        let dark = render(&f);
        f.lit = true;
        let lit = render(&f);
        assert!(px(&lit, 36, 36).1 > px(&dark, 36, 36).1);
    }

    #[test]
    fn 大きな字の行は小さな字より画素が多い() {
        let mut f = face(&["BACK"], None);
        let count = |b: &[u8]| b.chunks(3).filter(|p| *p == [240, 240, 240]).count();
        let small = count(&render(&f));
        f.big = Some(0);
        assert!(count(&render(&f)) > small);
    }

    /// 15 キーを 5 × 3 に並べた見本の PNG を書き出す（目で確かめる用）。
    /// `KDECK_OUT=<dir> cargo test -- --ignored 見本` で <dir>/kdeck-labels-*.png を書く。
    #[test]
    #[ignore]
    fn 見本の画像を書き出す() {
        use crate::api::{Fade, State};
        use crate::view::{Mode, Slot, faces};
        let out = std::path::PathBuf::from(std::env::var("KDECK_OUT").expect("KDECK_OUT"));
        let base = State {
            active: "01_uv".into(),
            shaders: vec!["01_uv".into(), "02_time_sin".into(), "03_bad".into()],
            bpm: 128.0,
            bpm_measured: true,
            fade: Fade {
                fading: true,
                target: "02_time_sin".into(),
                beats: 4.0,
                ..Default::default()
            },
            fps: 59.9,
            features: vec!["keys".into(), "previews".into(), "llm_slots".into()],
            llm_slots: Some(vec!["llm_a".into(), "llm_b".into()]),
            ..Default::default()
        };
        let slots: Vec<Option<Slot>> = base
            .shaders
            .iter()
            .map(|n| {
                Some(Slot {
                    name: n.clone(),
                    image: None,
                    compile_error: n == "03_bad",
                })
            })
            .collect();
        let mut no_llm = base.clone();
        no_llm.llm_slots = Some(vec![]);
        no_llm.fade.beats = 0.5;
        let mut old = base.clone();
        old.features.clear();
        old.llm_slots = None;
        let cases = [
            ("fade", base.clone(), Mode::Fade, true, None),
            (
                "cut-pressed-tap",
                base.clone(),
                Mode::Cut,
                true,
                Some(crate::view::KEY_TAP),
            ),
            ("no-llm-beat-min", no_llm, Mode::Fade, true, None),
            ("old-kagelife", old, Mode::Fade, false, None),
        ];
        for (name, st, mode, keys, lit) in cases {
            let mut fs = faces(Some(&st), &slots, mode, keys);
            if let Some(k) = lit {
                fs[k].lit = true;
            }
            let gap = 8;
            let (w, h) = (5 * (SIZE + gap), 3 * (SIZE + gap));
            let mut sheet = image::RgbImage::new(w as u32, h as u32);
            for (k, f) in fs.iter().enumerate() {
                let b = render(f);
                let (ox, oy) = (
                    (k % 5) * (SIZE + gap) + gap / 2,
                    (k / 5) * (SIZE + gap) + gap / 2,
                );
                for y in 0..SIZE {
                    for x in 0..SIZE {
                        let i = (y * SIZE + x) * 3;
                        sheet.put_pixel(
                            (ox + x) as u32,
                            (oy + y) as u32,
                            image::Rgb([b[i], b[i + 1], b[i + 2]]),
                        );
                    }
                }
            }
            sheet
                .save(out.join(format!("kdeck-labels-{name}.png")))
                .unwrap();
        }
    }

    #[test]
    fn 同じ入力なら同じ画像() {
        let f = face(&["x"], Some(color::ERROR));
        assert_eq!(render(&f), render(&f));
    }
}
