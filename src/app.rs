//! 押されたキーを制御口の呼び出しに変え、表示する状態を持つ。デバイスとは独立。
//!
//! kagelife が feature `previews` と `keys` を持つときは、数字キーの割り当てとプレビューを
//! GET /v1/previews から取り、スロットのキーは POST /v1/keys で押す（割り当てはデッキ側で再計算しない）。
//! 持たない古い kagelife では、shaders の先頭 9 個をファイル名順で 1〜9 とみなし、switch / crossfade を呼ぶ。

use crate::api::{ApiError, Control, FEATURE_KEYS, FEATURE_PREVIEWS, PreviewKey, State};
use crate::render::SIZE;
use crate::tap::{TapResult, Tapper};
use crate::view::{self, Face, Mode, SLOT_KEYS, Slot};
use base64::Engine;
use std::rc::Rc;
use std::time::Instant;

/// スロットの数（数字キー 1〜9）。
const SLOTS: usize = SLOT_KEYS.len();

/// 前回の previews から作ったスロット。jpeg の文字列が同じなら復号し直さない。
#[derive(Default)]
struct PreviewSlots {
    frame: u64,
    slots: Vec<Option<Slot>>,
    jpegs: Vec<Option<String>>,
}

pub struct App<C: Control> {
    ctl: C,
    state: Option<State>,
    previews: Option<PreviewSlots>,
    mode: Mode,
    tapper: Tapper,
}

impl<C: Control> App<C> {
    pub fn new(ctl: C) -> Self {
        Self {
            ctl,
            state: None,
            previews: None,
            mode: Mode::Fade,
            tapper: Tapper::kagelife_default(),
        }
    }

    fn has(&self, feature: &str) -> bool {
        self.state.as_ref().is_some_and(|s| s.has(feature))
    }

    /// スロットのキーを POST /v1/keys で押すか。
    fn use_keys(&self) -> bool {
        self.has(FEATURE_PREVIEWS) && self.has(FEATURE_KEYS)
    }

    /// GET /v1/previews を取るべきか（feature `previews` がある間だけ）。
    pub fn wants_previews(&self) -> bool {
        self.has(FEATURE_PREVIEWS)
    }

    /// GET /v1/state を取り直す。失敗したら offline の表示にする。
    pub fn poll(&mut self) {
        match self.ctl.state() {
            Ok(s) => {
                if self.state.is_none() {
                    eprintln!("kagelife-deck: kagelife に繋がった");
                }
                if !s.has(FEATURE_PREVIEWS) {
                    self.previews = None;
                }
                self.state = Some(s);
            }
            Err(e) => {
                if self.state.is_some() {
                    eprintln!("kagelife-deck: kagelife から切れた: {e}");
                }
                self.state = None;
                self.previews = None;
            }
        }
    }

    /// GET /v1/previews を取る。frame が前回と同じなら何もしない。
    pub fn poll_previews(&mut self) {
        if !self.wants_previews() {
            return;
        }
        let p = match self.ctl.previews() {
            Ok(p) => p,
            // 切れたかどうかは state の取り直しで決める
            Err(ApiError::Offline(_)) => return,
            Err(e) => {
                eprintln!("kagelife-deck: previews を取れない: {e}");
                self.previews = None;
                return;
            }
        };
        if self
            .previews
            .as_ref()
            .is_some_and(|old| old.frame == p.frame)
        {
            return;
        }
        let old = self.previews.take().unwrap_or_default();
        let mut next = PreviewSlots {
            frame: p.frame,
            slots: vec![None; SLOTS],
            jpegs: vec![None; SLOTS],
        };
        for k in &p.keys {
            let Some(i) = digit_slot(k) else { continue };
            let image = match &k.jpeg {
                None => None,
                Some(b64) if old.jpegs.get(i).and_then(Option::as_ref) == Some(b64) => old
                    .slots
                    .get(i)
                    .and_then(Option::as_ref)
                    .and_then(|s| s.image.clone()),
                Some(b64) => decode_preview(b64),
            };
            next.jpegs[i] = k.jpeg.clone();
            next.slots[i] = Some(Slot {
                name: k.name.clone(),
                image,
                compile_error: k.compile_error,
            });
        }
        self.previews = Some(next);
    }

    /// スロット 1〜9 の中身。
    fn slots(&self) -> Vec<Option<Slot>> {
        if let Some(p) = &self.previews {
            return p.slots.clone();
        }
        let mut v: Vec<Option<Slot>> = vec![None; SLOTS];
        if let Some(s) = &self.state {
            for (i, name) in s.shaders.iter().take(SLOTS).enumerate() {
                v[i] = Some(Slot {
                    name: name.clone(),
                    image: None,
                    compile_error: false,
                });
            }
        }
        v
    }

    /// key が押された。押した後にすぐ state を取り直すべきなら true。
    pub fn press(&mut self, key: usize, now: Instant) -> bool {
        let Some(s) = &self.state else {
            // offline: 何も呼ばず、すぐ繋ぎ直しを試す
            return true;
        };
        let has_keys = s.has(FEATURE_KEYS);
        let result = match key {
            view::KEY_MODE => {
                self.mode = match self.mode {
                    Mode::Fade => Mode::Cut,
                    Mode::Cut => Mode::Fade,
                };
                return false;
            }
            view::KEY_TAP => {
                // 1 回目は今の BPM を送り直して拍頭を合わせる（kagelife の set_bpm は呼んだ時刻を拍頭にする）
                let bpm = match self.tapper.tap(now) {
                    TapResult::First => s.bpm,
                    TapResult::Measured(b) => b,
                };
                self.ctl.set_bpm(bpm)
            }
            view::KEY_STATUS => return true,
            // kagelife の 0 は Shift に関わらず即時切替なので shift は送らない
            view::KEY_BACK if has_keys => self.ctl.key("0", None),
            view::KEY_BEAT_DOWN if has_keys => self.ctl.key("[", None),
            view::KEY_BEAT_UP if has_keys => self.ctl.key("]", None),
            k => {
                let Some(i) = view::slot_of_key(k) else {
                    // 無効のキー（keys の無い kagelife の 0・[・]）
                    return false;
                };
                if self.use_keys() {
                    let digit = (i + 1).to_string();
                    self.ctl.key(&digit, Some(self.mode == Mode::Fade))
                } else {
                    let Some(slot) = self.slots().into_iter().nth(i).flatten() else {
                        return false;
                    };
                    match self.mode {
                        Mode::Fade => self.ctl.crossfade(&slot.name),
                        Mode::Cut => self.ctl.switch(&slot.name),
                    }
                }
            }
        };
        if let Err(e) = result {
            eprintln!("kagelife-deck: キー {key} の操作に失敗: {e}");
        }
        true
    }

    pub fn faces(&self) -> Vec<Face> {
        view::faces(
            self.state.as_ref(),
            &self.slots(),
            self.mode,
            self.has(FEATURE_KEYS),
        )
    }

    #[cfg(test)]
    pub fn online(&self) -> bool {
        self.state.is_some()
    }

    #[cfg(test)]
    pub(crate) fn ctl_mut(&mut self) -> &mut C {
        &mut self.ctl
    }
}

/// previews の key（"1"〜"9"）を 0 始まりのスロット番号にする。
fn digit_slot(k: &PreviewKey) -> Option<usize> {
    match k.key.as_bytes() {
        [d @ b'1'..=b'9'] => Some((d - b'1') as usize),
        _ => None,
    }
}

/// base64 の JPEG を 72×72 の RGB にする。読めなければ None（文字の表示に戻す）。
pub fn decode_preview(b64: &str) -> Option<Rc<[u8]>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
        .ok()?
        .to_rgb8();
    let s = SIZE as u32;
    let img = if img.dimensions() == (s, s) {
        img
    } else {
        image::imageops::resize(&img, s, s, image::imageops::FilterType::Triangle)
    };
    Some(img.into_raw().into())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::api::Previews;
    use crate::view::{KEY_BACK, KEY_BEAT_DOWN, KEY_BEAT_UP, KEY_MODE, KEY_STATUS, KEY_TAP, color};
    use std::time::Duration;

    /// 単色の 72×72 JPEG を base64 にする（テスト用）。
    pub fn jpeg_b64(rgb: [u8; 3]) -> String {
        let mut out = Vec::new();
        let px = rgb.repeat(72 * 72);
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode(&px, 72, 72, image::ExtendedColorType::Rgb8)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(out)
    }

    #[derive(Default)]
    struct Fake {
        state: Option<State>,
        previews: Option<Previews>,
        calls: Vec<String>,
        fail: Option<ApiError>,
    }

    impl Control for Fake {
        fn state(&mut self) -> Result<State, ApiError> {
            self.calls.push("state".into());
            self.state
                .clone()
                .ok_or_else(|| ApiError::Offline("down".into()))
        }
        fn switch(&mut self, name: &str) -> Result<(), ApiError> {
            self.calls.push(format!("switch {name}"));
            self.result()
        }
        fn crossfade(&mut self, name: &str) -> Result<(), ApiError> {
            self.calls.push(format!("crossfade {name}"));
            self.result()
        }
        fn set_bpm(&mut self, bpm: f64) -> Result<(), ApiError> {
            self.calls.push(format!("bpm {bpm}"));
            self.result()
        }
        fn previews(&mut self) -> Result<Previews, ApiError> {
            self.calls.push("previews".into());
            self.previews.clone().ok_or(ApiError::Status {
                code: 404,
                error: "not_found".into(),
                message: "".into(),
            })
        }
        fn key(&mut self, key: &str, shift: Option<bool>) -> Result<(), ApiError> {
            self.calls.push(match shift {
                Some(s) => format!("key {key} shift={s}"),
                None => format!("key {key}"),
            });
            self.result()
        }
    }

    impl Fake {
        fn result(&self) -> Result<(), ApiError> {
            match &self.fail {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
    }

    fn st(n: usize, features: &[&str]) -> State {
        State {
            active: "s01".into(),
            shaders: (1..=n).map(|i| format!("s{i:02}")).collect(),
            bpm: 120.0,
            features: features.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn app(state: State, previews: Option<Previews>) -> App<Fake> {
        let mut a = App::new(Fake {
            state: Some(state),
            previews,
            ..Default::default()
        });
        a.poll();
        a.poll_previews();
        a.ctl_mut().calls.clear();
        a
    }

    fn pk(key: &str, name: &str, jpeg: Option<String>, err: bool) -> PreviewKey {
        PreviewKey {
            key: key.into(),
            name: name.into(),
            jpeg,
            compile_error: err,
        }
    }

    fn previews(frame: u64, keys: Vec<PreviewKey>) -> Option<Previews> {
        Some(Previews {
            size: 72,
            fps: 10.0,
            frame,
            keys,
        })
    }

    // --- previews・keys の無い古い kagelife ---

    #[test]
    fn 繋がらなければ_offline_で_繋がれば戻る() {
        let mut a = App::new(Fake::default());
        a.poll();
        assert!(!a.online());
        assert_eq!(a.faces()[0].lines, vec!["offline"]);
        a.ctl_mut().state = Some(State::default());
        a.poll();
        assert!(a.online());
        assert_eq!(a.faces()[KEY_STATUS].bg, color::STATUS_OK);
    }

    #[test]
    fn 古い_kagelife_は先頭9個を1から9とみなし_crossfade() {
        let mut a = app(st(12, &[]), None);
        assert!(!a.wants_previews());
        // idx 5 = スロット 4
        assert!(a.press(5, Instant::now()));
        // idx 12 = スロット 9（s10 以降は並ばない）
        a.press(12, Instant::now());
        assert_eq!(a.ctl_mut().calls, vec!["crossfade s04", "crossfade s09"]);
        let f = a.faces();
        assert_eq!(f[12].lines, vec!["s09"]);
        assert_eq!(f[12].badge.as_deref(), Some("9"));
    }

    #[test]
    fn 古い_kagelife_の_cut_は_switch() {
        let mut a = app(st(3, &[]), None);
        assert!(!a.press(KEY_MODE, Instant::now()));
        assert_eq!(a.faces()[KEY_MODE].lines, vec!["CUT"]);
        a.press(2, Instant::now());
        assert_eq!(a.ctl_mut().calls, vec!["switch s03"]);
        a.press(KEY_MODE, Instant::now());
        assert_eq!(a.faces()[KEY_MODE].lines[0], "FADE");
    }

    #[test]
    fn 古い_kagelife_では_0_と括弧は無効() {
        let mut a = app(st(3, &[]), None);
        for k in [KEY_BACK, KEY_BEAT_DOWN, KEY_BEAT_UP] {
            assert!(!a.press(k, Instant::now()));
            assert_eq!(a.faces()[k].bg, color::DISABLED);
        }
        assert!(a.ctl_mut().calls.is_empty());
    }

    #[test]
    fn 空のスロットは何も呼ばない() {
        let mut a = app(st(3, &[]), None);
        assert!(!a.press(5, Instant::now()));
        assert!(a.ctl_mut().calls.is_empty());
    }

    // --- previews・keys のある kagelife ---

    #[test]
    fn previews_の割り当てとプレビューを使う() {
        let p = previews(
            5,
            vec![
                pk("1", "01_uv", Some(jpeg_b64([200, 0, 0])), false),
                pk("4", "zz_llm", None, false),
                pk("9", "bad", None, true),
            ],
        );
        let a = app(st(3, &["keys", "previews"]), p);
        assert!(a.wants_previews());
        let f = a.faces();
        let img = f[0].image.as_ref().expect("プレビューが無い");
        assert_eq!(img.len(), 72 * 72 * 3);
        assert!(img[0] > 150 && img[1] < 60, "色が違う: {:?}", &img[..3]);
        // まだ描いていないスロットは名前
        assert_eq!(f[5].lines, vec!["zz_llm"]);
        assert!(f[5].image.is_none());
        // コンパイル失敗
        assert_eq!(f[12].frame, Some(color::ERROR));
        // shaders の 2 番目（s02）は previews に無いので出ない
        assert!(f[1].lines.is_empty() && f[1].image.is_none());
    }

    #[test]
    fn スロットのキーは_keys_で押す_fade_は_shift() {
        let p = previews(1, vec![pk("1", "a", None, false)]);
        let mut a = app(st(1, &["keys", "previews"]), p);
        a.press(0, Instant::now());
        a.press(KEY_MODE, Instant::now());
        a.press(6, Instant::now()); // スロット 5（割り当てが無くても kagelife に任せる）
        a.press(KEY_BACK, Instant::now());
        a.press(KEY_BEAT_DOWN, Instant::now());
        a.press(KEY_BEAT_UP, Instant::now());
        assert_eq!(
            a.ctl_mut().calls,
            vec![
                "key 1 shift=true",
                "key 5 shift=false",
                "key 0",
                "key [",
                "key ]"
            ]
        );
    }

    #[test]
    fn keys_の_404_や_400_でも落ちず_繋がったまま() {
        let p = previews(1, vec![]);
        let mut a = app(st(1, &["keys", "previews"]), p);
        a.ctl_mut().fail = Some(ApiError::Status {
            code: 404,
            error: "not_found".into(),
            message: "".into(),
        });
        assert!(a.press(10, Instant::now()));
        a.ctl_mut().fail = Some(ApiError::Status {
            code: 400,
            error: "invalid_key".into(),
            message: "".into(),
        });
        assert!(a.press(KEY_BEAT_UP, Instant::now()));
        assert!(a.online());
    }

    #[test]
    fn frame_が同じなら作り直さず_変われば名前も反映する() {
        let p = previews(7, vec![pk("1", "old", None, false)]);
        let mut a = app(st(1, &["keys", "previews"]), p);
        assert_eq!(a.faces()[0].lines, vec!["old"]);
        // frame が同じまま名前だけ違う応答は無視する
        a.ctl_mut().previews = previews(7, vec![pk("1", "new", None, false)]);
        a.poll_previews();
        assert_eq!(a.faces()[0].lines, vec!["old"]);
        a.ctl_mut().previews = previews(8, vec![pk("1", "new", None, false)]);
        a.poll_previews();
        assert_eq!(a.faces()[0].lines, vec!["new"]);
    }

    #[test]
    fn 同じ_jpeg_は復号し直さない() {
        let j = jpeg_b64([0, 0, 200]);
        let mut a = app(
            st(1, &["previews"]),
            previews(1, vec![pk("1", "a", Some(j.clone()), false)]),
        );
        let first = a.faces()[0].image.clone().unwrap();
        a.ctl_mut().previews = previews(2, vec![pk("1", "a", Some(j), false)]);
        a.poll_previews();
        let second = a.faces()[0].image.clone().unwrap();
        assert!(Rc::ptr_eq(&first, &second));
    }

    #[test]
    fn previews_だけで_keys_が無ければ名前で_crossfade() {
        let p = previews(1, vec![pk("2", "llm_a", None, false)]);
        let mut a = app(st(3, &["previews"]), p);
        a.press(1, Instant::now());
        assert_eq!(a.ctl_mut().calls, vec!["crossfade llm_a"]);
        assert_eq!(a.faces()[KEY_BACK].bg, color::DISABLED);
    }

    #[test]
    fn previews_が取れなければ文字の表示に戻る() {
        let mut a = app(
            st(2, &["keys", "previews"]),
            previews(1, vec![pk("1", "x", None, false)]),
        );
        a.ctl_mut().previews = None; // 404
        a.poll_previews();
        assert_eq!(a.faces()[0].lines, vec!["s01"]);
    }

    #[test]
    fn previews_が無い_kagelife_には取りに行かない() {
        let mut a = app(st(2, &[]), None);
        a.poll_previews();
        assert!(a.ctl_mut().calls.is_empty());
    }

    #[test]
    fn 読めない_jpeg_は名前の表示() {
        let mut a = app(
            st(1, &["previews"]),
            previews(1, vec![pk("1", "a", Some("@@@".into()), false)]),
        );
        a.poll_previews();
        assert_eq!(a.faces()[0].lines, vec!["a"]);
    }

    // --- 共通 ---

    #[test]
    fn 一回目のタップは今の_bpm_で拍頭を合わせ_二回目で測った値を送る() {
        let mut a = app(st(1, &[]), None);
        let t = Instant::now();
        assert!(a.press(KEY_TAP, t));
        assert!(a.press(KEY_TAP, t + Duration::from_millis(500)));
        assert_eq!(a.ctl_mut().calls, vec!["bpm 120", "bpm 120"]);
        a.press(KEY_TAP, t + Duration::from_millis(1100));
        // 1100ms に 2 間隔 → 109.09...
        assert!(
            a.ctl_mut().calls[2].starts_with("bpm 109.0"),
            "{:?}",
            a.ctl_mut().calls
        );
    }

    #[test]
    fn offline_では押しても呼ばず_取り直しだけ() {
        let mut a = App::new(Fake::default());
        a.poll();
        a.ctl_mut().calls.clear();
        assert!(a.press(0, Instant::now()));
        assert!(a.press(KEY_TAP, Instant::now()));
        assert!(a.ctl_mut().calls.is_empty());
    }
}
