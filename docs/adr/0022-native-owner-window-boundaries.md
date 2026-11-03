# ADR 0022: 所有ウィンドウの無効化・非アクティブ化による失効

状態：診断ホストの実WndProcと実DLLで検証。一般アプリの通知保証ではない。
2026-10-22。Issue #4 / draft PR #10。ADR 0020・0021の後続。

## 問題と決定

入力ホストはWM_KILLFOCUSを所有者境界として転送していたが、アプリの非アクティブ化、
windowの無効化、モード取消を同じ経路へ接続していなかった。
TSFの自然focus通知だけで履歴を保証できないため、所有WndProcで観測した境界も転送する。

| 通知 | 失効条件 |
| --- | --- |
| WM_KILLFOCUS | 従来どおり常に |
| WM_ACTIVATEAPP | wParamがFALSE（アプリ非アクティブ化） |
| WM_ENABLE | wParamがFALSE（window無効化） |
| WM_CANCELMODE | 常に。モード取消も保守的に追跡・要求を失効 |

WM_ACTIVATEAPPの意味は[公式仕様](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-activateapp)、
WM_ENABLEの同期配送は[公式仕様](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-enable)に従う。
正のactivation/enable通知は新しい許可ではなく、既存の正常な追跡を取り消す理由にも使わない。

WindowとEditorは同じinput_boundary判定を使う。境界では救済本文をredactし、
Composingの再利用を禁止して既存の有限eventへcleanupを集約する。単純な追跡と入力途中のhigh surrogateも除去する。
Broker付きホストは従来のRevokeContextで保留対象・grantも取り消す。signal失敗でもgrant取消を先に試みる。
再有効化・再アクティブ化で旧要求・追跡・救済本文を復活させない。

Composing.owner_boundary自体はCOM・本文取得・編集を行わず、所有loopでcompositionをcleanupする。
境界handler全体のCOM呼出しを除去した意味ではない。既存のRevokeContextは所有thread上で実行する。
この変更は待機キーの再送、一般アプリへのhook、前面windowの強制取得を追加しない。

## Windows標準処理

境界callbackがhandledを返しても、エラーを返しても、通知はDefWindowProcへ渡す。
特にWM_CANCELMODEの[標準処理](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-cancelmode)は
menuやscrollbarの処理取消とmouse captureの解除を行うため、IME側で飲み込まない。
callbackエラーは既存のinput_errorに記録し、所有loopへ返す。文字入力の消費規則は維持する。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- input-notify-testは30ケース。既存24件に、非アクティブ化/復元・無効化/復元・モード取消・
  正の通知による正常追跡の保持と、callbackのhandled/error双方での標準capture解除の6件を追加した。
  境界世代の更新、high surrogate除去、旧入力の非復活、本文/Undo/adapter本文取得の維持を確認する。
- broker-input-testは11ケース。既存8件に3種の境界を追加し、保留中にgrantを取り消す。
  状態を復元してから遅い結果を配送しても本文・Undo・adapter本文取得を増やさない。
- recovery-input-testは34ケース。既存31件に3種の境界を追加した。
  確認画面からコピー要求を予約して境界を通し、本文除去・古い画面の解除・再表示後もコピー拒否を確認する。
  操作警告のticketと元の期限は維持し、利用者のclipboardは変更しない。

無効化/復元は実EnableWindowとその同期OS通知を使う。非アクティブ化・モード取消・正の通知は
所有HWNDへSendMessageWし、実WndProcから検証する。物理的な他アプリへの切替や自然なOS通知配送の試験とは区別する。
captureは同じthreadの診断windowでSetCapture/GetCaptureし、WM_CANCELMODE後の解除を確認する。
他processの入力を奪ったりSendInputで補ったりしない。

Test-Spike.ps1 -SkipDesktopでRust 71件・fmt・各構成Clippy・build・load_smoke・self/edit/keyが成功。
追跡25・composition伸縮29・同期入力22・Backspace 27・close 4も再確認した。
ログはtarget/issue-4-window-boundary-static.log、window-boundary-final-input-notify-test.log、
window-boundary-final-broker-input-test.log、window-boundary-final-recovery-input-test.log、
window-boundary-tracking-input-test.log、window-boundary-composing-input-test.log、
window-boundary-transaction-input-test.log、window-boundary-backspace-input-test.log、window-boundary-close-test.log
（全てtarget/配下）。成功したnative検査のTSF/COM解除・DLLアンロードを確認した。

今回、省略なし全回帰は再実行していない。前面化を拒否されたwindow/input/delayedの3検査は依然未通過。
所有ホストが観測していない履歴の復元、一般アプリの保護情報・composition寿命・標準Undo、
製品の配置/認証・待機行列とHTTPは残る。Issue #4は閉じず、PR #10はdraftを維持する。
