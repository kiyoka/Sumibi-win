# ADR 0014: 未確認入力の有限救済と明示コピー

状態：所有する診断editorの実装と専用ホストでの検証。製品用の待機行列や一般アプリ対応の完了ではない。
2026-10-22。Issue #4 / draft PR #10。ADR 0012・0013の後続。

## 背景

SetTextを試みた後の失敗では、本文を変更済みかもしれないため元の入力を再送しない。
一方、実際には入力されなかった文字を利用者が回収する経路がなかった。
文書へ自動的に戻すことと、利用者の確認後に明示コピーすることを分ける。

## 決定と保持範囲

Coreにメモリー内のRecoveryを追加する。所有editorにつき1件、挿入本文は最大1024 UTF-16単位。
現デモは同時に1 editorだけを作る。製品のプロセス全体予算・複数入力先の管理は未実装である。
本文・救済状態はDebug/Clone/serializationを提供せず、ファイル・ログ・worker/IPCへ渡さない。

1. DLLを呼ぶ前に枠と挿入サイズのbuffer容量を予約する。この時点では本文をbufferへコピーしない。
2. NOT_ATTEMPTED/APPLIEDでは予約を解放する。通常入力や、保護状態で拒否された入力を保存しない。
3. APPLIED_UNTRACKED/UNCERTAINでは操作・結果・期限を保存する。本文書込後の追加allocationに依存せず、
   元の挿入引数だけを予約済みbufferへコピーできる。文書のGetTextは呼ばない。
4. コピー可能な本文の保持には、予約時からの保護通知世代の一致と、失敗後の新しい肯定的な入力保護検査を必要とする。
   InputWatchのscope/status/compartment/focus履歴を使い、変更後に戻した条件でも本文を保存・再許可しない。
   cleanup拒否や他composition、未知条件等で検査不能なら操作メタデータだけを残す。
5. Backspaceは回数と結果だけを保存する。削除済み本文を読み直したり、記録外のprefixを推測したりしない。
   コピー操作は許可しない。結果は利用者が文書で確認する。

元の救済が満杯なら、次の入力を消費するDLL取引へ入らず、古い追跡を失効させて
その場の通常ホスト入力へ一度だけ返す。前の救済を上書きしない。遅延キーをキューへ入れない。
予約の失敗・ID枯渇・不正サイズでも同じ扱い。Coreは同じticketの重複完了や別ticketの削除を拒否する。
入力結果が分からない状態での自動再入力・再削除・rollback・別欄への適用は追加しない。

## 期限と失効

診断経路の保持期限は失敗結果の確定から10分。タイトルと確認画面で、期限による破棄を事前表示する。
コピー失敗や画面の開閉で期限を延長しない。Coreの取得は期限ちょうどから拒否する。
所有ループはMsgWaitのtimeoutを残り期限とし、追加入力なしでも枠と画面を解除する。
期限をまたいだ瞬間に「取得可能データなし」を無期限待機と誤認しない。
短い期限のnative fixtureで実際のtimeoutからの解除も検証する。

期限後の画面消去は所有ループが動けることを前提とする。ブロックしたOS callback中に
ハードリアルタイムで画面・メモリーを消去できる保証ではない。
メモリーのみであり、クラッシュやプロセス終了後の復元は保証しない。終了時は全状態と確認画面を破棄する。
明示コピー後のOSクリップボードは利用者が発行したデータであり、救済期限に合わせて自動的に消さない。

保持中の保護条件変更・復元、focus喪失、再検査失敗では本文を取り除き、コピー不能の操作情報だけにする。
表示済みなら古いプレビューも所有ループで閉じる。通知callbackから本文を読む・コピーする・編集することはない。
未通知の変更や、アプリが通常テキストと誤報告した欄を全て識別できるという保証ではない。

## 明示的な操作

transaction-demoとrecovery-demoだけが次のキーを解釈する。IMEのグローバルキー登録ではない。

| 操作 | 結果 |
| --- | --- |
| F8 | 現在の救済の確認画面を開く。自動的には本文を表示しない |
| 確認画面のコピー欄／画面を開いた状態のF10 | 有効な挿入本文をクリップボードへコピーして救済を解除する。文書へは書かない |
| F9／確認画面の破棄欄 | 現在の救済を明示破棄する |
| 確認画面の× | 画面だけ閉じ、元の期限まで保持する |
| Esc | 入力追跡の停止を解除するが、救済は保持する。満杯の間の新規入力は通常ホスト経路 |

プレビューは先頭160 UTF-16単位までで、surrogate pairを分割しない。省略を明示し、コピーは全文を対象にする。
表示は編集・標準コピー機能のないSTATICを使用する。`&`をアクセラレータとして解釈しない。
確認画面は入力欄のfocusを奪わず、操作欄もfocusを移さないSTATIC通知を使う。キーボード操作はF9/F10で提供する。
通常のBUTTONがfocusを移して正しく保護失効したため、保護検査を緩めず操作部を変更した。

native callbackは有限の操作フラグだけを更新し、実際の処理は所有ループで行う。
画面とキー要求は救済ticketへ結び付け、古いF8/F9/F10が別のデータを表示・コピー・破棄しない。
同時にコピーと破棄が届いた場合は破棄を優先し、両要求を消費して古いコピーを次へ持ち越さない。
外部APIへ入る前にRefCellの借用を解放する。

## クリップボード境界

本文コピーは確認操作でのみ実行する。コピー先の確保に失敗しても救済を残す。
SystemClipboardはCF_UNICODETEXT用のGMEM_MOVEABLEを確保し、終端NULを含めて初期化してから
OpenClipboard、EmptyClipboard、SetClipboardData、CloseClipboardを行う。
公開直前にもticket・期限・保護履歴とメタデータを検査する。
SetClipboardData成功後にだけhandleの所有をOSへ渡し、それ以前の失敗では自分で解放する。
コピー失敗では元の救済・期限を維持し、自動再試行しない。

## 検証と制限

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
Coreは救済7件を追加し、workspaceは57件（Core 24・Protocol 22・IPC 6・TIP 5）。
予約・容量・重複完了・ticket・期限ちょうど・不正本文・Backspaceの非保存・本文除去を検証する。

recovery-input-testの25ケースは実DLLと専用の合成入力欄を使う。
正常/拒否の非保持、SetText変更前/変更後失敗、適用後の追跡失敗、Backspace情報、容量、Esc、
明示コピーと失敗/再試行、破棄、×と再表示、期限切れコピー拒否、owner timeout、close、focus、
最大本文、旧ticket、確認前F10拒否、キーボードコピー、保護変更/復元、コピー処理中の保護失効と古い操作の拒否を確認する。
救済操作で文書本文・Undo・SetText回数・実DLLの本文取得回数が増えないことも確認する。

自動試験では利用者のクリップボードを変更しないよう、同じClipboard境界を試験用メモリーへ差し替える。
確認画面のnative通知と所有ループまでを通すが、実OSクリップボード公開の自動検証とは扱わない。
SystemClipboardはビルド・ClippyとAPI所有の照合まで。手動確認時の明示コピーは実クリップボードを使う。
ログはtarget/issue-4-input-recovery.logとtarget/issue-4-input-recovery-regression.log。
Test-Spike.ps1を省略なしで実行し、workspace 57件、fmt、workspace/TIP spikeなし/Host全featureのClippy、
build・load_smoke・既存native検査・pipe/peerも成功した。TSF/COM解除・DLLアンロードと診断process残留なしを確認。
最終版で救済25ケースを含む全回帰検査を再実行した。

recovery-demoは固定の合成入力で書込前失敗を再現し、利用者がF8から確認できる。
ユーザー文書・HTTP・資格情報は使わない。[開発手順](../DEVELOPMENT.md#未確認入力の救済を試す)を参照。
製品の待機行列・プロセス全体の容量・前方Delete/任意位置編集・一般アプリ・常駐Broker・HTTPは未完了。
Issue #4は閉じず、PR #10はdraftを維持する。

根拠：[Clipboardの利用](https://learn.microsoft.com/en-us/windows/win32/dataxchg/using-the-clipboard)、
[SetClipboardDataの所有権](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setclipboarddata)、
[STATICの通知と表示](https://learn.microsoft.com/en-us/windows/win32/controls/static-control-styles)。
