# ADR 0016: 認証済み子Brokerを再利用する有限接続

2026-10-22 / Issue #4 / PR #10。前提は[固定process認証](0003-pinned-desktop-peers.md)と
[結果解放までの容量予約](0015-broker-admission-through-delivery.md)。worker向け通信APIと専用の自動検証を追加する。
本ADR時点の入力デモは単発Brokerを使う。後続の[ADR 0017](0017-reusable-worker-and-owner-delivery.md)でEXE workerと所有STA配送を接続した。今回のChannelは、製品用の常駐Broker探索・一般アプリへの配送とは別の実装段階。

## 起動と認証

`Channel::connect`は呼出し側が信頼する絶対パスから子を明示起動し、同じpipeと子processを複数要求で使う。
bootstrapの`serve-channel`で起動方式を区別し、従来の`serve`は単発経路のままとする。
wire版1やJSON形式は変更しない。pipe名・起動IDは毎回乱数で生成する。

継承するのは親process query/synchronize、停止event、起動完了eventの3個だけ。
Launched/PinnedProcess/Pipeを共有し、Hello前と各I/Oで従来のkernel PID・作成時刻・user/logon SID・
session・medium integrity等を照合する。起動元と通信相手は実process objectへ固定する。
名前・コマンドライン・Readyの一致だけで相手を信用しない。既定Hostの変換拒否も維持する。
固定結果にはspike featureとDiagnosticFixed起動の両方が必要で、DisabledのReady後は本文を送らず接続を終了する。

これは起動時に信頼済みパスを指定する方式であり、署名・配置検査、ログオン単位の共有探索、
別アプリからの再接続、製品の同意・資格情報を代替しない。

## 要求と資源の所有

| 資源 | 今回の上限・規則 |
| --- | --- |
| 接続 | 単発ジョブと同じprocess内4枠を共有。idle Channelも数える |
| 要求と結果 | 接続ごとに同期1要求。Reply保持中は次要求を拒否し、送信・ID消費を行わない |
| 本文 | 原文・結果とも1024 UTF-16単位。待機本文キューを作らない |
| 接続寿命 | 最大64要求、server idle 60秒、要求受付/実行は最大10分。最終返信後は最大10秒の終了待ち。自動再起動・再送なし |
| 待機 | 接続期限は最大60秒指定、handshakeは最大10秒。要求期限も最大60秒の絶対単調期限 |
| pipe | 既存の合計8 endpoint・同時1操作・取消完了までbuffer保持を維持 |

Channelは`Reservation`をArcで保持し、Replyにも同じ予約を渡す。Channelまたは元のReplyが残る間は枠を返さない。
closeは通信と子の終了を行うが、予約解放にはfacadeとReplyのDropが必要。
本文はReplyに非公開で置き、`text()`で借用だけを返す。Debug/Clone/serializationを追加しない。
呼出し側が明示的に複製した本文までを容量計数するものではない。

clientは新しい接続IDと起動IDでHello/Readyを一度だけ交換し、要求ごとに接続内IDを増やす。
serverは同じsession/generationのConvertだけを受け入れ、重複・過去・wrapした要求IDと
設定/認可版の不一致を拒否する。返信はIDと全revisionをclientが照合する。
Reply.requestは接続内wire IDであり、TSF所有側の要求IDや入力先IDの代用にしない。
不正引数は送信前に拒否し、その後の正常要求でIDを飛ばさない。

## 取消・失敗・終了とスレッド

停止eventは起動、I/O、診断変換待機、serverのidle readに共通で、保留readの後ろへCancelを積まない。
今回の取消は接続全体を終了する。取消した接続を再利用して古い結果を回収しない。
I/O・protocol・policy不一致・期限・寿命上限による失敗でChannelを永久に閉じる。
再利用には呼出し側が明示的に新しいChannelと対象要求を作る必要があり、自動再試行はしない。
直前のReply保持によるCapacityと、不正引数の事前拒否だけは接続を維持する。

serverは最後の返信直後にprocessを消さず、clientが認証付きreadを完了するまで停止event等を有限時間待つ。
close/Dropは従来のLaunched guardで子を停止・回収する。取消待ちに反応しなければ、その起動handleの子だけを終了する。
OS失敗時にもprocessの消滅を保証する、という意味ではない。

Channelのconnect/convert/close/Dropはすべて**blockingのworker専用API**。
TSFやwindow callback内で呼ばない。COM対象・cookie・DLL参照・callbackを保持しない。
専用検査も全処理をworkerで行う。製品のEXE workerへのdispatch、所有STAへの結果配送、
本文取得前のキー容量予約との接続は次段階。既存Bridge/BrokerJobをこのAPIへ切り替えた段階ではない。

## 検証と限界

2026-10-22 / Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- `broker-channel-test`は実子processで14項目を確認。Disabled拒否、同じ起動peerでの連続要求、
  Reply保持による拒否とID維持、事前拒否/最大UTF-16、設定版/認可版不一致、要求前/保留中の取消、
  保留期限、送信前期限切れ、64要求上限、単発ジョブと共通の4枠/close後Reply、起動拒否後の枠と親handle解放を扱う。
- `broker-channel-denied-test`は通常ビルドHostで2項目を確認。DisabledのReady拒否と診断変換器の拒否。
  Test-Spike.ps1は通常Host検査後にspike Hostをビルドし、連続要求を検証する。
- 純粋Rustにserverの要求順序・wrap拒否とpolicy拒否の2件を追加。workspaceは61件（IPC 10件）。
- 全回帰は省略なしのTest-Spike.ps1。fmt、Clippy 3構成、build、従来の入力/救済/容量/取消/終了、
  TSF/COM解除・DLLアンロード、pipe/peerを確認。ログはtarget/issue-4-broker-channel-regression.log。
- 保留期限では、clientとserverのどちらが先に期限へ達するかによりTimeoutまたはDisconnectedとなる。
  いずれも接続を破棄し、結果を適用・再送しない。60秒idle・10分全体の実時間経過試験は未実行。
- 初回の全回帰は既存broker-key-testのfocus-restoredでGetFocusの一致検査が一度失敗した。
  単独実行と、検査条件を変えない全回帰の再実行は成功。原因は未特定で、フォーカス検査を緩和していない。
  初回ログはtarget/issue-4-broker-channel-regression-focus-failure.log、単独ログはtarget/issue-4-broker-channel-focus-check.log。

対象は合成本文と明示起動した専用子process。一般アプリの実打鍵・Undo・常駐共有dispatch、
同じ接続内での個別Cancel継続、動的policy通知、HTTPは未実装。Issue #4とdraft PRを維持する。

実装根拠：[channel.rs](../../crates/sumibi-ipc/src/channel.rs)、
[bootstrap.rs](../../crates/sumibi-ipc/src/bootstrap.rs)、[broker.rs](../../crates/sumibi-ipc/src/broker.rs)、
[channel_probe.rs](../../crates/sumibi-setup/src/channel_probe.rs)。
