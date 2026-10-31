# ADR 0070: WinHTTP の非同期要求の寿命

## 実装範囲

`sumibi-winhttp`へHTTPS POST要求の準備、初期本文の非同期送信、応答受信と解放を追加した。
Hostの`native_https::prepare`は検証済みDestinationから呼び、診断用HTTPを拒否する。
`send_body`はHostの送信guardを通し、SENDREQUEST_COMPLETEまで保持する。
`receive_body`は送信guardを解放した後で呼ぶ。受信には送信時の期限を引き継ぎ、
取消済みの結果を拒否する。結果採用前のpolicy revision・credential再照合は呼出側の責務。
初回変換adapterとの接続は[ADR 0071](0071-native-chat-policy-transport.md)を参照。
外部APIや正常なTLS接続の通信試験はまだ行っていない。

WinHTTPのSECURE_DEFAULTSで非同期とTLS 1.2以上を選び、HTTPS要求にする。
リダイレクト、Cookie、自動認証を無効化し、証明書失効確認を有効にする。
証明書エラーを無視する設定は使用しない。現状は直接接続に限り、
製品のproxy方針とその試験は残る。

## 所有と取消の前提

同期要求を別スレッドから閉じて取消す方式は、WinHTTPの競合条件を満たさない。
非同期要求でも、API関数の実行とCloseHandleを競合させず、関数が戻った後に閉じる。
閉じた後もcallbackが届くため、WinHttpCloseHandleの復帰だけでcontextを解放しない。

要求専用contextのArc参照をnative側へ渡し、最後のHANDLE_CLOSINGで回収する。
contextがconnectionとsessionも保持し、最後の通知より前に親を解放しない。
要求はprocess合計8件までとし、contextの解放まで枠を保持する。
明示的なclose待ちは最大60秒。期限切れやclose失敗でも枠を早く返さない。
close失敗で通知が届かなければ枠は回復せず、追加の保持は8件で止まる。

初期本文は64 KiB、キーは2560 ASCII bytes以下に制限し、改行等を拒否する。
送信本文とheaderをcontextが保持し、取消・期限切れでも最終callback前に解放しない。
送信は各要求につき1回で、再送を試みない。送信待ちは最大60秒の単調時計期限と
50msごとの取消確認を使い、API呼出しの復帰後に要求handleを閉じる。
REQUEST_ERRORの固定分類とSENDREQUEST_COMPLETEの本文長・pointerを確認する。
guardが送信処理を呼ばず成功を返した場合も、Host側で成功として扱わない。

## 応答受信

送信完了後にReceiveResponseの通知を待ち、固定サイズの同期header照会を行う。
HTTP 200とapplication/json（charset=utf-8の指定も可）のみを受け入れる。
応答header全体の上限は16 KiB、照会する各header値は127 UTF-16 code unitsまで。
Content-Type・Content-Length・Content-Encoding・Transfer-Encodingの重複、圧縮、
Content-LengthとTransfer-Encodingの併記は拒否する。
Content-Lengthがある場合は1〜65536 bytesに制限し、最終的な実受信長とも照合する。
長さのない応答とchunked応答も、累積本文64 KiBを超える時点で中断する。空本文は拒否する。
ここで確認するのはHTTP metadataと本文サイズで、JSONの内容・候補抽出は既存parserの責務。

8 KiBの固定bufferをcontext内のUnsafeCellで保持し、非同期ReadDataにはbytes-readの
出力pointerを渡さない。callbackはpointer・長さを確認して完了をmutexで通知するだけで、
次のreadはcallbackから呼ばずownerのloopで開始する。ownerはnative呼出しの復帰と
READ_COMPLETEの両方を待ってから本文をcopyする。取消・期限切れではcopyせずに閉じ、
最終HANDLE_CLOSINGまでbufferを保持する。要求枠の8件上限は受信にも共通する。

## 確認と残作業

外部サービスを呼ばずに、要求準備、8件の枠、超過拒否、明示closeと
Drop後の最終callbackによるcontext解放をWindows上で試験した。
不正なhost・path・portはnative handle作成前に拒否した。
localhostの模擬serverに偽キーと合成本文を送り、native送信完了と2回目の送信拒否を
確認した。このplain HTTP constructorはprivateな試験用で、public入口はHTTPSのみ。
TLS応答を返さないlocalhostのsocketで、接続待ちの取消と期限切れ、最終callback後の
本文contextの解放を確認した。証明書の信頼設定や既存キーを変更していない。
受信では分割された上限長の本文、chunked本文、不完全な本文・chunk、重複・過大header、
圧縮や異常statusの拒否と、header/body待機の取消・期限切れ・最終context解放を試験した。
このWindows環境ではheaderが来ない場合、送信完了通知も待機する挙動を観測したため、
試験では送受信両段階を同じ取消と期限で扱う。

初回変換transportへの接続を追加したが、正常な実TLS通信の検証は残る。
既存の診断用TcpStream通信は独立したままである。

根拠:
[WinHttpOpen](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpopen)、
[WinHttpCloseHandle](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpclosehandle)、
[Concurrency in WinHTTP](https://learn.microsoft.com/en-us/windows/win32/winhttp/concurrency-in-winhttp)、
[WinHttpSetStatusCallback](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpsetstatuscallback)。
送信bufferと完了通知: [WinHttpSendRequest](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpsendrequest)、
[WINHTTP_STATUS_CALLBACK](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nc-winhttp-winhttp_status_callback)。
受信とheader照会: [WinHttpReceiveResponse](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpreceiveresponse)、
[WinHttpReadData](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpreaddata)、
[WinHttpQueryHeaders](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpqueryheaders)。
