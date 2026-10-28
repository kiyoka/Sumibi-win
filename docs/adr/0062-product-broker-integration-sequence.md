# ADR 0062: 製品Brokerの統合順序と完了境界

2026-10-28 / Issue #4 / PR #10。状態: 実装計画。以下の製品経路は未接続。

## 現在の境界

`sumibi-ipc`には固定process handleによる相互認証、有限な非同期pipe、
最大8件の進行中I/Oから完了可能な接続を選ぶ待機処理、単発Job、再利用Channel、
診断専用の2接続Brokerがある。
診断の政策通知は有限な`WriteBatch`で先に2本の書込を開始し、完了した
接続ごとに結果を返す。部分書込の残りは同じ接続で再開して、次回は別接続を
先に確認する。開始・完了に失敗した接続だけの結果も返せる。Hostライブラリでは
3接続と受付差替えを所有側ループで確認したが、製品Host EXEの継続I/O調停ループは
まだ接続していない。
`AcceptTask`は固定processを認証するOS acceptを有限期限の専用threadへ置き、
所有側のpollに認証済みpipeだけを返す。子Brokerは2本の受付を並行開始し、
2本目が未接続の間に1本目のHelloとReadyを完了する。所有側では次の受付を
待ちながら既存接続の政策失効・再許可とworker完了を処理できることを実pipeで確認。
共有pipe groupは2～8 instanceを同じACLとendpoint枠で作る。Hostは生存中の
認証済み接続から、次の固定peerを対象に`begin_replacement_accept`できる。
3接続の有効化・結果配送、1接続の切断後の4件目の受付を実pipeで確認した。
唯一のclient processが退出した後も認証済みserver handleを保持していれば
同じgroupの次の受付を作れる。
誤った同ユーザーprocessの受付失敗は既存接続を切断せず、生存接続から差替えを作る。
OS受付が期限内でも所有側の結果回収が期限後なら、その認証済みpipeを公開しない。
shutdownでは全受付taskのStopを先に立て、受付workerと変換workerの実際の退出を
同じ有限期限で確認する。未終了taskは保持して再試行し、終了前に同じpipe名の
Brokerを起動したと報告しない。
製品Hostの信頼できるpeer発見と、全接続が消えたときの安全な再起動は未接続。
`BrokerWorkLoop::advance_with_accept`はHost政策の公開を先に確認し、完了済み受付を
一件だけ取り出してkeyを予約し、受付が未完了ならworkerと既存pipeを進める。
受付の期限切れは`AcceptFailed`として返し、他の認証済み接続は維持する。
`advance_manual_with_accept`は同じ政策元を確認してから現行宛先の送信workerへ渡す。
異なる政策元なら受付taskを消費せず、正しい元で後から同じpipeを受け付ける。
設定変更の受領が接続済み・未予約のpipeより先に完了した場合、そのpipeのReadyは
受領済み最新版だけを含む。
診断子Brokerの初期Hello/Ready、政策通知、2要求のworker結果もこの所有側入口を使う。
政策失効では`acknowledge_version`が同じpoolの取消workerを回収し、版の配送と
実際のworker終了を確認してから再許可後の次要求へ進む。
連続する取消回収では退役済み完了をchannelから排出し、生存中の完了だけを
所有側へ保持する。これにより古い完了が8件のchannelを占めても次のworkerの
終了を妨げない。
破棄した受付のOS枠は
有限期限で戻り、生存中の別接続から追加instanceを作り直せることを確認した。
別の実processを固定した受付ではそのprocessの接続を受理し、同じlogon SIDの
別processからの接続を`WrongPeer`で拒否した。子Brokerの2本は引き続き同一親process
からの接続であり、異なるTIP間の共有・発見までは示していない。
Hostの単体結合検査では、一つの`BrokerPolicyRuntime`に親processと別の子processを
それぞれ固定し、子processのHello・Convert・結果を通した。子process終了後も親側の
既存接続で次要求を処理した。子fixtureのclient側は標準Fileで接続しているため、
この検査だけではBroker側processの相互認証を示さない。製品TIPを対象にした
配置・共有発見と所有TSFへの結果配送は引き続き別の受入条件である。
共有Hostの長寿命受付には、信頼できるTIP process発見と接続ごとのpeer固定が必要。
`WriteBatch`が返すのは投入時の位置であり、製品の呼出し側がその位置を
接続key・世代・政策版・Broker起動IDへ結び付けて完了を処理する。
受信側は`FrameInbox`で分割・連結frameの未読部分を接続ごとに保持し、
`ReadBatch`で最大8本のReadを先に開始でき、実pipe単体検査で遅い接続との独立性を
確認した。子process診断のHelloとConvertは所有側の`poll_once`で接続を巡回する。
`poll_active_message`は分割frameの途中でも別接続の
完成frameを先に処理でき、部分frameを10秒で退役させる。診断では失効・
再許可後の次要求に使用したが、製品の継続調停ループには未接続。
`poll_reserved_hello`はHello途中にも政策公開を挟める。2本の部分Helloを
公開時に切断してreceiptを完了する実pipe検査を通した。
OS受付時にFanoutへ接続keyを予約するため、先に受け付けたHelloが分割されていても
後のHelloを先に有効化できる。予約の退役後もkeyは再利用しない。
`poll_once`は公開を先頭に、期限切れ、Hello、active frameを順に1件進め、
active接続は巡回して読む。診断の再許可後要求にこの入口を使う。
Hello待ちとactiveは合わせて最大8本である。
Host側の`BrokerWorkLoop`は政策公開、完了回収、`poll_once`とworker投入を一つの
所有側入口で進める。診断の再許可後要求に接続した。取消済みworkerが8枠を占めても
新要求へ固定エラーを返す。製品の継続acceptとHTTPS送信adapterはまだ接続していない。
`accept`は認証済みpipeのkey予約前に未公開政策を配送する。`advance_manual`は
Runtimeとworkerが同じHost政策元を参照することを確認し、現行宛先を使って
`convert_ordered`へ渡す。実loopback HTTPの成功、503、失効、宛先変更を通した。
製品HTTPS adapterと長寿命のOS accept調停は引き続き必要である。
`sumibi-protocol`には接続ごとの`BrokerSession`と、最大8接続・各1通知の
`PolicyFanout`がある。`sumibi-host`には設定・資格情報・明示同意の
`PolicyAuthority`、送信開始順序を固定する`manual::convert_ordered`、
Hostの版をBrokerの受付ゲートとFanoutへ渡す`BrokerPolicyRelay`がある。
Host側の`BrokerPolicyRuntime`は、認証済みpipeのReady、有限な政策通知、
失敗先の物理切断と配送receiptを一つのworker側で処理する
（[ADR 0063](0063-host-policy-runtime.md)）。製品Host EXEの主ループは未接続。
Convert受付時の`JobTicket`は接続単位のStopイベントを持ち、失効・期限・
切断・不正frame・終端で取消される。有限な`BrokerWorkers`へ診断workerを渡し、
失効したworkerの完了破棄と回収を子processで確認した。製品HTTPS workerは未接続。
登録不要の`spike`診断分岐ではHost EXEからこの実行部を起動し、各OS accept
直後の接続key予約、2接続への政策配送、許可中の結果と失効時の取消を実pipeで
検査した。既定Hostは同じ診断モードを拒否する。結果公開はHost政策元とBroker
ゲートを最大500msの書込完了まで保持し、別Broker起動IDの遅延完了を拒否する。
ReadyもHost政策元のguardを最大500msの書込まで保持し、変更が先に確定した
旧版Readyを拒否する。
Hostの単体結合検査では、Relayの公開通知を同一processの認証済み2本の
実pipeへ書き、各完了で配送receiptを進め、client側の受信版を照合した。
片方が通知前に閉じた場合も、生存側の配送と失敗先の退役を維持し、
失敗先のserver pipeを実際に閉じるまでreceiptを完了させなかった。
実子Brokerの2接続検査は、同じ親processから開いた2本のpipeである。
異なるTIP processによる共有Hostの発見と接続、Hostの政策元から実pipeへの
配送、設定UIの完了通知はまだ製品で結ばれていない。

## 統合の責務

`sumibi-host`のEXEを製品Brokerの構成起点とする。Hostが一つの
`PolicyAuthority`と`BrokerPolicyRelay`、Broker受付ゲート、Fanoutを所有する。
`sumibi-ipc`は認証済みtransport、有限I/O、停止・期限・peer終了を扱い、
資格情報やTSF対象を所有しない。`sumibi-protocol`はOS参照を持たず、
接続と要求の順序を判定する。

Brokerは接続keyとpipeを対応付け、OS peerを検証してからHelloをFanoutへ渡す。
Readyの完全な書込後にだけ`ready_delivered`を呼ぶ。接続keyはOS pipeを受け付けた
時点で発行し、不正Helloで閉じた接続のkeyもBrokerの寿命中に再利用しない。
同じclient寿命IDの再接続は新しい世代と単調増加した要求番号を
要求する。接続数、待機要求、I/O、通知の各容量を受付前に確保する。
認証失敗、Ready書込失敗、不正frame、切断、通知書込失敗は対象接続だけを閉じる。
Brokerの政策元や状態機械そのものが失敗した場合は、本文受付を停止する。

要求受付、Fanoutの変更、結果の準備、政策の公開順序は一つの調停点で決める。
pipeの読書き中にFanoutを借用し続けない。各接続のI/O作業は有限な件数と期限で
処理し、完了を接続key・世代・政策版付きで調停点へ返す。遅い接続のI/Oが他の
接続の通知配送を無期限に止めない。通知frameは一度だけ取得し、書込が完全に
終わった版だけを`notice_delivered`へ渡す。失敗した接続は退役・closeし、
同じ通知を再送しない。結果の書込はHost政策元の`authorize_result`と
`BrokerPolicyGate::authorize_for_publish`のguardを完全な書込まで保持し、
政策変更との順序を固定する。調停点のFanout
操作と結果側のguard取得でlock順序を逆にしない。

Hostの設定変更はRelayが最新版へまとめて公開する。公開後に退役keyのpipeを
閉じ、生存接続の通知書込または切断を記録する。設定UIに「変更完了」を返す
条件は、Hostの保存だけではなく、受付ゲートが該当版へ進み、変更時点の対象
接続がその版を受け取るか切断されたことである。途中で新しい版に置き換わった
場合も、古い版の通知が届いたと偽らず、最新版の配送・切断を完了条件にする。
新しい接続はその時点の最新版でReadyを受ける。

`BrokerPolicyRelay::publish_current`は公開直前の接続keyを有限な
`DeliveryReceipt`にBroker起動IDと共に保存する。Ready書込中の接続と、
公開時に退役した接続も
対象に含める。通知frameの完全な書込をFanoutへ報告した後、またはOS pipeを
実際に閉じた後にだけreceiptを進める。後続の版の配送は先の版のreceiptも
満たせるが、receiptはその場合`Superseded`を返し、古い版そのものが届いたと
報告しない。単なるFanout退役やHost保存だけでも完了としない。設定UIは完了後に
最新の政策元を読み、表示内容を決める。receiptへの待機・通知経路は、製品Hostの
I/O調停点で接続する。
別Brokerの同じ接続keyからの遅延完了は、起動IDが違うため拒否する。

Hostの送信開始guardは、診断loopback HTTPの資格情報headerと本文の
書込を失効と順序付けた（[ADR 0064](0064-outbound-send-start-order.md)）。
製品HTTPS adapter、worker I/Oの取消、設定UIの完了待機への接続は残る。

### 実送信と失効の順序

製品transportは最初の要求byteを書き始める直前を`SendStarted`の順序点にする。
Hostの政策元が許可版・接続key・要求番号・資格情報の同一性を確認し、
その順序点までの間に失効が確定した場合はheaderも本文も送らない。順序点より先に
失効が来た場合はworkerとsocketを取消し、旧結果の公開を拒否する。ただし
すでに送信されたbyteは取り戻せないため、UIへ「失効前から進行中の送信を
取り消した」と「未送信の要求を拒否した」を混同して報告しない。

設定UIの操作完了は、政策版の保存、Brokerの受付停止、対象接続への通知または
切断に加え、旧版の未送信workerが送信開始できない状態になったことを確認する。
workerを止めてもOS I/Oが残る場合は、そのI/Oの終端または有限期限を待つ。
送信開始と失効を別lockで判断したり、snapshot検査の成功だけを送信許可と
見なしたりしない。検査では、接続直後・header書込直前・本文書込直前の失効、
送信中の取消、同じbyteの資格情報差替え、設定の失効後再許可を意図的に
割り込ませ、本文の送信回数とUI完了時点を観測する。

Brokerの結果だけではTSF編集を許可しない。所有STAに戻した後、context、
保持範囲、同長別本文、選択、保護条件、入力・設定・認可版を再検査して
一度だけ適用する。対象を失った要求は再送しない。

## 実装と検証の順序

1. Hostの調停点と接続key付きの有限I/O完了経路を作る。有限の待機処理と
   現行の2接続診断を
   使い、Ready失敗、通知失敗、遅い接続、取消、期限切れ、再接続が他接続を
   巻き込まないことを検査する。
2. 一つのHost内でPolicyAuthorityの変更をRelayから実pipeへ届ける。
   設定・認可版だけの変更、失効と再許可、結果公開との競合を含める。
   UI完了条件と切断後の再接続版を固定する。
3. 異なるTIP processから同じHostへ接続する発見・起動・相互認証を作る。
   パスやpipe名だけを信頼の根拠にせず、配置・署名・更新時の所有関係を
   [ADR 0045](0045-machine-registration-boundary.md)と合わせて検査する。
   AppContainerは別の成立確認まで拒否する。
4. 所有TSFの結果適用前再検査と製品キー配送を統合し、専用storeから
   一般アプリへ広げる。Notepad等で実打鍵、1回Undo、別欄・同長別本文・
   読取不能の非適用を確認する。

この順序では恒久的なTSF登録を先行させない。実APIと資格情報の永続保存も
Issue #4の診断成功から有効化しない。現時点の2接続検証の範囲は
[ADR 0061](0061-two-client-broker-pipe-diagnostic.md)に記録する。
