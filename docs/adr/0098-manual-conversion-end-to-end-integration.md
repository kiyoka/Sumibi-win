# ADR 0098: 手動変換の縦断統合と受入条件の達成

状態：検証済み（Issue #5 完了）。
2026-11-03。Issue #5 / PR 対象。前提は ADR 0089、ADR 0096、ADR 0097。

## 文脈

Issue #1 の最小試作と言語選定、Issue #4 の Core・認証付きIPC・TSF入力基盤を経て、
手動LLM変換（Ctrl+J）を Host EXE、設定形式版、同意管理、Windows 資格情報マネージャー、
および模擬／本番 HTTP(S) まで縦断的に統合し、安全な手動変換パイプラインを確立する。

Mac版の仕様（`docs/MAC_PARITY.md`）および技術設計（`docs/TECHNICAL_OVERVIEW.md`）との
整合性を保ちつつ、Windows 固有の TSF / STA 寿命モデルおよびプロセス間相互認証と統合する。

## 決定事項

### 1. 書記素分割の固定と 1,000 書記素上限

- 書記素分割は `unicode-segmentation 1.13.3` (Unicode 17.0.0) へ厳格に固定する。
  `sumibi_host::first_conversion::CONVERSION_UNICODE_VERSION: (17, 0, 0)` を公開し、
  `sumibi_core::BACKSPACE_UNICODE_VERSION` と完全一致することをテストで検証する。
- 原文（`source`）、現在の変換結果（`current_result`）、ユーザー辞書（`user_dictionary`）の
  合計書記素数を計算し、総合上限 1,000 書記素（`MAX_TOTAL_GRAPHEMES`）を厳格に検査する。
  超過時は `PayloadError::TotalGraphemesExceeded` を返す。
- 各入力要素について、TSF / IPC の制約である最大 1,024 UTF-16 単位（`MAX_TEXT_UNITS`）、
  最大 4,096 UTF-8 バイト、およびオブジェクト置換文字（`\u{fffc}`）の非含有を検査する。
- ユーザー辞書が渡された場合は `<user_dictionary>` タグを含むプロンプトを動的に構築し、
  渡されない場合は「ユーザー辞書は登録されていません。」とする。

### 2. 接続先別の同意と認可版変更による旧結果破棄

- 送信先（`Destination`）ごとに独立した同意（`manual_consent: Option<Destination>`）を管理する。
  接続先・モデル・キーの変更および同意取消のたびに `authorization` または `settings` バージョンを更新する。
- 送信開始前に `authorize_send_start` で版番号・接続先・同意・資格情報の一致を検査し、不一致時は直ちに拒否する。
- 飛行中のリクエストも、応答受領後の `authorize_result` で最新版番号と再照合し、版不一致時は結果を適用せず破棄する。

### 3. 模擬HTTPおよび本番HTTPSの安全制約

- 模擬HTTP（`diagnostic_http.rs`）および本番HTTPS（`native_https.rs` / `sumibi-winhttp`）において、
  リダイレクトを自動追跡せず拒否する（`TransportError::Redirect`）。
- 自動再送を行わず、エラー発生時は即時に固定エラー分類（`TransportError`）として返却する。
- タイムアウト上限は 60 秒とする。
- 応答サイズ上限は 64 KB（`MAX_RESPONSE_BYTES`）とし、超過時は `ResponseTooLarge` とする。

### 4. 通信失敗時の原文維持とエラー解除不要での次回即時再開

- サーバーの 500 エラー、接続切断、タイムアウトなどの通信失敗時、`ticket.cancel()` を呼び出すことで
  TSF 文書本文を一切変更せず原文のまま維持する。Undo 履歴も汚染しない。
- エラーモーダルの OK ボタン押下などのエラー解除操作を一切不要とし、直後の打鍵（Ctrl+J）で即座に
  次の変換を開始できる。

### 5. 設定の原子的保存・破損回復と非漏洩の保証

- 設定レコード形式版 `Record.version = 1` を固定する。
- サイドカーロック（`.settings.json.lock`）と一時ファイルリネームによる原子的保存を行う。
- 破損ファイル（不正JSON、未知の形式版、巨大ファイル）に対する安全なロード拒否（`SettingsError::Invalid`）と、
  明示的保存による回復を保証する。
- 設定ファイル、IPC 電文、ログ出力に文書本文（source / candidate）および機密キーを紛れ込ませない。

## 検証

1. **手動変換 E2E 統合テスト（`crates/sumibi-host/tests/manual_conversion_e2e.rs`）**:
   - `test_unicode_and_combined_grapheme_limits`: Unicode 17.0.0 整合、1000書記素境界、絵文字、各不正文字排除。
   - `test_manual_conversion_pipeline_with_diagnostic_http`: 模擬HTTP、未同意拒否、キー欠損拒否、500エラー時の固定分類と原文維持、エラー解除なしでの即座の再試行成功、同意取消による旧結果破棄。
   - `test_settings_atomic_save_and_corruption_recovery`: 設定形式版v1、原子的保存、破損回復、本文・キーの非漏洩。
2. **TIP セッション単体テスト（`crates/sumibi-tip/src/session.rs`）**:
   - `drain_sink_cancels_ticket_on_worker_failure_and_leaves_state_clean`: 失敗イベント受信時の安全なチケット取消。
   - `session_denies_conversion_hotkey_when_unconsented_or_missing_key`: 未同意・キー欠損時の fail-closed。
3. **実機 TSF E2E プローブ（`crates/sumibi-setup/src/session_keys.rs` / `session-key-test`）**:
   - 実 TSF / Window / TextStore / メッセージポンプ環境にて全 8 シナリオ完走を確認：
     - Scenario 1: 未同意時の即時拒否・本文未読・原文維持
     - Scenario 2: 同意後の自律発見・Ctrl+J変換・DrainWindowでの非同期置換
     - Scenario 3: 1回Undoでの完全復元
     - Scenario 4: 通信失敗（HTTP 500）での原文維持・Undo履歴非汚染
     - Scenario 5: エラー解除操作なしでの即座の再試行成功・置換・Undo復元
     - Scenario 6: 同意取消時の即時拒否・本文未読・原文維持
     - Scenario 7: キー欠損時の即時拒否・本文未読・原文維持
     - Scenario 8: COM/TSF 登録解除・DLL アンロード
4. **ワークスペース全体検証**:
   - `cargo test --workspace --locked` (BypassSandbox: true) 全通過。
   - `cargo clippy --workspace --all-targets --locked -- -D warnings` (0 warnings)。
   - `cargo fmt --all --check` クリーン。
