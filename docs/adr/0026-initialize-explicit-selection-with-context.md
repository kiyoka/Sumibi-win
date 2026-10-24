# ADR 0026: contextを指定した初回選択変換

状態：診断DLLと専用ACPホストで実装・検証。一般アプリの入力adapterの完成ではない。
2026-10-22。Issue #4 / draft PR #10。ADR 0009・0010・0025の後続。

## 問題と決定

composing/transactionホストのF7は、文字入力でTrackerが作成される前には既存の選択範囲を変換できなかった。
context引数のない旧PrepareConversionは、初期化済みTrackerだけを操作する契約だからである。
初期化のための架空の文字入力や空のInsertを行わず、独立したISpikeInputPreparer::Prepare(context)を追加する。
既存Input/Monitor/Writer/Eraser ABIのvtableは変更しない。

ホストは自分のcontextを渡し、Controlの既存in_context経路でowner STA・再入・Controller寿命・
COM identityを検査する。必要ならTrackerと通知watchを作ってから既存のprepareへ接続する。
contextを切り替える場合は旧Trackerの取消を先に完了する。workerにCOMやDLL callbackを渡さない。
未確認書込によるControlのwrite_blocked、ホストのuncertain/owner boundaryによる停止も維持する。

Prepareは本文を取得しない。入力runのないcollapsed caretでは拒否し、既存全文を推測しない。
非空の明示選択または有効な記録runだけを対象にし、1024 UTF-16単位を上限とする。
focused contextと肯定的なscope/status/compartment、他compositionの拒否は従来のgateを共用する。
unknown・private・readonlyや同期lock拒否から、通常欄や全文へ切り替えない。

## 選択と寿命

明示選択ではSetSelectionを省き、範囲とactive endの向きをそのまま維持する。
自動選択が必要なのはcollapsed caretから有効な記録runを取り出す場合だけとする。
必要な旧自己composition/下線のcleanupは同期writeで行い、選択・保護を再照合してReadyを公開する。
準備前後の保護境界世代も比較し、観測した状態変更を復元後の現在値だけで許可しない。

新しい明示選択ではcompositionを開始しないため、診断EXEの共有composition sinkがなくても準備できる。
これは文字入力compositionのsink寿命を解決した意味ではない。
Readyは現在の通知世代に結び付け、CheckSourceで一度だけ消費する。
選択・本文・保護情報が変更後に復元された場合も、古いReadyを復活させない。
実際の本文取得、Brokerの容量予約、送信前照合、同期適用と原文再照合は既存経路で行う。
準備の初期化をAPI送信や利用者同意の代替にしない。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
composing-input-testへ初回選択の17ケースを追加し、既存29件と合計46件が成功した。

- 初回F7の成功、逆向き選択の保持、最大1024単位、診断composition sink欠損での成功。
- 記録なしのcollapsed caret、private、readonly、lock拒否、他composition、null contextの拒否。
- 取消、待機中の通常ホスト編集、保護状態の変更後復元で旧結果を拒否。
- 準備後の選択・保護・同長本文の変更後復元を拒否し、Readyの二度目の使用も拒否。

初回fixtureは文字入力やBeforeInputを呼ばず、専用storeへ合成本文と選択を設定する。
Prepare単体と拒否ケースではadapter本文取得・DLL SetText・Undoを増やさない。
成功時は選択範囲だけを固定結果へ置換し、1回Undoで本文・選択・向きが戻り、Redoで再適用できる。
取消/失効後の結果配送では本文を再取得・変更しない。
計測はDLLのadapter counterであり、MSCTF内部の全照会をゼロとする主張ではない。

ADR 0025の保留Delete fixtureも事前の実入力を取り除き、既存選択のF7→Delete→旧結果拒否へ更新した。
同期入力42・Backspace37・書記素ホスト22・救済35・通知30・追跡25・broker入力11・close4が成功した。
Test-Spike.ps1 -SkipDesktopも成功し、Rust83件・fmt・各構成Clippy・build・load_smoke・self/edit/keyを確認した。
成功したnative検査はTSF/COM解除とDLLアンロードを確認している。

再現ログはtarget/issue-4-fresh-selection-before.log。
最終ログはtarget/issue-4-fresh-selection-static.logとfresh-selection-<検査名>.log（すべてtarget/配下）。
検査は専用ホストからEditor.input/診断ABIを呼び、実TSF・Broker worker・正式な通知を検証する。
物理キーと通常TSFキー配送の受入には読み替えず、前面化で未通過のwindow/input/delayedの3検査を維持する。
一般アプリの保護情報・非ACP互換・標準Undo、文字入力compositionの製品sink寿命、
選択のないDLL前方削除・任意位置の追跡継続・製品配置/認証・待機行列は残る。
