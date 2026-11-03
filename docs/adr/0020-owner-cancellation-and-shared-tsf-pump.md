# ADR 0020: 所有ホストによる要求・入力の失効と共通TSFメッセージ処理

2026-10-22 / Issue #4 / PR #10。基準は9c959ae。
専用ホストの要求寿命と検証経路を補強する。一般アプリの対応完了ではない。

## 判断

前段階では入力デモのmessage loopだけをITfMessagePumpへ接続した。
直接編集・Broker待機・保護検査・選択composition・標準RichEdit等の共通helperには
user32だけのPeekMessageが残っていたため、ここもTSF wrapperへ接続する。

一方、pumpを変更するだけで全てのfocus履歴が通知されるとは仮定しない。
今回の直接adapter fixtureでは、フォーカスを別文書へ移して戻した後、通知だけに任せた要求が
sourceを返せる場合を観測した。thread managerのIUnknownはWindowsの現在値と一致していた。
調査時の記録はtarget/issue-4-owner-pump-manager-identity.log。
通知欠落・遅延の原因を確定したとは扱わず、これを一般アプリでの自動失効の保証に読み替えない。

協調する所有ホストがfocus/input/lifecycle境界を検出した場合は、通知配送を待たず要求を明示失効させる。
従来の直接Broker試験はticketを破棄していたが、今回は保持中のticket自体を取消可能にする。

## ManualEdit.cancelの契約

cancel(&mut self)はcaptureしたthreadでのみ成功し、繰り返し呼べる。
所有threadの確認後、Guardを永久に失効し、Coreの保留要求とsnapshotの原文bufferを解放する。
COM呼出し・本文取得・編集セッション・SetText・Undo・待機を行わない。
解放は通常のメモリー所有権によるもので、memoryのsecure eraseを保証するAPIではない。

取消後はsourceが原文を返さず、finishはwrite要求前に拒否する。
欄・選択・原文・policyを元に戻しても旧ticketを再利用しない。
同じ欄で再開する場合は新しい明示要求と新しいID/identityでcaptureする。
旧ticketの遅延finishを拒否しても新しいticketには作用しない。

通信の停止・予約枠と結果の破棄は従来どおり所有ホストの責務で、cancelはBrokerを待たない。
broker-testのfocus-restoredでは、境界を検出した所有ホストがticket.cancel、job.cancelを呼び、
ticketを保持したままfocusを往復してsourceの拒否を確認する。
通知による既存の失効やsource/finishの対象・保護検査も維持する。
Rust APIの追加であり、診断COM interfaceのvtable・wire/JSON版1は変更しない。

## 入力ホストの明示境界と遅延cleanup

最初の全回帰では前段階のinput-notify-test/focusのevent待機が再び失敗した。
記録はtarget/issue-4-owner-cancel-regression-focus-failure.log。
ADR 0019の単独・全回帰成功はその実行の観測であり、pump変更だけでfocus履歴を常に受け取れるという保証にはしない。

Composing.owner_boundaryは所有ホストが検出した境界を記録する。
WM_KILLFOCUSから呼び、再利用禁止flagとwrapしない世代を更新して既存のmanual-reset eventをsignalする。
この関数はCOM・本文取得・編集を行わない。focusがcleanup前に戻っても禁止flagは残る。
入力ホストのWM_KILLFOCUSでは従来のBroker grant取消も維持し、signal失敗でもそちらの取消を先に完了させる。

所有loopがeventを受け、DLL Input controlのCancelを呼んで追跡・自己composition・下線をcleanupする。
成功時だけ禁止を解除するが、そのCOM呼出し中に世代が変わった場合は新しい境界を消さない。
拒否時は変換を止めたまま、同じ世代で繰り返し書込を試みない。
状態回復通知によるcleanup後も、新しい明示入力が取消を確認するまで旧変換を許可しない。
新しい文字を一度だけ入力し、新しい追跡を開始する。旧入力を再送・再適用しない。

prepareは境界禁止・世代枯渇を拒否する。送信前のone-shot checkerも世代を保持し、
外部COMの前後で同じ世代を検査する。cleanupでflagが解除されても、境界前に作ったcheckerは復活しない。
checkerは従来どおりworker開始前に解放し、COMや世代Cellをworkerへ渡さない。

通知検査のfocusをowner-focusへ改称し、owner-focus-restored、owner-focus-burst、
owner-ready-restored、owner-boundary-deniedを追加した。合計24ケース。
所有者が明示境界を転送する試験であり、自然なOSのfocus通知だけを待つ試験の代替成功とは呼ばない。
追加キーなしのcleanup、復元後拒否、1000回の集約、境界前checkerの拒否、lock拒否中の非回転、
回復後に新しい1文字だけを追跡することを確認した。
個別ログはtarget/issue-4-owner-boundary-notify.log、最終版はtarget/issue-4-owner-boundary-final-input-notify-test.log。

## 共通pumpと所有

共通pump_tsf_messagesは保持済みのITfMessagePumpを引数に取り、同期の所有loopだけで呼ぶ。
各exerciseの開始時に所有managerから取得し、本文capture前からworker配送・終了まで同じ参照を保持する。
待機helperにもその参照を渡し、workerやIPCへは渡さない。

PeekMessageWのHRESULTとmessage有無を区別し、通常messageがなくてもTSF側の処理を通す。
取得したmessageはTranslateMessage/DispatchMessageへ渡す。従来の256件上限を維持し、
上限超過・API失敗・WM_QUITを検証失敗として返す。worker取消や期限を無条件に延長しない。
共通helperを使うBroker・キー・保護・composition・描画・保持range・native RichEditが対象。
前段階の入力デモはWindowが保持するwrapperを引き続き使う。

この変更で全focus試験を自然なOS通知へ置き換えたとはしない。
broker-key-testとowned-composition-testの特定focusケースは従来どおり実DLL callbackを明示的に呼ぶ。
その結果と入力通知24ケースのevent待機・所有者転送、今回の要求取消を区別する。
一般アプリに専用ホストのcallback・境界転送・共有sinkの存在を仮定しない。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1 / 自動検証。

range-target-testへ次の4ケースを追加し、合計16ケースが成功した。

| ケース | 確認内容 |
| --- | --- |
| cancel-before-source | 原文返却前の明示取消でsource/finishを拒否 |
| cancel-after-source | 原文返却後も所有ホストが要求を退役でき、遅延finishを拒否 |
| cancel-repeat | 取消を重複しても成功し、要求は復活しない |
| cancel-new-ticket | 新しいticketの保持中に旧finishを拒否し、新要求は正常変換・1回Undo |

取消・旧source/finishでadapterのGetText counter、SetText、Undoを増やさない。
counterはsetupにリンクしたTIPライブラリのもので、OS内部の本文参照は別である。
broker-test（TSF 19＋容量6）、実DLLキー25、所有composition 18、標準RichEdit拒否7も個別実行して成功。
個別ログはtarget/issue-4-owner-pump-各コマンド名.log。
Rustテスト数は66件のままで、TSFの要求寿命はnative試験で検証する。
最終版のTest-Spike.ps1 -SkipDesktopは成功。66件・fmt・各構成Clippy・build・load_smoke・self/edit/keyを確認した。
ログはtarget/issue-4-owner-boundary-static.log。省略したdesktop検査はログにも明示する。
範囲16・通知24は最終版を個別再実行して成功し、それぞれtarget/issue-4-owner-boundary-final-コマンド名.logへ保存した。

全回帰の試行はtarget/issue-4-owner-boundary-regression.log。
範囲16・通知24・同期入力22・Backspace 27・救済25等は成功したが、window-testで停止した。
現在のセッションは別windowが前面にあり、SetForegroundWindowを試しても所有windowが前面にならなかった。
input-testとdelayed-testも個別実行では実キー処理が成立しなかった。
実キー経路を直接DLL callbackへ置き換えて成功扱いにはしない。
require_foregroundによる明示検査を加え、最終版の3検査とも前面化拒否として記録した。
この3検査は未通過であり、全回帰成功とは報告しない。対話desktopでの再検証が必要。
最終ログはtarget/issue-4-owner-boundary-final-window-test.log、final-input-test.log、final-delayed-test.log。
close-test 4件・ipc-test・ipc-peer-testは独立に実行して成功した。

## 次の受入条件

一般アプリで取得できる保護metadata・隠し文字・領域境界、通知されない履歴の扱いは未完了。
RichEditの変換/標準Undo、非ACPアプリ、所有compositionの製品用sink寿命、配置/認証と非同期配送も残る。
取得・送信前・適用時の検査と明示境界取消を合わせ、通知だけに依存した対応保証をしない。

根拠：[ITfMessagePump](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfmessagepump)、
[ITfThreadMgr.SetFocus](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfthreadmgr-setfocus)、
[TF_GetThreadMgr](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-tf_getthreadmgr)。
