# ADR 0051: 保護状態と許可が復元した後の旧結果拒否

2026-10-25 / Issue #4 / PR #10。

`broker-test`の実子Broker・認証済みIPC・Core・実DLL・所有TSF編集を通る
縦断検査に、`revoke-regranted`と`read-restored`を追加した。どちらも
変換要求を送った後、結果を適用する前に状態を変更する。

- `revoke-regranted`は許可を取り消して認可版を進め、再許可してさらに版を進める。
- `read-restored`はTSF storeを読取不能として`OnStatusChange(TS_SD_LOADING)`を
  通知し、その後読取可能に戻して`OnStatusChange(0)`を通知する。

両ケースとも旧結果を拒否し、本文とUndo履歴を変えない。最終状態が許可または
読取可能であっても、要求中に発生した失効の履歴を消してはならない。
`broker-test`は追加後21ケースとTSF/COM解除・DLLアンロードが成功した。

検査対象は専用の合成本文とプロセス内TSF登録である。一般アプリ、製品の
設定UI、恒久的なTSF登録を検証したものではない。
