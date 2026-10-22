# ADR 0015: Brokerの受取・破棄まで保持する容量予約

2026-10-22 / Issue #4 / PR #10。対象は[単発Broker](0004-one-shot-broker-and-owner-edit.md)と
[診断EXE bridge](0005-dll-key-broker-bridge.md)。常駐Brokerや一般アプリの入力待機行列は引き続き未実装。

## 問題と判断

従来のReservationはworkerだけが保持し、worker終了時に枠を返していた。
呼出し側が完了済みBrokerJobや取り出したCompletionを保持していても、次の4件を開始できた。
各channelが容量1でも、未受取の結果・event・jobのprocess内合計を4件には制限できなかった。

予約1件をArcで共有し、worker・BrokerJob・Completionの最後の所有者が解放されるまで数える。
未起動Reservationも含め合計4件を維持する。workerの終了、shutdownによるjoin、結果の取出しだけでは枠を返さない。
Reservation自体も非公開fieldを持たせ、外部から未計数の値を直接構築できないようにする。
各所有者の終了順序が変わっても、実行中のworkerや元の結果bufferが残る間は新たな予約を許可しない。
結果本文をleaseから切り離してmoveしないよう、Completionは非公開のoutcomeを持ち、
`outcome()`はborrowした`&str`か固定Errorを返す。Debug/Cloneは追加しない。
呼出し側が明示的に作る本文の複製までをこの予約で追跡する設計ではない。

BrokerJobを先にDropした場合もworkerは同じ予約を保持する。Dropは従来どおり取消通知だけで待たない。
送信先消滅でtry_sendが失敗した場合は結果も破棄する。起動前検査やthread生成の失敗もRAIIで枠を返す。
TSF対象、cookie、COM参照、callbackは予約に含めず、workerへ追加しない。

## キーと結果配送

DLLの変換キーは引き続きReserveを本文取得より先に呼ぶ。満杯ならキーを消費せず、要求や原文を待機させない。
所有loopは単一結果を受け取り、元のID・版・入力先を再検査して適用または破棄する。
bridgeは外部COM呼出し前にjobをRefCellから外すが、その呼出し中はCompletion自身の予約が存続する。
結果受取中の再入でもprocess内の上限を迂回できない。解除後は同じ入力欄で新しいキー操作を受け付ける。
古いキーや原文を後から自動再送しない。

この4件は現在のEXE内の単発ジョブ容量であり、製品の遅延キー容量案1024件・16384 UTF-16単位や、
救済のprocess合計容量の採用ではない。一般アプリのDLL配置とBroker共有には別途設計・検証が必要。

## 検証

2026-10-22 / Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- 純粋Rustに2件追加。16 threadの同時予約でも4件まで。所有者を先に破棄してもworker相当の
  保持者が解放するまで枠が戻らず、最後の解放後に正確に1枠戻る。workspace合計59件（IPC 8件）。
- broker-testに実子processを使う容量6ケースを追加（従来のTSF 19ケースも維持）。
  正常結果とpolicy拒否それぞれで、完了・join後の未受取4件、取り出した結果4件を保持してjobを破棄した状態、
  1件解放・起動前拒否・全解放後の再利用を確認する。結果は各jobで一度だけ取得できる。
- broker-key-testは19ケース。未起動予約・完了済みjob・取り出した結果の3種類で満杯にし、
  同じ実DLLのキーcallbackが本文取得前に拒否し、保留要求・Undoを作らないことを確認する。
  容量を解放すると同じ入力欄で変換でき、1回Undoで原文へ戻る。物理キー操作の試験ではない。
- Test-Spike.ps1を省略なしで実行。fmt、Rust 59件、Clippy 3構成、build、既存native検査、
  TSF/COM解除・DLLアンロード、pipe/peerを確認。全回帰ログはtarget/issue-4-job-admission-regression.log。

実装根拠：[job.rs](../../crates/sumibi-ipc/src/job.rs)、
[broker_bridge.rs](../../crates/sumibi-setup/src/broker_bridge.rs)、
[broker_host.rs](../../crates/sumibi-setup/src/broker_host.rs)、
[broker_keys.rs](../../crates/sumibi-setup/src/broker_keys.rs)。
