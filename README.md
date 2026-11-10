# kagelife-deck

Stream Deck MK.2 から [kagelife](https://github.com/cyokozai/kagelife) を操作する Linux の常駐アプリです。kagelife と同じ機械で動かし、kagelife の制御口 v1（localhost の HTTP/JSON）を呼びます。

- 左 3 列 × 3 行のキーは、kagelife の数字キー 1〜9 と同じスロットです。kagelife が対応していれば、各キーにそのシェーダの動くプレビューが出ます
- タップ BPM、fade と cut の切替、拍数の増減、状態の表示を右 2 列に置いています
- kagelife が落ちても、Stream Deck を抜き差ししても、自動で繋ぎ直します
- 非同期ランタイムを使わずスレッド 1 本で動き、描画内容が変わったキーだけを送ります

## キー配置

キー番号は 行 × 5 + 列（左上が 0）です。

| | 列 0 | 列 1 | 列 2 | 列 3 | 列 4 |
|---|---|---|---|---|---|
| 行 0 | 1 | 2 | 3 | TAP（タップ BPM） | 状態 |
| 行 1 | 4 | 5 | 6 | FADE / CUT の切替 | 0（戻る） |
| 行 2 | 7 | 8 | 9 | `[`（拍数を減らす） | `]`（拍数を増やす） |

- **1〜9**: kagelife の数字キーと同じシェーダに切り替えます。FADE のときはクロスフェード（kagelife の Shift＋数字）、CUT のときは即時切替です。どのシェーダがどの数字かは kagelife が決めます（ファイル名順。`-llm-slots` で変わります）
  - 左上に数字、active のシェーダに緑の枠、フェード先に橙の枠、コンパイルに失敗したシェーダに赤の枠と `ERR` を出します
- **TAP**: 叩いた間隔から BPM を求めて kagelife に送ります。測り方は kagelife の内蔵タップと同じです（2 秒を超えて空くと測り直し、直近 8 回の平均、40〜300 に丸める）。1 回目のタップは今の BPM のまま拍頭を合わせます。キーには今の BPM を出し、測定値・設定値のときは青、既定の 120 のときは灰色にします
- **FADE / CUT**: 1〜9 を押したときの切替の方式です。FADE のときは今の拍数も出します
- **0**: kagelife の `-llm-slots` の「戻る」（直前のシェーダへの即時切替）です
- **`[` `]`**: クロスフェードの拍数を減らす・増やす（kagelife の `[` `]` と同じ）
- **状態**: kagelife に繋がっていれば `ON` と fps、コンパイルエラーがあれば赤地に `ERR` を出します

kagelife に繋がっていないときは全キーに `offline` と出し、繋がると自動で戻ります。

### 古い kagelife で使うとき

kagelife の `GET /v1/state` の `features` に `previews` と `keys` が無いとき（それらを足す前の kagelife）は、次のように動きます。

- 1〜9 にはプレビューの代わりにシェーダの名前を出し、`shaders` の先頭 9 個をファイル名順で 1〜9 とみなします。`-llm-slots` の割り当ては反映されません
- 0・`[`・`]` は灰色になり、押しても何もしません

## 導入（Ubuntu 系 x86_64）

### 1. ビルド

ローカルに Rust を入れず、コンテナの中でビルドできます。

```sh
docker build -t kagelife-deck-dev -f container/Dockerfile container
docker run --rm -v "$PWD":/src -w /src kagelife-deck-dev cargo build --release
install -m 755 target/release/kagelife-deck ~/.local/bin/kagelife-deck
```

ホストに Rust がある場合は、`sudo apt install pkg-config libudev-dev` の後に `cargo build --release` でもビルドできます。

### 2. udev ルールと plugdev

一般ユーザーが Stream Deck を開けるようにします。

```sh
sudo install -m 644 packaging/40-kagelife-deck.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
groups | grep -q plugdev || sudo usermod -aG plugdev "$USER"   # 入っていなければ足し、ログインし直す
```

ルールを入れた後に Stream Deck を挿し直してください。ルールは Stream Deck MK.2（USB ID `0fd9:0080`）と Scissor Keys 版（`0fd9:00a5`）を対象にしています。

### 3. systemd --user で常駐させる

```sh
mkdir -p ~/.config/systemd/user
cp packaging/kagelife-deck.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now kagelife-deck
journalctl --user -u kagelife-deck -f    # ログを見る
```

kagelife より先に起動しても構いません。kagelife が起動すると自動で繋がります。

## 使い方

```text
kagelife-deck [--poll-hz <Hz>] [--brightness <0-100>] [--control-file <path>]
```

| 引数 | 既定 | 意味 |
|---|---|---|
| `--poll-hz` | 2 | `GET /v1/state` を取り直す頻度（0.1〜30）。プレビューはこれと別に 10 Hz で取ります |
| `--brightness` | 60 | Stream Deck の明るさ（0〜100） |
| `--control-file` | 下を参照 | kagelife の発見ファイル |

発見ファイルは次の順に探します。

1. `--control-file`
2. 環境変数 `KAGELIFE_CONTROL_FILE`
3. `$XDG_CACHE_HOME/kagelife/control.json`
4. `~/.cache/kagelife/control.json`

3 と 4 は kagelife（Go の `os.UserCacheDir`）と同じ規則です。kagelife とこのアプリで環境変数が違うと、別の場所を見てしまいます。systemd --user から起動するときは特に気をつけてください。

## kagelife の制御口 v1 への依存

kagelife の制御口 v1（kagelife の `docs/adr/ADR-006-control-api.md`）だけを使います。プレビューと `0`・`[`・`]` のキーには、制御口の任意機能 `previews` と `keys` を足した kagelife が要ります。無い kagelife でも、上の「古い kagelife で使うとき」の形で動きます。

| 使う口 | 用途 |
|---|---|
| `GET /v1/state` | active・フェード・BPM・fps・エラー・`features`（既定 2 Hz、キーを押した直後にも取り直す） |
| `POST /v1/crossfade` `{"name"}` | 古い kagelife で FADE のときの 1〜9（beats は送らず、kagelife の今の拍数を使う） |
| `POST /v1/switch` `{"name"}` | 古い kagelife で CUT のときの 1〜9 |
| `POST /v1/bpm` `{"bpm"}` | TAP |
| `GET /v1/previews` | `features` に `previews` があるとき。プレビューと数字キーの割り当て（10 Hz、`frame` が前回と同じなら何もしない） |
| `POST /v1/keys` `{"key","shift"}` | `features` に `previews` と `keys` があるとき。1〜9（FADE なら `shift: true`）・0・`[`・`]` |

- 発見ファイルの `version` が 1 以外なら繋ぎません
- kagelife を再起動するとポートとトークンが変わります。接続の失敗と 401 のときは発見ファイルを読み直します
- 要求には契約にあるフィールドだけを入れ、応答の知らないフィールドは無視します

## 開発

```sh
docker build -t kagelife-deck-dev -f container/Dockerfile container
docker run --rm -v "$PWD":/src -w /src kagelife-deck-dev cargo test
docker run --rm -v "$PWD":/src -w /src kagelife-deck-dev cargo clippy --all-targets -- -D warnings
```

- デバイスと HTTP は trait の後ろに隠してあります。キー配置・タップ BPM・発見ファイル・差分送信は単体テストで確かめ、HTTP クライアントは偽サーバに対してテストします
- `examples/fake_kagelife.rs` は偽の kagelife です。`cargo run --example fake_kagelife -- /tmp/control.json` で起動し、`kagelife-deck --control-file /tmp/control.json` で繋ぐと、本物の kagelife なしで動かせます（previews を 10 fps で返します）
- 主な依存は次のとおりです
  - [elgato-streamdeck](https://github.com/OpenActionAPI/rust-elgato-streamdeck)（MPL-2.0、OpenDeck と同じ crate）
  - ureq（TLS なし）
  - embedded-graphics（MIT OR Apache-2.0）。キーの文字には、この crate に同梱の mono フォント（FONT_7X14・FONT_6X10、ASCII）を使います。フォントファイルは別に同梱していません

## ライセンス

未定です。
