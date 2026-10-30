# ADR 0061: 二接続の実pipeでBroker通知順序を検査する

2026-10-26 / Issue #4 / PR #10。

`PolicyFanout`とHostの公開境界は複数接続を扱えるが、従来の実子Broker診断は
一つのpipeだけだった。各接続の認証、Readyと政策通知、片方の不正要求後の
継続をOSのpipe I/Oまで通す診断を追加した。

診断専用の`serve-fanout`は、同じランダムな名前で2つのnamed pipe instanceを
作る。先頭だけに`FILE_FLAG_FIRST_PIPE_INSTANCE`を付ける。既存の単発Jobと
Channelは引き続き最大1 instanceとし、この診断だけ最大2 instanceにする。
3つ目のinstanceの作成も実際に拒否されることを確認する。
各instanceは従来と同じlogon SID限定ACL、ローカル限定、overlapped I/O、
endpoint予約を使う。受入れ後には両方のpipeについて、起動時に固定した親の
process handleとkernel報告PID、作成時刻、user/logon SID、session、
integrityを照合する。同じ名前のinstanceでも、どちらが先に接続するかは
仮定せず、認証後のHelloのsession IDで診断用の論理接続を割り当てる。
HelloのID自体はOS peer認証の代わりではない。

診断では2つのClientSessionへReadyを送り、一方に保留中の要求を作る。
最初に片方の100ms要求だけを実時間で期限切れにし、timer処理より先に
届いた旧Worker結果を破棄する。接続は維持する。
他方の5秒要求は結果を受け、期限切れ側も次の要求番号で続行できる。
認可版2への失効を両方へ配送し、保留要求と古い結果を破棄する。認可版3で
再許可した後、他方のclientが新しい版で要求し、固定結果を受ける。
両接続の政策通知は2本のoverlapped書込を先に開始し、最大8件のI/Oから
完了可能な接続を選ぶ有限の待機処理を通す。部分書込があれば残りを同じ有限期限で
書き切り、
完全な配送後にだけ各接続の通知を承認する。これは同時に開始できるI/Oの
確認であり、遅い任意clientを独立に監視する製品調停点の完成ではない。
続いて最初のclientが新しい要求を保留したまま、他方が未発行番号のCancelを
送る。Brokerはその接続だけを退役・closeし、認可版4の失効を残るclientへ
届けて保留要求を取り消す。clientは退役したpipeの切断も確認する。
親側の2本の接続を入れ替えた順序でも各診断を行う。
もう一方のclientが保留中でも、有効なCancelは自分の要求だけを取り消す。
その旧結果を捨て、同じ接続の次の要求を成功させた後、未来番号のCancelで
その接続だけを退役させる。Cancelと次要求のframeが一度のpipe読取へ
合流しても、診断側の有限な受信bufferで順に処理する。

未配送通知を持つ接続を次の認可変更時に退役させる診断も行う。
認可版2の失効を片方だけに書き込み、もう片方の版2通知を意図的に未配送にする。
版3の再許可時には未配送側だけを退役・pipe切断し、継続側へ版3を届ける。
継続側は新しい要求の結果を受ける。これは実pipe上の退役・切断を検査する
固定fixtureであり、任意の遅い外部clientを監視する製品accept loopではない。
別の診断では、片方がHelloを送った後、Readyを受け取る前にpipeを閉じる。
Brokerはもう片方へのReadyを完了させた後、切断先へのReadyの`WriteFile`が
失敗することを確認し、その接続だけを退役させる。生き残った接続の新しい
要求は固定結果を受ける。
さらに片方が誤ったBroker起動IDのHelloを送った場合、その認証済みpipeだけを
拒否・切断する。もう片方の正しいHelloはReadyと結果へ進む。

別の診断では、Ready後に片方のclientがpipeを閉じる。Brokerはその切断を
閉じた後にもう一方のclientが要求を送る。Brokerは切断した接続をまだ通知先として
保持し、失効通知の`WriteFile`が切断エラーを返すことを確認して
その接続だけを退役させる。残るclientには失効と再許可を順に届け、古い要求を
取り消し、新しい認可版の要求にだけ固定結果を返す。失敗した通知は再送しない。
退役したinstanceを同じ子Broker内で作り直し、同じclient寿命IDが世代2で
再接続する。OS peerを改めて認証した後、現在の認可版3でReadyを送り、
再接続先と生き残った先の両方で新しい要求の結果を確認する。再接続は
元の要求の自動再送を含まない。切断前に再接続側の要求1を完了させ、
要求2を保留したまま切断する。失効後の旧結果を捨て、再接続後は
要求3だけを新しく送ることも実pipeで確認する。
追加instanceは認証済みで生存中の同じペアのserver pipeからだけ作る。
名前だけを渡してserver pipeの所有を省く入口は設けない。
先頭instanceは初回作成時に名前の占有を確認するが、2本のclient割当順は
固定しない。再作成時もOS peerの相互認証を行う。
診断をウォームアップ後に繰り返し、親processのhandle件数が元へ戻ることを確認する。

通常Hostはこの診断serviceを拒否し、`spike` featureを明示したHostだけが
固定本文を扱う。通常Hostによる拒否は5種類の診断モードそれぞれで検査する。
`tools/Test-Broker.ps1`は拒否、2接続、既存Channel/Workerを
順に実行し、終了時に通常Hostへ戻す。マシン全体のTSF登録と実API送信は
行わない。

これは一つの親processから開いた2本のpipeを検査する。異なるTIP processの
信頼できる発見・配置、常駐Brokerの一般的なaccept loop、Hostの設定変更元、
設定UIの完了条件、所有TSFでの適用前再検査はまだ製品へ接続していない。
Windowsの複数instanceと早着接続の扱いは
[Named Pipe Operations](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-operations)と
[ConnectNamedPipe](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-connectnamedpipe)に従う。

2026-10-28追記: このADRの`serve-fanout`は引き続き2 instanceである。
別の`policy-runtime`診断はBroker自身のアンカー1件と親process向け2件の
3 instanceへ移した（[ADR 0065](0065-authenticated-shared-broker-listener.md)）。
