# ADR 0057: Brokerの接続順序を状態機械で検証する

2026-10-25 / Issue #4 / PR #10。

認証済みpipeのBroker側は、Hello、要求、結果、policy通知を個別の分岐で
組み立てていた。単発Jobと再利用Channelの両方で同じ順序制約を守るため、
OS非依存の`BrokerSession`を`sumibi-protocol`へ追加した。

起動時に固定したBroker IDと、受信Helloのsession・generationを照合する。
接続中のConvertは一件だけとし、要求番号は単調増加させる。古いCancelと
古い完了は新しい要求を消費せず、未発行の未来番号と異なる接続世代は接続を閉じる。
結果は要求の版、policyの版、許可、期限、本文を検査してから生成する。
policy変更は保留中の要求を取り消し、通知frameと取消番号を同時に返す。
設定版だけの増加を認め、許可の変更には認可版の増加を必須とする。

この型はI/O、OS peer認証、policy変更元、TSF対象を持たない。呼出し側は
Helloより前にOS peerを認証し、policy通知を次の本文受付より先に送るか
接続を閉じる。`BrokerPolicyGate`はBrokerの認可元として残し、状態機械と
同じ更新を適用する。診断Hostの単発Jobと再利用Channelはこの型から
Readyと通常応答を生成する。意図的に不正な未来IDを送る診断モードだけは
状態機械を経由せずwire frameを作る。

Protocol単体ではHello誤認、世代違い、二重要求、取消後の古い完了、
未来Cancel、policy更新、期限ちょうど、期限後の成功拒否、固定Timeout応答を
検査する。実子の`broker-test`、`broker-channel-test`、`broker-worker-test`、
実DLLの`broker-key-test`で単発・再利用・所有TSF配送を回帰した。
これらは合成本文と診断用固定結果であり、製品Brokerの常駐・設定配信や
一般アプリ対応を成立させるものではない。
