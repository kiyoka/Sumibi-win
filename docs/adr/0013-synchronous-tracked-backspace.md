# ADR 0013: 記録済み入力末尾の同期Backspace

状態：診断経路の実装と専用ホストでの検証。製品キー配送や一般アプリ対応の完了ではない。
2026-10-22。Issue #4 / draft PR #10。ADR 0012の後続。

## 背景

transaction-demoの文字挿入は本文・caret・compositionを同じwrite cookieで更新するが、
Backspaceは従来のホスト削除後にmetadataを更新していた。repeatによる複数文字の削除も含め、
記録済みの入力末尾は、一度のSetTextと同じcookieでのcomposition更新として扱う。
書込を試みた後の失敗で再入力・再削除しないADR 0012の契約を維持する。

## 決定

既存ISpikeInputWriterのvtableを拡張せず、独立ISpikeInputEraser.Backspaceを追加する。
context、Unicode scalar数のcount、disposition出力だけを同じ所有STAで受け取る。
cookie・COM target・記録本文をworker/IPCへ渡さない。

Core InputSession.backspace_rangeは、完全一致する現在のInputPointと記録済みUTF-16だけから
削除する末尾範囲を計算する。現在caretが記録末尾にあり、選択がcollapsed、countが1〜1024で、
要求全体が記録範囲内に収まる場合だけ許可する。文書本文や既存prefixを読んで推測しない。
補助面文字のsurrogate pairは一緒に数える。書記素cluster分割ではなくUnicode scalar単位であり、
結合文字やZWJ列をひとまとめに消す一般的な編集仕様を確定した意味ではない。

countが記録されたscalar数を越える場合は要求全体を拒否する。DLLが一部だけ消した後で
残りをホストへ渡すことはしない。上限超過・0・旧世代・選択・禁止状態でも書込前に拒否する。
計画は記録を消費しないが、アダプターの失敗時は従来通り古い追跡を失効させる。

### 同期編集

1. 入力前観測でfocus、選択、保護状態、通知世代、正確な自己所有compositionを確認する。
   Coreの追跡範囲と保持中のcomposition範囲も一致させる。
2. Coreに削除後のprivate candidateを作る。成功するまでTrackerへ公開しない。
3. 一度の同期read/write cookie内で対象を再検査し、削除するACP rangeを作る。
   正確な自己COM identityが排他的に存在し、範囲が同じcontextの記録末尾内であり、
   scope/status/compartment、表示属性、通知世代が有効な場合だけ先へ進む。
4. 書込開始を先にUNCERTAINへ記録し、range.SetTextへ空のUTF-16を一度だけ渡す。
   raw HRESULTが厳密なS_OKの場合だけAPPLIED_UNTRACKEDへ進む。
5. 同じcookie内でcaretを削除開始点へ移す。残りがあれば同じcomposition identityを縮め、
   自分の下線だけを更新する。全削除なら自分のcompositionを終了し、下線を解除する。
   空compositionを残して後の入力を認可しない。他compositionを終了・採用しない。
6. cookie解放後の安定した選択・保護履歴も確認し、Core candidateと世代を確定してAPPLIEDを返す。
   全削除時は記録を空にし、次の実入力だけが新しいcompositionを開始できる。

本文・caretの書込関数と、cookie解放後の確定処理は文字挿入と共有する。
DLLからのGetText、追跡済み本文全体の書き直し、削除結果を推測したrollbackは行わない。
通常のF7 capture/source/結果適用のidle gateは変更しない。

### 拒否・失敗・ホストへの返却

dispositionの4状態とpanic境界はADR 0012と同じ。NOT_ATTEMPTEDの場合だけ、
元のホストBackspaceをその場で一度処理できる。専用ホストは受け取ったrepeat数を従来通り処理する。
範囲越えや選択削除を、無許可のDLL本文取得や部分削除で代替しない。
このfallbackのUndo単位はホストが決める。診断ホストはrepeatごとの履歴を維持する。

APPLIED_UNTRACKED/UNCERTAINでは、再削除・rollbackせず追跡と変換を停止し、診断タイトルで確認を表示する。
通知PollやCancel(false)は停止を解除しない。Esc等の明示boundaryで解除した後、
新しく挿入した文字だけから追跡を再開する。変更前に失敗したstoreと、変更後に失敗したstoreの両方を扱う。
結果不明の実際の本文は自動判定せず、未確認入力の保持・救済UIは依然未実装である。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
Coreに3件の試験を追加し、workspaceは50件（Core 17・Protocol 22・IPC 6・TIP 5）。
Coreではscalar単位の計画と非消費、旧世代・不許可・選択・範囲外の拒否、最大容量を確認する。

backspace-input-testはロードした実DLLと専用の合成ACP欄で27ケースを検証する。

- 末尾削除、補助面文字、repeat一括削除、全削除、Undo、Redo、削除後のF7と1回Undo、新規追跡、close。
- 未追跡、空欄、選択、caret移動、private、readonly、lock拒否、他composition、強制終了、記録開始点越え。
- SetTextの変更前／変更後失敗、caret失敗、status/scope/compartmentの変更・復元、不正count、最大1024単位の全削除。

正常な追跡内repeatはSetTextが一度だけで、同じwrite-lock番号によりcompositionを更新し、
1回Undoで一括削除前へ戻る。部分削除は同じIUnknown identityを維持し、全削除はcompositionと下線を残さない。
元のprefix/suffix、本文・Undo・SetText回数を検査し、失敗後のF7停止、再削除禁止、Esc後の新規追跡も確認する。
ロード済み実DLLの本文取得counterは、削除だけでは増えない。MSCTF内部の照会の無読取保証ではない。

ログ：target/issue-4-backspace-transaction.log、target/issue-4-backspace-transaction-regression.log。
Test-Spike.ps1を省略なしで実行し、workspace 50件、fmt、workspace/TIP spikeなし/Host全featureのClippy、
build・load_smoke・既存native検査・pipe/peerも成功した。TSF/COM解除・DLLアンロード、診断process残留なしを確認した。
起動手順は[開発手順](../DEVELOPMENT.md#同期入力取引を試す)。

transaction-demoのBackspaceは今回の経路を使用する。前方Delete、追跡内の任意位置編集、
一般アプリの物理打鍵と標準Undo、製品キー消費、非同期write待機、有限容量・期限付き救済、
常駐Broker、HTTP・同意・設定・資格情報は未完了である。Issue #4は閉じない。

根拠：[ITfRange.SetText](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfrange-settext)、
[ITfComposition.ShiftEnd](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcomposition-shiftend)。

後続：[ADR 0014](0014-bounded-input-recovery.md)で未確認入力の救済を追加した。
Escでは救済を消さず、明示破棄・コピー・期限で解除する。旧native試験の追跡再開ではF9の明示破棄も行う。
