# ADR 0063: Host の政策配送実行部

2026-10-26 / Issue #4 / PR #10。状態: 部分実装。

## 実装した境界

`BrokerPolicyRuntime`は一つのBroker起動IDについて、Hostの`PolicyAuthority`、
受付ゲート、`PolicyFanout`、`BrokerPolicyRelay`、認証済みpipeと単調増加する
接続keyを同じworker側で扱う。OS peer認証とHelloの読取は呼出し側が済ませる。
Readyを完全に書いてから接続を有効にし、HelloやReadyが失敗した試行のkeyも
再利用しない。

Hostの最新政策版を公開すると、退役先のpipeを閉じ、生存先の通知を有限な
`WriteBatch`で並行開始する。各接続の書込完了だけをFanoutと配送receiptへ
報告する。失敗先はFanoutから退役させ、OS pipeを実際に閉じてからreceiptを
進める。失効で取り消した要求は接続keyと要求番号の組で呼出し側へ返す。
政策公開の内部状態や待機処理が失敗した場合は全接続を閉じ、実行部を終端する。

要求frameを受ける入口はHostの政策元を受付前とConvert受付直後に読み直す。
Relayでまだ公開されていない変更があれば、そのpipeを閉じて旧本文を渡さない。
古い要求を新版で自動再送しない。Host内の実pipe検査で2接続への許可・失効、
片側切断、Ready失敗、誤ったHello、未公開変更、終端失敗を確認した。
同じclient寿命IDで再接続した場合も、旧世代Helloと既に受け付けた要求番号を
拒否し、新しい世代の単調増加した要求だけを受け付けた。未来IDの不正Cancelは
その接続だけを退役させ、他方の保留要求と次要求を維持した。

## 残る境界

この実行部は製品Host EXEのaccept/read/worker結果ループにはまだ接続していない。
接続keyの発行は現在Helloを渡す`attach`の時点であり、OS accept直後の発行と
Hello読取待ちが前後する場合の順序は未実装。異なるTIP processから共有Hostを
発見・起動する経路、設定UIの完了待機、worker結果の公開guardとpipe書込、
所有TSFへの適用前再検査も残る。

政策元の二度の読直しは、外部の設定変更と送信開始を原子的にしない。
実APIを有効にする前に、[ADR 0062](0062-product-broker-integration-sequence.md)の
送信開始・失効の順序を実装する。製品の要求受付時は、公開すべきHost変更を
先に処理し、接続key・世代・政策版・Broker起動IDをworker完了に保持する。
恒久的なTSF登録と実API送信は今回行っていない。
