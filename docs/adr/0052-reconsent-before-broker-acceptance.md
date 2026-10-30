# ADR 0052: 再許可後に届いた旧要求をBrokerが拒否する

2026-10-25 / Issue #4 / PR #10。

認証済みの実子Brokerと名前付きpipeを通す`broker-channel-test`に、
`regrant-at-accept`診断モードを追加した。合成Convertを受信した後、要求を
受付済みにする前にBroker側の認可版を取消で1回、再許可でさらに1回進める。
最終状態が`allowed=true`でも、古い認可版のConvertは拒否し、受付番号を
進めない。Brokerは結果を作らず、最新版の`PolicyChanged`を返す。

Clientは`PolicyDenied`を受けてChannelを退役させ、旧要求を再送しない。
検査は子process終了まで確認する。通常ビルドのHostはこの診断モードを拒否する。

このモードはspike専用の合成政策変更である。製品の設定・認可変更元、通知配送、
一般アプリのTSF入力はまだ接続していない。
