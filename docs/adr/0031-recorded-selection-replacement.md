# ADR 0031: 記録内の選択置換・削除後の追跡継続

状態：Core・実DLL・専用ACPホストで実装・検証。Issue #4 / draft PR #10。
[ADR 0030](0030-tracked-shift-selection.md)の選択後の編集境界を更新する。

## 決定

transaction-demoで記録内をShift＋左右で選択した後、文字置換・Delete・Backspaceでも
前後の記録と同じcompositionを保持する。対象は正確な現在の観測と一致する記録内の非空選択だけ。
従来は選択置換/Backspaceで通常ホストへ戻し、選択Deleteも常に旧runを退役していた。
全削除ではrunと自己compositionを終了する。全体を非空文字へ置き換える場合は同じcompositionを継続する。

Coreのreplacement_rangeは、記録と現在の選択の一致、硬い記録境界、両端のscalar境界、
置換後の合計1024 UTF-16単位以内を検査して結果の全範囲を返す。
長さは「記録長 − 選択長 ＋ 供給長」で計算し、最大長の記録でも削除分を使って置換できる。
計画は本文取得・heap確保・記録の消費を行わない。
replaced_selectionは完了後のcaret、context/保護/世代と供給UTF-16を照合し、
私有候補の選択部分だけを置き換える。不正な観測では旧runを退役し、新規runを推測しない。

Backspace/前方Deleteの既存計画も、正確な非空選択なら元のUTF-16範囲だけを一度削除する。
countは1〜1024を検査するが、選択を消した後で残りrepeatを続けない。
選択のない削除と異なり、明示的な部分選択を周囲の書記素へ広げない。
削除後に国旗等が再結合した場合、次のcollapsed caretの操作で記録を再分割する。
旧ホスト観測用inserted/backspacedとcollapsed専用inserted_at_caretの条件は維持する。

## DLL・失効した選択・救済

既存のISpikeInputWriter/ISpikeSelectionEraser/ISpikeInputEraserを使い、ABIは追加・変更しない。
選択置換は既存replace/write_body、選択Backspaceは共通eraseを使う。
選択Deleteは書込前の観測と記録が正確に一致する場合だけ同じreplaceへ進む。
同期write内で旧compositionの全範囲・identity、選択端点と向き、保護/属性と通知世代を再照合し、
SetTextは対象に一度だけ実行する。caretと残った全範囲の下線/compositionも同じcookieで更新する。
結果の全範囲は削除・供給長から独立に照合し、cookie後の検査が済むまでCoreの候補を公開しない。

外部選択変更や保護履歴で失効したrunは継続しない。記録のない明示選択Deleteは従来どおり
旧runを退役し、一時compositionで削除して終了する。失効後の通常の選択置換は新しい供給文字だけを
新規runにでき、残った既存本文を採用しない。記録外の本文取得範囲も広げない。

SetText試行前からUNCERTAIN、成功後APPLIED_UNTRACKED、全検証後だけAPPLIEDとする。
試行後の失敗では再入力・再削除・rollbackを行わず、F7を停止して明示確認へ進む。
既存RecoveryのInsert/DeleteSelection/Backspaceを再利用する。
置換の救済本文は新しい供給断片だけで、削除された文字や前後の記録は追加しない。
削除は操作情報のみでコピー不可。保護履歴変更では供給断片も除去する。
共有4枠満杯ならDLLへ入らず追跡を退役し、元のホスト編集を一度だけ扱う。

残ったrunのcollapsed caretからのF7は更新後の記録全体だけを変換する。
一般アプリのadapter/キー配送/通知/保護情報、診断sinkを使わないcompositionの寿命、標準Undo、
Home/End等の追跡継続と製品の待機キー順序・常駐Broker・配置/認証は引き続き残る。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
Coreに6件追加して74件、workspace合計114件。部分/全体/先頭/末尾の置換、両方向削除とrepeat、
全削除、削除分を差し引いた最大容量と超過の非消費、不正完了/供給文字の退役、
硬い記録境界と旧APIの非拡張、RI再結合とscalar境界の明示部分選択を検査した。

transaction-input-testに33件追加して148件。以前のShift選択の3ケースも追跡継続へ更新した。
前後方向の選択、部分/全体/先頭/末尾の置換、WM_CHAR repeat、Delete/Backspaceとrepeat/全削除、
置換/削除後のF7、RI再結合後のBackspace、最大容量/超過、各操作のslot不足、readonly/private/他composition、
外部選択変更後の非採用、SetText変更前/後失敗、caret変更前/後失敗、status/scope/compartment変更後復元を検証した。
同一compositionと残った全範囲、同じwrite cookie、1回Undo/Redoでの本文・選択範囲・向き復元、
本文取得不変と書込試行後の非再操作を確認する。外部選択変更後の置換F7は新しい「z」だけを変換する。
最大長の下線描画は従来の固定幅制限を維持し、実TSFの全範囲とUndo/Redoを直接検査した。

recovery-input-testに4件追加して45件。実DLLで記録と選択を作ってから共有4枠を満たし、
選択置換/Delete/BackspaceがDLL書込なしの1回編集となり、既存ticketと本文取得counterを変えないことを確認した。
1回Undoは本文・範囲・向きを復元する。変更後に失敗した選択置換の救済payloadと明示コピーは供給した「z」だけ。
Clipboardは試験用メモリーへ差し替え、利用者のClipboardを変更しない。

Backspace55・書記素ホスト22・composition/初回選択46・通知30・追跡25・broker入力11・close4も成功。
Test-Spike.ps1 -SkipDesktopでRust114件・fmt・Clippy 3構成・build・load_smoke・self/edit/keyが成功した。
成功したnative検査はTSF/COM解除とDLLアンロードも確認した。
ログはtarget/issue-4-recorded-replacement-static.logとrecorded-replacement-<検査名>.log（すべてtarget/配下）。

fixtureはEditor helper/診断ABIへ操作を明示して実TSFを使う専用storeの検査で、物理打鍵や一般アプリの互換を証明しない。
前面化で未通過のwindow/input/delayedの3検査は維持し、省略なし全回帰の成功には読み替えない。
