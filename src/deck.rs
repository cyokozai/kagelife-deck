//! Stream Deck を trait の後ろに隠す。差分送信と押下の検出はここで行い、実機の部分は hw.rs。

use crate::view::Face;
use std::time::Duration;

/// 1 台の Stream Deck。
pub trait Deck {
    /// key に 72×72 の RGB（行優先、上が先）を送る（flush まで反映されない）。
    fn set_image(&mut self, key: u8, rgb: &[u8]) -> Result<(), String>;
    fn flush(&mut self) -> Result<(), String>;
    /// 最大 timeout 待ってキーの状態（押されていれば true）を返す。何も来なければ None。
    fn read_buttons(&mut self, timeout: Duration) -> Result<Option<Vec<bool>>, String>;
}

/// 前回送った描画内容を覚え、変わったキーだけを返す。
#[derive(Default)]
pub struct KeyCache {
    last: Vec<Option<Face>>,
}

impl KeyCache {
    /// 次はすべて送り直す（デバイスを繋ぎ直したとき）。
    pub fn invalidate(&mut self) {
        self.last.clear();
    }

    /// faces のうち前回と違うキーの番号を返し、覚えている内容を faces にする。
    pub fn changed(&mut self, faces: &[Face]) -> Vec<usize> {
        self.last.resize(faces.len(), None);
        let mut keys = Vec::new();
        for (i, f) in faces.iter().enumerate() {
            if self.last[i].as_ref() != Some(f) {
                self.last[i] = Some(f.clone());
                keys.push(i);
            }
        }
        keys
    }
}

/// 前回の状態 prev と今回の cur から、新しく押されたキーを返す。prev は cur に更新する。
pub fn pressed_edges(prev: &mut Vec<bool>, cur: &[bool]) -> Vec<usize> {
    let edges = cur
        .iter()
        .enumerate()
        .filter(|&(i, &down)| down && !prev.get(i).copied().unwrap_or(false))
        .map(|(i, _)| i)
        .collect();
    prev.clear();
    prev.extend_from_slice(cur);
    edges
}

/// 変わったキーだけを描いて送り、flush する。送ったキーの数を返す。
pub fn sync_keys(
    deck: &mut dyn Deck,
    cache: &mut KeyCache,
    faces: &[Face],
    render: impl Fn(&Face) -> Vec<u8>,
) -> Result<usize, String> {
    let keys = cache.changed(faces);
    if keys.is_empty() {
        return Ok(0);
    }
    let sent = (|| {
        for &k in &keys {
            deck.set_image(k as u8, &render(&faces[k]))?;
        }
        deck.flush()
    })();
    if let Err(e) = sent {
        // どこまで届いたか分からないので、次は全部送り直す
        cache.invalidate();
        return Err(e);
    }
    Ok(keys.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Rgb, color};

    fn face(t: &str) -> Face {
        Face {
            bg: color::SHADER,
            fg: color::WHITE,
            frame: None,
            lines: vec![t.into()],
        }
    }

    #[derive(Default)]
    struct FakeDeck {
        sent: Vec<u8>,
        flushes: usize,
        fail: bool,
    }

    impl Deck for FakeDeck {
        fn set_image(&mut self, key: u8, rgb: &[u8]) -> Result<(), String> {
            if self.fail {
                return Err("unplugged".into());
            }
            assert_eq!(rgb.len(), 3);
            self.sent.push(key);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), String> {
            self.flushes += 1;
            Ok(())
        }
        fn read_buttons(&mut self, _: Duration) -> Result<Option<Vec<bool>>, String> {
            Ok(None)
        }
    }

    fn tiny(f: &Face) -> Vec<u8> {
        let Rgb(r, g, b) = f.bg;
        vec![r, g, b]
    }

    #[test]
    fn 初回は全部_同じなら送らない_変わったキーだけ() {
        let mut c = KeyCache::default();
        let mut v: Vec<Face> = (0..3).map(|i| face(&i.to_string())).collect();
        assert_eq!(c.changed(&v), vec![0, 1, 2]);
        assert_eq!(c.changed(&v), Vec::<usize>::new());
        v[1].bg = color::ACTIVE;
        assert_eq!(c.changed(&v), vec![1]);
        v[2].lines.push("ERR".into());
        v[0].frame = Some(color::ERROR);
        assert_eq!(c.changed(&v), vec![0, 2]);
    }

    #[test]
    fn 無効にすると全部送り直す() {
        let mut c = KeyCache::default();
        let v = vec![face("a"), face("b")];
        c.changed(&v);
        c.invalidate();
        assert_eq!(c.changed(&v), vec![0, 1]);
    }

    #[test]
    fn 押下の立ち上がりだけを拾う() {
        let mut prev = vec![];
        assert_eq!(pressed_edges(&mut prev, &[false, true, false]), vec![1]);
        // 押しっぱなしは拾わない。離して押し直したら拾う
        assert_eq!(pressed_edges(&mut prev, &[false, true, true]), vec![2]);
        assert_eq!(pressed_edges(&mut prev, &[false, false, true]), Vec::<usize>::new());
        assert_eq!(pressed_edges(&mut prev, &[false, true, true]), vec![1]);
    }

    #[test]
    fn 同期は変わったキーだけ送って_flush() {
        let mut d = FakeDeck::default();
        let mut c = KeyCache::default();
        let mut v = vec![face("a"), face("b"), face("c")];
        assert_eq!(sync_keys(&mut d, &mut c, &v, tiny).unwrap(), 3);
        assert_eq!(sync_keys(&mut d, &mut c, &v, tiny).unwrap(), 0);
        v[2].bg = color::TARGET;
        assert_eq!(sync_keys(&mut d, &mut c, &v, tiny).unwrap(), 1);
        assert_eq!(d.sent, vec![0, 1, 2, 2]);
        // 何も送らないときは flush もしない
        assert_eq!(d.flushes, 2);
    }

    #[test]
    fn 送れなかったら次は全部送り直す() {
        let mut d = FakeDeck {
            fail: true,
            ..Default::default()
        };
        let mut c = KeyCache::default();
        let v = vec![face("a"), face("b")];
        assert!(sync_keys(&mut d, &mut c, &v, tiny).is_err());
        d.fail = false;
        assert_eq!(sync_keys(&mut d, &mut c, &v, tiny).unwrap(), 2);
    }
}
