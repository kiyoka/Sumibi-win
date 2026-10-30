# ADR 0028: 記録範囲内のcaretからの同期Backspace

状態：Core・実DLL・専用ACPホストで実装・検証。Issue #4 / draft PR #10。
[ADR 0027](0027-tracked-caret-and-forward-delete.md)の内部Backspaceの制約を更新する。

後続の[ADR 0029](0029-tracked-internal-insertion.md)で内部挿入の追跡継続も実装した。
以下の内部挿入の残課題と件数は、このADRの実装段階の記録である。

## 決定

transaction-demoで記録範囲内を左右移動した後も、Backspaceで同じ入力runを継続する。
削除前後に残った記録文字を保持し、F7はその全体だけを変換する。
記録外の既存本文をGetTextして補わず、記録の開始と終端を硬い境界として扱う。

Coreのbackspace_rangeとforward_delete_rangeは、方向付きの有限削除計画を共有する。
Unicode 17.0.0のBoundariesを記録UTF-16から毎回作り直し、repeat全体が記録内に収まる場合だけ
対象範囲を返す。開始位置や過大countでは一部を先に消さない。前の削除で国旗などが再結合し、
caretが新しい書記素の内部にある場合は、その書記素全体を削除する。
計画は既存本文取得・heap確保・記録の消費を行わない。

deleted_backwardは、計画と一致する削除後caret、同じcontext/composition世代、増加した通知世代、
肯定的な保護状態を検査し、私有候補の該当部分だけを除く。空ならrunを退役する。
不正な完了観測では旧記録を残さない。旧Before/After観測用のbackspacedは末尾限定のままとする。
その経路には任意位置の削除を証明できるcountがないため、同期取引の完了と混同しない。

## DLL・救済・失敗

既存ISpikeInputEraser::Backspace(context, count, disposition)を再利用し、ABIを追加しない。
Tracker::eraseは前方Deleteと同じ処理で、古い自己compositionの全範囲とidentity、caret、
保護情報と通知世代を同期write内で再照合する。本文を一度だけ空文字へ置換し、caretと
同じcompositionの全範囲を同じcookieで更新する。全削除では自己compositionを終了する。
検証後にだけCoreの候補を公開する。

Recovery::Backspace(count)の既存共有予約を使い、削除本文を取得・保存・コピーしない。
共有4枠の満杯・記録端越え・書込前拒否は追跡を退役し、元のホスト操作全体へ一度だけ戻す。
その通常操作はホスト自身の本文で行う。DLLの記録外取得を許可する意味ではない。
SetText試行前からUNCERTAINとし、成功後はAPPLIED_UNTRACKED、全検証後だけAPPLIEDとする。
試行後の失敗では元のBackspaceを再実行せず、既存の停止・明示確認へ進む。rollbackもしない。

内部挿入・任意選択編集の追跡継続、一般アプリのadapter/保護情報/標準Undo、
診断sinkを使わない製品compositionの寿命とキー配送は引き続き未完了。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
Coreに5件追加して56件、workspace合計96件。内部削除の前後保持、repeatと記録端、
RI再結合、無効な完了観測の退役、最大1024単位の記録を途中から削除する境界を確認した。

backspace-input-testに18件追加し合計55件が成功した。内部削除・repeat・開始までの削除・全削除・
削除後のF7、開始境界とrepeat超過、slot不足、本文の変更前/変更後失敗、caret失敗、
status/scope/compartment変更後復元、private/readonly/他composition、RI再結合を扱う。
同一compositionと下線の全範囲、同じwrite cookie、1回Undo/Redoでの本文とcaret復元、
書込試行後の非再操作、救済の操作情報とコピー不可を確認する。F7の正常な原文取得を除き、
DLLのadapter本文取得counterは増えない。

recovery-input-testは共有4枠満杯での追跡済み内部Backspaceを加え38件。
実DLLの挿入と移動で記録を作ってから他所有者の4枠を満たし、DLL SetTextなし、元のホスト削除1回、
1回Undo/Redo、既存ticketと本文取得counterの不変を確認した。
同期入力68・書記素ホスト22・composition/初回選択46・通知30・追跡25・broker入力11・close4も成功。
Test-Spike.ps1 -SkipDesktopでRust96件・fmt・Clippy 3構成・build・load_smoke・self/edit/keyが成功した。
成功したnative検査はTSF/COM解除とDLLアンロードも確認した。

ログはtarget/issue-4-internal-backspace-static.log、同native.log、同<検査名>.log（すべてtarget/配下）。
追加fixtureはEditor helper/診断ABIを直接呼ぶ専用storeの検査で、物理打鍵や一般アプリの互換を証明しない。
前面化で未通過のwindow/input/delayedの3検査は維持し、省略なし全回帰の成功には読み替えない。
