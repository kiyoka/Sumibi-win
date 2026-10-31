# ADR 0075: 正常TLS通信と証明書拒否の公開試験

## 明示実行する範囲

`tools/Test-NativeHttps.ps1`で、ignoredにした3件のInternet試験を実行する。
通常のworkspace testでは実行しない。接続先、合成本文、ダミーキーはtest内の固定値で、
設定ファイル、Credential Manager、利用者の入力本文からは取得しない。
再送・接続先の切替、証明書エラーの無視や証明書storeの操作は行わない。

Postman Echoの`https://postman-echo.com/post`でnativeのPOSTとJSON受信を検査する。
echoは送った内容をJSONで返す試験サービスであり、変換APIとして使わない。
[Postman Echo公式説明](https://learning.postman.com/docs/developer/echo-api)、
[POST endpoint](https://www.postman.com/postman/free-public-apis/documentation/2fuvoml/postman-echo)。

`https://expired.badssl.com/`と`https://self-signed.badssl.com/`では、
証明書検証が拒否されることを確認する。
[badsslの公式test一覧](https://github.com/chromium/badssl.com)。
APIキーはいずれも固定の`sumibi-synthetic-test-key`で、実キーではない。
正常証明書の要求では合成本文を送信し、不正証明書では送信の完了を認めない。
各要求の期限は20秒で、送受信を通して延長しない。

## 固定エラー分類

nativeの即時失敗を生成Rust wrapperのHRESULT_FROM_WIN32から分類し、
callbackのWINHTTP_ASYNC_RESULTと同じ分類関数を使う。
Timeout・Cancelled・Certificate・Tls・Nativeの固定分類だけを返す。
証明書の期限・名前・CA・失効確認・失効・用途・形式とSECURE_FAILUREはCertificate、
secure channelの一般的な失敗はTlsとする。
[WinHTTP error messages](https://learn.microsoft.com/en-us/windows/win32/winhttp/error-messages)。

不正証明書のtestはCertificateを要求し、Timeout・接続不能や一般的なTLS交渉失敗を
成功として扱わない。従来のHost transportではこれらのnative失敗をConnectionへ、
BrokerではUnavailableへ分類する。protocolへ秘密やサーバー由来のerror本文を追加しない。

## 検証結果

1. WinHTTPの製品向けHTTPS constructorを使い、Postman Echoへ固定JSONをPOSTし、
   HTTP 200・JSON media type・有限サイズと本文markerを確認した。最終callback後の
   context解放も確認した。
2. Hostのordered送信境界と実際のNativeExchangeを通し、固定モデル・合成入力を送った。
   echo JSONを受信して返送されたmodelと本文を照合できたが、Chat候補ではないため
   InvalidResponseとして拒否した。正常TLSを変換成功と取り違えないことを検査した。
3. badsslの期限切れ・自己署名の2接続をCertificateとして拒否し、context解放を確認した。

3件ともこのWindows環境で成功した。検査のための信頼設定の変更や実キー登録はない。
外部サービスとネットワークに依存するので、利用不能なら試験も失敗する。
`--offline`はCargo依存を取得しない指定で、この明示試験の通信は行われる。

実変換APIからの候補取得、proxy環境、信頼できるHost発見・一般アプリ接続、
自動常駐の検証は残る。正常な公開TLS通信は、これらの完了を示すものではない。
