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

`BrokerBoot::begin_accept`は新しいOS instanceを作る前に、待機受付を含む
接続履歴の残数と、アンカーを除いた同時instance枠を確認する。残り履歴を
待機受付が確保した場合や同時枠が埋まった場合は`Capacity`を返し、受付の
失敗や接続終了後に再試行できる。履歴を実際に使い切った場合は`Exhausted`を返す。
OS認証済みでHello待ちの接続も履歴枠を確保する。Helloが失敗した場合は
履歴を消費せず枠を返し、成功した場合に履歴を消費する。

異なるprocessを固定した複数の受付に同じpipe名を使うと、clientは特定の
instanceを選べず、別の固定peer用instanceに接続して拒否され得る。
`BrokerBoot::begin_private_accept`はpeerごとに新しい単一instanceのpipe名と
受付IDを作る。同じBootの政策・履歴・同時枠で管理し、OS peerの照合後に
のみBrokerへ渡す。名前とBoot IDの信頼できる配布とclient側のHost認証は
呼出し側に必要であり、この入口自体は製品の発見方式を完成させない。

`Channel::connect_pinned`は信頼経路で固定済みのHost handleと個別pipe名、
Boot IDを受け、OS相互認証の後にHello/Readyを完了する。保持する結果の容量、
版の照合、通知確認と期限は従来の起動型Channelと共通である。共有Hostの
Channelを閉じてもHost自体は停止しない。`Worker::new_pinned`はその接続を
worker thread上で遅延開始し、予約・取消・結果回収に既存の有限枠を使う。
`Mode::Pinned`はworker用であり、子Hostの起動引数には使えない。
Readyの政策版を入力側へ届ける製品経路は別途必要である。
workerは最初の要求前も固定Hostのprocess handleを待ち、Hostが終了すれば
自ら退役して以後の予約を拒否する。

通常終了は値を消費する`BrokerBoot::shutdown`で行う。期限切れなら稼働中の
Bootを`Expired`で返し、60秒を超える期限なら`InvalidDeadline`で返す。
回収が未完了なら退役中のBootを`Pending`で返す。
どちらも所有者が保持して期限を改めて再試行する。成功時だけアンカーを解放する。

`BrokerBoot::rotate`は旧接続・受付・workerの退役を確認し、旧アンカーを
解放してから新しいランダム名と起動IDのBootを作る。期限は両段階で共用する。
呼出し前に期限が切れていれば`Expired`、60秒超なら`InvalidDeadline`で
稼働中のBootを返し、受付を止めない。
`Pending`は旧Bootを返すが、旧接続は既に閉じている。所有者はそのBootを
保持して退役を再試行し、元の接続を再開させない。`Start`は旧Bootが完全に
解放された後の新規作成失敗を示す。所有者は新規起動を改めて試す。
新しい名前の配布とTIPの再接続は、配置と発見を担う製品側の責務とする。

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
- `BrokerBoot`の0-client時に失効・再許可receiptを完了し、後続Readyで最新版を確認。
  認証付きpipeから診断loopback HTTPへ送信して、結果を同じpipeへ一度配送。
- 起動handleで固定した別processを`BrokerBoot`から受付し、Hello・Convert・結果、
  子process退出、旧Boot退役とアンカー解放後の同名pipe再作成を確認。子fixtureは
  標準File接続なのでclient側のBroker相互認証はこの検査の範囲外。
- 待機受付を回収してから切替え、旧名の解放と新名でのHello・Readyを実pipeで確認。
  切替時に政策元の最新版を読み直す。退出しないworkerでは切替えを保留して
  旧Bootを保持する。この時点で旧clientは切断済みで、退出後の再試行で成功。
- 切替呼出し時点で期限が過ぎているか60秒を超えていれば退役を始めず、旧Bootの実pipe受付と
  Hello・Readyが引き続き成立することを確認。終了APIでも同じ期限前判定と
  待機受付の回収後のアンカー解放を確認。
- 実pipeで64回の受付・Hello・Ready・切断を行い、履歴上限で新規受付を拒否。
  切替後は履歴予算と接続keyが新Bootで初期化される。旧起動IDのHelloを拒否して
  その接続keyを消費し、次の正しいHelloを新しいkeyで有効化することも確認。
- 2 instanceのBootで待機受付と認証済み接続の各状態から余分な受付を
  OS instance作成前に拒否し、接続終了後は同じBootで受付枠を再利用した。
  履歴最後の1件を待機受付が確保している間は`Capacity`で拒否し、期限切れで
  枠が回復することを確認した。実際に使い切った後は`Exhausted`を返す。
- 同じBootで親と別の子processに別名の受付を用意し、各受付IDと接続を照合。
  子からHello・Convert・結果まで確認した。子fixtureは標準File接続であり、
  client側のHost相互認証と製品の名前配布はこの検査の対象外。
- 別の結合検査では、起動した2子processへHostの固定process handleをそれぞれ
  複製し、専用stdinでpipe名・Boot IDと共に渡した。各子は`Pipe::connect`で
  Hostを照合し、Hostも各子を固定した上で同じBootへ接続し、変換結果を受信した。
  誤Hostを固定した子はHello送信前に拒否し、別processからの接続もHostが拒否する。
  その後は新しい名前で正しい子を受付できる。これは起動済み試験子に限定した
  受渡しであり、一般アプリへの配布・共有Host発見は未実装。
- 共有HostへのChannelと非同期workerを別々の子processから使用し、先の子が
  終了した後も後のworkerが変換できることを確認。workerの取消で接続とHostの
  jobが回収され、古い政策版の要求は本文送信前に拒否される。
  要求前のHost終了もworkerが検出し、以後の予約を拒否する。
- 診断用子Brokerの親process向け2受付、政策通知、取消、結果。`tools/Test-Broker.ps1`と
  `tools/Test-Spike.ps1 -SkipDesktop`が成功し、既定HostとパッシブTIPへ復元。

製品Hostの常駐起動、インストール済みBrokerとTIPの信頼できる共有発見、
設定UI、製品HTTPS adapter、一般アプリの所有TSFへの結果適用は別の受入作業。
