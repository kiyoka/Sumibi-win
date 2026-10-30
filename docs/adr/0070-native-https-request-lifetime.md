# ADR 0070: WinHTTP の非同期要求の寿命

## 実装範囲

`sumibi-winhttp`へHTTPS POST要求の準備と解放だけを追加した。
Hostの`native_https::prepare`は検証済みDestinationから呼び、診断用HTTPを拒否する。
この時点では送信APIを公開せず、DNS照会・TLS接続・本文送信の実通信を確認していない。

WinHTTPのSECURE_DEFAULTSで非同期とTLS 1.2以上を選び、HTTPS要求にする。
リダイレクト、Cookie、自動認証を無効化し、証明書失効確認を有効にする。
証明書エラーを無視する設定は使用しない。現状は直接接続の準備に限り、
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

## 確認と残作業

外部サービスを呼ばずに、要求準備、8件の枠、超過拒否、明示closeと
Drop後の最終callbackによるcontext解放をWindows上で試験した。
不正なhost・path・portはnative handle作成前に拒否した。

本文buffer、送信完了・受信・エラーcallback、取消中I/O、期限、応答サイズ、
同意guardの保持範囲を接続してから送信を公開する。
localhost模擬通信と実TLSの検証を経て製品transportへ接続する。
既存の診断用TcpStream通信は独立したままである。

根拠:
[WinHttpOpen](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpopen)、
[WinHttpCloseHandle](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpclosehandle)、
[Concurrency in WinHTTP](https://learn.microsoft.com/en-us/windows/win32/winhttp/concurrency-in-winhttp)、
[WinHttpSetStatusCallback](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpsetstatuscallback)。
