# ADR 0060: Hostの政策版をBrokerへ配送する境界

2026-10-25 / Issue #4 / PR #10。

Hostの設定・資格情報・明示同意は`PolicyAuthority`が一つの版付き状態として
保持する。Broker側は`BrokerPolicyGate`で新規要求と結果公開を判定し、
`PolicyFanout`で各接続の失効通知を順序付ける。両者を手動で別々に更新すると、
一方だけが新しい政策版になる危険がある。

`BrokerPolicyRelay`はHostの公開用政策版を期限付きで待ち、変化があれば
`BrokerPolicyGate::publish`へ渡す。作成時にHost、Brokerゲート、Fanoutの
初期版が一致することを要求する。公開が失敗した場合、通知カーソルを進めず
エラーを返す。公開に成功した場合だけカーソルを進め、接続を閉じるべきkeyを
呼出し側へ返す。複数のHost変更は最新版一件へまとめられる。

RelayはBrokerのworker側で一つだけ動かし、同じFanoutを使う要求受付・
通知書込と直列化する。呼出し側は退役したpipeを閉じ、残る接続の
`PolicyChanged`を完全に書いてから配送確認する。Relayは資格情報・送信先を
wireへ出さず、TSFの所有スレッドを待機させない。

Hostで資格情報設定と許可を続けて行い、2接続へ一つの許可版を配り、続く失効版を
同じ2接続へ配る単体検査を加えた。実pipeへの複数client接続、設定UIの完了通知、
所有TSFでの適用前再検査は次の製品統合で扱う。
