# ADR 0003: 起動ハンドルへ結び付けたデスクトップIPC

2026-10-21 / Issue #4 / 通常デスクトップの基盤と専用2プロセス診断を実装。
製品Brokerの発見・起動、TSF配送、インストールまで完成した判断ではない。

## 認証の基準

`sumibi-ipc`は、信用できる起動経路から受け取ったプロセスハンドルを基準にする。
相手が送ったPIDをOpenProcessして信用するAPIは公開しない。
PinnedProcessはハンドルをquery/synchronize権限だけで複製し、PID・作成時刻とトークンの初期状態を保持する。
pipeのkernel PIDをその相手と照合し、同じPIDから開いた実プロセスの作成時刻とトークンも再検査する。
照合の前後に元の固定ハンドルの生存を確認し、PIDの再利用だけで別プロセスを受け入れない。

確認するトークン情報はユーザーSID、ログオンSID、Windowsセッション、integrity、
AppContainerと昇格状態。実行中の自プロセスとも一致させる。
初期対応はmedium integrity（0x2000）、非昇格、非AppContainer、非impersonation threadに限定する。
不明な情報・照会失敗は拒否する。threadのidentify level tokenも許可しない。
トークンが既知の対応状態であることを確かめてからアプリケーションI/Oを公開する。

SIDが同じだけ、名前が存在するだけ、同じexeのパスだけでは受け入れない。
受信したHello/Readyや公開nonceもOSプロセスの証明ではない。
この実装は署名・配置の検証、同一ユーザーの侵害プロセスへの完全耐性を保証しない。
製品の起動元がどのハンドルを信頼してよいかは、配布・起動方式と合わせて引き続き確定する。

## pipeと起動時の受渡し

pipe名の末尾とprotocolの識別子はBCryptGenRandomの128bit値から作り、ゼロ値は拒否する。
bootstrapから受け入れる末尾は32桁の小文字16進数だけ。パスやリモート名を受け取らず、
固定のローカル名前空間へ組み立てる。これらは公開の照合値であり秘密の認証トークンではない。

serverにはログオンSID限定のprotected DACL、PIPE_REJECT_REMOTE_CLIENTS、
FILE_FLAG_FIRST_PIPE_INSTANCEを指定する。pipe instanceは1つ。
clientはSECURITY_SQOS_PRESENT / SECURITY_IDENTIFICATIONを指定し、
想定外serverがclientをimpersonateする権限を与えない。
clientは接続先を照合するまでHelloも送らず、serverはclientを照合するまで本文を読み取れない型にする。
同名の先取りや接続数の妨害は成功扱いにせず、その接続試行を失敗させる。

`sumibi-setup ipc-peer-test`は親をserver、同じ実行ファイルから起動した子をclientとする専用診断。
親はCreateProcessが返す子ハンドルを固定し、子へは親のquery/synchronizeハンドルと
終了イベントだけをPROC_THREAD_ATTRIBUTE_HANDLE_LISTで明示継承する。
子は継承フラグを直ちに解除して、再継承しない複製を保持する。
CREATE_NO_WINDOWで起動し、shellを経由しない。相関用のpipe末尾・Broker起動ID・
数値ハンドル・固定試験名はbootstrap引数へ渡すが、引数だけでは認証にならない。
SID、pipe名、nonce、本文、資格情報を出力しない。異常時は自分で起動した子だけを終了させる。

この親子の起動契約は、既存アプリにロードされたTIPが製品Brokerへ到達するための
最終的な起動・発見方式を決定した意味ではない。

## 非同期I/O・上限・停止

server/clientともFILE_FLAG_OVERLAPPEDを使い、Connect/Read/Writeに個別のmanual-reset eventを持つ。
PIPE_NOWAITやFlushFileBuffersによる待機は使わない。
Operationはendpointを排他的に借用し、同時に1操作だけを開始できる。
workerが開始した操作をevent・停止event・相手process handleで待つ。TSFスレッドでwaitしない。
clientのopenはbusy/missing時に待機・再試行しない。

| 上限 | 値・適用範囲 |
| --- | --- |
| endpoint | プロセス合計8。serverの作成／client open前にpermitを確保 |
| pipe instance | 名前ごとに1、先取り拒否 |
| kernel要求バッファ | 入出力各4096バイトを指定。Windowsの内部割当量そのものの保証ではない |
| 読取操作 | 4096バイト。上位Decoderへ少しずつ渡す |
| 書込操作 | 最大262148バイト（frame上限＋header）。所有Vecを受け取り、空・過大は開始前に拒否 |
| 同時I/O | endpointごとに1。内部に送信・完了の待ち行列を持たない |
| 期限 | 単調時計の絶対期限、現在から最大60秒。部分書込も同じ期限を共有 |
| 取消drain | 最大1秒。停止済み・期限切れはI/O開始前にも拒否 |

OVERLAPPED本体とバッファを固定heapに置き、成功・失敗・dropの全経路で寿命を保つ。
取消はCancelIoExだけで完了扱いにせず、eventとGetOverlappedResultで終了を確認する。
途中取消・期限・破棄はその接続を閉じた状態にし、残りの本文を次の要求として再送しない。
ドライバーの完了を1秒以内に確認できなければRetiredで失敗させ、kernelが参照し得る
バッファ・event・pipe・process handleをプロセス終了まで保持する。
そのendpointのpermitも返さず、閉じたPipeから別操作を開始できないため、保持は合計8件までに限定される。
この保守的な未完了保持経路はコード上の安全策で、実ドライバーの完了不能を再現した試験ではない。

endpoint上限はこのcrate経由のhandleに適用する。Broker全体のworker数、TSF結果キュー、
追跡入力の容量予約・救済保持を完成した意味ではない。製品側も消費前に容量を予約する必要がある。

## 検証（2026-10-21）

通常ユーザーの実行で、warmup後に以下の14確認が成功した。

1. 2プロセスが相互の実PID・作成時刻・トークンを照合し、Hello/Ready→Convert/Result→Core.finishを通過。
2. clientに固定された想定serverと、実serverが異なる場合はHello前に拒否。
3. serverに固定された想定clientと、実clientが異なる場合は本文読取前に拒否。
4. OS peerが正しくてもBroker起動IDが違えばConvert前に拒否。
5. 停止済みの書込は開始せず、相手側でも1バイトも来ないことを確認。
6. 期限切れの書込も同様に非送信。
7. 実際にpendingのreadを停止eventで取消し、完了をdrain。
8. pending readの期限切れとdrain。
9. pending readをdropしたときの取消と接続失効。
10. 相手processの終了でpending readの待機を解除。
11. 相手が読まない状態でpending writeを取消。
12. clientのいないaccept待機を停止eventで取消。
13. endpoint上限の拒否と解放後の再作成。同名serverの二重作成拒否も各接続で確認。
14. warmup後の親process handle数へ復帰。正常経路もwarmup後にもう一度実行。

純粋な単体4件では、ユーザー／ログオンSID・session・integrity・AppContainer・昇格の
差の拒否、同じ数値PIDで作成時刻が違う場合の拒否、permitとnonce入力制限を検査する。
実際にPIDを再利用させたり、別ユーザーでログオンしたり、リモート端末から接続した試験ではない。

Coreでの照合対象は子の合成Targetと固定結果だけ。実TSF編集・入力欄の本文をこのpipeへ接続していない。
製品Brokerのprotocol dispatch、非同期workerから所有TSF threadへの配送、composition・スコープ分類・
一般アプリのUndoを、Issue #4の次段階として残す。AppContainerや昇格のためにDACLを広げない。

ログは `target/issue-4-peer-verification.log`、単体・既存TSF診断は `target/issue-4-peer-core.log`。

## APIの根拠

- [Token information](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-gettokeninformation)
- [Server PID](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeserverprocessid)
- [Restricted inheritance](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)
- [Overlapped pipe I/O](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-server-using-overlapped-i-o)
- [CancelIoExの寿命契約](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelioex)

資料のAPI契約と、この診断で実測した範囲を区別する。
