//! ssh 越しのモード（`--ssh <host>`）。
//!
//! kagelife は別の機械（VJ 用の Linux 機）の 127.0.0.1 で待ち受ける。この機械からは
//! 1. `ssh <host> cat <path>` で相手の発見ファイルを読み
//! 2. `ssh -N -L 127.0.0.1:<空きポート>:<相手の addr> <host>` のトンネルを子プロセスで張り
//! 3. `127.0.0.1:<空きポート>` と相手のトークンで、今までどおり HTTP を送る。
//!
//! ssh の待ち（接続・認証・トンネルの準備）は裏のスレッドで行い、主ループ（Stream Deck のキーの読み出し）を止めない。
//! 主ループからの `locate` は、裏のスレッドが最後に作った結果を返すだけで、待たない。
//!
//! トークンはメモリの中だけで扱う。ログ・エラー文・ファイル・子プロセスの引数に出さない。

use crate::api::Locate;
use crate::discovery::{self, Discovery, DiscoveryError};
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, Instant};

/// `--remote-control-file` の既定。相手のシェルが `~` を展開する。
pub const DEFAULT_REMOTE_FILE: &str = "~/.cache/kagelife/control.json";
/// ssh の接続の待ち（秒）。`-o ConnectTimeout` に渡す。
pub const CONNECT_TIMEOUT_SECS: u32 = 5;
/// `ssh cat` 全体の待ちの上限（接続 5 秒 + 認証・読み出し）。超えたら子プロセスを止める。
pub const READ_LIMIT: Duration = Duration::from_secs(10);
/// トンネルのローカルのポートに繋がるまでの待ちの上限。
pub const READY_LIMIT: Duration = Duration::from_secs(10);
/// トンネルの準備を確かめる間隔。
pub const READY_STEP: Duration = Duration::from_millis(100);
/// 裏のスレッドがトンネルの子プロセスの生死を確かめる間隔。
pub const CHECK_EVERY: Duration = Duration::from_secs(1);
/// 張り直しの間隔の最小と最大。失敗が続く・何も変わらないと倍にしていく。
pub const BACKOFF_MIN: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// 相手の発見ファイルを読む ssh の引数（プログラム名を除く）。
pub fn cat_args(host: &str, remote_file: &str) -> Vec<String> {
    vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"),
        host.into(),
        "cat".into(),
        remote_file.into(),
    ]
}

/// トンネルを張る ssh の引数（プログラム名を除く）。
pub fn forward_args(host: &str, local_port: u16, remote_addr: &str) -> Vec<String> {
    vec![
        "-N".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "ServerAliveInterval=5".into(),
        "-o".into(),
        "ServerAliveCountMax=2".into(),
        "-L".into(),
        format!("127.0.0.1:{local_port}:{remote_addr}"),
        host.into(),
    ]
}

/// 相手の発見ファイルを解析する。エラー文に中身（トークン）を出さない。
pub fn parse_remote(bytes: &[u8]) -> Result<Discovery, String> {
    discovery::parse(bytes).map_err(|e| match e {
        // serde の文は値をそのまま出すことがあるので、中身を伝えない
        DiscoveryError::Parse(_) => "相手の発見ファイルを解析できない（JSON の形が違う）".into(),
        e => format!("相手の{e}"),
    })
}

/// トンネルの子プロセス。
pub trait TunnelProc: Send {
    /// まだ動いているか
    fn alive(&mut self) -> bool;
    /// 止める（止まるまで待つ）。何度呼んでもよい
    fn stop(&mut self);
}

/// ssh の実行。テストでは偽物に差し替える。
pub trait SshExec: Send {
    /// ssh を走らせて標準出力を返す（`cat` 用）。終了コードが 0 以外ならエラー
    fn read(&mut self, args: &[String]) -> Result<Vec<u8>, String>;
    /// ssh を子プロセスで走らせたままにする（トンネル用）
    fn spawn(&mut self, args: &[String]) -> Result<Box<dyn TunnelProc>, String>;
    /// 127.0.0.1 の空きポート
    fn free_port(&mut self) -> Result<u16, String>;
    /// 127.0.0.1:port に繋がるか（短い待ちで 1 回だけ試す）
    fn ready(&mut self, port: u16) -> bool;
}

struct Tunnel {
    proc: Box<dyn TunnelProc>,
    remote_addr: String,
    local_port: u16,
}

/// 発見ファイルの読み出しとトンネルの張り直しを受け持つ（待ちを含むので裏のスレッドで回す）。
pub struct Tunneler<E: SshExec> {
    exec: E,
    host: String,
    remote_file: String,
    tunnel: Option<Tunnel>,
    ready_limit: Duration,
    ready_step: Duration,
    log: Box<dyn FnMut(String) + Send>,
}

impl<E: SshExec> Tunneler<E> {
    pub fn new(exec: E, host: &str, remote_file: &str, log: Box<dyn FnMut(String) + Send>) -> Self {
        Self {
            exec,
            host: host.to_string(),
            remote_file: remote_file.to_string(),
            tunnel: None,
            ready_limit: READY_LIMIT,
            ready_step: READY_STEP,
            log,
        }
    }

    /// 相手の発見ファイルを読み、必要ならトンネルを張り直す。
    /// 返す Discovery の addr はローカルの `127.0.0.1:<port>`、token は相手のもの。
    pub fn refresh(&mut self) -> Result<Discovery, String> {
        let bytes = self
            .exec
            .read(&cat_args(&self.host, &self.remote_file))
            .map_err(|e| {
                format!(
                    "ssh {} で発見ファイル {} を読めない: {e}",
                    self.host, self.remote_file
                )
            })?;
        let remote = parse_remote(&bytes)?;
        if remote.addr.parse::<SocketAddr>().is_err() {
            return Err(format!(
                "相手の発見ファイルの addr が「IP:ポート」でない: {}",
                remote.addr
            ));
        }
        let reuse = match self.tunnel.as_mut() {
            Some(t) => t.remote_addr == remote.addr && t.proc.alive(),
            None => false,
        };
        if !reuse {
            self.stop();
            self.open(&remote.addr)?;
        }
        let port = self.tunnel.as_ref().map(|t| t.local_port).unwrap_or(0);
        Ok(Discovery {
            addr: format!("127.0.0.1:{port}"),
            ..remote
        })
    }

    /// トンネルを張り、ローカルのポートに繋がるまで待つ。
    fn open(&mut self, remote_addr: &str) -> Result<(), String> {
        let port = self.exec.free_port()?;
        let mut proc = self
            .exec
            .spawn(&forward_args(&self.host, port, remote_addr))
            .map_err(|e| format!("トンネルの ssh を起動できない: {e}"))?;
        let deadline = Instant::now() + self.ready_limit;
        loop {
            if !proc.alive() {
                proc.stop();
                return Err(format!(
                    "トンネルの ssh が終わった（{} へ 127.0.0.1:{port} → {remote_addr}）",
                    self.host
                ));
            }
            if self.exec.ready(port) {
                break;
            }
            if Instant::now() >= deadline {
                proc.stop();
                return Err(format!(
                    "トンネルが {:?} 以内に使えるようにならない（{}）",
                    self.ready_limit, self.host
                ));
            }
            std::thread::sleep(self.ready_step);
        }
        (self.log)(format!(
            "ssh {} のトンネルを張った: 127.0.0.1:{port} → {remote_addr}",
            self.host
        ));
        self.tunnel = Some(Tunnel {
            proc,
            remote_addr: remote_addr.to_string(),
            local_port: port,
        });
        Ok(())
    }

    /// トンネルの子プロセスが死んでいるか（張っていなければ false）。
    pub fn tunnel_dead(&mut self) -> bool {
        match self.tunnel.as_mut() {
            Some(t) => !t.proc.alive(),
            None => false,
        }
    }

    /// トンネルを止める。
    pub fn stop(&mut self) {
        if let Some(mut t) = self.tunnel.take() {
            t.proc.stop();
        }
    }
}

impl<E: SshExec> Drop for Tunneler<E> {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 張り直しの間隔。
#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    step: Duration,
    next: Option<Instant>,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max,
            step: min,
            next: None,
        }
    }

    /// now に試してよいか。
    pub fn ready(&self, now: Instant) -> bool {
        self.next.is_none_or(|n| now >= n)
    }

    /// 次に試せるまでの待ち。
    pub fn wait(&self, now: Instant) -> Duration {
        self.next
            .map_or(Duration::ZERO, |n| n.saturating_duration_since(now))
    }

    /// 試した後に呼ぶ。progressed は「結果が変わって前に進んだ」。前に進めば間隔を最小に戻し、
    /// 失敗や同じ結果が続けば倍にしていく。
    pub fn after(&mut self, now: Instant, progressed: bool) {
        if progressed {
            self.step = self.min;
            self.next = Some(now + self.min);
        } else {
            self.next = Some(now + self.step);
            self.step = (self.step * 2).min(self.max);
        }
    }
}

/// 裏のスレッドが作った最新の結果。
struct Published {
    current: Option<Discovery>,
    error: String,
}

/// ssh 越しの発見。主ループからは待たずに最新の結果を受け取る。
pub struct SshLocator {
    tx: Option<Sender<()>>,
    shared: Arc<Mutex<Published>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl SshLocator {
    /// 裏のスレッドを立てて、すぐに 1 回目の読み出しを始める。
    pub fn start<E: SshExec + 'static>(
        tunneler: Tunneler<E>,
        backoff: Backoff,
        check_every: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Published {
            current: None,
            error: "ssh で繋ぐ準備をしている".into(),
        }));
        let s2 = shared.clone();
        let worker = std::thread::Builder::new()
            .name("kagelife-deck-ssh".into())
            .spawn(move || worker(tunneler, backoff, rx, s2, check_every))
            .expect("ssh の裏のスレッドを立てられない");
        Self {
            tx: Some(tx),
            shared,
            worker: Some(worker),
        }
    }
}

impl Locate for SshLocator {
    /// 裏のスレッドに見つけ直しを頼み、待たずに最新の結果を返す。
    fn locate(&mut self) -> Result<Discovery, String> {
        if let Some(tx) = &self.tx {
            let _ = tx.send(());
        }
        let p = lock(&self.shared);
        p.current.clone().ok_or_else(|| p.error.clone())
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Drop for SshLocator {
    fn drop(&mut self) {
        // 送り口を閉じると裏のスレッドはトンネルを止めて抜ける
        self.tx = None;
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

/// 裏のスレッド。頼まれたとき（最初・主ループの失敗）とトンネルが死んだときに見つけ直す。
/// 間隔は Backoff に従い、待ちは recv_timeout に任せる（ビジーループにしない）。
fn worker<E: SshExec>(
    mut t: Tunneler<E>,
    mut backoff: Backoff,
    rx: Receiver<()>,
    shared: Arc<Mutex<Published>>,
    check_every: Duration,
) {
    // 最初の 1 回は頼まれなくても行う
    let mut wanted = true;
    let mut last_error = String::new();
    loop {
        let wait = if wanted {
            backoff.wait(Instant::now()).min(check_every)
        } else {
            check_every
        };
        match rx.recv_timeout(wait) {
            Ok(()) => {
                wanted = true;
                // 溜まった頼みはまとめて 1 回にする
                while rx.try_recv().is_ok() {}
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                t.stop();
                return;
            }
        }
        if t.tunnel_dead() {
            (t.log)(format!("ssh {} のトンネルが切れた。張り直す", t.host));
            t.stop();
            lock(&shared).current = None;
            wanted = true;
        }
        let now = Instant::now();
        if !(wanted && backoff.ready(now)) {
            continue;
        }
        wanted = false;
        let r = t.refresh();
        let mut p = lock(&shared);
        match r {
            Ok(d) => {
                let progressed = p.current.as_ref() != Some(&d);
                backoff.after(now, progressed);
                p.current = Some(d);
                last_error.clear();
            }
            Err(e) => {
                backoff.after(now, false);
                if e != last_error {
                    (t.log)(e.clone());
                    last_error = e.clone();
                }
                p.current = None;
                p.error = e;
            }
        }
    }
}

/// 動いている ssh の子プロセスの登録簿。終了の合図・panic のときに全部止めるため（孤児の ssh を残さない）。
pub struct Registry {
    children: Mutex<Vec<(u64, Child)>>,
    next: AtomicU64,
}

/// このプロセスの登録簿。
pub static CHILDREN: Registry = Registry::new();

impl Registry {
    pub const fn new() -> Self {
        Self {
            children: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
        }
    }

    fn add(&self, c: Child) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.children).push((id, c));
        id
    }

    /// None: 登録簿に無い（止めた・kill_all で止まった）。Some(None): 動いている。Some(Some(s)): 終わった
    fn try_wait(&self, id: u64) -> Option<Option<ExitStatus>> {
        let mut g = lock(&self.children);
        let (_, c) = g.iter_mut().find(|(i, _)| *i == id)?;
        Some(c.try_wait().ok().flatten())
    }

    /// 登録簿から外す。kill が真なら止めてから外す。
    fn remove(&self, id: u64, kill: bool) {
        let c = {
            let mut g = lock(&self.children);
            g.iter()
                .position(|(i, _)| *i == id)
                .map(|at| g.swap_remove(at).1)
        };
        if let Some(mut c) = c {
            if kill {
                let _ = c.kill();
            }
            let _ = c.wait();
        }
    }

    /// 登録した子プロセスを全部止める。block が偽なら、ロックが取れないとき何もしない（panic の中で使う）。
    pub fn kill_all(&self, block: bool) {
        let mut g = if block {
            lock(&self.children)
        } else {
            match self.children.try_lock() {
                Ok(g) => g,
                Err(TryLockError::Poisoned(p)) => p.into_inner(),
                Err(TryLockError::WouldBlock) => return,
            }
        };
        for (_, mut c) in g.drain(..) {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        lock(&self.children).len()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// cmd を走らせ、limit 以内に終われば標準出力を返す。終わらなければ止めてエラーにする。
/// エラー文には標準エラーだけを入れる（標準出力にはトークンが入るため）。
pub fn run_limited(reg: &Registry, mut cmd: Command, limit: Duration) -> Result<Vec<u8>, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("起動できない: {e}"))?;
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b);
            }
            b
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
    );
    let id = reg.add(child);
    let deadline = Instant::now() + limit;
    let status = loop {
        match reg.try_wait(id) {
            None => return Err("止められた".into()),
            Some(Some(s)) => break s,
            Some(None) if Instant::now() >= deadline => {
                reg.remove(id, true);
                // 読み手のスレッドは待たない（孫のプロセスがパイプを持ったままのことがある）
                return Err(format!("{limit:?} 以内に終わらない"));
            }
            Some(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    reg.remove(id, false);
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if status.success() {
        return Ok(out);
    }
    let msg = String::from_utf8_lossy(&err);
    let msg = msg.trim();
    // 長いときは末尾だけ
    let msg: String = {
        let n = msg.chars().count();
        msg.chars().skip(n.saturating_sub(300)).collect()
    };
    if msg.is_empty() {
        Err(format!("{status} で終わった"))
    } else {
        Err(format!("{status} で終わった: {msg}"))
    }
}

/// 本物の ssh を走らせる。
pub struct SystemSsh {
    program: String,
    reg: &'static Registry,
}

impl SystemSsh {
    pub fn new() -> Self {
        Self::with_program("ssh", &CHILDREN)
    }

    /// 走らせるプログラムと登録簿を選ぶ（テスト用）。
    pub fn with_program(program: &str, reg: &'static Registry) -> Self {
        Self {
            program: program.to_string(),
            reg,
        }
    }
}

impl Default for SystemSsh {
    fn default() -> Self {
        Self::new()
    }
}

/// 登録簿に載せたトンネルの子プロセス。
struct RegisteredProc {
    reg: &'static Registry,
    id: u64,
}

impl TunnelProc for RegisteredProc {
    fn alive(&mut self) -> bool {
        match self.reg.try_wait(self.id) {
            Some(None) => true,
            Some(Some(_)) => {
                self.reg.remove(self.id, false);
                false
            }
            None => false,
        }
    }

    fn stop(&mut self) {
        self.reg.remove(self.id, true);
    }
}

impl SshExec for SystemSsh {
    fn read(&mut self, args: &[String]) -> Result<Vec<u8>, String> {
        let mut c = Command::new(&self.program);
        c.args(args);
        run_limited(self.reg, c, READ_LIMIT)
    }

    fn spawn(&mut self, args: &[String]) -> Result<Box<dyn TunnelProc>, String> {
        let child = Command::new(&self.program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // ssh の警告（転送の失敗など）はこのプロセスの標準エラーにそのまま出す
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| e.to_string())?;
        let id = self.reg.add(child);
        Ok(Box::new(RegisteredProc { reg: self.reg, id }))
    }

    fn free_port(&mut self) -> Result<u16, String> {
        let l = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        Ok(l.local_addr().map_err(|e| e.to_string())?.port())
    }

    fn ready(&mut self, port: u16) -> bool {
        let a = SocketAddr::from(([127, 0, 0, 1], port));
        TcpStream::connect_timeout(&a, Duration::from_millis(200)).is_ok()
    }
}

#[cfg(test)]
mod system_tests {
    //! 本物の子プロセスを使うテスト（ssh の代わりに sh・sleep を走らせる）。
    use super::*;
    use std::process::Command;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.args(["-c", script]);
        c
    }

    #[test]
    fn 期限付きの実行は標準出力を返す() {
        let r = Registry::new();
        let out = run_limited(&r, sh("printf hello"), Duration::from_secs(5)).unwrap();
        assert_eq!(out, b"hello");
        assert_eq!(r.len(), 0, "終わった子プロセスは登録簿から外す");
    }

    #[test]
    fn 失敗の文には標準エラーを入れ_標準出力は入れない() {
        let r = Registry::new();
        let e = run_limited(
            &r,
            sh("printf SECRET-out; echo boom >&2; exit 3"),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(e.contains("boom"), "{e}");
        assert!(!e.contains("SECRET-out"), "{e}");
    }

    #[test]
    fn 期限を過ぎたら止めてエラーにする() {
        let r = Registry::new();
        let t = Instant::now();
        let e = run_limited(&r, sh("sleep 5"), Duration::from_millis(200)).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert!(!e.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn 子プロセスは止めれば死に_kill_all_で全部止まる() {
        let r: &'static Registry = Box::leak(Box::new(Registry::new()));
        let mut ex = SystemSsh::with_program("sleep", r);
        let mut a = ex.spawn(&["30".to_string()]).unwrap();
        let mut b = ex.spawn(&["30".to_string()]).unwrap();
        assert!(a.alive() && b.alive());
        assert_eq!(r.len(), 2);
        a.stop();
        assert!(!a.alive());
        assert_eq!(r.len(), 1);
        r.kill_all(true);
        assert!(!b.alive());
        assert_eq!(r.len(), 0);
        b.stop(); // 2 度目でも困らない
    }

    #[test]
    fn 空きポートと繋がるかの確かめ() {
        let r: &'static Registry = Box::leak(Box::new(Registry::new()));
        let mut ex = SystemSsh::with_program("ssh", r);
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(ex.ready(l.local_addr().unwrap().port()));
        let p = ex.free_port().unwrap();
        assert_ne!(p, 0);
        assert!(!ex.ready(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    const TOKEN: &str = "SECRET-token-9f8e7d";

    fn remote_json(addr: &str, version: u32) -> Vec<u8> {
        format!(
            r#"{{"addr":"{addr}","token":"{TOKEN}","pid":42,"shader_dir":"/home/u/s","version":{version}}}"#
        )
        .into_bytes()
    }

    /// 偽の ssh が記録したこと。
    #[derive(Default)]
    struct FakeLog {
        reads: Vec<Vec<String>>,
        spawns: Vec<Vec<String>>,
        /// spawn した子プロセスの生死（false にすると死んだことになる）
        alive: Vec<Arc<AtomicBool>>,
        stopped: Vec<Arc<AtomicBool>>,
        logs: Vec<String>,
    }
    type Shared = Arc<Mutex<FakeLog>>;

    struct FakeProc {
        alive: Arc<AtomicBool>,
        stopped: Arc<AtomicBool>,
    }
    impl TunnelProc for FakeProc {
        fn alive(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
        fn stop(&mut self) {
            self.alive.store(false, Ordering::SeqCst);
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    /// 読み出しのたびに files を 1 つずつ返す（尽きたら最後のものを返し続ける）。
    struct FakeExec {
        log: Shared,
        files: Vec<Result<Vec<u8>, String>>,
        next_port: u16,
        /// spawn した子プロセスを最初から死んだものにする
        spawn_dead: bool,
    }
    impl SshExec for FakeExec {
        fn read(&mut self, args: &[String]) -> Result<Vec<u8>, String> {
            self.log.lock().unwrap().reads.push(args.to_vec());
            if self.files.len() > 1 {
                self.files.remove(0)
            } else {
                self.files[0].clone()
            }
        }
        fn spawn(&mut self, args: &[String]) -> Result<Box<dyn TunnelProc>, String> {
            let mut l = self.log.lock().unwrap();
            l.spawns.push(args.to_vec());
            let alive = Arc::new(AtomicBool::new(!self.spawn_dead));
            let stopped = Arc::new(AtomicBool::new(false));
            l.alive.push(alive.clone());
            l.stopped.push(stopped.clone());
            Ok(Box::new(FakeProc { alive, stopped }))
        }
        fn free_port(&mut self) -> Result<u16, String> {
            self.next_port += 1;
            Ok(self.next_port)
        }
        fn ready(&mut self, _: u16) -> bool {
            true
        }
    }

    fn tunneler(files: Vec<Result<Vec<u8>, String>>) -> (Tunneler<FakeExec>, Shared) {
        let log: Shared = Arc::default();
        let exec = FakeExec {
            log: log.clone(),
            files,
            next_port: 50000,
            spawn_dead: false,
        };
        let l2 = log.clone();
        let mut t = Tunneler::new(
            exec,
            "pc01",
            DEFAULT_REMOTE_FILE,
            Box::new(move |m| l2.lock().unwrap().logs.push(m)),
        );
        t.ready_limit = Duration::from_millis(50);
        t.ready_step = Duration::from_millis(5);
        (t, log)
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn cat_の引数の形() {
        assert_eq!(
            cat_args("pc01", "~/.cache/kagelife/control.json"),
            s(&[
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "pc01",
                "cat",
                "~/.cache/kagelife/control.json"
            ])
        );
    }

    #[test]
    fn トンネルの引数の形() {
        assert_eq!(
            forward_args("pc01", 50001, "127.0.0.1:43210"),
            s(&[
                "-N",
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=5",
                "-o",
                "ServerAliveCountMax=2",
                "-L",
                "127.0.0.1:50001:127.0.0.1:43210",
                "pc01"
            ])
        );
    }

    #[test]
    fn 相手の発見ファイルの版が1以外は受け付けない() {
        let e = parse_remote(&remote_json("127.0.0.1:1", 2)).unwrap_err();
        assert!(e.contains('2'), "{e}");
        assert!(parse_remote(&remote_json("127.0.0.1:1", 1)).is_ok());
    }

    #[test]
    fn 解析のエラー文に中身を出さない() {
        // token が数になっていると、serde の文は値をそのまま出す
        let bad = br#"{"addr":"127.0.0.1:1","token":987654321,"version":1}"#;
        let e = parse_remote(bad).unwrap_err();
        assert!(!e.contains("987654321"), "{e}");
    }

    #[test]
    fn 最初はファイルを読んでトンネルを張り_ローカルの_addr_と相手のトークンを返す() {
        let (mut t, log) = tunneler(vec![Ok(remote_json("127.0.0.1:43210", 1))]);
        let d = t.refresh().unwrap();
        assert_eq!(d.addr, "127.0.0.1:50001");
        assert_eq!(d.token, TOKEN);
        let l = log.lock().unwrap();
        assert_eq!(l.reads, vec![cat_args("pc01", DEFAULT_REMOTE_FILE)]);
        assert_eq!(
            l.spawns,
            vec![forward_args("pc01", 50001, "127.0.0.1:43210")]
        );
    }

    #[test]
    fn 相手の_addr_が同じでトンネルが生きていれば張り直さない() {
        let (mut t, log) = tunneler(vec![Ok(remote_json("127.0.0.1:43210", 1))]);
        let a = t.refresh().unwrap();
        let b = t.refresh().unwrap();
        assert_eq!(a, b);
        let l = log.lock().unwrap();
        assert_eq!(l.reads.len(), 2);
        assert_eq!(l.spawns.len(), 1);
    }

    #[test]
    fn 相手の_addr_が変わったら古いトンネルを止めて張り直す() {
        let (mut t, log) = tunneler(vec![
            Ok(remote_json("127.0.0.1:43210", 1)),
            Ok(remote_json("127.0.0.1:43999", 1)),
        ]);
        t.refresh().unwrap();
        let d = t.refresh().unwrap();
        assert_eq!(d.addr, "127.0.0.1:50002");
        let l = log.lock().unwrap();
        assert_eq!(l.spawns[1], forward_args("pc01", 50002, "127.0.0.1:43999"));
        assert!(l.stopped[0].load(Ordering::SeqCst), "古いトンネルを止める");
        assert!(!l.stopped[1].load(Ordering::SeqCst));
    }

    #[test]
    fn トンネルが死んだら気づいて張り直す() {
        let (mut t, log) = tunneler(vec![Ok(remote_json("127.0.0.1:43210", 1))]);
        t.refresh().unwrap();
        assert!(!t.tunnel_dead());
        log.lock().unwrap().alive[0].store(false, Ordering::SeqCst);
        assert!(t.tunnel_dead());
        let d = t.refresh().unwrap();
        assert_eq!(d.addr, "127.0.0.1:50002");
        assert_eq!(log.lock().unwrap().spawns.len(), 2);
        assert!(!t.tunnel_dead());
    }

    #[test]
    fn トンネルがすぐ死ねばエラーにして止める() {
        let (mut t, log) = tunneler(vec![Ok(remote_json("127.0.0.1:43210", 1))]);
        t.exec.spawn_dead = true;
        assert!(t.refresh().is_err());
        assert!(log.lock().unwrap().stopped[0].load(Ordering::SeqCst));
        assert!(!t.tunnel_dead(), "張れていないので死んだトンネルも無い");
    }

    #[test]
    fn 読めなければエラーで_トンネルを張らない() {
        let (mut t, log) = tunneler(vec![Err("ssh: connect timed out".into())]);
        let e = t.refresh().unwrap_err();
        assert!(e.contains("pc01"), "{e}");
        assert!(log.lock().unwrap().spawns.is_empty());
    }

    #[test]
    fn 相手の_addr_が_socketaddr_でなければ受け付けない() {
        let (mut t, log) = tunneler(vec![Ok(remote_json("-oProxyCommand=x", 1))]);
        assert!(t.refresh().is_err());
        assert!(log.lock().unwrap().spawns.is_empty());
    }

    #[test]
    fn 止めればトンネルの子プロセスを止める() {
        let (mut t, log) = tunneler(vec![Ok(remote_json("127.0.0.1:43210", 1))]);
        t.refresh().unwrap();
        drop(t);
        assert!(log.lock().unwrap().stopped[0].load(Ordering::SeqCst));
    }

    #[test]
    fn トークンを子プロセスの引数_ログ_エラー文に出さない() {
        // 成功・addr の変化・版の違い・読めない・トンネルの死をひととおり通す
        let (mut t, log) = tunneler(vec![
            Ok(remote_json("127.0.0.1:43210", 1)),
            Ok(remote_json("127.0.0.1:43999", 1)),
            Ok(remote_json("127.0.0.1:43999", 2)),
            Err("ssh: Connection refused".into()),
            Ok(br#"{"addr":"127.0.0.1:1","token":["SECRET-token-9f8e7d"],"version":1}"#.to_vec()),
            Ok(remote_json("127.0.0.1:43999", 1)),
        ]);
        let mut errors = Vec::new();
        for i in 0..6 {
            if i == 5 {
                log.lock()
                    .unwrap()
                    .alive
                    .last()
                    .unwrap()
                    .store(false, Ordering::SeqCst);
                assert!(t.tunnel_dead());
            }
            match t.refresh() {
                Ok(d) => errors.push(format!("{d:?}")),
                Err(e) => errors.push(e),
            }
        }
        drop(t);
        let l = log.lock().unwrap();
        assert!(l.spawns.len() >= 3);
        for a in l.reads.iter().chain(l.spawns.iter()).flatten() {
            assert!(!a.contains(TOKEN), "引数にトークン: {a}");
        }
        for m in l.logs.iter().chain(errors.iter()) {
            assert!(!m.contains(TOKEN), "文にトークン: {m}");
        }
        assert!(!l.logs.is_empty());
    }

    #[test]
    fn 間隔は失敗や同じ結果が続くと倍になり_前に進めば戻る() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut b = Backoff::new(ms(100), ms(350));
        assert!(b.ready(t0));
        b.after(t0, false);
        assert!(!b.ready(t0 + ms(99)));
        assert!(b.ready(t0 + ms(100)));
        assert_eq!(b.wait(t0 + ms(40)), ms(60));
        b.after(t0, false);
        assert!(!b.ready(t0 + ms(199)));
        assert!(b.ready(t0 + ms(200)));
        b.after(t0, false);
        b.after(t0, false);
        // 上限で止まる
        assert!(b.ready(t0 + ms(350)));
        b.after(t0, true);
        assert!(b.ready(t0 + ms(100)));
        assert_eq!(b.wait(t0 + ms(200)), Duration::ZERO);
    }

    #[test]
    fn locate_は待たずに返し_失敗が続いても_ssh_を詰めて呼ばない() {
        let (t, log) = tunneler(vec![Err("ssh: Connection timed out".into())]);
        let mut loc = SshLocator::start(
            t,
            Backoff::new(Duration::from_millis(100), Duration::from_secs(10)),
            Duration::from_millis(20),
        );
        let start = Instant::now();
        let mut calls = 0;
        let mut slowest = Duration::ZERO;
        while start.elapsed() < Duration::from_millis(450) {
            let t = Instant::now();
            assert!(loc.locate().is_err());
            slowest = slowest.max(t.elapsed());
            calls += 1;
        }
        drop(loc);
        let reads = log.lock().unwrap().reads.len();
        assert!(calls > 1000, "locate を詰めて呼べている: {calls}");
        assert!(slowest < Duration::from_millis(50), "{slowest:?}");
        // 0・100・300 ms の 3 回（裏のスレッドの起き方で 1 回ずれてもよい）
        assert!((2..=4).contains(&reads), "ssh を {reads} 回呼んだ");
    }

    #[test]
    fn locate_は裏で張ったトンネルを返し_死んだら自分で張り直す() {
        let (t, log) = tunneler(vec![Ok(remote_json("127.0.0.1:43210", 1))]);
        let mut loc = SshLocator::start(
            t,
            Backoff::new(Duration::from_millis(10), Duration::from_millis(40)),
            Duration::from_millis(10),
        );
        let wait_for = |loc: &mut SshLocator, addr: &str| {
            let start = Instant::now();
            loop {
                if let Ok(d) = loc.locate()
                    && d.addr == addr
                {
                    return d;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(3),
                    "{addr} にならない"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let d = wait_for(&mut loc, "127.0.0.1:50001");
        assert_eq!(d.token, TOKEN);
        log.lock().unwrap().alive[0].store(false, Ordering::SeqCst);
        // locate を呼ばなくても、裏のスレッドが死んだトンネルに気づいて張り直す
        std::thread::sleep(Duration::from_millis(200));
        assert!(log.lock().unwrap().spawns.len() >= 2);
        wait_for(&mut loc, "127.0.0.1:50002");
        drop(loc);
        let l = log.lock().unwrap();
        assert!(
            l.stopped.last().unwrap().load(Ordering::SeqCst),
            "終わるときにトンネルを止める"
        );
    }
}
