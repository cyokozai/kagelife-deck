//! 実機の Stream Deck（elgato-streamdeck crate、同期 API）。テストの外にある唯一のデバイス依存部分。

use crate::deck::Deck;
use crate::render::SIZE;
use crate::runner::Opener;
use elgato_streamdeck::images::convert_image_with_format;
use elgato_streamdeck::info::ImageFormat;
use elgato_streamdeck::{
    StreamDeck, StreamDeckInput, list_devices, new_hidapi, refresh_device_list,
};
use hidapi::HidApi;
use image::{DynamicImage, RgbImage};
use std::time::Duration;

/// 15 キーの Stream Deck（MK.2 とその仲間）を探して開く。
pub struct HwOpener {
    api: Option<HidApi>,
    brightness: u8,
    /// 「見つからない」を探すたびに出さないための印
    reported_missing: bool,
}

impl HwOpener {
    pub fn new(brightness: u8) -> Self {
        Self {
            api: None,
            brightness,
            reported_missing: false,
        }
    }

    fn missing(&mut self, why: String) -> Option<Box<dyn Deck>> {
        if !self.reported_missing {
            eprintln!("kagelife-deck: Stream Deck が見つからない（挿されるまで探し続ける）: {why}");
            self.reported_missing = true;
        }
        None
    }
}

impl Opener for HwOpener {
    fn open(&mut self) -> Option<Box<dyn Deck>> {
        if self.api.is_none() {
            match new_hidapi() {
                Ok(a) => self.api = Some(a),
                Err(e) => return self.missing(format!("hidapi: {e}")),
            }
        }
        let api = self.api.as_mut()?;
        if let Err(e) = refresh_device_list(api) {
            return self.missing(format!("一覧の更新: {e}"));
        }
        let Some((kind, serial)) = list_devices(api)
            .into_iter()
            .find(|(k, _)| k.key_count() == 15 && k.is_visual())
        else {
            return self.missing("15 キーの機種が無い".into());
        };
        let sd = match StreamDeck::connect(api, kind, &serial) {
            Ok(sd) => sd,
            // 権限が無いとき（udev ルール・plugdev）はここに来る
            Err(e) => return self.missing(format!("{kind:?} を開けない: {e}")),
        };
        if let Err(e) = sd.reset().and_then(|_| sd.set_brightness(self.brightness)) {
            return self.missing(format!("初期化: {e}"));
        }
        eprintln!("kagelife-deck: Stream Deck を開いた: {kind:?}");
        self.reported_missing = false;
        Some(Box::new(HwDeck {
            format: kind.key_image_format(),
            sd,
        }))
    }
}

struct HwDeck {
    sd: StreamDeck,
    format: ImageFormat,
}

impl Deck for HwDeck {
    fn set_image(&mut self, key: u8, rgb: &[u8]) -> Result<(), String> {
        let img = RgbImage::from_raw(SIZE as u32, SIZE as u32, rgb.to_vec())
            .ok_or("キー画像の大きさが合わない")?;
        // 機種ごとの向き・反転・JPEG への変換は crate に任せる
        let data = convert_image_with_format(self.format, DynamicImage::ImageRgb8(img))
            .map_err(|e| e.to_string())?;
        self.sd.write_image(key, &data).map_err(|e| e.to_string())
    }

    fn flush(&mut self) -> Result<(), String> {
        self.sd.flush().map_err(|e| e.to_string())
    }

    fn read_buttons(&mut self, timeout: Duration) -> Result<Option<Vec<bool>>, String> {
        match self.sd.read_input(Some(timeout)) {
            Ok(StreamDeckInput::ButtonStateChange(v)) => Ok(Some(v)),
            Ok(_) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::render;
    use crate::view::{Face, color};
    use elgato_streamdeck::info::Kind;

    /// 実機に送る直前までの計算量の目安（10 Hz で 9 スロットが毎回変わる場合の 10 秒分）。
    /// `cargo test --release -- --ignored --nocapture 描画と変換` で測る。
    #[test]
    #[ignore]
    fn 描画と変換の計算量() {
        let fmt = Kind::Mk2.key_image_format();
        let t = std::time::Instant::now();
        let mut bytes = 0;
        for frame in 0..100u32 {
            for slot in 0..9u8 {
                let face = Face {
                    bg: color::BLACK,
                    fg: color::WHITE,
                    frame: Some(color::ACTIVE),
                    lines: vec![],
                    badge: Some((slot + 1).to_string()),
                    image: Some(vec![(frame as u8).wrapping_mul(slot); SIZE * SIZE * 3].into()),
                    big: None,
                    lit: false,
                };
                let rgb = render(&face);
                let img = RgbImage::from_raw(SIZE as u32, SIZE as u32, rgb).unwrap();
                bytes += convert_image_with_format(fmt, DynamicImage::ImageRgb8(img))
                    .unwrap()
                    .len();
            }
        }
        println!(
            "900 キー分（10 Hz × 9 キー × 10 秒）: {:?}、JPEG 計 {bytes} バイト",
            t.elapsed()
        );
    }
}
