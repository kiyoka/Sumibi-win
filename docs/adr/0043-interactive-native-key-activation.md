# ADR 0043: 前面化が拒否された実TSFキー検証を対話操作で続ける

状態：診断コマンドを追加し、対話desktopで実TSFキー検査が成功。Issue #4 / draft PR #10。

## 問題

`window-test`は実TSFの通常キー配送を調べる前に、診断ウィンドウが前面にあることを
確認する。この環境ではWindowsが`SetForegroundWindow`を拒否し、文書関連付け・下線・
専用ホストUndoの確認後に`0x80070005`で停止した。DLL callbackを直接呼ぶ検査だけでは、
実際のTSFキーマネージャー配送の受入れにはならない。

## 決定

既存の`window-test`は変更せず、`window-test-interactive`を追加する。
前面化が拒否された時だけ診断windowのtitleと標準出力でクリックを案内し、
最大30秒、所有STAのTSF message pumpを回しながら前面化を待つ。
利用者が診断windowをクリックして前面化した場合に限り、既存の
`window-test`と同じ`ITfKeystrokeMgr::TestKeyDown/KeyDown`検証を続ける。
期限切れやwindow終了では失敗を返し、通常のCOM/TSF解除とDLLアンロードを行う。
クリックの代行、他アプリのフォーカス操作、前面化条件の迂回は行わない。

## 検証と限界

CLIとspike build、Rust workspace136件・Clippy 3構成・fmtを確認した。
既存の`key-test`は実DLLで成功した。画面操作できない実行環境で新コマンドを起動すると、
文書関連付け・下線・取消・専用ホストUndoまで通り、30秒後に専用の前面化失敗を返した。
cleanupの失敗やDLL参照残りは検出されなかった。
ログは`target/issue-4-interactive-window-*.log`。
2026-10-23、利用者が診断windowのtitle barをクリックして前面化した後の対話desktopで
`window-test-interactive`を再実行し、終了コード0を確認した。Esc/Enterは
`ITfKeystrokeMgr::TestKeyDown/KeyDown`を通り、focus通知、5状態の描画、
COM/TSF登録解除、DLLアンロードも成功した。続く通常の`window-test`、
`input-test`、`delayed-test`もそれぞれ終了コード0で成功した。
ログは`target/issue-4-user-click-window-test.log`と
`target/issue-4-user-active-{window,input,delayed}-test.log`。
これらは専用診断ホストの自動検査であり、物理キーボードからの打鍵、
Notepadなど一般アプリの入力、標準Undoの受入れではない。
同日、専用`transaction-demo`で利用者が実際に`nihongo`を打鍵し、F7で「日本語」、
Ctrl+Zで`nihongo`へ戻ることと、×で閉じられることを利用者が画面で確認した。
プロセス終了と標準エラー出力が空であることも確認した。デモは操作・本文をログに記録しないため、
文字表示の結果は利用者の報告による。Shift選択やIME切替、一般アプリのキー配送・標準Undoは未確認。
起動時ログは`target/issue-4-manual-transaction-demo.log`。
