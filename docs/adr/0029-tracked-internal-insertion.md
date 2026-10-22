# ADR 0029: 記録範囲内のcaretへの同期挿入

状態：Core・実DLL・専用ACPホストで実装・検証。Issue #4 / draft PR #10。
[ADR 0027](0027-tracked-caret-and-forward-delete.md)と[ADR 0028](0028-tracked-internal-backspace.md)の後続。

## 決定

transaction-demoで記録範囲内を左右移動した後の文字挿入も、同じ入力runとcompositionで継続する。
従来は内部への挿入をDLL書込前に拒否し、元のホスト入力を一度反映して追跡を退役していた。
現在は挿入前後の記録を保持し、内部caretからのF7も更新後の記録全体だけを変換する。
記録外の本文取得・推測、任意選択の置換追跡、一般アプリへのキー配送は追加しない。

Coreにinserted_at_caretを追加する。既存の正確な観測とcollapsed caretを要求し、同じcontextと
composition世代、増加した通知世代、供給文字列のUTF-16と挿入後caretを照合する。
記録内のscalar境界だけに挿入し、surrogate内部は許可しない。以前の削除で再結合した書記素の
内部caretには挿入でき、次の移動/削除計画で書記素を分割し直す。
既存記録と供給文字列の合計が1024 UTF-16単位を超える場合は候補を作らない。
新しいrunを作るAPIではなく、不正な完了観測では旧runを退役する。

供給文字列の共通検査をvalid_insertionへまとめる。既存のinsertedはホスト編集後の観測用として
末尾への連続追加だけを維持する。途中入力の観測から既存runを無条件に継続する変更にはしない。
同期取引は本文反映前に私有候補を作り、全検証後にだけ公開する。

## DLL・救済・失敗

既存ISpikeInputWriter::Insertとreplace/write_bodyを再利用し、ABIは変更しない。
既存compositionがある場合は、記録全体とTrackerのextentを照合し、候補の終端が供給長だけ増えることを検査する。
同じ同期write内で自己compositionの全範囲・identity、caret、保護条件、表示属性と通知世代を再照合する。
既存範囲の保護検査も書込前に行い、途中挿入のために保護条件を緩和しない。
SetTextは挿入位置だけに一度実行し、caret・下線・同じcompositionの終端を同じcookieで更新する。
本体を読み直して合成する方式ではない。

共有Recoveryの既存Operation::Insert(len)を使う。予約できなければDLLを呼ばず追跡を退役し、
元のホスト入力を一度だけ扱う。1024単位超過も書込前に拒否する。
SetText試行前からUNCERTAIN、成功後はAPPLIED_UNTRACKED、cookie後の検証を含めて成功した場合だけAPPLIED。
試行後の失敗では自動再入力もrollbackも行わず、F7を停止し、明示確認へ進む。
救済は供給された新しい断片だけを保持でき、既存の記録前後を追加しない。保護履歴が変わった場合は断片も除去する。

任意選択編集の追跡継続、一般アプリのadapter/保護情報/通知/標準Undo、診断sinkを使わない
製品compositionの寿命、製品の待機キー順序と常駐Broker・配置/認証は引き続き残る。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
Coreに6件追加して62件、workspace合計102件。先頭/途中/末尾への挿入、結合文字後の削除、
再結合した国旗を分離する挿入、不正な観測と供給UTF-16、合計容量の境界を検査した。
従来の途中挿入観測が新しい供給文字だけを記録する検査も維持する。

transaction-input-testへ20件を追加し88件。既存の内部挿入fallback検査も追跡継続へ更新した。
先頭/途中/複合文字/WM_CHAR repeat、F7、結合文字挿入後の実DLL Backspace、国旗再結合後の挿入、
最大容量と超過、slot不足、readonly/private/他composition/lock拒否、本文変更前/後・caret失敗、
status/scope/compartmentの変更後復元を扱う。同一compositionと全範囲、同じwrite cookie、
1回Undo/Redo、本文取得不変、試行後の非再入力とF7拒否を確認した。

最大長の初期fixtureでは記録外のprefix/suffixも付けて専用入力欄の上限を超え、試験が停止した。
空の入力欄で最大長を作る形へ修正した。さらに固定幅の描画器は長い下線を描画できないため、
最大長では実際のTSF compositionのidentity/全範囲とUndo/Redoを直接検査する。
通常サイズでは下線表示も検査する。最大長の描画成功へ読み替えず、描画器の条件は緩和しない。
超過の試験は直接ABIでDLL本文書込なしを検査する。専用ホスト自体の1024単位上限も維持する。

recovery-input-testへ2件追加し40件。実DLLで途中caretを作った後に共有4枠を満たし、
DLL SetTextなしで元の文字入力1回、1回Undo/Redo、既存ticket不変を確認した。
変更後に失敗する途中挿入では、救済payloadと明示コピーが供給した「z」だけであることを確認する。
Clipboardは試験用メモリーへ差し替え、コピー時の本文/Undo/adapter本文取得の不変も検査した。

Backspace55・書記素ホスト22・composition/初回選択46・通知30・追跡25・broker入力11・close4も成功。
Test-Spike.ps1 -SkipDesktopでRust102件・fmt・Clippy 3構成・build・load_smoke・self/edit/keyが成功した。
成功したnative検査はTSF/COM解除とDLLアンロードも確認した。
ログはtarget/issue-4-internal-insertion-static.logとinternal-insertion-<検査名>.log（すべてtarget/配下）。

追加fixtureはEditor helper/診断ABIを直接呼ぶ専用storeの検査で、物理打鍵や一般アプリの互換を証明しない。
前面化で未通過のwindow/input/delayedの3検査は維持し、省略なし全回帰の成功には読み替えない。
