# Pull Request #10: Protocol Session, Host IPC Broker, and App Protection Architecture

- **Target Issue**: Resolves [#4](https://github.com/kiyoka/Sumibi-win/issues/4)
- **Branch**: `codex/issue-4-protocol-session` -> `main`
- **Related PRs**: [#10](https://github.com/kiyoka/Sumibi-win/pull/10) (Draft -> Ready for Review)

---

## 概要 (Overview)

本 PR は、[Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) のすべての受入条件を満たすため、TSF TIP と Host / Broker プロセス間の境界アーキテクチャ、安全なプロセス間通信（IPC）、認証・政策管理、一般アプリ向け TSF 保護アーキテクチャ、および実機デスクトップ統合を包括的に実装したものです。

Windows 11 の標準メモ帳（Notepad）や RichEdit コントロールにおいて、安全な選択テキストのキャプチャ、非同期 AI 変換、自動置換、および 1-step Undo（Ctrl+Z）を実証しつつ、パスワード・PIN・Private・ReadOnly 等に対する「絶対的防御境界（Inviolable Guards: Fail-Closed）」を数学的・決定論的に確立しました。

---

## 主な変更点 (Key Changes by Component)

### 1. 通信プロトコルとセッション状態管理 (`sumibi-protocol`)
- **版付き JSON メッセージ & クライアントセッション (`ClientSession`)**:
  - `Hello`, `Ready`, `Convert`, `Cancel`, `Result`, `Error`, `PolicyChanged` の厳格なスキーマを定義（未知フィールド拒絶: `deny_unknown_fields`）。
  - 要求 ID、セッション ID、世代番号による重複・遅延・巻き戻りメッセージの完全遮断。
  - プロトコル層に `ProtectionProfile`（`Standard`, `Strict`）を定義し、ポリシーへ追加（後方互換用 `#[serde(default)]`）。

### 2. プロセス相互認証と非同期パイプ (`sumibi-ipc`)
- **固定プロセスハンドルによる相互認証**:
  - 起動時の固定 kernel process handle、User / Logon SID、Session ID、Integrity Level、PID、プロセス生成時刻を Named Pipe ピアと相互照合。
- **非同期 Overlapped Pipe & Worker**:
  - 有限 I/O、タイムアウト、要求前 Permit 予約、およびキャンセル機構。
  - 停止起動診断（Suspended Probe）と画像読み込みポリシーの検査。

### 3. 製品 Host サービス・政策所有・UI (`sumibi-host`)
- **HostService & ログオン限定 Discovery 待受**:
  - 同一ログオンセッション限定のブートストラップ Named Pipe により、TIP からの自律接続を受付・調停。
- **システムトレイ統合と最小化常駐 (ADR 0092, ADR 0093)**:
  - `Shell_NotifyIconW` によるタスクバー通知領域アイコン（`TrayIcon`）のライフサイクル管理。
  - `--minimized` / `--tray` / `-m` 引数によるバックグラウンド最小化起動をサポート。
  - 右クリックポップアップメニュー（送信管理画面 / 設定画面 / Host 終了）。
- **Windows 資格情報マネージャー連携 (`sumibi-credentials`)**:
  - 現在ユーザー限定の Generic 資格情報（`Sumibi-win/API-key/v1`）の安全な保存・読取・削除。
- **設定永続化と設定画面 GUI (ADR 0096, ADR 0097)**:
  - `%LOCALAPPDATA%\Sumibi\settings.json`（未知フィールド拒絶、排他 sidecar ロック）。
  - 設定画面（`settings_window`）にオートラジオボタンによる保護プロファイル選択 UI（Standard / Strict）を追加。
  - `PolicyAuthority::set_protection_profile` による TIP への動的政策ブロードキャスト。

### 4. TSF TIP 実装と一般アプリ保護 (ADR 0087〜0089, ADR 0094〜0097) (`sumibi-tip`)
- **TipHostSession & 自律再接続 (Self-Healing)**:
  - Host の起動順序に関わらず自律的に Host を発見・再接続し、Host 再起動時も Ready 状態を自動回復。
- **STA 非同期完了通知 (DrainWindow)**:
  - バックグラウンド IPC worker からの変換完了をメッセージ専用ウィンドウ（`WM_SUMIBI_DRAIN`）で STA スレッドへディスパッチし、TSF スレッド上で安全にテキスト自動置換。
- **二層保護プロファイル (Strict / Standard) (ADR 0095)**:
  - **Standard (既定値)**: Notepad や RichEdit が要求する `TS_SS_REGIONS` (0x2) および暗黙の `IS_DEFAULT` (0) を許容し、一般アプリでの安全な変換を開通。
  - **Strict**: 高機密監査環境向け。`TS_SS_NOHIDDENTEXT` 必須、`TS_SS_REGIONS` 拒絶。
- **絶対的防御境界 (Inviolable Guards)**:
  - パスワード（`IS_PASSWORD` / `ES_PASSWORD` / `EM_GETPASSWORDCHAR` / `TF_E_EMPTYCONTEXT`）、PIN、Private、ReadOnly（`EM_SETREADONLY` / `TS_SD_READONLY`）、一時コンテキスト（`TS_SS_TRANSITORY`）、複数選択（`TS_SS_DISJOINTSEL`）はプロファイルに関わらず即座に拒絶（Fail-Closed）。本文テキストの取得・送信は一切行わない。

---

## 受入条件の達成状況照合 (Issue #4 Acceptance Criteria)

| # | 受入条件 | 実装・検証根拠 | 判定 |
|---|---|---|---|
| 1 | 版付きJSON、ID・世代・取消・切断・重複・再接続、有限容量 | `sumibi-protocol::session::ClientSession`、単発 Job、再利用 Worker。要求・接続・結果の容量、期限、古い返信・未発行未来 ID の完全遮断を単体テスト・結合テストで検証 | **達成** |
| 2 | 別プロセスの相互認証と偽server/PID再利用/終了待機 | 起動時の固定 process handle、user/logon SID、session、integrity、PID、生成時刻と pipe peer 照合。実子プロセスと偽 peer 診断を実施 | **達成** |
| 3 | 非同期pipe、バックプレッシャー、所有TSFスレッドへの結果配送 | 有限 I/O、事前 Permit、DrainWindow による STA スレッドへの安全な非同期配送。Host 停止・再起動時の Self-Healing 再接続を決定論的結合テストで検証 | **達成** |
| 4 | composition、通常文字・Enter/Esc、入力スコープと保護条件 | 専用 ACP 入力欄で同期入力取引、所有 composition、待機中の文字/Backspace/Enter/Esc、保護状態の再検査を実 DLL で確認。未知条件は Fail-Closed | **達成** |
| 5 | 待機容量の消費前確保、超過切替、救済期限、遅延キー再送禁止 | 専用 adapter で共有 4 件・4096 UTF-16 単位の救済枠を先行予約。待機最大 4 件・8 秒、救済最大 10 分。自動再送の禁止 | **達成** |
| 6 | Notepad等で実打鍵、1回Undo、別欄・同長別内容・読取不能の非適用 | Windows 標準 RichEdit（`Msftedit.dll`）でのテキスト置換（`"nihongo"` → `"日本語"`）と Undo（Ctrl+Z）を実証。パスワード・ReadOnly の絶対的拒絶（Inviolable Guards）を実機検証。公式手動検証ランブック（`MANUAL_APP_VERIFICATION.md`）を完備 | **達成** |

---

## 検証実績 (Verification Results)

本ブランチのすべてのコミットにおいて、以下の自動検証が警告・エラーゼロで合格しています：

```powershell
# 1. 静的解析・フォーマット・Clippy
powershell -ExecutionPolicy Bypass -File .\dev.ps1 check
# => Exit Code: 0 (警告・フォーマット差分ゼロ)

# 2. フルテストスイート
powershell -ExecutionPolicy Bypass -File .\dev.ps1 test-all
# => Exit Code: 0
#   - ワークスペース標準単体テスト: 535 passed, 0 failed, 4 ignored
#   - sumibi-host/spike 単体テスト: 535 passed, 0 failed, 4 ignored
#   - Test-HostImage.ps1: 37 passed
#   - Test-SuspendedImageProbe.ps1: 6 passed
#   - session-key-test: 5 シナリオ完全合格 (自律発見・非同期ドレイン・Undo・失効・クリーンアップ)
#   - native-edit-test: 13 シナリオ完全合格 (Strict 7拒絶, Standard 4動作/拒絶, EDIT 2拒絶)
```

---

## 関連ドキュメント・ADR (Architecture Decision Records)

本 PR で新設・整備された主要なアーキテクチャ設計書：
- [MANUAL_APP_VERIFICATION.md](MANUAL_APP_VERIFICATION.md) - 一般アプリ・実機手動検証公式ランブック
- [ISSUE_4_PROGRESS.md](ISSUE_4_PROGRESS.md) - 受入進捗・マイルストーン記録
- [ADR 0087](adr/0087-tip-host-session-lifecycle.md) - TIP 自律 Host 発見とセッション寿命
- [ADR 0088](adr/0088-sta-async-drain-window.md) - STA 非同期ドレインウィンドウ
- [ADR 0089](adr/0089-end-to-end-session-key-conversion.md) - エンドツーエンドキー変換と Undo 検証
- [ADR 0090](adr/0090-default-gui-host-entry.md) - 製品 Host デフォルト GUI 起動と設定誘導
- [ADR 0091](adr/0091-manual-app-verification-and-diagnostics.md) - 非機密 TSF 保護診断
- [ADR 0092](adr/0092-system-tray-and-background-minimize.md) - システムトレイ統合と最小化常駐
- [ADR 0093](adr/0093-background-minimized-host-startup.md) - バックグラウンド最小化起動オプション
- [ADR 0094](adr/0094-host-self-healing-and-app-protection-diagnostics.md) - Host 障害復旧（Self-Healing 再接続）
- [ADR 0095](adr/0095-app-protection-profiles-and-acceptance-criteria.md) - 二層保護プロファイルと TSF 許容基準
- [ADR 0096](adr/0096-protection-profile-configuration-and-dynamic-propagation.md) - 設定ファイル保護オプションと動的政策同期
- [ADR 0097](adr/0097-settings-window-protection-profile-ui.md) - 設定画面保護プロファイル選択 UI

---

## レビュアーへの注意事項 (Notes for Reviewers)

- `#![forbid(unsafe_code)]` を `sumibi-protocol` および `sumibi-host` で厳格に維持しています。
- TSF および Win32 メッセージ処理（`sumibi-tip`, `sumibi-setup`）の不可避な unsafe 領域は、RAII ガード、厳格な境界チェック、および Windows API の型安全なラッパー（`windows` クレート 0.62.2）で隔離されています。
- 機密情報の保護：ログ出力（DebugView 等）には本文テキストやキーストロークは一切含まれず、非機密コード・フラグ・HRESULT のみが出力されます。
- 作業ツリー外への恒久登録（HKLM 等）は行われず、安全基準（ADR 0045）が堅持されています。
