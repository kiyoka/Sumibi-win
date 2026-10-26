# ADR 0063: Host の政策配送実行部

2026-10-26 / Issue #4 / PR #10。状態: 部分実装。

## 実装した境界

`BrokerPolicyRuntime`は一つのBroker起動IDについて、Hostの`PolicyAuthority`、
受付ゲート、`PolicyFanout`、`BrokerPolicyRelay`、認証済みpipeと単調増加する
接続keyを同じworker側で扱う。OS peer認証とHelloの読取は呼出し側が済ませる。
Readyを完全に書いてから接続を有効にし、HelloやReadyが失敗した試行のkeyも
再利用しない。

`reserve`はOS accept後、Helloを読む前に接続keyを確保する。未完了の
Helloを持つpipeは要求を受け付けない。設定変更を公開した場合は、通知の
宛先にならないこのpipeを実際に閉じてから配送完了を返す。keyは次の接続に
再利用しない。製品のaccept loopは各accept直後に`reserve`を呼ぶ必要がある。

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

登録を要しない`policy-runtime`診断モードで、製品Host EXEから同じ実行部を
呼ぶ。既定Hostはモードを拒否し、`spike`ビルドだけが起動する。親から
認証済みの2 pipeを接続し、Readyの拒否版、許可版、Convert受付、失効と
該当要求の取消を実子process越しに検査する。2通りのpipe割当順と親handleの
回収も確認する。資格情報は診断専用の固定値であり、HTTP送信は行わない。

Worker完了の公開入口はBroker起動ID・接続key・要求番号・政策版・完了時刻を
`ReplyContext`として受け取る。別Brokerの遅延完了を拒否し、Hostの政策元と
Broker受付ゲートのguardを結果frameの有限な書込が終わるまで保持する。
書込の許容期限は開始から最大500ms。失効が先なら旧版の結果を出さず、
書込が先なら失効側がguardの解放を待つ。固定エラーも同じ境界で公開し、
書込失敗はその接続だけを閉じる。実pipe検査で成功結果、固定エラー、
失効後の旧結果拒否と別Brokerの完了拒否を確認した。診断子processでも
一方の要求へ成功結果を返し、他方の保留要求だけを失効時に取り消す。

## 残る境界

この実行部は製品Host EXEの診断分岐にだけ接続した。継続する製品用の
accept/read/worker結果ループはまだない。診断では2本ともOS acceptしてから
`reserve`するため、各accept直後のkey発行と複数のHello読取待ちが重なる
順序は未検査。異なるTIP processから共有Hostを
発見・起動する経路、設定UIの完了待機、実worker完了の調停と結果公開、
所有TSFへの適用前再検査も残る。

政策元の二度の読直しは、外部の設定変更と送信開始を原子的にしない。
実APIを有効にする前に、[ADR 0062](0062-product-broker-integration-sequence.md)の
送信開始・失効の順序を実装する。製品の要求受付時は、公開すべきHost変更を
先に処理し、接続key・世代・政策版・Broker起動IDをworker完了に保持する。
恒久的なTSF登録と実API送信は今回行っていない。
