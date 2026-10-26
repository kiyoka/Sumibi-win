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
認可版2への失効を両方へ配送し、保留要求と古い結果を破棄する。認可版3で
再許可した後、他方のclientが新しい版で要求し、固定結果を受ける。
続いて最初のclientが新しい要求を保留したまま、他方が未発行番号のCancelを
送る。Brokerはその接続だけを退役・closeし、認可版4の失効を残るclientへ
届けて保留要求を取り消す。clientは退役したpipeの切断も確認する。
親側の2本の接続を入れ替えた順序でも同じ検査を行う。

別の診断では、Ready後に片方のclientがpipeを閉じる。Brokerはその切断を
観測した後も通知先として保持し、失効通知の実書込が失敗することを確認して
その接続だけを退役させる。残るclientには失効と再許可を順に届け、古い要求を
取り消し、新しい認可版の要求にだけ固定結果を返す。失敗した通知は再送しない。
退役したinstanceを同じ子Broker内で作り直し、同じclient寿命IDが世代2で
再接続する。OS peerを改めて認証した後、現在の認可版3でReadyを送り、
再接続先と生き残った先の両方で新しい要求の結果を確認する。再接続は
元の要求の自動再送を含まない。

通常Hostはこの診断serviceを拒否し、`spike` featureを明示したHostだけが
固定本文を扱う。`tools/Test-Broker.ps1`は拒否、2接続、既存Channel/Workerを
順に実行し、終了時に通常Hostへ戻す。マシン全体のTSF登録と実API送信は
行わない。

これは一つの親processから開いた2本のpipeを検査する。異なるTIP processの
信頼できる発見・配置、常駐Brokerの一般的なaccept loop、Hostの設定変更元、
設定UIの完了条件、所有TSFでの適用前再検査はまだ製品へ接続していない。
Windowsの複数instanceと早着接続の扱いは
[Named Pipe Operations](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-operations)と
[ConnectNamedPipe](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-connectnamedpipe)に従う。
