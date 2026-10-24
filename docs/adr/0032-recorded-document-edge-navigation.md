# ADR 0032: 記録内にある文書端への移動・選択

状態：Core・実DLL・専用の単一行ACPホストで実装・検証。Issue #4 / draft PR #10。
[ADR 0031](0031-recorded-selection-replacement.md)の後続。

## 決定

transaction-demoのHome/EndとShift＋Home/Endで、移動先の文書端が記録内にある場合は追跡を継続する。
この専用ホストのHome/Endは文書の先頭/末尾へ移動する。一般アプリの複数行・折り返し行の端へ
移動する意味を推測せず、文書端の操作を独立した診断ABIとして提供する。

文書先頭はACP 0、末尾は実contextの位置情報から確認する。
Homeは記録の開始が0の場合だけ、Endは記録の終端が実際の文書末尾と一致する場合だけ継続できる。
既存prefixがあるHomeや既存suffixがあるEndを、記録の端へ読み替えない。
記録外へ移動する場合は書込前に拒否して旧runを退役し、元のホスト移動全体を一度だけ扱う。

CoreのEdgeMoveは方向・Shift・active endpoint・文書末尾位置・countというmetadataだけを持つ。
edge_rangeは正確な現在の記録、選択のscalar境界、文書/記録の包含関係と1〜1024のcountを検査する。
Shiftは既存anchorを保持し、Home/Endの切替で縮小・反転する。通常移動は選択を解除する。
repeatは同じ文書端への操作として扱い、回数分の左右移動へ変換しない。既に端にある場合も同じ記録を保持する。
計画は本文取得・heap確保・記録消費を行わず、moved_to_edgeは正確な完了端点とcontext/保護/世代を照合する。
通常のShift選択とは私有候補の完了検査を共有する。計画だけではOS編集を許可しない。

## DLL・位置の照合・救済

独立ISpikeDocumentNavigator::MoveEdge(context, to_end, extend, count, disposition)を追加する。
to_end/extendは0/1、countは1〜1024。既存のvtableとaction値は変更しない。
owner STA・再入禁止・Controller/DLL寿命・write_blockedは従来のControlと共有する。
専用ホストだけが自身のHome/Endをこの文書端ABIへ対応付け、Ctrl/Alt修飾は従来どおり境界として扱う。

実DLLはGetStart/GetEndとITfRangeACP::GetExtentから位置だけを取り、同じcontextのcollapsed rangeであること、
先頭が0であることを検査する。既存の肯定的な選択/保護検査も前後に行う。
文書全体のGetTextや属性列挙を追加しない。記録外の本文を補って移動先を探す方式ではない。

本文とcompositionを変更しない同期writeを通常の左右/Shift選択と共有する。
自己compositionのidentity/全範囲、選択端点・向き、保護と通知世代を再照合し、SetSelectionだけを一度実行する。
文書末尾位置は計画時、write cookie内の移動前後、cookie後の完了時に再照合する。
不成立なら候補を公開しない。文書端のmetadataが変わった場合も古い位置へ移動を確定しない。
SetSelection直前からUNCERTAIN、成功後APPLIED_UNTRACKED、全検証後だけAPPLIEDとし、
反映後の失敗でもホストで元の移動を繰り返さない。rollbackもしない。

共有RecoveryのMoveToEdgeはHome/End・Shift・countだけを保持し、本文payloadとコピー機能を持たない。
予算はDLL呼出し前に予約し、満杯なら既存救済を変更せず通常ホスト操作へ戻す。
救済表示の結果文言も、本文を変更しない操作を含めて「操作」と表現する。

一般アプリの行端/折り返し、キー配送・通知・保護情報、診断sinkを使わないcompositionの寿命、標準Undo、
製品の待機キー順序・常駐Broker・配置/認証は引き続き残る。未通知の全変更履歴を検出する保証ではない。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
Coreに6件追加して80件、workspace合計120件。文書端と記録端の非混同、Shift anchorの保持と反転、
通常移動の選択解除、repeat/no-op、最大1024単位・不正count/文書位置の非消費、不正完了の退役、
RI再結合後のscalar anchorと後続編集を検査した。Recoveryの既存検査にも新操作のpayload拒否とcount境界を加えた。

transaction-input-testに26件追加して174件。Home/End・Shift付き選択・反転/解除・repeat/no-op、
prefix/suffixの片方だけが記録外の各条件、移動/選択後のF7、後続入力/Backspace、slot不足、
SetSelection変更前/後失敗、readonly/private/他composition、不正ABI、外部選択変更とRI再結合を扱う。
本文・Undo・DLL SetText・adapter本文取得を増やさず、同一compositionと下線全範囲を保持する。
F7変換と後続編集は1回Undoで本文と対象を戻す。

Home/Endは再実行しても同じ位置になり得るため、位置だけで二重実行を否定しない。
診断storeにDLL SetSelectionとホスト選択の別counterを追加し、正常時はDLL1回/ホスト0回、
書込前fallbackはDLL0回/ホスト1回、変更前/後失敗はDLL1回/ホスト0回と直接再要求拒否を確認した。

recovery-input-testは共有4枠満杯のHome/Shift＋Endを加え47件。
実DLLで記録と内部caretを作ってから枠を満たし、DLL選択0回・元のホスト選択1回、
本文/Undo/SetText/adapter本文取得と既存ticketの不変を確認した。
Backspace55・書記素ホスト22・composition/初回選択46・通知30・追跡25・broker入力11・close4も成功。
Test-Spike.ps1 -SkipDesktopでRust120件・fmt・Clippy 3構成・build・load_smoke・self/edit/keyが成功した。
成功したnative検査はTSF/COM解除とDLLアンロードも確認した。
ログはtarget/issue-4-document-edge-static.logとdocument-edge-<検査名>.log（すべてtarget/配下）。

fixtureはEditor helper/診断ABIへ操作とShift状態を明示して実TSFを使う専用単一行storeの検査。
一般アプリのHome/Endや物理打鍵・自然な修飾キー取得の受入には読み替えない。
前面化で未通過のwindow/input/delayedの3検査は維持し、省略なし全回帰の成功には読み替えない。
