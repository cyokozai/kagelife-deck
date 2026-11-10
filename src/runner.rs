//! 主ループ。スレッドは 1 本で、待ちはデバイスの読み出しの timeout か sleep に任せる（ビジーループにしない）。
//!
//! 1 巡でやること:
//! 1. デバイスが無く、探す時刻になっていれば探す
//! 2. state を取り直す時刻になっていれば取り直す（kagelife が previews を持てば、プレビューも 10 Hz で取る）
//! 3. 描画内容が変わったキーだけを送る
//! 4. 次の時刻までデバイスの入力を待つ（デバイスが無ければ sleep）。押されたら処理して、すぐ state を取り直す

use crate::api::Control;
use crate::app::App;
use crate::deck::{Deck, KeyCache, pressed_edges, sync_keys};
use crate::render::render;
use std::time::{Duration, Instant};

/// デバイスを開く。見つからなければ None。
/// プレビューを取る間隔（kagelife の描く上限と同じ 10 fps）。
pub const PREVIEW_EVERY: Duration = Duration::from_millis(100);

pub trait Opener {
    fn open(&mut self) -> Option<Box<dyn Deck>>;
}

pub struct Runner<C: Control, O: Opener> {
    pub app: App<C>,
    opener: O,
    deck: Option<Box<dyn Deck>>,
    cache: KeyCache,
    buttons: Vec<bool>,
    poll_every: Duration,
    scan_every: Duration,
    preview_every: Duration,
    next_poll: Instant,
    next_scan: Instant,
    next_preview: Instant,
}

impl<C: Control, O: Opener> Runner<C, O> {
    pub fn new(app: App<C>, opener: O, poll_every: Duration, scan_every: Duration) -> Self {
        let now = Instant::now();
        Self {
            app,
            opener,
            deck: None,
            cache: KeyCache::default(),
            buttons: Vec::new(),
            poll_every,
            scan_every,
            preview_every: PREVIEW_EVERY,
            next_poll: now,
            next_scan: now,
            next_preview: now,
        }
    }

    #[cfg(test)]
    pub fn has_deck(&self) -> bool {
        self.deck.is_some()
    }

    fn drop_deck(&mut self, why: &str) {
        eprintln!("kagelife-deck: Stream Deck を切り離した: {why}");
        self.deck = None;
        self.next_scan = Instant::now();
    }

    /// 1 巡する（待ちを含む）。
    pub fn turn(&mut self) {
        let now = Instant::now();
        if self.deck.is_none() && now >= self.next_scan {
            self.next_scan = now + self.scan_every;
            if let Some(d) = self.opener.open() {
                self.deck = Some(d);
                self.cache.invalidate();
                self.buttons.clear();
            }
        }
        if now >= self.next_poll {
            self.app.poll();
            self.next_poll = Instant::now() + self.poll_every;
        }
        let previews = self.app.wants_previews();
        if previews && Instant::now() >= self.next_preview {
            self.app.poll_previews();
            self.next_preview = Instant::now() + self.preview_every;
        }
        if let Some(d) = self.deck.as_mut()
            && let Err(e) = sync_keys(d.as_mut(), &mut self.cache, &self.app.faces(), render)
        {
            self.drop_deck(&e);
        }

        let mut deadline = self.next_poll;
        if previews {
            deadline = deadline.min(self.next_preview);
        }
        if self.deck.is_none() {
            deadline = deadline.min(self.next_scan);
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        let Some(d) = self.deck.as_mut() else {
            std::thread::sleep(wait);
            return;
        };
        match d.read_buttons(wait) {
            Ok(Some(state)) => {
                for k in pressed_edges(&mut self.buttons, &state) {
                    if self.app.press(k, Instant::now()) {
                        self.next_poll = Instant::now();
                        self.next_preview = Instant::now();
                    }
                }
            }
            Ok(None) => {}
            Err(e) => self.drop_deck(&e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiError, PreviewKey, Previews, State};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    #[derive(Default)]
    struct Log {
        calls: Vec<String>,
        sent: Vec<u8>,
        opens: usize,
        frame: u64,
    }
    type Shared = Rc<RefCell<Log>>;

    struct FakeCtl {
        log: Shared,
        state: Option<State>,
    }
    impl Control for FakeCtl {
        fn state(&mut self) -> Result<State, ApiError> {
            self.log.borrow_mut().calls.push("state".into());
            self.state.clone().ok_or(ApiError::Offline("x".into()))
        }
        fn switch(&mut self, n: &str) -> Result<(), ApiError> {
            self.log.borrow_mut().calls.push(format!("switch {n}"));
            Ok(())
        }
        fn crossfade(&mut self, n: &str) -> Result<(), ApiError> {
            self.log.borrow_mut().calls.push(format!("crossfade {n}"));
            Ok(())
        }
        fn set_bpm(&mut self, b: f64) -> Result<(), ApiError> {
            self.log.borrow_mut().calls.push(format!("bpm {b}"));
            Ok(())
        }
        fn previews(&mut self) -> Result<Previews, ApiError> {
            let mut l = self.log.borrow_mut();
            l.calls.push("previews".into());
            l.frame += 1;
            Ok(Previews {
                frame: l.frame,
                keys: vec![PreviewKey {
                    key: "1".into(),
                    name: "a".into(),
                    jpeg: Some(crate::app::tests::jpeg_b64([0, 0, (l.frame * 40) as u8])),
                    compile_error: false,
                }],
                ..Default::default()
            })
        }
        fn key(&mut self, k: &str, shift: Option<bool>) -> Result<(), ApiError> {
            self.log
                .borrow_mut()
                .calls
                .push(format!("key {k} {shift:?}"));
            Ok(())
        }
    }

    /// 読み出しのたびに inputs を 1 つずつ返す。Err("gone") で抜かれたことにする。
    struct FakeDeck {
        log: Shared,
        inputs: VecDeque<Result<Option<Vec<bool>>, String>>,
    }
    impl Deck for FakeDeck {
        fn set_image(&mut self, key: u8, rgb: &[u8]) -> Result<(), String> {
            assert_eq!(rgb.len(), 72 * 72 * 3);
            self.log.borrow_mut().sent.push(key);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn read_buttons(&mut self, _: Duration) -> Result<Option<Vec<bool>>, String> {
            self.inputs.pop_front().unwrap_or(Ok(None))
        }
    }

    /// open のたびに decks を 1 台ずつ渡す（無ければ見つからない）。
    struct FakeOpener {
        log: Shared,
        decks: VecDeque<FakeDeck>,
    }
    impl Opener for FakeOpener {
        fn open(&mut self) -> Option<Box<dyn Deck>> {
            self.log.borrow_mut().opens += 1;
            self.decks.pop_front().map(|d| Box::new(d) as Box<dyn Deck>)
        }
    }

    fn keys(down: &[usize]) -> Vec<bool> {
        (0..15).map(|i| down.contains(&i)).collect()
    }

    fn setup(
        decks: Vec<Vec<Result<Option<Vec<bool>>, String>>>,
        state: Option<State>,
    ) -> (Runner<FakeCtl, FakeOpener>, Shared) {
        let log: Shared = Rc::default();
        let decks = decks
            .into_iter()
            .map(|inputs| FakeDeck {
                log: log.clone(),
                inputs: inputs.into(),
            })
            .collect();
        let app = App::new(FakeCtl {
            log: log.clone(),
            state,
        });
        let opener = FakeOpener {
            log: log.clone(),
            decks,
        };
        // 取り直しは 1 時間に 1 回（テストの中では押下の直後だけ取り直す）。探すのは毎巡
        let r = Runner::new(app, opener, Duration::from_secs(3600), Duration::ZERO);
        (r, log)
    }

    fn online() -> Option<State> {
        Some(State {
            active: "a".into(),
            shaders: vec!["a".into(), "b".into()],
            bpm: 120.0,
            ..Default::default()
        })
    }

    #[test]
    fn 繋がったら全キーを送り_変わらなければ送らない() {
        let (mut r, log) = setup(vec![vec![]], online());
        r.turn();
        assert!(r.has_deck());
        assert_eq!(log.borrow().sent.len(), 15);
        assert_eq!(log.borrow().calls, vec!["state"]);
        r.turn();
        assert_eq!(log.borrow().sent.len(), 15);
        // 取り直しの時刻はまだ来ていない
        assert_eq!(log.borrow().calls, vec!["state"]);
    }

    #[test]
    fn 押したら操作して_すぐ取り直す() {
        let (mut r, log) = setup(
            vec![vec![Ok(Some(keys(&[1]))), Ok(Some(keys(&[])))]],
            online(),
        );
        r.turn(); // 開く・取る・送る・押下を読む → crossfade b
        assert_eq!(log.borrow().calls, vec!["state", "crossfade b"]);
        r.turn(); // 押下の直後なので取り直す
        assert_eq!(log.borrow().calls, vec!["state", "crossfade b", "state"]);
    }

    #[test]
    fn 方式の切替は描き直すだけ() {
        let (mut r, log) = setup(vec![vec![Ok(Some(keys(&[8])))]], online());
        r.turn();
        r.turn();
        // 方式のキー（idx 8）だけ送り直し、制御口は呼ばない
        assert_eq!(&log.borrow().sent[15..], &[8]);
        assert_eq!(log.borrow().calls, vec!["state"]);
    }

    #[test]
    fn 抜かれたら探し直し_挿し直したら全キーを送り直す() {
        let (mut r, log) = setup(vec![vec![Err("gone".into())], vec![]], online());
        r.turn();
        assert!(!r.has_deck());
        assert_eq!(log.borrow().opens, 1);
        r.turn();
        assert!(r.has_deck());
        assert_eq!(log.borrow().opens, 2);
        assert_eq!(log.borrow().sent.len(), 30);
    }

    #[test]
    fn デバイスが無くても回る() {
        let (mut r, log) = setup(vec![], None);
        r.turn();
        r.turn();
        assert!(!r.has_deck());
        assert_eq!(log.borrow().opens, 2);
        assert!(log.borrow().sent.is_empty());
    }

    #[test]
    fn offline_の表示から繋がると描き直す() {
        let (mut r, log) = setup(vec![vec![Ok(Some(keys(&[0])))]], None);
        r.turn(); // offline で 15 キー。押下 → 取り直しの予約
        assert_eq!(log.borrow().sent.len(), 15);
        r.app_ctl_set_state(online());
        r.turn();
        // シェーダ 2 個・空き 8 個・下段 5 個。offline から変わったキーはすべて送る
        assert_eq!(log.borrow().sent.len(), 30);
    }

    #[test]
    fn previews_があれば取りに行き_変わったスロットだけ送る() {
        let mut s = online().unwrap();
        s.features = vec!["previews".into(), "keys".into()];
        let (mut r, log) = setup(vec![vec![]], Some(s));
        r.preview_every = Duration::ZERO;
        r.turn();
        assert_eq!(log.borrow().calls, vec!["state", "previews"]);
        assert_eq!(log.borrow().sent.len(), 15);
        r.turn();
        // frame が進み、スロット 1 の色が変わった → idx 0 だけ送る
        assert_eq!(log.borrow().calls, vec!["state", "previews", "previews"]);
        assert_eq!(&log.borrow().sent[15..], &[0]);
    }

    #[test]
    fn previews_が無ければ取りに行かない() {
        let (mut r, log) = setup(vec![vec![]], online());
        r.preview_every = Duration::ZERO;
        r.turn();
        r.turn();
        assert_eq!(log.borrow().calls, vec!["state"]);
    }

    impl Runner<FakeCtl, FakeOpener> {
        fn app_ctl_set_state(&mut self, s: Option<State>) {
            self.app.ctl_mut().state = s;
        }
    }
}
