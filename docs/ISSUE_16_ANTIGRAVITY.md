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
