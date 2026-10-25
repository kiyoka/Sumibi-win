# ADR 0017: 再利用Broker workerと所有STAへの結果配送

2026-10-22 / Issue #4 / PR #10。[ADR 0016](0016-authenticated-reusable-broker-channel.md)の
blocking ChannelをEXE workerへ閉じ込め、既存の診断Bridge・実DLLキー・入力デモへ接続する。
一般アプリ用の常駐共有Broker探索や製品の配置方式は引き続き未実装。

## キー消費前の予約と所有

`Bridge::Reserve`は本文取得前にworkerと要求Permitを確保する。workerがなければ共通4枠の
Reservationを1件取得し、有限eventとEXE threadを作る。この時点では子Brokerの起動・pipe I/O・待機を行わない。
idle workerがあれば同じ枠を再利用し、要求のたびに追加のglobal枠を取らない。

| 段階 | 所有と動作 |
| --- | --- |
| Reserve | EXE所有STAが要求IDと専用Permitを取得。要求IDは非zero・単調増加、解放後も再利用しない |
| 本文取得 | DLLが同期readで対象・原文・保護・revisionを検査。失敗なら対象を破棄してCancel |
| Start | UTF-16を有限Stringへコピーし、必要な送信前検査を一度だけ行う。COMを含むcheckerはここで解放 |
| worker | 容量1のCommandから所有String・ID・版・絶対期限だけを受領。初回にChannelを開き、以後同じ子とpipeを再利用 |
| 結果待ち | 容量1のCompletionとeventで所有STAを起こす。元のReplyとPermitを結果に保持 |
| 配送 | 所有STAがIDを照合し、DLLが新しい同期writeで対象を再検査。一度だけ適用または破棄 |
| 次の入力 | Completionの解放後だけ次のPermitを発行。通常はidle接続を再利用 |

Startの引数検査後に絶対単調期限を固定し、原文コピー・送信前検査・dispatch・起動・I/Oを同じ期限に含める。
要求・結果が存続する間は専用Permitを解放しない。結果を取り出してpendingを外した後も、
DLLへの外部COM呼出しが終わるまで次のReserveを拒否する。
Bridgeの配送guardは、workerの寿命上限到達時にも再入Reserve・再帰drain・shutdownを拒否する。
一つのcontextの結果を別の入力先へ付け替えず、wire IDを所有STAの要求IDへ代用しない。

## 有限資源

worker・単発ジョブ・直接Channelは同じprocess内4枠を共有する。
worker facade・thread・要求Permit・元の結果は同じArc予約を保持し、最後の解放後にだけ枠を返す。
worker内Channelへ予約を引き渡す内部入口を設け、同じworkerで枠を二重に消費しない。
idle workerも計数し、閉じた旧workerが子の回収中ならその枠も残す。

各workerはCommand 1件・Completion 1件・要求Permit 1件。新しい本文を後ろへ積まない。
原文・結果は各1024 UTF-16単位で、Debug/Clone/serializationを追加しない。
pipeの8 endpoint、同時1操作と取消bufferの保持条件は維持する。
Channelの最大64要求・idle 60秒・受付/実行10分・最終返信後10秒の条件も維持する。
64番目の正常結果は配送でき、その後のReserve前にworkerをretiredにする。
原文に`U+FFFC`がある場合は`Request::validate`でsubmit前に拒否し、
同じworkerの次の正常要求がwire ID 1から始まることを実子Brokerで確認する。
これは製品の遅延キー待機行列や救済のprocess合計容量の採用ではない。

## 取消・失敗・終了

CancelはまずDLLの対象を論理的に失効させ、合致する要求IDのworkerだけを停止する。
古いCancelやPermitの解放で新しい要求を取り消したり解放したりしない。
明示取消の印と、通常エラー/寿命上限によるclosed状態を分ける。
内部Channelの後始末もStopを通知するため、それを利用者取消と誤認してPolicyDenied等を上書きしない。
既に届いたCompletionも、明示取消後のoutcome取得ではCancelledを返す。DLL側の対象再検査も維持する。

StopがCommand eventより先に観測された場合も、既に受領したCommandには終端結果を一度返す。
workerの異常終了で結果channelが切れた場合は、所有STAがpending対象を破棄する。
完了を取り出した後にEXE側pending IDと一致しない場合も、既に結果eventは消費済みなので
pendingを残さない。結果のIDでworkerを停止し、DLL側pending対象を取消してDiscardedを返す。
通信失敗・取消・期限・寿命上限で接続を閉じ、同じ要求を自動再送しない。
次の明示変換操作で閉じたworkerを外し、新しいworker/Channel/対象を予約できる。
既に取得した古い本文を新しい接続へ持ち越す処理ではない。

Channelのconnect/convert/close/Dropと子回収はworkerで行う。
Worker facadeのDropは停止通知だけで待たず、workerはEXEのコードだけを実行する。
COM対象・edit cookie・DLL参照・DLL callbackをworkerへ渡さない。
明示shutdownはwindow/TSF callbackを出た所有loopで最大3秒待ち、threadをjoinする。
facade破棄後の検査では、実thread handleを複製してthread終了を待つ。
終了eventの観測だけで全handleの解放まで終わったと仮定しない。

## 検証

2026-10-22 / Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- `broker-worker-test`は11項目。予約/結果保持中の拒否、同一peerと連続wire ID、古いCancel、
  別workerのPermit/ID/mode/原文/期限の拒否、予約取消、queued/保留取消、Ready拒否、期限、
  完了後取消、64番目の配送と次要求前のretirement、thread/facade終了後の結果容量、所有者Dropとhandle解放を確認。
- `broker-key-test`は25ケース。従来19件を再利用経路へ切り替え、通常の連続変換、古いCancel、
  配送中のReserve/drain/shutdown再入、2回目のEsc、本文変更後の復元、取消後の明示再開を追加。
  同じ子PIDと増加するwire ID、所有側の別要求ID、1回Undo、旧結果の不適用、TSF/COM解除・DLLアンロードを確認。
- 実DLLのSetTextからEXE bridgeへ再入し、配送中の操作拒否を確認する。fixture callbackは所有STA内だけで保持し、workerへ渡さない。
- 純粋Rustに要求IDの再利用/wrap拒否とbusy/古い解放の2件を追加。workspaceは63件（IPC 12件）。
- `Test-Spike.ps1`を省略なしで実行。fmt・Clippy 3構成・build、直接Channel、単発Broker、
  入力/追跡/composition/救済/通知/取消/終了、pipe/peerを確認する。
  最終ログはtarget/issue-4-reusable-worker-regression.log。利用者の文書・HTTP・クリップボードは使用しない。

broker-demo、tracking/composing/transaction/recovery-demoの通常変換は今回のworkerを使う。
直接アダプターのbroker-testは単発Broker経路を残し、旧経路と共通容量を回帰検証する。
一般アプリの実打鍵・Undo・DLL寿命、常駐共有探索・署名/配置、個別Cancel後の同じ接続の継続、
動的policy通知・HTTP・同意・設定・資格情報は未完了。Issue #4は継続し、PRはdraftを維持する。

実装根拠：[worker.rs](../../crates/sumibi-ipc/src/worker.rs)、
[broker_bridge.rs](../../crates/sumibi-setup/src/broker_bridge.rs)、
[worker_probe.rs](../../crates/sumibi-setup/src/worker_probe.rs)、
[broker_keys.rs](../../crates/sumibi-setup/src/broker_keys.rs)。
