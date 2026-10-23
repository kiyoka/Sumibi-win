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
load_smoke、self/edit/keyの基本検査が成功した（`target/issue-4-wait-limits-static.log`）。
小さい件数上限と本文上限を独立に拒否し、補助面文字を2単位として数え、
本文なし操作は件数だけを占有し、確認済み配送後だけ再受付することを確認した。

実DLLの`waiting-input-test`は初回`fifo`で固定変換が適用されず失敗した。
変更前の保存済みEXE/DLLでも同じ入力・期待値で失敗した。別の実行では
`window-test`と待機検査の開始時に前面化拒否`0x80070005`が起きた。
対話desktopで前面化後に待機検査の開始チェックを通過した実行も、
初回`fifo`が原文のままで失敗しており、原因は未特定である。
`waiting-input-test-interactive`も追加した。最初の専用ウィンドウで前面化が
拒否された時だけ最大30秒、所有STAのTSFメッセージを処理しながら利用者の
クリックを待つ。前面化できなければ明示的に失敗し、結果の成功には含めない。
今回の対話コマンド実行では前面化できず30秒後の失敗を確認したが、26ケース完走は未確認。
次回の失敗位置を絞るため、本文やキーを出さずに待機の停止理由、Brokerの状態、
救済結果区分をassertionに添える。
現時点で実DLL待機回帰を成功とは記録しない。
ログは`target/issue-4-wait-limits-{input,input-retry,baseline,window,explicit-foreground,interactive-waiting}.log`。
最終静的検査は`target/issue-4-wait-limits-static-interactive.log`。
対話desktopで再実行し、初回`fifo`で変換が適用されない原因を特定してから、
容量境界まで通ることを確認する必要がある。
一般アプリのキー配送・標準Undo・製品の全体予算も本変更の受入範囲外である。
