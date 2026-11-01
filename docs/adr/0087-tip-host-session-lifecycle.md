# ADR 0087: 一般アプリ内 TIP の自律 Host 発見・セッション維持と安全なキー変換結合 (TipHostSession)

作成日: 2026-11-02 / Issue #4 / draft PR #10
状態: 策定・実装

## 1. 背景と課題

ADR 0085 でログオン限定ブートストラップ待受による Host / TIP 相互発見プロトコルを策定し、ADR 0086 で常駐 Host サービス待受ループ（`HostService`）を実装した。
これにより、常駐 Host 側は任意の一般アプリ内 TIP からの接続を受け付け、一回限りプライベートパイプへのハンドオフを継続的に行う準備が整った。

一方、TSF クライアント側（`crates/sumibi-tip`）では、これまで `spike` feature（開発・検査用機能）に基づき、外部ハーネス（`sumibi-setup`）から明示的に Transport インターフェースや Context を注入されるテスト構成に依存していた。

一般アプリ（Notepad、エクスプローラー等の Windows 標準アプリ）で製品として動作するためには、以下の要件を満たす製品向けセッション管理機構（`TipHostSession`）が必要である：

1. **TSF STA スレッドへの完全な非ブロッキング保証**:
   - TSF のライフサイクル（`Activate`, `Deactivate`）やキーイベント（`OnPreservedKey`, `OnKeyDown`）のコールバック内で、パイプの接続待ち・ハンドオフ・ポリシー同期・ネットワーク変換等のブロッキング I/O を一切行ってはならない。
   - すべての IPC 待機とポリシー更新はバックグラウンドスレッドで非同期に処理し、STA 側にはアトミックな状態共有のみを行う。
2. **自律的な Discovery 接続と再接続（Self-Healing Connection）**:
   - TIP のアクティブ化時、ログオン限定ブートストラップパイプ（`current_bootstrap_pipe_name()`）に対して自律的に接続を試行。
   - Host がまだ起動していない場合や、Host が途中で再起動された場合でも、一般アプリをクラッシュさせたりフリーズさせたりすることなく、バックオフを伴うバックグラウンド再試行を行う。
3. **Fail-Closed な保護判定ゲートとの結合**:
   - 変換ホットキー（Ctrl+J）が押下された際、`protection::check` で現在の入力コンテキストの InputScope、ReadOnly、Transitory 状態等を厳格に検証。
   - パスワード欄、PIN 入力欄、未承認の InputScope、あるいは保護状態不明の場合は即座に拒否（`Ok(false.into())`）し、キーをスルー。入力本文の読取や外部送信は一切行わない。
4. **同意（Consent）連動の送信保護**:
   - Host 側のポリシーが `allowed == true`（明示同意済み）の場合に限り、`ManualEdit::capture` で選択テキストを取得し、Worker へ変換をディスパッチする。
   - 未同意時やポリシー失効時は、テキスト取得を一切行わず、予約も行わない。
5. **1回 Undo とテキスト適用の完全性**:
   - 変換完了後は所有 TSF コンテキストの Read/Write セッション内でテキストを適用し、エディタ本来の 1 回 Undo（Ctrl+Z）を保証する。

## 2. アーキテクチャと設計

### 2.1 セッション管理構造 (`TipHostSession`)

```mermaid
flowchart TD
    TSF["TSF (OS / 一般アプリ STA)"] -->|Activate / OnPreservedKey| Session["TipHostSession (STA-local)"]
    Session -->|非ブロッキング状態照合| State["Shared State (Atomic / Arc)"]
    Session -->|保護判定| Protection["protection::check (InputScope, TS_STATUS)"]
    Session -->|テキストキャプチャ & 適用| ManualEdit["ManualEdit (TSF Write Session & 1-time Undo)"]

    BackgroundThread["Background Worker Thread"] -->|1. Discovery 接続 & Handoff| HostDiscovery["Host (Bootstrap Pipe)"]
    BackgroundThread -->|2. Private Pipe 接続 & 政策監視| HostPrivate["Host (Private Pipe)"]
    BackgroundThread -->|3. 政策同期 (Ready/Allowed)| State
    BackgroundThread -->|4. 変換ディスパッチ| HostPrivate
    HostPrivate -->|変換結果| BackgroundThread
    BackgroundThread -->|結果通知 (STA PostMessage / Event)| Session
```

### 2.2 ライフサイクル

1. **Activate**:
   - `TipHostSession::start()` を呼び出し、バックグラウンド接続ワーカーを起動。
   - ホットキー（Ctrl+J: `COMMAND_CONVERT`）を `ITfKeystrokeMgr::PreserveKey` で登録。
   - メタデータ監視シンクを登録。
2. **Discovery & Policy Sync (Background)**:
   - バックグラウンドスレッドで `Worker::prepare_discovery` を実行。
   - ログオン限定ブートストラップパイプに接続し、相互プロセス認証（`PinnedProcess`）とハンドオフを経てプライベートパイプを確立。
   - Host からの政策更新（`prepared_policy()`）を受信し、`allowed` フラグと政策版を `Shared State` に反映。
3. **OnPreservedKey (STA)**:
   - (a) `Shared State` をチェックし、Host が接続済みかつ `allowed == true` でない場合は即座に `Ok(false.into())` を返し、キーをスルー（入力本文は読まない）。
   - (b) `protection::check` を実行し、InputScope が `IS_TEXT` 等の許可スコープかつ ReadOnly でないことを検証。不適合なら即座に `Ok(false.into())` でスルー。
   - (c) `Worker::reserve` で 1 枠予約。
   - (d) `ManualEdit::capture` で選択テキストを取得。
   - (e) バックグラウンド Worker へ変換要求（`Worker::start`）をディスパッチ。
4. **Complete & Apply (STA)**:
   - 変換完了後、TSF の非同期編集セッションを要求し、`ticket.commit(result)` でテキストを適用。
5. **Deactivate**:
   - ホットキー登録を解除（`UnpreserveKey`）。
   - バックグラウンド Worker に停止シグナルを送り、安全にドレイン切断。

## 3. 利点と保護保証

1. **一般アプリへのゼロ・インパクト**:
   Host が未起動、停止中、あるいはクラッシュした場合でも、一般アプリの文字入力・キーストローク・TSF 処理は一切遅延・ブロックされない。
2. **絶対的プライバシー保護**:
   パスワード欄や機密入力欄ではホットキー処理そのものがバイパスされ、入力本文のキャプチャコードへの到達自体が構造的に遮断される。
3. **完全なプロセス整合性**:
   PID のみによる偽装を排除し、固定した kernel process handle とログオン SID による相互検証を維持する。
