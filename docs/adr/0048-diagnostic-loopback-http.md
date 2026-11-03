# ADR 0048: ローカル模擬HTTPによる手動送信境界の検証

2026-10-25 / Issue #4 / PR #10。

ADR 0047の手動送信ゲートの下に、`DiagnosticLoopbackHttp`を追加した。これは
ローカル模擬サーバー専用のHTTP/1.1クライアントであり、製品APIの仕様や
製品用HTTPSアダプターではない。接続先は数値loopbackアドレス、または
127.0.0.1へ固定解決する`localhost`に限る。`https`を受け取った場合は拒否する。

診断契約はPOST、`{"text": ...}`のJSON本文、Bearerの合成キー、
`{"text": ...}`のJSON応答とする。送信前に同意・接続先・資格情報・版・期限を
手動送信ゲートで確認し、戻り時にも再確認する。HTTP側は自動再送せず、
3xxを拒否し、401/403・429・その他の非200応答を区別する。
応答のheaderとbodyはそれぞれ8192バイト以下。Content-Lengthが必須で、
重複やTransfer-Encoding、型違い、余分なJSON項目を拒否する。
期限は接続、書込、読取の各操作に残り時間を設定し、結果を返す前にも確認する。

実際のloopbackソケットを使う検査で、正しい要求、転送拒否、HTTP失敗分類、
長さ超過、重複長、期限切れ、HTTPS誤用を確認した。テストのキーと本文は合成値。
外部の接続先へ送信するテストはない。

製品通信は別途実装する。接続先と本文の正式契約、Windowsの資格情報・設定の
権限元、TLS、プロキシ、応答取消、ポリシー変更と送信開始の競合を決める必要がある。
WinHTTPの同期handleを他スレッドから閉じて取消す設計は採用しない。
[Microsoftの同時実行の説明](https://learn.microsoft.com/en-us/windows/win32/winhttp/concurrency-in-winhttp)と
[WinHTTP option一覧](https://learn.microsoft.com/en-us/windows/win32/winhttp/option-flags)を踏まえ、
製品アダプターは非同期処理で期限と取消を検証してから接続する。
