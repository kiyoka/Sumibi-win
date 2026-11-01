# ADR 0085: ログオン限定ブートストラップ待受による Host / TIP 相互発見プロトコル

作成日: 2026-11-01 / Issue #4 / draft PR #10
状態: 策定・実装

## 1. 背景と課題

これまで `sumibi-host` と `sumibi-tip` (TSF DLL) のプロセス間接続・認証テストでは、テストランナー（親プロセス）が子プロセスを起動し、その起動ハンドル（launch handle）を保持して `DuplicateHandle` により相手に渡すことで相互の `PinnedProcess` を確立していた。

しかし製品運用環境において：
- `sumibi-host.exe` はユーザーのログオンセッション内で常駐起動する。
- `sumibi-tip.dll` はユーザーが起動した一般アプリケーション（Notepad, エクスプローラー等）のプロセス内で TSF スレッド（STA）によって動的にロードされる。
- 両プロセスは親子関係を持たず、直接 handle を継承・受渡しする経路が存在しない。

また、以下のセキュリティ原則・制約を厳格に維持しなければならない：
- **PID 単独の信頼禁止**: PID やプロセス名、パイプ名のみを信頼の根拠とせず、カーネルが管理する固定プロセスオブジェクト（`PinnedProcess`：プロセス生成時刻 `Stamp`、Logon SID、Integrity、AppContainer 拒否）による相互照合を維持する。
- **Fail-closed**: 相手プロセスの検証に失敗した場合、または保護状態が不明な場合は、いかなるデータも渡さず即座に切断する。
- **TSF 所有 STA の非ブロッキング**: TSF 所有スレッド（STA / callback）でディスク・署名・ネットワーク・IPC の同期待ちを行わない。すべての発見・接続シーケンスは `Worker` バックグラウンドスレッドで実行する。
- **送信未同意時の入力保護**: 送信未同意または準備未完了（Ready 前）の間は、入力本文を取得・送信しない。

## 2. アーキテクチャとプロトコル設計

### 2.1 ログオン限定ブートストラップパイプ
Host は起動時、自身のログオンセッションにバインドされた固定命名規則のブートストラップパイプサーバーを作成して待機する。

- パイプ名形式: `\\.\pipe\Sumibi.Bootstrap.v1.{logon_sid}`
- ACL: `D:P(A;;GA;;;{logon_sid})`
  同一ログオンセッションの通常デスクトッププロセスのみがアクセス可能。他ユーザーおよび別ログオンセッションからの接続は OS カーネルによって完全に遮断される。
- 同時接続: 複数アプリからの同時接続要求に備え、インスタンス数枠（最大 8 件）内で待機・補充を行う。

### 2.2 相互検証（Mutual Peer Verification）
クライアント（TIP）がブートストラップパイプへ接続した直後、双方は相手の PID をカーネルから取得し、同一ログオンセッション内で `OpenProcess` して `PinnedProcess` を構築・相互照合する。

1. **Host 側（サーバー）**:
   - `GetNamedPipeClientProcessId` でクライアント PID を取得。
   - `PinnedProcess::from_peer_pid(pid)` によりクライアントの handle を固定。
   - クライアントが同一ログオンセッション、非昇格、非 AppContainer、通常整合性（Medium Integrity: 0x2000）であることを検証。
   - `pipe.require_peer(&client_peer)` で、パイプに接続しているクライアントが固定した `client_peer` と寸分違わず一致することをカーネルレベルで検証。
2. **TIP 側（クライアント）**:
   - `GetNamedPipeServerProcessId` で Host の PID を取得。
   - `PinnedProcess::from_peer_pid(pid)` により Host の handle を固定。
   - `pipe.require_peer(&host_peer)` で、接続先パイプサーバーが固定した `host_peer` と完全に同一であることを検証。
   - `host_peer.observed_image_path()` を取得し、Host 実行バイナリの同一性・保護配置・署名を検証（開発・テスト時は構成されたポリシーに従う）。

いずれかの検査が失敗した場合、パイプは即時切断され、以降のやり取りは行われない。

### 2.3 一回限りの Private Peer Handoff
相互認証が成功したブートストラップパイプ上で、既存の `handoff` プロトコル（ADR 0065）を実行する：

1. Host は `BrokerBoot::begin_private_accept(client_peer)`（または `HostControl::try_accept_private_peer`）を呼び出し、当該クライアント専用の一回限りプライベートパイプ名 `name: Nonce` と `broker: Nonce` を発行。
2. Host は `handoff::send(pipe, &client_peer, &name, &broker, stop, deadline)` を送信。
3. TIP は `handoff::receive(pipe, &host_peer, stop, deadline)` で受領。
4. 転送完了後、ブートストラップ接続は正常切断・破棄される。

### 2.4 本番プライベートセッションへの接続
TIP は受領した `name` と `broker` を用い、`Worker::prepare_pinned(reservation, name, broker, host_peer, deadline)` を呼び出して一回限りプライベートパイプ `\\.\pipe\Sumibi-peer-{name}` に接続する。
Host 側は既存のワーカーループでこのプライベートパイプを受け付け、Hello/Ready ハンドシェイクおよび政策ディスパッチを開始する。

## 3. 利点と保護保証

1. **ゼロ・トラストな発見**:
   固定パイプ名は発見の「窓口」に過ぎず、一切の認証情報や本文を流さない。実際の通信は相互に kernel handle を固定・検証した後に発行される使い捨ての private pipe 上で行われる。
2. **完全な PID 再利用・偽装耐性**:
   パイプ接続中にカーネルから PID を取得し、即座に handle を取得して生成時刻 `Stamp` を記録する。接続中のパイプ実体と handle の同一性を `require_peer` で担保するため、PID 再利用の隙間は存在しない。
3. **STA ブロッキングの完全排除**:
   ブートストラップ接続、Host 検証、handoff 受信、本番プライベートパイプ接続の全シーケンスは `Worker` スレッド内で非同期に実行される。TSF 所有スレッドは準備状態をイベント経由で非ブロッキングに監視するだけである。
