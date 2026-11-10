//! 押されたキーを制御口の呼び出しに変え、表示する状態を持つ。デバイスとは独立。

use crate::api::{ApiError, Control, State};
use crate::tap::{TapResult, Tapper};
use crate::view::{self, Face, Mode};
use std::time::Instant;

pub struct App<C: Control> {
    ctl: C,
    state: Option<State>,
    page: usize,
    mode: Mode,
    tapper: Tapper,
}

impl<C: Control> App<C> {
    pub fn new(ctl: C) -> Self {
        Self {
            ctl,
            state: None,
            page: 0,
            mode: Mode::Fade,
            tapper: Tapper::kagelife_default(),
        }
    }

    /// GET /v1/state を取り直す。失敗したら offline の表示にする。
    pub fn poll(&mut self) {
        match self.ctl.state() {
            Ok(s) => {
                // シェーダが減ってページが無くなったら収める
                let pc = view::page_count(s.shaders.len());
                if self.page >= pc {
                    self.page = pc - 1;
                }
                if self.state.is_none() {
                    eprintln!("kagelife-deck: kagelife に繋がった");
                }
                self.state = Some(s);
            }
            Err(e) => {
                if self.state.is_some() {
                    eprintln!("kagelife-deck: kagelife から切れた: {e}");
                }
                self.state = None;
            }
        }
    }

    /// key が押された。押した後にすぐ state を取り直すべきなら true。
    pub fn press(&mut self, key: usize, now: Instant) -> bool {
        let Some(s) = &self.state else {
            // offline: 何も呼ばず、すぐ繋ぎ直しを試す
            return true;
        };
        let n = s.shaders.len();
        let result = match key {
            view::KEY_PREV => {
                self.page = view::step_page(self.page, -1, n);
                return false;
            }
            view::KEY_NEXT => {
                self.page = view::step_page(self.page, 1, n);
                return false;
            }
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
            k => {
                let Some(name) = view::shader_at(s, self.page, k) else {
                    return false;
                };
                let name = name.to_string();
                match self.mode {
                    Mode::Fade => self.ctl.crossfade(&name),
                    Mode::Cut => self.ctl.switch(&name),
                }
            }
        };
        if let Err(e) = result {
            eprintln!("kagelife-deck: キー {key} の操作に失敗: {e}");
        }
        true
    }

    pub fn faces(&self) -> Vec<Face> {
        view::faces(self.state.as_ref(), self.page, self.mode)
    }

    pub fn online(&self) -> bool {
        self.state.is_some()
    }

    #[cfg(test)]
    pub(crate) fn ctl_mut(&mut self) -> &mut C {
        &mut self.ctl
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{KEY_MODE, KEY_NEXT, KEY_PREV, KEY_STATUS, KEY_TAP, color};
    use std::time::Duration;

    #[derive(Default)]
    struct Fake {
        state: Option<State>,
        calls: Vec<String>,
        fail_actions: bool,
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
            self.action_result()
        }
        fn crossfade(&mut self, name: &str) -> Result<(), ApiError> {
            self.calls.push(format!("crossfade {name}"));
            self.action_result()
        }
        fn set_bpm(&mut self, bpm: f64) -> Result<(), ApiError> {
            self.calls.push(format!("bpm {bpm}"));
            self.action_result()
        }
    }

    impl Fake {
        fn action_result(&self) -> Result<(), ApiError> {
            if self.fail_actions {
                Err(ApiError::Status {
                    code: 404,
                    error: "not_found".into(),
                    message: "".into(),
                })
            } else {
                Ok(())
            }
        }
    }

    fn online(n: usize) -> App<Fake> {
        let st = State {
            active: "s00".into(),
            shaders: (0..n).map(|i| format!("s{i:02}")).collect(),
            bpm: 120.0,
            ..Default::default()
        };
        let mut a = App::new(Fake {
            state: Some(st),
            ..Default::default()
        });
        a.poll();
        a.ctl_mut().calls.clear();
        a
    }

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
    fn fade_ではシェーダのキーで_crossfade() {
        let mut a = online(3);
        assert!(a.press(1, Instant::now()));
        assert_eq!(a.ctl_mut().calls, vec!["crossfade s01"]);
    }

    #[test]
    fn cut_ではシェーダのキーで_switch() {
        let mut a = online(3);
        assert!(!a.press(KEY_MODE, Instant::now()));
        assert_eq!(a.faces()[KEY_MODE].lines, vec!["CUT"]);
        a.press(2, Instant::now());
        assert_eq!(a.ctl_mut().calls, vec!["switch s02"]);
        a.press(KEY_MODE, Instant::now());
        assert_eq!(a.faces()[KEY_MODE].lines[0], "FADE");
    }

    #[test]
    fn 空のキーは何も呼ばない() {
        let mut a = online(3);
        assert!(!a.press(5, Instant::now()));
        assert!(a.ctl_mut().calls.is_empty());
    }

    #[test]
    fn ページを送ってから押すと次のページのシェーダ() {
        let mut a = online(12);
        a.press(KEY_NEXT, Instant::now());
        a.press(1, Instant::now());
        assert_eq!(a.ctl_mut().calls, vec!["crossfade s11"]);
        // 2 ページしかないので、次で 1 ページ目に回り、前で 2 ページ目に回る
        a.press(KEY_NEXT, Instant::now());
        a.press(KEY_PREV, Instant::now());
        a.press(0, Instant::now());
        assert_eq!(a.ctl_mut().calls.last().unwrap(), "crossfade s10");
    }

    #[test]
    fn シェーダが減ったらページを収める() {
        let mut a = online(25);
        a.press(KEY_PREV, Instant::now()); // 3 ページ目へ回る
        assert_eq!(a.faces()[KEY_PREV].lines[2], "3/3");
        a.ctl_mut().state.as_mut().unwrap().shaders.truncate(5);
        a.poll();
        assert_eq!(a.faces()[KEY_PREV].lines[2], "1/1");
    }

    #[test]
    fn 一回目のタップは今の_bpm_で拍頭を合わせ_二回目で測った値を送る() {
        let mut a = online(1);
        let t = Instant::now();
        assert!(a.press(KEY_TAP, t));
        assert!(a.press(KEY_TAP, t + Duration::from_millis(500)));
        assert_eq!(a.ctl_mut().calls, vec!["bpm 120", "bpm 120"]);
        a.press(KEY_TAP, t + Duration::from_millis(1100));
        // 1100ms に 2 間隔 → 109.09...
        assert!(a.ctl_mut().calls[2].starts_with("bpm 109.0"), "{:?}", a.ctl_mut().calls);
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

    #[test]
    fn 操作が失敗しても落ちない() {
        let mut a = online(2);
        a.ctl_mut().fail_actions = true;
        assert!(a.press(0, Instant::now()));
        assert!(a.online());
    }
}
