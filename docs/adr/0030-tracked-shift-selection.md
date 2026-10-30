# ADR 0030: 記録範囲内のShift選択と選択解除

状態：Core・実DLL・専用ACPホストで実装・検証。Issue #4 / draft PR #10。
[ADR 0029](0029-tracked-internal-insertion.md)の後続。一般アプリの選択編集の完成ではない。

後続の[ADR 0031](0031-recorded-selection-replacement.md)で記録内の選択置換/削除後の追跡も継続へ進めた。
以下の編集境界と件数は、このADRの実装段階の記録である。

## 決定

transaction-demoでShift＋左右を使って記録範囲内を選択する場合、同じ入力runとcompositionを保持する。
選択の始点を固定して拡大・縮小し、始点を越えたら向きを反転する。
通常の左右キーは方向側の端へ選択を解除し、repeatの残り回数だけ移動する。
従来はShift移動だけで旧runを退役していた。

Coreのselection_rangeは、記録UTF-16と検証済みの選択端点・active endpointだけから次の範囲と向きを計画する。
Unicode 17.0.0のBoundariesを使い、記録外の本文取得・推測やheap確保を行わない。
両端は記録内のscalar境界に限定し、surrogate内部は許可しない。前の削除によって書記素が再結合した場合、
scalar境界のanchorは維持し、動かすcaretは方向側の書記素端へ進める。
repeatが硬い記録端を越える場合は全体を拒否し、一部を先に移動しない。
selectedは正確な計画と完了後の端点、context/保護/世代を照合し、本文を変更せずrunを更新する。
CoreのInputPointは端点を持ち、向きの正しさは呼出側が別途検証する。計画だけではOS編集を許可しない。

## DLL・方向の照合・救済

独立ISpikeTrackedSelector::Extend(context, backward, count, disposition)を追加する。
backwardは0/1、countは1〜1024。既存ISpikeTrackedEditorのaction値と全vtableは変更しない。
owner STA・再入禁止・Controller/DLL寿命・write_blockedは既存Controlと共有する。
通常の左右移動とShift選択はTracker::move_selectionを共有する。

InputMetadataへactive_startを追加し、安定した観測、同期write内の選択検査、cookie後の完了、
明示選択のPrepare/CheckSourceで端点と向きを照合する。追跡経路では向き不明やinterim選択を拒否する。
通知なしに向きだけが変わった場合も、古い安定観測やReadyを再利用しない。
変更後に元へ戻した通知履歴も引き続き拒否する。未通知で変更・復元された全履歴を検出する保証ではない。

同期writeでは自己compositionの全範囲・identity、元の選択と向き、保護条件と通知世代を再照合する。
SetSelectionだけを行い、本文・下線範囲・composition全範囲・Undoは増やさない。
SetSelection直前からUNCERTAIN、成功後APPLIED_UNTRACKED、全検証後だけAPPLIEDとし、
反映された後に失敗を返す場合も元のShift移動を再実行しない。

操作前に共有Recovery枠を予約し、ExtendSelectionの方向とcountだけを保持する。
本文payloadやコピー機能は持たない。共有4枠満杯や書込前拒否なら追跡を退役し、
元のホスト選択操作全体を一度だけ扱う。既存救済を上書きしない。

## F7と残る編集境界

非空の選択があるF7は明示選択を優先し、選んだ範囲だけを変換する。向きも維持して準備する。
通常の左右キーで選択を解除した後は記録全体を保持し、途中挿入やBackspace等を継続できる。
collapsed caretからのF7は従来どおり現在の記録全体だけを使う。

選択したままの文字置換とBackspaceは旧追跡を退役して元のホスト編集を一度だけ扱う。
明示選択Deleteは既存の同期削除を行い、旧runと一時compositionを終了する。
これらの編集後に残る前後のrunを保持する機能は未実装。
Home/End、Ctrl/Alt修飾、Ctrl+A、記録外の選択や外部選択変更も追跡継続の対象外とする。
一般アプリのadapter/キー配送/通知/保護情報、診断sinkを使わない製品compositionの寿命と標準Undo、
製品の待機キー順序・常駐Broker・配置/認証は引き続き残る。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
Coreに6件追加して68件、workspace合計108件。anchorの保持・縮小・collapse・反転、
通常左右のcollapseと残りrepeat、全repeatの記録端検査と最大1024単位、不正完了の退役、
再結合した書記素内部のscalar anchor、collapse後の挿入継続を検査した。
既存Recovery検査も新操作のpayload拒否とcount境界へ広げた。

transaction-input-testに27件追加して115件。旧Shift退役検査も追跡継続へ更新した。
左右選択・縮小・反転、通常左右の解除/repeatと後続入力、前後両方向の明示選択F7、境界越え/slot不足、
SetSelection変更前/後失敗、readonly/private/他composition、不正ABI、向きだけの未通知変更、
変更後復元、Ready後の向き変更、不明なactive endpoint、RI再結合、選択置換/Backspaceのfallback、
明示選択Deleteの退役とHome境界を検査した。選択だけでは本文・DLL SetText・Undo・adapter本文取得を増やさない。
同一compositionと下線全範囲、F7で選択範囲だけの変換と1回Undoでの本文・範囲・向き復元、
選択解除後の入力継続、失敗時の非再操作とコピー不可の救済を確認した。

recovery-input-testは共有4枠満杯での追跡済みShift選択を加え41件。
実DLLで作った記録から元のホスト選択を一度だけ扱い、選択の向き・本文/Undo/SetText不変・既存ticket不変を確認した。
Backspace55・書記素ホスト22・composition/初回選択46・通知30・追跡25・broker入力11・close4も成功。
Test-Spike.ps1 -SkipDesktopでRust108件・fmt・Clippy 3構成・build・load_smoke・self/edit/keyが成功した。
成功したnative検査はTSF/COM解除とDLLアンロードも確認した。
ログはtarget/issue-4-tracked-selection-static.logとtracked-selection-<検査名>.log（すべてtarget/配下）。

fixtureはEditor helper/診断ABIへShift状態を明示して実TSFを使う専用storeの検査。
物理キー・自然なShift取得・一般アプリの互換を証明しない。
前面化で未通過のwindow/input/delayedの3検査は維持し、省略なし全回帰の成功には読み替えない。
