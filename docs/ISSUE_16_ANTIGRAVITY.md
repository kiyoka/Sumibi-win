# Antigravityチャット欄のTransitory状態 (Issue #16)

2026-11-06。修正版DLL (PR #20) がロードされたAntigravityのチャット欄で、ユーザーによる通常打鍵とCtrl+Jを観測した。

- Host接続・送信許可: true
- TSF状態: dynamic=0x40, static=0x0c
- キーボード無効・空コンテキスト: いずれも0
- 旧判定: TransitoryContext、打鍵記録を破棄し、captureをE_ACCESSDENIEDで拒否

[ChromiumのGetStatus実装](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/ui/base/ime/win/tsf_text_store.cc) は、通常のストアにもTRANSITORYとNOHIDDENTEXTを設定する。[MicrosoftのTS_STATUS定義](https://learn.microsoft.com/en-us/windows/win32/api/textstor/ns-textstor-ts_status) でTRANSITORYが示すのは短い使用周期であり、合成中やパスワード欄を直接意味しない。

Standardに限り、この組み合わせを状態検査で許容する。NOHIDDENTEXTのない短命コンテキスト、Strict、読取専用、ロード中、未知フラグ、不連続選択、機密スコープは引き続き拒否する。既存の合成・コンテキスト・フォーカス・選択・原文一致検査と、非同期結果適用直前の再検証は変更しない。状態フラグの変化も既存のStamp比較に含まれる。

診断の拒否理由は実際のプロファイルで評価する。打鍵記録の不一致診断には本文を含めない。

この修正は状態ゲートの一つを解消する。Chromiumの範囲操作やUndoの成立、他の入力欄・アプリの互換性まで実機確認済みとはしない。再テストでは通常打鍵→Ctrl+J、明示選択→Ctrl+J、Ctrl+Z、フォーカス変更時の結果破棄を確認する。

## 原文が残る問題と範囲保持

同日19時台の再検証では、状態ゲート修正後に要求が送信されるようになったが、
ユーザーから `nihongo` → Ctrl+J の結果が `nihongo日本語` になるとの報告があった。
DLL が修正版で、`denial=None`、要求の dispatch まで進んだことも診断で確認した。

Chromium の `TSFTextStore::RequestLock` は、入力サービスが IME の場合、
`SetSelection` だけの変更を renderer の `SetEditableSelectionRange` へ転送しない。
従来は非同期変換の前に `select_preceding_run` で内部選択を変えていたため、
TSF 内部の選択とアプリ側のカーソル位置がずれたまま結果を挿入する経路になっていた。

製品の手動変換は `capture_selection_or_run` に変更した。明示選択があれば優先し、
カーソルだけの場合は、打鍵記録と完全一致する直前範囲を読み取りセッション内で保持する。
その際に `SetSelection` を呼ばず、実際の選択／カーソルと置換対象の範囲を別に記録する。
送信前は選択と保護条件を再検証し、結果の適用直前は原文の一致も再検証する。
フォーカス、通知、設定・同意、期限の既存ガードは維持する。

専用 TSF ストアの `edit-test` に、カーソルを保った部分置換、原文不一致の拒否、
カーソル移動後の拒否を追加した。28 件の編集ケース、3 件の属性ケース、
8 件のプレビューケースが成功。CI でもこの実 TSF 検査を実行する。

`0.1.0-antigravity-range-fix` をこの PC に導入し、Antigravity を再起動して
送信許可を再付与したところ、ユーザーがチャット欄で
`nihongo` → Ctrl+J → **`日本語` だけになる**ことを確認した。
他アプリの互換性や全操作の成立をこの結果だけで保証するものではない。

追加の実機確認では、マウスで `nihongo` の7文字を選択して Ctrl+J を押した場合も
`日本語` だけになることをユーザーが確認した。
ただし通常変換後の Ctrl+Z は **1回目で空欄、2回目で `nihongo` に戻る**。
Chromium の `CommitTextAndEndCompositionIfAny` が原文の削除と変換結果の挿入を
別操作で行う経路と整合する。1回で原文へ戻す Undo は未解決であり、
合成・編集トランザクションの扱いについて追加対応が必要。
この制限を残して Issue #16 全体を完了扱いにはしない。
