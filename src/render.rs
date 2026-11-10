//! Face を 72×72 の RGB に描く。文字は embedded-graphics 同梱の FONT_7X14（ASCII）。

use crate::view::{Face, Rgb};
use embedded_graphics::{
    mono_font::{
        MonoTextStyle,
        ascii::{FONT_6X10, FONT_7X14},
    },
    pixelcolor::Rgb888,
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, StrokeAlignment},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use std::convert::Infallible;

/// キー画像の一辺（MK.2）。
pub const SIZE: usize = 72;
/// 行の高さ（FONT_7X14 の字の高さ）。
const LINE_H: i32 = 14;
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

    // 行の塊を上下の中央に置く
    let text_style = MonoTextStyle::new(&FONT_7X14, rgb(face.fg));
    let layout = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Top)
        .build();
    let total = LINE_H * face.lines.len() as i32;
    let top = (SIZE as i32 - total) / 2;
    for (i, line) in face.lines.iter().enumerate() {
        let p = Point::new(SIZE as i32 / 2, top + LINE_H * i as i32);
        let _ = Text::with_text_style(line, p, text_style, layout).draw(&mut c);
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
    fn 同じ入力なら同じ画像() {
        let f = face(&["x"], Some(color::ERROR));
        assert_eq!(render(&f), render(&f));
    }
}
