# ADR 0044: 待機入力行列の件数と本文量を有限に設定する

状態：Coreと専用waiting adapterへ接続。Issue #4 / draft PR #10。

## 問題

Coreの待機行列は最大1024イベント・16384 UTF-16単位に固定されていた。
専用adapterは別途、共有救済4枠・4096単位を消費前に予約していたが、
Core自身の受付上限はその診断経路の容量を表していなかった。
製品の全対象にまたがる容量を決める際にも、固定最大値を変更するしかなかった。

## 決定

`waiting_input::Limits::new(max_events, max_text_units)`で、1〜1024件・
1〜16384 UTF-16単位の有限な上限を作る。既存の`WaitingInput::new`は従来の
最大値を維持し、`with_limits`だけが指定値を使う。本文のないBackspace・Enter・
Convertも件数を占める。UTF-16本文は受付済みの行列だけを数え、確認済みで
先頭を除いた後に枠を再利用する。容量拒否は既存の入力やticketを変えない。

専用waiting adapterは、既存の共有救済予算と同じ4件・4096単位を
Coreへ明示的に渡す。これまでどおり共有救済枠を先に予約し、成功後にだけ
元の入力を消費できる。Coreの上限は共有予算の代替ではなく、既に救済へ移った
レコードや別所有者・別プロセスを数えない。デモの8秒待機・10分救済も維持する。
製品の容量値、予算の所有者、プロセス間の予約と超過時のキー動作は未決。

## 検証と限界

Coreへ1件を追加し、workspace Rust137件、fmt、Clippy 3構成、spike build、
load_smoke、self/edit/keyの基本検査が成功した（`target/issue-4-normal-waiting-diagnostic-static.log`）。
小さい件数上限と本文上限を独立に拒否し、補助面文字を2単位として数え、
本文なし操作は件数だけを占有し、確認済み配送後だけ再受付することを確認した。

最初の通常テストは`fifo`で原文のまま失敗し、変更前の保存済みEXE/DLLでも再現した。
前面化を確認しても失敗したため、入力本文・キーを記録せず、待機の停止理由、
Broker状態、救済結果区分を検査失敗へ添えた。2件ともDeferredで配送前に停止し、
Broker結果を確認すると`UnsupportedAppContainer`だった。診断CLIを起動した
AppContainer環境は既存の相互認証が明示的に拒否する。前面化拒否`0x80070005`は
別の条件で、変換失敗の原因ではなかった。

Bridge生成時に`PinnedProcess::current()`で同じmedium・非昇格・非AppContainer条件を
先に検査し、非対応環境では入力を受付ける前に`desktop IPC: UnsupportedAppContainer`
として失敗する。IPC認証を緩めず、未適用入力を成功に読み替えない。
`waiting-input-test-interactive`と`waiting-demo-test-interactive`は、最初のwindowが
500ms連続で前面の状態になるまで、最大30秒、所有STAのTSF message pumpを回す。
Windowsが前面化を拒否した場合は利用者がwindowをクリックでき、時間切れは失敗する。

通常ユーザーのmedium・非AppContainer対話desktopで、直前版の実DLL
`waiting-input-test-interactive`26ケースと`waiting-demo-test-interactive`59ケースが成功し、
TSF/COMの登録解除とDLLアンロードまで確認した。ログは
`target/issue-4-waiting-stable-foreground-input.log`と
`target/issue-4-waiting-stable-foreground-demo.log`。通常の`waiting-demo-test`は
同じ権限でも初回`fifo`の成否が変動した。対話版で最初のwindowの前面状態が
安定するのを待って再確認したが、通常版の変動理由は未特定である。
通常版の初回windowも、所有STAのmessageを処理しながら500msの連続前面状態を
最大2秒待ち、成立しなければ変換前に明示して失敗するようにした。
待機デモのassertionには、前面状態・所有境界・待機停止理由・Brokerの失敗区分を
本文やキーなしで添えた。非前面での失敗は確認したが、この変更後の通常版59ケース
完走は対話desktopで未確認である（`target/issue-4-normal-demo-stabilized.log`）。
今回の対話版再実行も前面状態が成立せず30秒で停止した
（`target/issue-4-normal-waiting-diagnostic-interactive.log`）。
2026-10-24に通常ユーザーの対話desktopで再検証し、現版の通常
`waiting-demo-test`59ケース、対話版59ケース、通常`waiting-input-test`26ケースが
すべて成功した。各コマンドでTSF/COM登録解除とDLLアンロードも成功した。
これは上記の未確認状態を解消する実行記録であり、一般アプリ・物理キーの受入ではない。
初期失敗の観測ログは
`target/issue-4-waiting-interactive-diagnostic*.log`。
一般アプリのキー配送・標準Undo・製品の全体予算も本変更の受入範囲外である。
