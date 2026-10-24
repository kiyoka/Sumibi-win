# ADR 0010: 所有ホストの入力compositionを伸縮する

2026-10-22 / Issue #4 / PR #10。[ADR 0009](0009-cooperating-host-input-run.md)の入力範囲記録を
実DLLのcompositionへ接続する。基準は前段階6325527と、このADRを含む変更。

## 責務と入力の反映

専用composing-demoのホストは、入力前に実DLLのISpikeInput.BeforeInputを呼ぶ。
本文は従来のTextStore処理で一度だけ変更し、成功後にAfterInputで実際の挿入文字を通知する。
Backspaceではホストが削除した後のカーソルから、記録済みの末尾だけを縮める。
追跡開始・更新・後始末が拒否されても元の入力を取り消したり、再送したりしない。

本文の反映はホスト、compositionと下線の操作はその後の別の同期writeで行う。
この2つを原子的なTSF本文入力取引として完成したとは扱わない。
一般アプリから任意の打鍵後本文を同じ方法で取得できるという前提も置かない。
この経路は専用ホストの協力を受ける診断用で、既存demo/tracking-demoの動作は維持する。

DLL内のTrackerがInputWatch、Core InputSession、自己所有composition、下線範囲を保持する。
本文を持つ型にDebugを実装せず、記録は最大1024 UTF-16単位。既存prefix/suffixを取得・推測しない。
Coreに範囲の非消費参照と末尾削除を追加した。不正UTF-16、サロゲートの分断、
追跡開始より前の削除、選択・context・世代の不一致では記録を破棄する。
末尾まで消した場合はcompositionと下線を終了する。

## メタデータ専用の所有例外

入力済み範囲で初めてStartCompositionし、以降は同じオブジェクトのShiftEndで範囲を伸縮する。
開始点を既存本文へ広げない。各操作は所有STA、フォーカスcontext、選択、保護Stampを再検査する。
GUID_PROP_ATTRIBUTEの下線は記録範囲内の自分のatomだけを解除し、既存の別属性は上書きしない。

OwnedComposition.require_exclusiveは最大2件だけを照会する。
1件目が正確なS_OK/1/非nullで保持中のIUnknown identityと一致し、
2件目が正確なS_FALSE/0/nullである場合だけ許可する。
他compositionのowner・range・本文を問い合わせない。同じ原文や座標で所有を推測しない。
ShiftEndは正確なS_OK、同一context、操作後の正確な範囲と所有identityも確認する。

この例外は保護メタデータの照会だけに限定する。既存の本文取得・source・結果適用は、
従来どおりcompositionがないcontextを必要とする。生存中の自己所有compositionを理由に
GetTextを許可する変更は加えない。MSCTF内部の本文照会を防ぐ保証でもない。

## 通知と一度だけの要求

Trackerは直前の通知世代と保護Stampを保存し、次の入力・F7の前に照合する。
自分のcomposition・下線更新でも通知が発生するため、Coreの入力世代と生のTSF通知世代を分ける。
Coreの世代は実際のホスト編集後だけ進め、メタデータ更新完了後の通知世代を次回の基準にする。
外部編集・選択復元・保護状態変更で基準が不一致なら古い記録を破棄し、
その後の新しい入力分だけを新しい範囲として開始する。

ホストは所有者callbackのcomposition counterも比較する。
通知のない空compositionの開始・終了を見たら、古い追跡を取消す。
自分のメタデータ操作後のcounterだけを次の基準とし、workerへ値やCOM参照を送らない。
未通知の空compositionの履歴を一般ホストで必ず検出できるとは扱わない。

F7では記録を一度だけ消費し、自分のcompositionと下線を終了してから通常の保護検査を通す。
対象を選択し、記録原文・範囲・Stamp・通知世代をReadyとして保持する。
EXE bridgeのStartはworker開始前にCheckSourceを一度だけ呼び、実DLLが取得した原文と完全照合する。
EXE側のcounter不一致も拒否する。Readyは成功・失敗とも消費し、Start未到達でもF7終了時に解除する。
自動選択の印を残すため、失敗後のF7で古い範囲を明示選択として再利用しない。
Shift選択等の利用者操作では印を解除し、通常の選択変換として扱う。

## 寿命・失敗・制限

ISpikeInputは既存controlのvtableを拡張しない独立した診断用ABI。
controllerへの弱参照、所有thread、独立ObjectLease、再入ガード、最大1個のTrackerを持つ。
Before/After/Prepare/CheckSource/Cancelだけを公開し、Rust型やCOM targetをworkerへ渡さない。
各COM呼出し前にRefCellの借用を手放す。別thread・失効controller・再入では通常操作を拒否する。
Cancelはcontroller失効後も所有STAでメタデータの後始末だけを行える。

ホストは移動・制御操作・focus喪失・終了でCancelする。通常入力の追跡許可は
従来のBroker/PreviewのGrantとは独立し、一般ホスト用の認可契約にはしない。
外部編集等の通知だけでは通知callback中に書かず、次の入力・F7・明示取消・終了時に検出・後始末する。
一般アプリでの即時表示解除や通知配送を完成したとは扱わない。

書込lock/readonlyで後始末を拒否されたら、本文操作を禁止したまま最大1件の所有情報と範囲を保持する。
次の所有者操作で後始末を再試行し、古い追跡を復活させない。Dropは同期のbest effort。
開始拒否のsink参照残留には[ADR 0008](0008-owned-selection-composition.md)の共有EXE sinkを使う。
他compositionを終了・取得・引継ぎせず、強制終了後に同じ範囲へ別objectができても旧記録を使わない。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1 / 2026-10-22。

- Coreに2件追加。末尾削除・再入力、全削除、サロゲート境界、開始点越え、旧世代・異なるcontext等を検証。
  workspaceは47件（Core 14、Protocol 22、IPC 6、TIP 5）。
- composing-input-testは専用ホスト29ケース。全ケースの初期正常経路では、実DLLの所有CLSIDを確認し、
  続く入力でもIUnknown identityが同じであることを確認する。開始拒否等4件ではその代わりに
  通常入力が一度だけ反映される本文・3回の入力履歴と、回復後の新しい1文字だけの追跡を確認する。
- 伸長、Backspaceによる縮小・補助面文字の2単位削除・全削除、既存prefix/suffixの保持、
  移動・外部編集復元、他composition・空compositionの履歴、強制終了と同じ範囲の別object、
  private/復元/入力時private、readonly・lock拒否と回復、明示選択、focus、送信直前の原文/選択不一致を検証する。
- 通常の本文取得が生存中のcompositionを拒否するケース、待機中Esc/Enter/追加入力、
  自動選択の再利用拒否、1回Undo、実ウィンドウへの同期SC_CLOSEとcomposition解除も扱う。
- fmt・workspace/TIP spikeなし/Host全featureのClippy・buildと、Test-Spike.ps1の全検査が省略なしで成功。
  既存のtracking 25・owned-composition 18・composition 16・保護39・Brokerキー17・Broker入力8・
  Broker 19、self/edit/key/window/input/close/delayed、pipe/peerも再確認した。
  TSF/COM解除・DLLアンロードと、診断process残留なしを確認。
  ログはtarget/issue-4-composing-input.log（先行27ケース）とtarget/issue-4-composing-regression.log（最終29ケース）。

物理打鍵・Notepad等の一般アプリ、他製品IMEとの競合、標準Undoは未検証。
原子的な本文入力とcompositionの取引、製品キー配送・待機入力/救済、一般アプリ内の寿命管理、
製品Brokerの発見・常駐dispatch、HTTP・同意・設定・資格情報は引き続き未完了。Issue #4は閉じない。

根拠：[ITfComposition::ShiftEnd](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcomposition-shiftend)、
[ShiftStart](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcomposition-shiftstart)。

後続の[ADR 0011](0011-owner-loop-input-invalidation.md)で、次の入力を待たず外部通知から
所有ループを起こして失効・cleanupする仕組みを追加した。このADR時点の遅延検出の制限を拡張する。
