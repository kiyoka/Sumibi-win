# ADR 0064: 手動変換の送信開始と失効の順序

2026-10-27 / Issue #4 / PR #10。状態: 送信境界の部分実装。

## 順序点

`PolicyAuthority::authorize_send_start`はHostの政策lockを保持し、要求の
設定版・認可版、宛先、明示同意と資格情報の`Arc`同一性を照合する。
`manual::convert_ordered`はTransportに`SendGate`を渡す。Transportは接続と
送信内容の準備を済ませた後、資格情報headerと本文の最初のbyteを書く直前に
`write_authorized`を一度呼び、全ての要求byteの書込完了までguardを保持する。
Adapterがこのcallbackを使わず成功を返しても結果を拒否する。
この入口は接続単位の`SendAdmission`も要求する。製品Brokerの`JobTicket`が
これを実装し、専用mutexと取消状態を送信書込の間保持する。切断・期限・失効が
先なら未送信byteを拒否する。送信が先なら取消処理はStopを通知した上で
有限な書込の終了を待ち、その後に取消を確定する。

失効がguard取得より先に完了した場合は旧版を拒否し、要求byteは送らない。
guard取得が先の場合は失効操作が有限な書込完了を待ってから確定する。
既に送られたbyteは取り戻せない。結果の受信後にも政策版と資格情報の
同一性を再検査し、途中で失効または差替えがあれば結果を破棄する。
送信後の結果配送は別途`BrokerPolicyRuntime::publish_result`がHost元と
Brokerゲートを保持して順序を決める。

従来のsnapshot前後検査だけを行う`manual::convert`と、その
`ConversionTransport` portは単体テスト構成に限定した。
通常ビルドの手動送信入口は`convert_ordered`である。

## 検査した範囲

- mock transportが送信直前に失効したとき、書込callbackは呼ばれない。
- 送信後に失効した場合は結果を破棄する。gateを呼ばず成功を返すadapterも拒否。
- 同じbyte列の資格情報を差し替えた後も旧`Arc`識別子を拒否する。
- 実loopback TCPで接続後、header書込直前に失効させると、serverはEOFを
  受けるまで要求byteを一つも受信しない。
- Host同意が残る状態でも切断済み`JobTicket`からの送信は0回。
  送信中の別threadによる切断は、その書込が完了するまで確定しない。
- guard保持中は別threadの失効が完了しない。既存のloopback HTTPの許可、
  redirect拒否、response上限、期限の検査は新しい送信境界でも成功。

## 残る製品接続

現在の実transportは診断専用のloopback HTTPであり、HTTPSの製品adapterと
外部APIの呼出しはまだない。製品adapterは認証情報や本文を含む全要求byteを
必ず`SendGate`のcallback内で書き、途中のsocket I/Oを有限期限で終える必要がある。
このtraitはadapter実装の外側の書込まで機械的に禁止しないため、実装と
ネットワーク回帰で確認する。

`JobTicket`のStopを実workerのconnect/write/read待機とsocket closeに接続し、
送信開始後の失効でも残るI/Oを取消・回収する必要がある。設定UIの操作完了は
旧版の未送信worker停止とBrokerの通知または切断receiptまで待つ。
異なるTIP processの接続と所有TSFの適用前再検査は
[ADR 0062](0062-product-broker-integration-sequence.md)の順序で統合する。
