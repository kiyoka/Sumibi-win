# ADR 0094: Host 障害復旧（Self-Healing 再接続）の検証と一般アプリ TSF 保護診断の強化

2026-11-03 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) / [draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
状態: 承認済みの耐障害性および TSF 保護診断の機能強化。

## 状況

[Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) の受入条件には以下が含まれている：
1. **Host 寿命と障害回復**: 別プロセスとして稼働する常駐 Host の停止や再起動に対し、TIP が安全に切断を検知し、Host 復帰時に自律的に再接続して Ready 状態を回復すること。
2. **一般アプリでの保護条件と Fail-Closed**: Notepad 等の一般アプリにおいて、機密文書や未保護のコントロールに対して不用意に本文を取得・送信しないこと（Fail-Closed）。また、拒絶された理由を安全に観測できること。
3. **Windows 標準コントロールの拒絶契約の継続的検証**: Windows 標準の RichEdit（`Msftedit.dll`）および EDIT コントロールが、保護情報不足（`HiddenTextPossible` 等）により確実に拒絶される契約を自動回帰テストとして維持すること。

これまで、専用 TSF テキストストアでの自律変換や 1-step Undo は `session-key-test` で実証されていたが、Host の再起動シナリオに対する決定論的なテスト検証、一般アプリ内での TSF コンテキスト状態（`dwStaticFlags` やコンパートメント）のリアルタイム診断ログ、および `native-edit-test` の統合ランナーへの組み込みが未整備であった。

## 決定

### 1. Host 切断・再起動からの Self-Healing 自律再接続検証

- `crates/sumibi-tip/src/session.rs` に結合テスト `session_reconnects_after_host_restart_and_recovers_ready` を追加した。
- 以下のライフサイクルを決定論的に実証した：
  1. 第1の Host インスタンス（`host_service1`）を起動し、同意を付与。
  2. TIP セッション（`TipHostSession`）がログオン限定パイプから自律発見・接続し、Ready 状態（`is_ready() == true`）を達成。
  3. `host_service1.stop()` により Host を明示停止（プロセスクラッシュや手動終了を模擬）。
  4. TIP セッションのバックグラウンドワーカーが切断を即座に検知し、`is_ready() == false` / `is_connected() == false` の未接続・未同意（Fail-Closed）へ自律遷移。
  5. 第2の Host インスタンス（`host_service2`）を再起動し、同意を付与。
  6. TIP セッションがバックオフ再試行ループを通じて Host #2 を自動再発見・ハンドオフ接続し、再び Ready 状態（`is_ready() == true`）に回復することを確認。
- 同一ログオンセッション内でのテスト間パイプ競合を防ぐため、プロセス内グローバル同期ロック `HOST_TEST_LOCK` を導入した。

### 2. 一般アプリ向け非機密 TSF 保護診断の強化

- `crates/sumibi-tip/src/session.rs` の `on_preserved_key`（変換ホットキー Ctrl+J 押下時）において、`ManualEdit::capture` を試行する直前に、コンテキストメタデータを安全に照会して `OutputDebugStringW` へ出力する非機密診断ログを追加した：
  - `context.GetStatus()` による `dwDynamicFlags` および `dwStaticFlags` の生の値。
  - `input_status_denial(status)` による拒絶分類（`HiddenTextPossible`, `TransitoryContext`, `ReadOnly` 等）。
  - `GUID_COMPARTMENT_KEYBOARD_DISABLED` および `GUID_COMPARTMENT_EMPTYCONTEXT` のコンパートメント値。
- **安全性**: 一切の入力本文・ドキュメント文字列を含まず、固定の識別子・16進数フラグ・列挙型名のみを出力するため、外部監視ツール（DebugView 等）から機密情報を漏洩させることなく Fail-Closed 判定の根拠をリアルタイムで観測できる。

### 3. 統合開発ランナーへの `native-edit` タスク追加と回帰テスト自動化

- `tools/dev.ps1`、`dev.ps1`、および `Makefile` に `native-edit` タスクを新設した。
- `powershell -ExecutionPolicy Bypass -File .\dev.ps1 native-edit` により、Windows 標準の RichEdit（`RICHEDIT50W`）に対する 7 つのシナリオ（`plain-text`, `rich-text`, `private`, `default-scope`, `password`, `readonly`, `readonly-restored`）および EDIT コントロールに対する拒絶契約をワンコマンドで検証可能とした。
- `test-all` の中にも組み込み、全体回帰テストの標準ステップとして自動検証を保証した。

## 影響と検証

- **耐障害性の確立**: 常駐 Host がアップデートや再起動を行っても、一般アプリ内の TIP はクラッシュすることなく自律復旧することが実証された。
- **診断可能性の向上**: Notepad（Windows 11 の RichEditBox 基盤等）で Ctrl+J を押した際、`dwStaticFlags = 0x2`（`TS_SS_REGIONS`）により `HiddenTextPossible`（`0x80070005: E_ACCESSDENIED`）として安全に拒絶された事実を客観的・数学的に観測できる。
- **全体検証の全件合格**:
  - `dev.ps1 test-all`（533 passed, 0 failed, 4 ignored）全件合格。
  - `Test-HostImage.ps1`、`Test-SuspendedImageProbe.ps1` 全件合格。
  - `session-key-test`（全 5 シナリオ完全合格）。
  - `native-edit-test`（全 7 拒絶シナリオ完全合格）。
  - `dev.ps1 check`（警告・フォーマット差分ゼロ）。
