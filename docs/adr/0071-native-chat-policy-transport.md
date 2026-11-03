# ADR 0071: 初回変換とnative HTTPSの接続

## 実装範囲

`native_https::convert_chat_with_policy`から、既存の初回変換JSON本文、
WinHTTP送受信、候補parserを呼ぶ。製品向けの入口はHTTPSのみで、診断HTTPへの
切替や再送を行わない。キーの読取、設定保存や同意付与はこの入口では行わない。

モデルは要求のsettings・authorization版と一致するpolicy snapshotから取得する。
モデル未設定、古い版、無効な入力、異なる送信先、キー欠損や未同意では送信しない。
要求ごとのモデルを送信adapter内で保持し、後の設定変更で黙って差し替えない。

## 送信と結果の採用

`manual::convert_ordered`の既存境界を使う。送信開始時に同じauthorityの設定版、
送信先、明示同意とcredentialのArc同一性を照合し、送信guardを保持したまま
nativeの初期本文送信完了を待つ。接続の取消ticketも同じ送信境界を通る。
WinHTTPにはticketの取消確認を渡し、待機中も確認する。

送信guardを解放してから応答を受信する。期限は送信時から延長しない。
HTTP metadata・本文容量をnative層で検査した後、既存のChat parserが完了済みで
拒否のない1候補だけを抽出する。結果採用前に取消、同意、設定版、送信先と
credential同一性を再検査する。並行する設定・キー変更後の旧結果は成功として返さない。
最終的なpipe配送もBrokerのpolicy版・接続・要求IDの照合を通る。

## Brokerへの接続

`BrokerWorkLoop::advance_native_chat`とaccept付き入口、および
`BrokerBoot::advance_native_chat`を追加した。Brokerと異なるauthorityはacceptや
本文workerへの引渡し前に拒否する。診断Chatとnative Chatは同じ受付・worker・
固定エラー分類を使い、通信入口の選択を区別する。

native処理は有限worker内だけで行い、TSF callbackやBrokerの所有threadで
ネットワークを待たない。この追加だけでは常駐Hostを起動せず、保存設定にも
送信同意を追加しない。

## 検証と残作業

privateな模擬交換で、指定モデルを含む初回本文と1候補の成功経路を検査した。
送信guard直前と解放後のモデル変更、同意取消、キー置換と接続取消で、
未送信要求や旧結果が拒否されることを確認した。不正候補や固定通信エラーも確認した。
模擬交換の差替え入口は公開していない。

実際の認証済みpipeから専用Broker入口へ要求を渡し、HTTPを選んだ場合は
socketへ接続せずRejectedを返すことを確認した。HTTPSを選んだ場合はlocalhostの
peerでTLS ClientHelloを観測し、handshake完了前の切断をUnavailableとしてpipeへ
返した。実キー、証明書storeや外部サービスは使っていない。

成功経路のpolicy・JSON検査は模擬交換で、nativeの送受信検査は別のlocalhost
fixtureで行っている。正常なTLS経由での候補取得や外部APIの成功を証明するものではない。
正常TLS通信と非変換echo応答の拒否は[ADR 0075](0075-public-https-validation.md)で確認した。
同意UIと明示起動Hostの寿命管理は[ADR 0073](0073-host-consent-window-lifetime.md)へ続く。
実変換APIからの候補取得、proxy方針と一般アプリへの接続は残る。
