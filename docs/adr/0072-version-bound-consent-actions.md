# ADR 0072: 表示内容に結び付ける非同期の送信同意

## 表示と許可の一致

将来の同意画面には、現在の接続先だけを引数にした低水準の許可操作を使わない。
同じ接続先でも表示後にモデルやキーが変わり、利用者が見た条件と異なる許可になるため。

`ConsentOffer::try_capture`はauthorityのlockを待たず、取得できた場合だけ
接続先、モデル、キーの有無、現在の送信可否、settings・authorization版を返す。
キーの文字列は保持・公開しない。参照元authorityの同一性もoffer内に保持する。
取得不能ならNoneを返す。画面は「処理中」として許可を無効にし、古い表示を流用しない。
取得したofferの表示だけで送信を許可することはない。

UIは完全なHTTPS接続先・モデルとキー有無を表示し、明示操作後だけ
`ConsentTask::grant`へそのofferを渡す。処理開始時のpolicy lock下でsettings版、
authorization版、モデル、接続先とキーの存在を照合する。
モデル変更、接続先変更、同じ文字列を含むキー置換、許可・取消で版が変われば拒否する。
HTTPS・モデル・キーのいずれかが欠けたofferは許可taskを開始できない。
拒否された許可操作自体はpolicyを変更しない。

取消は表示版との一致を要求せず、同じauthorityの現在の同意を取り消す。
古い表示を理由に取消が拒否されることを避ける。キーや設定ファイルは削除しない。

## workerとBroker配信

許可・取消は専用workerでpolicyを更新する。開始と完了pollは送信lockを待たない。
設定保存・資格情報操作と共通の、同じauthorityにつき同時1件の枠を予約する。
Busyなら操作を開始していないため、UIは完了として表示せず後から再試行できる。
進行中のtaskを破棄しても明示操作は取り消さず、枠はworker終了まで保持する。

完了したworkerはauthority同一性と更新版を持つ`ConsentUpdate`を返す。
これはローカルpolicyの更新記録で、配信完了や現在の許可状態を保証しない。
同じauthorityを使う`BrokerBoot::acknowledge_consent_update`だけが受け入れ、
既存subscriberへの通知または切断、無効化されたworkerの終了まで確認する。
期限切れならreceiptを保持して再試行する。後続の変更に追い越されたreceiptを確認した
場合は、ackのcurrent policyを使う。古い許可receiptだけで「許可済み」と表示しない。

## 検証と残作業

合成キーを保持するメモリ内authorityで、offer取得の非許可性、必要条件の欠損拒否、
各変更後の古いoffer拒否、許可・取消の版更新と1回限りのpollを検査した。
送信guard保持中でもoffer取得とpollが待たないこと、資格情報・保存との同時開始拒否、
task破棄後の取消完了を確認した。

実際の認証済みpipeに未許可状態で接続し、許可と取消をPolicyChangedで配送した。
取消でworkerが終了してからackが戻ること、別authorityのreceipt拒否、
期限切れ後の再試行、古い許可receiptによる再許可がないことを確認した。
この試験で資格情報store、証明書storeや外部通信は操作していない。

同意画面との接続と明示起動Hostの終了調停は[ADR 0073](0073-host-consent-window-lifetime.md)へ続く。
安全なpeer発見と一般アプリ接続、自動常駐は残る。
Host再起動時の同意を復元しない既存方針は維持する。
