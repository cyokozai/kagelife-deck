//! タップ BPM。kagelife の internal/tempo/tapper.go（Tap と clampBPM）と同じ規則で求める。
//!
//! - timeout（kagelife は 2 秒）を超えて空いたタップは新しい測定の 1 回目
//! - 直近 window 個（kagelife は 8）のタップの最初と最後の間隔を、間隔の数で割って BPM にする
//! - 結果は 40〜300 に丸める。間隔が 0 以下なら 300

use std::time::{Duration, Instant};

/// BPM の下限（kagelife の tempo.MinBPM）。
pub const MIN_BPM: f64 = 40.0;
/// BPM の上限（kagelife の tempo.MaxBPM）。
pub const MAX_BPM: f64 = 300.0;

/// タップを受けたときに kagelife へ何を送るか。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TapResult {
    /// 測定の 1 回目。BPM はまだ決まらない（拍頭の合わせ直しだけ）
    First,
    /// 2 回目以降。この BPM を送る
    Measured(f64),
}

/// タップ時刻をスライド窓で持つ。
pub struct Tapper {
    timeout: Duration,
    window: usize,
    taps: Vec<Instant>,
}

impl Tapper {
    /// kagelife の main.go と同じ設定（2 秒・8 回）。
    pub fn kagelife_default() -> Self {
        Self::new(Duration::from_secs(2), 8)
    }

    /// window が 2 未満なら 2 に切り上げる（tempo.New と同じ）。
    pub fn new(timeout: Duration, window: usize) -> Self {
        let window = window.max(2);
        Self {
            timeout,
            window,
            taps: Vec::with_capacity(window),
        }
    }

    /// 時刻 t のタップを記録し、送るべき内容を返す。
    pub fn tap(&mut self, t: Instant) -> TapResult {
        if let Some(last) = self.taps.last()
            && t.saturating_duration_since(*last) > self.timeout
        {
            self.taps.clear();
        }
        self.taps.push(t);
        if self.taps.len() > self.window {
            let drop = self.taps.len() - self.window;
            self.taps.drain(..drop);
        }

        let n = self.taps.len();
        if n < 2 {
            return TapResult::First;
        }
        let span = self.taps[n - 1]
            .saturating_duration_since(self.taps[0])
            .as_secs_f64();
        TapResult::Measured(clamp_bpm(span, n - 1))
    }
}

/// span 秒に intervals 個の間隔があるときの BPM を 40〜300 に丸める（clampBPM と同じ）。
fn clamp_bpm(span: f64, intervals: usize) -> f64 {
    if span <= 0.0 {
        return MAX_BPM;
    }
    (60.0 / (span / intervals as f64)).clamp(MIN_BPM, MAX_BPM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(base: Instant, v: u64) -> Instant {
        base + Duration::from_millis(v)
    }

    #[test]
    fn 一回目は_first() {
        let mut tp = Tapper::kagelife_default();
        assert_eq!(tp.tap(Instant::now()), TapResult::First);
    }

    #[test]
    fn 二回目で間隔の_bpm() {
        let b = Instant::now();
        let mut tp = Tapper::kagelife_default();
        tp.tap(b);
        assert_eq!(tp.tap(ms(b, 500)), TapResult::Measured(120.0));
    }

    #[test]
    fn 窓の中の平均で求める() {
        let b = Instant::now();
        let mut tp = Tapper::kagelife_default();
        // 間隔 400, 600 → 平均 500ms → 120
        tp.tap(b);
        tp.tap(ms(b, 400));
        assert_eq!(tp.tap(ms(b, 1000)), TapResult::Measured(120.0));
    }

    #[test]
    fn 窓は直近8回() {
        let b = Instant::now();
        let mut tp = Tapper::kagelife_default();
        // 最初の 2 回は 1000ms 間隔、その後 500ms 間隔で 8 回。窓に残るのは 500ms 間隔だけ
        tp.tap(b);
        tp.tap(ms(b, 1000));
        let mut last = TapResult::First;
        for i in 1..=8 {
            last = tp.tap(ms(b, 1000 + 500 * i));
        }
        assert_eq!(last, TapResult::Measured(120.0));
    }

    #[test]
    fn 二秒を超えて空くと測り直し() {
        let b = Instant::now();
        let mut tp = Tapper::kagelife_default();
        tp.tap(b);
        tp.tap(ms(b, 500));
        assert_eq!(tp.tap(ms(b, 2501)), TapResult::First);
        assert_eq!(tp.tap(ms(b, 3001)), TapResult::Measured(120.0));
    }

    #[test]
    fn ちょうど二秒は続きとみなす() {
        let b = Instant::now();
        let mut tp = Tapper::kagelife_default();
        tp.tap(b);
        // 2000ms は timeout を「超えて」いない → 30 BPM を 40 に丸める
        assert_eq!(tp.tap(ms(b, 2000)), TapResult::Measured(MIN_BPM));
    }

    #[test]
    fn 速すぎる二度押しは上限に丸める() {
        let b = Instant::now();
        let mut tp = Tapper::kagelife_default();
        tp.tap(b);
        assert_eq!(tp.tap(ms(b, 50)), TapResult::Measured(MAX_BPM));
        // 同時刻（間隔 0）も上限
        let mut tp = Tapper::kagelife_default();
        tp.tap(b);
        assert_eq!(tp.tap(b), TapResult::Measured(MAX_BPM));
    }
}
