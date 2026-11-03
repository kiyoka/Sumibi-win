# ADR 0011: 入力追跡の失効を所有ループで処理する

2026-10-22 / Issue #4 / PR #10。前段階45a50e9と[ADR 0010](0010-cooperating-host-composition-growth.md)に追加する。
composing-demoは、外部変更の後に次の打鍵・F7を待たず、所有ループで古いcompositionと下線を解除する。
本文の反映方式やIPCのwire形式は変更しない。

## 通知と編集を分ける

InputWatchとManualEditのGuardは、従来のITfTextEditSink・thread manager・compartmentに加え、
ITfStatusSinkもcontextへ登録する。statusが元へ戻っても通知世代は進み、古い要求を復活させない。
特に、変換結果待ちの間にreadonlyへ変更して戻す履歴もManualEditのGuardで拒否する。
status購読を確立できなければ追跡や要求取得を拒否する。非対応を許可へ読み替えない。

通知callbackは失効と通知世代の更新、EXEへのChanged呼出しだけを行う。
本文を読まず、編集セッションも要求しない。RefCellの借用は外部COM呼出し前に解放する。
独立した診断用ISpikeInputMonitor（SetSignal/Poll）とISpikeInputSignal（Changed）を追加し、
既存ISpikeInputのvtableは変更しない。

EXEのSignalはmanual-reset eventを1個だけ保持する。本文・context・editor・DLL callbackを持たない。
通知回数に応じたキューやthreadを作らず、連続通知を同じeventへ集約する。
所有TextStoreのcomposition callbackも同じeventをsignalし、空compositionの開始・終了を補う。
一般アプリのowner sinkにこの協力を仮定しない。

所有するbroker_message_loopは、通常メッセージ・Broker完了eventと入力通知eventを同時に待つ。
通知を受けたら、callback/WndProcの外でeventをresetしてからPollする。
Trackerが保存した通知世代・保護状態・範囲・所有identityを再照合し、不一致なら記録を失効させ、
自分のcompositionと下線だけを片付ける。他compositionを終了・引継ぎしない。
自分の正常な伸縮で発生した通知は、更新完了時の基準と一致するので追跡を取消さない。
通知処理による本文置換・Undo履歴の追加・古いキーの再送は行わない。

## 拒否と再試行

cleanupがlock/readonlyで拒否されても、Core記録・入力前状態・Readyは先に失効させる。
保持する未完了cleanupは最大1件で、古い変換を再許可しない。
失敗直後の通知世代を保存し、同じ世代の余分なwakeだけでは再度writeを要求しない。
失敗したmetadataセッション自身の通知や連続signalによる再試行のループを避ける。
新しいstatus等の通知、明示的な入力・取消・終了では後始末を再試行できる。

readonly解除の正式なstatus通知で再度所有ループが起き、追加入力なしで後始末を完了する。
一般アプリでは同期writeを拒否されることがあり、即時に見た目まで消せる保証とはしない。
保留中のBroker結果は従来のManualEdit/Core gateも必要とし、statusの変更・復元で失効した結果は
本文を読み直したり書いたりせず破棄する。全てのstatus通知で通信workerを即時停止する契約ではない。

SetSignal(null)で購読先を外せる。再接続時は一度signalして、切断中の変更も次の通知なしで再検査する。
終了時は先にEXE側Signalを無効化し、TextStoreとDLLのsignal参照を外してからcleanupする。
古いsignal参照が残っても、Changedは新しいevent/HWNDや別editorへ作用しない。
event自体は最後のCOM参照までOwnedHandleで保持する。workerへ渡さない。
signal失敗はホストの正常な入力通知へ失敗を返さず、通常入力を再送しない。
配送不能時も、既存の入力前・F7・source・適用時の再検査を維持する。

## 検証

2026-10-22 / Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- input-notify-testの専用ホスト20ケース。正常な自己通知、本文変更/復元、選択変更/復元、
  private/復元、compartment復元、readonly/復元、lock拒否と回復、他composition、
  空composition履歴、強制終了、thread manager focus、通知burst、切断/再接続、
  終了後のsignal、signal失敗、変換待ちのreadonly復元を検証する。
- 通知時にはcompositionが残り、追加キーなしでownerのevent待機が起きた後に解除されることを確認。
  通知がメッセージ経由で遅れて配送される場合は2秒を上限に通常メッセージを処理し、
  非同期配送を「直ちにsignal済み」と仮定しない。診断用の同期Pollで検査を代替する前にeventを確認する。
- 1000回のsignalを集約し、同じ通知世代での再通知は失敗したcleanupのlock要求を増やさない。
  ロック拒否fixtureは、読取可能な間に正式なstatus通知を配送し、ownerがwakeを処理する前にlockを拒否する。
  Windows自身のstatus/layout読取と区別するため、同じ世代の再試行計測中にはmessage pumpを挟まない。
- GetModuleHandle/GetProcAddressでロード済み実DLLのSumibiSpikeBodyReadCountを取得する。
  EXEへ静的リンクされた別counterを使わず、通知cleanupで実DLLの本文取得が増えないことを確認する。
  MSCTF内部の本文照会まで防いだという意味ではない。
- 本文とUndo履歴を維持し、古いF7ではjobを開始しない。他compositionは残る。
  signal失敗でも通常文字は一度だけ反映され、正常な同一compositionの伸縮を継続できる。
- 最終検証ログはtarget/issue-4-input-notify.logとtarget/issue-4-input-notify-regression.log。
  Test-Spike.ps1を省略なしで実行し、workspace 47件、fmt・workspace/TIP spikeなし/Host全featureのClippy、
  build・load_smoke・self/edit/key、composing 29・tracking 25・owned-composition 18・composition 16・
  保護39・Brokerキー17・Broker入力8・Broker 19・window/input/close/delayed・pipe/peerも成功。
  TSF/COM解除・DLLアンロードと、最終検査後の診断process残留なしを確認。
  手順は[開発手順](../DEVELOPMENT.md#入力追跡の外部通知を検証する)を参照。

未完了：原子的なTSF本文入力取引、一般アプリの通知・非同期cleanup・標準Undo・製品キー配送、
待機入力/救済、製品Brokerの発見・常駐dispatch、HTTP・同意・設定・資格情報。
専用ホストの自動メッセージ試験を、物理打鍵や一般アプリの対応保証へ読み替えない。Issue #4は閉じない。

根拠：[ITfStatusSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfstatussink)、
[RequestEditSessionの同期要求制限](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontext-requesteditsession)。

後続：[ADR 0012](0012-synchronous-input-transaction.md)で、所有ホスト向けに本文挿入とcompositionを
同じwrite cookieで更新する経路を追加した。削除・待機・救済と一般アプリ統合は引き続き未完了。
