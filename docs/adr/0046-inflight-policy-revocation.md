# ADR 0046: 進行中の要求に対するポリシー失効通知

2026-10-25 / Issue #4 / PR #10。

認証済みの再利用Channelが変換を待っている間に `policy_changed` を受けた場合、
`ClientSession` は保留中の要求を取り消す。Channelは要求IDが一致する失効通知を
`PolicyDenied` として返し、接続と子Brokerを退役させる。結果は作らず、
旧要求を新しい接続へ自動再送しない。通常の `result` として配送する経路はない。

専用のspike診断モードは、合成要求を受けた後に認可版を増やして `allowed=false` を通知する。
`broker-channel-test` は実子processと名前付きpipeで、失効、結果なし、再利用不可、
子process終了を確認する。通常Hostはこの診断モードを拒否する。
通知の送信期限は当該要求期限以内に収め、pipeの60秒I/O上限を守る。

後続の同日変更で、次要求の前に認証済みpipeへ既に溜まった通知を確認する。
通知があれば本文のframe生成前に読み、`PolicyDenied`で接続を退役させる。
spikeの待機中失効モードと実子process検査で確認した。
[PeekNamedPipe](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-peeknamedpipe)
はpipeからデータを取り除かずに利用可能バイト数を返す。

検査直後に通知が届く競合は残る。製品の設定・認可変更元と通知の発行、
Broker側での送信直前のポリシー検査もまだない。
現行Channelは固定ポリシーの診断Brokerを明示起動するworker専用APIであり、
この検査は利用者の送信同意や製品での即時失効保証を示さない。

続く変更では、byte-stream pipeの1回のreadに複数frameが入っても、client側は
最初のeventまでを処理して残りを最大1 read分（4096 byte）だけ保持する。
部分frameは既存の`ClientSession` decoderが保持する。次のreadより先に残りを処理し、
`Ready`と失効通知を一度のwriteで送る実子Broker、分割`Ready`と結合通知の
単体検査で確認する。単発jobも同じ受信処理を使い、送信前に残りの通知を調べる。
結果と失効通知が同じreadに入った場合も、結果を所有側へ返す前に残りの通知を処理し、
結果を破棄して接続を退役させる。これも専用の実子Broker検査で確認する。
Server側は要求/応答のlockstepを維持し、pipelined Convertは受け付けない。
