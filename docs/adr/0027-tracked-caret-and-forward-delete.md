# ADR 0027: 記録範囲内のcaret移動と前方Delete

状態：Core・診断DLL・専用ACPホストで実装・検証。一般アプリの任意位置編集の完成ではない。
Issue #4 / draft PR #10。ADR 0013・0024〜0026の後続。

## 決定

transaction-demoの通常の左右移動を、実際に挿入して記録した範囲内なら追跡を保って処理する。
そのcaretからの前方Deleteも同期writeへ接続する。従来は全移動で記録を失効させ、
選択のないDeleteは常にホストへ戻していた。既存本文をGetTextして範囲を推測する方式にはしない。

CoreのRunは記録終端と現在のcaretを分け、終端はstartと記録UTF-16長から求める。
range/takeは現在のcaret位置にかかわらず記録全体を返す。
move_rangeは記録したUTF-16だけからUnicode 17.0.0の書記素移動を計画し、movedは
同じcontext/保護/世代と移動後の書記素境界を検査する。未検証の観測はrunを退役する。

Boundaries.move_withinは左右の硬い記録境界を越えるrepeatを全体として拒否する。
既存ホストのmove_caretとは異なり、記録端へclampして残りのキー回数を失わない。
forward_delete_range/deleted_forwardも同じ有限範囲を使い、指定した個数の書記素だけを削除する。
空になったrunは退役する。隣接した文字が新しい書記素になる場合は次の計画で再分割する。
削除後のcaretが新しい書記素の内部に入った場合、次の移動はその方向の端へ進み、
次のDeleteは含まれる書記素全体を対象にする。既存ホストの内部caret規則と合わせる。

## DLLとホスト

独立ISpikeTrackedEditor::Edit(context, action, count, disposition)を追加する。
actionは0=左、1=右、2=前方削除、countは1〜1024。既存ABIのvtableは変更しない。
owner STA・再入禁止・Controller/DLL寿命・停止状態は既存Controlと共通にする。
Coreの候補は実処理が検証できるまで公開しない。

| 操作 | 同期処理 |
| --- | --- |
| 左右移動 | 自己compositionのidentity/全範囲、caret、保護情報と通知世代を再照合し、SetSelectionだけを行う。本文/Undoは変更しない |
| 前方Delete | Backspaceと共通のerase/write_bodyで対象だけを空文字へ置換し、caretと同じcompositionの範囲を更新。全削除なら終了する |
| F7 | 内部caretからも現在の記録全体を変換元にする。prefix/suffixを含めず、送信前に記録と実原文を再照合する |

移動でもSetSelectionの直前にUNCERTAINへ進める。変更した後に失敗を返すstoreがあるため、
HRESULT失敗からホストの元の移動を再実行してよいとは判断しない。
SetSelection/SetText成功後はAPPLIED_UNTRACKED、cookie後の通知・保護検査まで成功した場合だけAPPLIEDにする。
試行後の失敗は変換を止め、確認へ進む。rollback・自動再移動・再削除は行わない。

ホストは操作前に共有Recovery枠を予約する。MoveCaretとForwardDeleteは操作情報だけを保持し、
本文用bufferやコピー可能なpayloadを持たない。満杯なら古い記録を退役して元のホスト操作へ一度だけ返す。
境界を越えるrepeat・未知の追跡・保護不成立も、書込/移動前のNOT_ATTEMPTEDだけをfallbackの許可に使う。
ホストは自身の全本文から通常の書記素移動/削除を行えるが、DLLの本文取得範囲を広げない。

Shift・Home/End・Ctrl/Alt修飾の移動は追跡を退役する。Backspaceは記録末尾のみを同期処理し、
内部caretでは通常ホストへ戻す。内部への文字挿入も旧compositionへ誤ってappendせず、
書込前に退役して元の入力を一度だけ反映する。記録末尾へ戻った後の入力/Backspaceは同じrunを継続できる。
製品での任意選択の置換・全キー配送・他IME協調や、一般アプリの保護情報/寿命/標準Undoは引き続き残る。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
Coreに8件を追加し51件、workspace合計91件。
移動後も記録全体を保持、境界越え/過大countの非消費、書記素repeat削除、全削除、
不正context/世代/選択の退役、内部挿入の非append、不正削除観測の退役、RI再結合後の再分割を確認した。
Recoveryの既存検査も新操作のpayload拒否とcount上限へ広げている。

transaction-input-testに26件を追加し合計68件が成功した。
左右移動、複合絵文字/国旗の前方削除とrepeat/全削除、内部caretのF7と削除後のF7、末尾へ戻ってappend/Backspace、
境界越え、slot不足、移動の変更前/変更後失敗、本文の変更前/変更後失敗、caret失敗、保護変更後復元、
private/readonly/他composition、不正action/count/pointer、Shift退役、内部挿入とRI再結合を検証した。
移動では本文・Undo・DLL SetTextを増やさず、削除は1回Undo/Redoで復元し、同じcomposition identityを維持する。
処理後の内部caretからF7すると、現在の記録全体だけを送信して固定結果を適用できる。
移動・削除自体ではDLLのadapter本文取得counterを増やさない。F7の正常な原文取得とは区別する。

recovery-input-testへ共有4枠満杯時の追跡済み移動/前方削除を2件追加し合計37件が成功した。
既存救済ticketを変更せず、移動はUndoなし、削除は1回Undo/Redoで元に戻せる。
Backspace37・書記素ホスト22・composition/初回選択46・通知30・追跡25・broker入力11・close4も成功した。
Test-Spike.ps1 -SkipDesktopでRust91件・fmt・各構成Clippy・build・load_smoke・self/edit/keyが成功した。
成功したnative検査はTSF/COM解除とDLLアンロードを確認している。

ログはtarget/issue-4-tracked-cursor-static.logとtracked-cursor-<検査名>.log（すべてtarget/配下）。
検査は同じEditor helper/診断ABIを直接呼ぶ専用fixtureで、物理キーや一般アプリの互換を証明しない。
前面化で未通過のwindow/input/delayedの3検査を維持し、省略なし全回帰の成功には読み替えない。
