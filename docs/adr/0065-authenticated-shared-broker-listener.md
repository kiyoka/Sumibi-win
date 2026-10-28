# ADR 0065: 認証付き共有Broker待受の寿命

2026-10-28 / Issue #4 / draft PR #10。状態: Hostライブラリと診断子Brokerで実装・検証済み。製品Hostの常駐起動とTIP発見は未接続。

## 判断

Brokerはランダムなpipe名の最初のinstanceを`SharedAnchor`で保持する。アンカーは
Broker自身のprocess handleを固定peerとして相互認証した接続であり、本文の読書きを
行わない。接続するTIPが0件になっても最初のinstanceを残し、同名pipeを第三者が
先に作る隙間を設けない。実clientの受付はアンカーから作る別instanceで行い、
それぞれの接続元processを信頼できる起動・発見経路で固定してから受付を始める。
pipe名、PID、同じユーザーのSID、Helloのsession IDだけをpeerの証拠にしない。

`MAX_ENDPOINTS`はprocess全体で8件。アンカーのserver側は1件を使うので、
実clientの同時受付と接続は最大7件とする。アンカーの自己client handleは
アプリケーションI/Oへ渡さず、8件のI/O予算の外に保持する。各実instanceは
logon SID限定ACL、ローカル限定、非同期I/O、固定processのOS peer検査を使う。

`BrokerBoot`は製品Host向けに、ランダムな名前と同じ値の起動ID、アンカー、
政策Runtime、worker所有側、待機受付を一つの寿命で保持する。受付は外部の
信頼経路で取得した`PinnedProcess`だけを受け取り、実clientが0件の期間も
アンカーを保持する。接続履歴の残数を所有側へ返し、枯渇時の再起動を判断できる。
`retire_for_restart`がfalseなら旧資源を保持して再試行し、trueの後に旧Bootを
破棄する。型自体はTIPの発見・署名・配置を行わない。

## 所有側の順序

1. HostがBroker起動IDとpipe名を用意し、`SharedAnchor::create`を完了する。
   初回作成は`FILE_FLAG_FIRST_PIPE_INSTANCE`で既存の同名pipeを拒否する。
2. Hostが信頼経路から得た`PinnedProcess`を渡し、
   `BrokerPolicyRuntime::begin_anchor_accept`で有限期限の受付を開始する。
   接続履歴の残数が0ならOS instanceを作る前に`Exhausted`を返す。
3. `BrokerWorkLoop::advance_with_accept`はHost政策の最新版を先に公開し、
   OS peer認証が終わったpipeだけに単調増加の接続keyを予約する。
   接続ごとにHelloとReadyを完了してから要求を受ける。
4. 受付中に期限切れ・誤peer・取消が起きた場合、そのinstanceだけを退役する。
   既存接続は継続し、アンカーから次の受付を作れる。
5. 接続履歴64件を使い切ったら新規受付を止める。上限前に始めた受付が後から
   完了した場合も`AdmissionRejected`として閉じ、既存接続の結果は維持する。
   製品Hostは旧接続とworkerを有限に退役させ、新しい起動ID・pipe名で更新する。
6. 終了時は`BrokerWorkLoop::retire_for_restart`で旧Runtimeの認証済みpipeを閉じ、
   jobと全受付taskを取消す。受付と変換workerの退出を同じ有限期限で確認し、
   未終了taskは保持して再試行する。旧clientとアンカーを解放した後に
   次のBrokerを起動する。旧worker結果は`Retired`として拒否する。

終了時の取消通知は進行中の送信mutexを待たない。workerが送信を終えるか
取消を観測するまでの退出確認を、所有側の有限期限で行う。期限切れなら
終了成功とは扱わず、残ったworkerを保持して再試行する。通常の政策変更では
送信順序を固定するため、従来どおり送信mutexの解放を待つ。政策元とBrokerの
状態が矛盾してRuntimeを終端する場合も、終了通知はmutexを待たない。

## 期限と失敗

`AcceptTask`はOS接続とpeer照合の完了後も期限を確認し、所有側が結果を期限後に
取り出した場合は認証済みpipeを公開しない。取消と結果到着が競合した場合は
取消を優先する。`SharedAnchor::create`の途中で失敗したときは受付workerを最大
500msで回収し、回収できなければ`Retired`を返す。これを安全に同名pipeを
再利用できたという成功にはしない。

政策変更が実client0件の間に起きても、`acknowledge_version`は版の公開と
旧workerの終了を確認する。後から来た接続のReadyはその最新版だけを含む。

## 検証した範囲

- 実pipeの3接続・4件目の差替え、0接続の期間をまたぐkey 1→2、最新版Readyと結果。
- 固定した別processではない同ユーザーprocessの拒否、唯一のclient process退出後の再受付。
- アンカーからの受付でも誤peer拒否後に同名pipeを保持し、次の正しいpeerを受理。
- アンカーからの待機受付が期限切れになった後も同名pipeを保持し、再受付を完了。
- 期限後の受付結果破棄、受付2件と変換workerの同時停止後の同名pipe再作成。
- アンカー1件と受付7件で8 endpoint枠、取消後の枠回復、接続履歴64件後の新規拒否と既存結果。
- 認証済みの旧接続、進行中worker、待機受付を同時に退役し、アンカー解放後の
  同名pipe再作成と旧結果の拒否。
- 送信mutexを保持したwriterがいても終了通知は先に完了し、退出しないworkerは
  期限切れとして報告され、解放後の再試行で回収される。
- 政策状態の矛盾による終端も送信mutexを待たず、接続とjobを退役させる。
- `BrokerBoot`で0件の期間を挟む2接続を同じ起動IDで処理し、退役後に旧pipe名を
  再作成。異なる政策元はOS受付の消費前に拒否し、正しい元で同じ受付を継続。
  待機受付だけがある状態でも退役とアンカー解放の順序を確認。
- 診断用子Brokerの親process向け2受付、政策通知、取消、結果。`tools/Test-Broker.ps1`と
  `tools/Test-Spike.ps1 -SkipDesktop`が成功し、既定HostとパッシブTIPへ復元。

製品Hostの常駐起動、インストール済みBrokerとTIPの信頼できる共有発見、
設定UI、製品HTTPS adapter、一般アプリの所有TSFへの結果適用は別の受入作業。
