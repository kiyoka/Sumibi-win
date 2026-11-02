# ADR 0089: 自律セッション（TipHostSession）によるエンドツーエンドTSF変換検証

## 状況

[ADR 0087](0087-tip-host-session-lifecycle.md) において、一般アプリ内 TIP が常駐 Host を自律発見・接続維持し、Fail-Closed にホットキーを結合する `TipHostSession` を導入した。
また、[ADR 0088](0088-sta-async-drain-window.md) において、TSF 所有 STA スレッドへのメッセージ専用ウィンドウ（`DrainWindow` / `WM_SUMIBI_DRAIN`）による非同期完了通知とテキスト自動適用（`DrainSink::drain`）を導入した。

これまでの `sumibi-setup` の診断コマンド（`broker-key-test` など）は、テスト用 spike インターフェース（`ISpikeBroker` / `ISpikeControl`）を介して直接 Bridge を Attach する方式を用いていた。
しかし、製品版の一般アプリ環境では、テスト用ハーネスによる明示的なコンテキスト許可（`GrantContext`）や Bridge Attach は存在せず、実 DLL の `TipHostSession` が自律的に Host を発見・認証・接続し、ホットキー押下時に TSF 編集セッション（`ManualEdit`）を通じてテキストを取得・送信し、非同期完了時に STA スレッドで置換適用する必要がある。

実アプリ（Notepad 等）への恒久登録に進む前に、専用 TSF テキストストア環境において、外部の独立常駐 Host（`HostService` / `broker_discovery_fixture`）と実 DLL 内の `TipHostSession` を組み合わせたエンドツーエンドの自律変換・Undo・同意失効ライフサイクルを検証する自動テストおよび診断コマンドが必要である。

## 決定

1. **`crates/sumibi-tip/src/keys.rs` のディスパッチ調停**:
   - `feature = "spike"` ビルドにおいて、`self.controller` が明示的 grant を持っている場合（`controller.has_grant(context) == true`）にのみ `controller.convert(context)` を呼び出す。
   - 明示的 grant を持たない通常コンテキスト（一般アプリや自律セッション検証）では、`self.session.upgrade()` による自律セッションディスパッチ（`session.on_preserved_key`）を実行する。
   - `not(feature = "spike")` の製品ビルドでは、従来通り `session.on_preserved_key` のみを直接呼び出す。
   - これにより、既存のすべての spike テストの互換性を 100% 維持しつつ、自律セッションのエンドツーエンド検証を可能にする。

2. **重複ホットキー登録の防御（`TF_E_ALREADY_EXISTS`）**:
   - `TipHostSession::register_preserved_key` および `Controller::grant` において、`keys.PreserveKey` の戻り値が `TF_E_ALREADY_EXISTS` (0x80040205) の場合は無害な既登録として許容し、エラーとしない。

3. **エンドツーエンド自律変換検証（`session-key-test`）の実装**:
   - `crates/sumibi-setup/src/session_keys.rs` を新設し、以下の一連のシナリオを単一のテストシーケンスとして実行する：
     1. **未同意時の Fail-Closed**: Host 未同意状態（`allowed == false`）でホットキー（Ctrl+J: `COMMAND_CONVERT`）を押下しても即時拒否され、テキストストアの読み取り（`reads`）が一切発生しないことを検証。
     2. **自律接続と Ready 遷移**: Host に同意（`try_grant()`）を与えると、TIP のバックグラウンドワーカーが自律的にログオン限定パイプを発見・ハンドオフ・接続し、`is_ready() == true` に遷移することを待機・検証。
     3. **キー変換・非同期 STA ドレイン・テキスト置換**: ホットキー（Ctrl+J）を押下すると変換が受付（`Ok(TRUE)`）され、Host への非同期送信・変換結果（"nihongo" → "日本語"）の受領、`WM_SUMIBI_DRAIN` による STA スレッド起床、`sink.drain()` によるテキストストアの自動置換（"prefix 日本語 suffix"）が完了することを検証。
     4. **1回 Undo**: 置換後のテキストストアに対して Undo を実行し、元の "prefix nihongo suffix" および選択範囲が完全復元されることを検証。
     5. **同意取消時の即時失効**: Host の同意を取り消し（`try_revoke()`）、TIP セッションが即座に `!is_ready()` に遷移し、以降のホットキー押下で再びテキストが読まれず即時拒否されることを検証。
     6. **安全なクリーンアップと DLL 解放**: TSF Deactivate 後、`TipHostSession` の安全な終了、`DrainWindow` の破棄、登録解除、および `DllCanUnloadNow == S_OK`（未解放 COM オブジェクトゼロ）を検証。

4. **CLI コマンド `session-key-test` の公開**:
   - `sumibi-setup session-key-test --dll <sumibi_tip.dll>` を追加し、実機環境で単独実行可能にする。

## 影響・境界

- **セキュリティ**:
  - PID・パイプ名単独の信頼は行わず、カーネルプロセスハンドルと生成時刻による相互ピア認証を維持。
  - 送信未同意時や未接続時は入力本文を一切取得・送信しない（Fail-Closed）。
  - 合成本文（"nihongo" → "日本語"）のみを使用し、機密情報の漏洩を防ぐ。
- **スレッド・アパートメント安全性**:
  - TSF 所有 STA スレッドではいかなるブロッキング I/O も行わず、バックグラウンドワーカーと `DrainWindow`（`WM_SUMIBI_DRAIN`）の非同期メッセージ通知のみで通信。
- **後方互換性**:
  - 既存のすべての spike テスト（`broker-key-test`、`broker-http-key-test`、`input-test` 等）は変更なく全件合格を維持。

## 検証結果

- `powershell -ExecutionPolicy Bypass -File .\dev.ps1 session-test`:
  - シナリオ 1（未同意 Fail-Closed: ホットキー拒否、本文読取 0 件）: OK
  - シナリオ 2（自律発見・接続、ホットキー受付、バックグラウンド変換 "nihongo" → "日本語"、`WM_SUMIBI_DRAIN` による STA 非同期置換）: OK
  - シナリオ 3（TextStore 1-step Undo による原文復元）: OK
  - シナリオ 4（同意失効時の即時 Fail-Closed）: OK
  - シナリオ 5（終了処理、COM/TSF プロファイルの登録解除と `DllCanUnloadNow`）: OK
- 全テストスイート（`powershell -ExecutionPolicy Bypass -File .\dev.ps1 test-all`）: **全件合格**。
