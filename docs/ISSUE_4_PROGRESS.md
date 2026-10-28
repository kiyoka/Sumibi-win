# Issue #4 の受入進捗

2026-10-28 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) /
[draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
ここでいう成功は、明記した診断Host・専用TSF入力欄・合成本文での結果に限る。
製品の一般アプリ対応や実API利用を意味しない。

| 受入条件 | 現在の根拠 | 残る条件 |
| --- | --- | --- |
| 版付きJSON、ID・世代・取消・切断・重複・再接続、有限容量 | `sumibi-protocol`の`ClientSession`、`sumibi-ipc`の単発Jobと再利用Channel/Worker。要求・接続・結果の容量、期限、古い返信と未発行の未来IDを検査 | 製品Brokerの接続・設定変更元を接続する |
| 別プロセスの相互認証と偽server/PID再利用/終了待機 | 起動時の固定process handle、user/logon SID・session・integrity・PID・生成時刻とpipe peer IDを照合。実子processと偽peer診断を実行 | 配置済みBrokerの信頼できる発見、署名・更新・保護された起動経路 |
| 非同期pipe、バックプレッシャー、所有TSFスレッドへの結果配送 | 有限I/Oと取消、要求前Permit、EXE workerから所有STAへの非同期結果配送。Host政策実行部の受付・通知・worker結果を所有側で調停し、異なる2試験processの実pipeから結果を返した | 製品Hostの長寿命運転、異なるTIP processと一般アプリへの配送 |
| composition、通常文字・Enter/Esc、入力スコープと保護条件 | 専用ACP入力欄で同期入力取引、所有composition、待機中の文字/Backspace/Enter/Esc、保護状態の再検査を実DLLで確認。未知条件は拒否 | 一般アプリの入力先ごとの保護情報、通知履歴、composition寿命、実キー配送 |
| 待機容量の消費前確保、超過切替、救済期限、遅延キー再送禁止 | 専用adapterで共有4件・4096 UTF-16単位の救済枠を先に予約。待機は最大4件・8秒、救済は最大10分。失敗した操作を自動再送しない | 製品の全入力先にまたがる容量・期限・超過時政策の確定と接続 |
| Notepad等で実打鍵、1回Undo、別欄・同長別内容・読取不能の非適用 | 専用TSF storeの実DLL検査で対象変更・本文変更・読取拒否・1回Undoを確認。標準RichEditは保護情報不足で拒否。標準EDITは今回の環境でfocused documentなし | Notepad等の実アプリで肯定的な保護条件と標準Undoを確認する。現在は未達 |

## 2026-10-25 の追加確認

- `U+FFFC`をCore、Protocol、Job/Worker、TSFの本文境界で拒否する。
  `range-target-test`は別client、結果中のオブジェクト記号、1024 UTF-16単位超過を含む19ケース。
- Brokerの設定版だけを増やす更新をProtocolの規則と一致させ、旧設定版の要求を拒否する。
  実子Brokerの`broker-channel-test`で、結果直後の設定変更により旧結果が配送されないことを確認。
- `broker-worker-test`で不正原文を子processへの送信前に拒否し、次の正常要求がwire ID 1から始まることを確認。
  `broker-test`では単発Jobの失敗開始後に予約枠が戻ることを確認。
- 完了IDが所有側のpending IDと食い違う異常時は、結果待機を残さず対象とworkerを退役する。
  通常の`broker-key-test`25ケースで配送と解除の回帰を確認。
- Broker側のHello、要求順序、取消、policy変更、結果期限を`BrokerSession`に集約。
  単発Jobと再利用Channelの実子Hostを接続し、Protocol単体、`broker-test`、
  `broker-channel-test`、`broker-worker-test`、`broker-key-test`で回帰した
  通知の書込完了前には次要求を受け付けない
  （[ADR 0057](adr/0057-broker-side-session-order.md)）。
- Hostのpolicy通知カーソルは設定版だけが進んだ場合も新しい状態を返す。
  通常のBroker結果は政策の読取guardをpipe書込まで保持し、更新との順序を
  固定した。Client/Broker/Fanoutの版遷移判定を共通化した
  （[ADR 0058](adr/0058-policy-publication-order.md)）。
- 複数接続向けの`PolicyFanout`を最大8接続・各1未配送通知で追加。
  同時session ID重複を拒否し、遅い接続だけを退役させる単体検査を追加。
  切断した接続keyの再利用を拒否する。
  Broker寿命中の接続履歴を暫定64件に抑え、再接続時の旧世代Helloを拒否する。
  同じclient寿命IDの要求番号も引き継ぎ、世代更新での番号再利用を拒否する。
  初期フレームの形式とBroker IDを接続一覧より先に検証する。
  接続別の期限切れは他接続の要求を維持し、遅延結果を捨てる。
  ClientSessionとの実フレーム往復では再接続後に要求番号2を受け付け、
  旧要求1の結果を捨てて新版の結果だけを配送した。
  Readyの書込完了前は要求と通知確認を拒否する。
  書込完了時は配送した政策版も照合し、古い・重複した完了通知で
  次の通知を承認しない。
  通知frameは各接続につき一度だけ書込担当へ取り出し、未取得の書込完了を拒否する。
  予約を迂回する公開照会APIも除いた。
  Ready書込中の接続は通常の通知取得から外し、Ready完了後または明示的な
  同一書込で取得する。Hostの結合検査では書込失敗後の再接続も確認した。
  診断用Brokerの実子Channelを
  1接続として通した。製品の複数client実I/Oは残る
  （[ADR 0059](adr/0059-bounded-broker-policy-fanout.md)）。
- Host内の設定・認可変更を期限付きで待てる`wait_for_change`を追加。
  成功した更新だけが配信担当を起こし、最新の公開用policyを返す。
  Brokerの認可判定と2件のProtocol接続へ失効を渡す結合検査で、
  保留中と待機中の双方に通知し、旧要求を拒否して旧結果を公開しないことを確認した。
  失効後の再許可が一つの最新通知にまとめられても、旧要求の結果は復活せず、
  新版の要求だけを受け付けることも確認した。
- `BrokerPolicyRelay`でHostの設定・同意状態からBrokerゲートとFanoutへ
  同じ最新版を渡す。初期版の不一致を拒否し、成功時だけカーソルを進める。
  公開結果から版付き通知frameの所有コピーと退役keyを取得できる。
  2接続への許可と失効、
  遅れた1接続だけの退役を検査した。pipe配送は呼出し側に残る
  （[ADR 0060](adr/0060-host-to-broker-policy-relay.md)）。
- Brokerの成功結果と固定エラーは要求番号・版の照合を共通化し、
  古いErrorの無視と未来ID・版違いの終端化を検査した。
- Broker内の認可判定と接続通知を一つの`publish`操作で進める。
  更新前の認可ゲートとFanoutの版不一致、またはFanoutの更新拒否では
  認可版を進めない。旧版結果の書込guardとも直列化する。
  製品Brokerでも使えるよう、書込完了まで保持する公開guard APIを用意した。
  Fanout内で結果を用意してlockを離した後に失効が入れば、書込guardは
  旧結果を拒否することも確認した。
- `cargo test --workspace --locked --offline --quiet`はローカルHTTPを許可した環境で199件成功。
  期限切れと再接続の追加後のProtocol単体は43件成功。
  `tools/Test-Broker.ps1`で既定Host拒否、実子Channel 24件、Worker 14件を
  正しいビルド順で再確認した。
  `tools/Test-Spike.ps1 -SkipDesktop`も成功し、終了後の既定Host拒否を
  実子Channelで再確認した。パッシブTIP DLLの事前確認も成功した。
  省略なしの対話検査は今回実行していない。
  現在のサンドボックスでは診断用localhost HTTPの4件が接続不可になるため、同じコマンドを通常ユーザー環境で再実行して切り分けた。

## 2026-10-26 の追加確認

- 診断専用Brokerで同じ名前のnamed pipeを2 instance作り、それぞれを固定した
  親process handleとOS peer情報で認証した。既存のJob/Channelは1 instanceのまま。
  pipeの割当順は仮定せず、認証後のHelloから診断用の論理接続を決める。
- 2接続の実I/Oで、保留要求の失効、待機側への通知、旧結果の破棄、再許可後の
  新版結果を確認した。片方の不正な未来Cancelだけを退役・切断し、残る接続の
  保留要求を次の失効で取消した。親側接続を入れ替えた順序でも確認した。
  片方の100ms要求だけを実時間で期限切れにし、他方の結果と期限切れ側の
  後続要求が維持されることも確認した。
  有効なCancelは片方の要求だけを取り消し、旧結果を破棄する。同じ接続の
  次の要求は成功し、その間もう片方の保留要求は維持された。合流した
  Cancelと次要求のframeを順に受信する経路も通した。
  一方の政策通知を未配送にしたまま次版を公開すると、その接続だけを退役・
  pipe切断し、継続側が新版の結果を受け取れることも実I/Oで確認した。
  Hello後に片方が閉じた場合もReady書込失敗先だけを退役し、残る接続の
  Readyと結果配送を維持した。
  誤ったBroker起動IDのHelloもそのpipeだけを拒否・切断し、正常側の
  Readyと結果を維持した。
  2本のpipeの論理client割当を入れ替え、上記の診断を両順序で確認した。
  通常Hostは5種類すべてのfanout診断モードを子process起動時に拒否した。
  片方がReady後にpipeを閉じる診断も追加し、通知の`WriteFile`が切断エラーを
  返すことを実I/Oで確認した。失敗先だけを退役し、残る接続へ
  失効・再許可を配送した後、
  新版の要求から結果を受け取った。
  退役したpipe instanceを同じ子Brokerで作り直し、同じclient寿命IDの世代2を
  改めて認証した。最新の認可版でReadyを受け、再接続側と継続側の両方で
  新しい要求が成功した。旧要求の自動再送は行わない。
  再接続側は切断前に要求1を完了し、要求2を保留したまま切断した。
  旧結果を破棄し、世代2では要求3から新規に送った。
  診断の反復後、親processのhandle件数が基準値へ戻ることも確認した。
  既定Hostはこの診断serviceを拒否し、検査後に既定Hostへ戻した
  （[ADR 0061](adr/0061-two-client-broker-pipe-diagnostic.md)）。
  これは一つの親processの2本であり、常駐Brokerの異なるTIP process接続ではない。
- `cargo test --workspace --locked --offline --quiet`は199件成功。
  `tools/Test-Broker.ps1`で既定Hostの拒否と2接続診断を含むBroker回帰が成功し、
  通常Hostへ復元した。`tools/Test-Spike.ps1 -SkipDesktop`も成功し、
  パッシブTIPと通常Hostへ復元した。恒久的なTSF登録は行っていない。
- Host側の政策公開に、公開直前の全接続keyを保持する有限な配送receiptを追加。
  Ready書込中の接続も含み、通知の完全な書込または実pipeのcloseまで設定変更の
  完了を保留する。公開時に退役した接続もcloseを待ち、古い版の通知や重複した
  完了報告は受け付けない。Hostの該当単体検査を追加し、全Rustテスト200件が成功。
  設定UIへの実際の完了通知と製品pipeへの配送は未接続。
- Worker完了が期限timerより先に到着した場合も、期限後の成功結果と
  Timeout以外の固定エラーをその要求だけで破棄する。接続を退役させず、
  次の単調増加した要求を受け付ける単体検査を追加した。実子Brokerの
  2 pipe診断もこの順序に変更し、両方のpipe割当順で通した。
  全Rustテスト201件が成功。
- 配送receiptの境界として、変更時に接続がなければ即時完了し、後から接続した
  clientのReadyは最新版になることを検査した。古い通知が既に完全に書かれ、
  UI側の確認だけが遅れた場合は、後続版の完全な配送でも先のreceiptを満たせる。
  この場合は`Superseded`として返し、古い版そのものが届いたと扱わない。
  全Rustテスト203件が成功。
- 配送receiptにBroker起動IDを含めた。別Brokerで再利用された接続keyからの
  遅延した配送・close完了を拒否し、旧receiptの対象を誤って完了にしない。
- Fanoutへの接続試行は、不正Hello・重複session・容量超過でも接続keyを消費する。
  拒否されたkeyを次のpipeへ使わず、遅延した完了通知の宛先を固定する。
- Workerが期限後に不正な結果本文を返した場合は、本文検証より先に期限切れの
  要求を終える。接続を退役させず、次の要求へ進めることを検査した。
- 2接続診断の政策通知は両pipeのoverlapped書込を先に開始し、最大8件から
  完了可能なI/Oを選んで確認する。部分書込も有限期限内に完了してから配送を承認する。
  製品の一般的なI/O調停点と遅いclientの独立監視は引き続き必要。
- transportの待機処理を、実named pipeで2本ともRead保留中の状態から検査した。
  片方への書込だけで該当I/Oを選び、もう片方も後続書込で完了する。両方が再び
  保留中にStopを通知すると、取消処理を通して終了する。全Rustテスト204件が成功。
- 2本のReadを保留したまま片方だけ75msで期限切れにし、もう片方のpipeが
  後続のbyteを受け取れることを実I/Oで確認した。全Rustテスト205件が成功。
- CIのWindows runnerは昇格processで、相互認証が意図的に拒否するため、
  実named pipeの3件はその環境だけ実I/Oを行わない。通常ユーザーのローカル環境では
  同じ3件を実I/Oで通した。昇格を許可して検査をすり抜ける変更は行っていない。
- 2本のRead保留中に片方のclient pipeを閉じ、切断した側だけが失敗し、
  生存側は後続のbyteを受信できることを確認した。全Rustテスト206件が成功。
- 2本のReadを異なるStopで保留し、片方だけを取消してcloseしても、
  残る接続のReadが後続のbyteを受信した。通常ユーザーの実pipeで確認した。
  全Rustテストは207件。
- 書込が部分完了した場合は同じoperationの残りを再開し、もう一方のpipeと
  再び完了可能なI/Oを選ぶ。64 KiBの連続byteを実pipeで読み戻し、
  模擬した部分完了では未送信suffixだけを再開した。診断Brokerの2接続回帰も通した。
  全Rustテスト209件が成功。
- 部分完了と次chunkの間でStopが通知された場合は、残りのbyteを送らず
  そのpipeの操作を拒否することを実pipeで確認した。全Rustテスト210件が成功。
- 共通の`Pipe::write_all`も同じ部分書込継続処理を使うよう統一した。
  全Rustテスト210件、実子Broker回帰、既定Hostの拒否検査が成功した。
- 読み手のいないpipeへ最大I/Oサイズを書いて保留中でも、もう一方のpipeへの
  1 byte通知が先に書き終わり、受信できた。遅い側だけを取消した後も速い側は
  生存した。通常ユーザーの実pipeで確認し、全Rustテスト211件が成功。
- 最大8接続の有限な`WriteBatch`へ通知書込の開始・部分継続・完了選択を
  集約し、診断Brokerもこれを使用する。失敗した接続の結果だけを返し、他の
  書込を維持する。開始時に片方が取消済みでも、別接続が通知を受信したことを
  実pipeで確認した。全Rustテスト212件が成功。
- Hostの`PolicyAuthority`から`BrokerPolicyRelay`で許可版を公開し、
  同一process内の認証済み2本の実pipeへ通知を書いた。各書込の完了後に
  Fanoutと配送receiptを進め、両clientが同じ版を受信したことを通常ユーザーの
  実pipeで検査した。
  全Rustテスト213件が成功。常駐製品Brokerへの接続と設定UIは未実装。
- Host公開後、片方のclientがpipeを閉じた場合はその通知だけが切断失敗と
  なり、Fanoutから退役した。生存側には同じ版が届き、失敗したserver pipeを
  実際に閉じた後にだけ配送receiptが完了した。全Rustテスト214件が成功。
- `BrokerPolicyRuntime`にHostの政策元、受付ゲート、Fanout、Relayと認証済みpipeを
  集約した。Ready書込後の接続有効化、許可・失効の2接続配送、失敗先だけの
  物理切断、配送receipt、取消対象の接続keyと要求番号、誤ったHelloやReady失敗後の
  key消費、未公開の設定変更での旧本文拒否、内部状態不一致後の終端を実pipeで確認。
  同じclient寿命IDの再接続では、旧世代Helloと要求番号の再使用を拒否した。
  未来IDの不正Cancelはその接続だけを退役させ、他方の保留要求と次要求を維持した。
  全Rustテスト222件が成功（[ADR 0063](adr/0063-host-policy-runtime.md)）。
  製品Host EXEの主ループと異なるTIP processへの接続は未実装。
- 登録不要の`policy-runtime`診断モードをHost EXEへ接続した。既定Hostは拒否し、
  `spike` Hostだけが親から認証した2本のpipeにReady、許可、失効を配送する。
  許可版で受けたConvert要求は失効時に該当接続だけ取り消した。接続順を反転した
  試行と親handleの回収も`tools/Test-Broker.ps1`で確認し、通常Hostへ復元した。
- Host実行部はOS accept後、Hello読取前に接続keyを予約できる。政策公開中に
  Hello待ちのpipeが残れば閉じてから完了を返し、そのkeyは再利用しない。
  通常ユーザーの実pipe回帰を追加し、全Rustテスト223件とClippyが成功。
  製品accept loopの各accept直後に予約する経路と継続運転は引き続き必要
  （[ADR 0063](adr/0063-host-policy-runtime.md)）。
- Host実行部にBroker起動ID・接続key・要求番号・政策版を結び付けた
  結果／固定エラー公開を追加。Host政策元とBroker受付ゲートを最大500msの
  書込完了まで保持し、失効が先なら旧結果を拒否する。別Brokerの遅延完了も拒否。
  実pipe検査で成功結果、固定エラー、失効後の旧結果拒否を確認した。
  子process診断でも一方へ成功結果を返し、他方の保留要求だけを失効時に取消した。
  全Rustテスト225件、Clippy、Broker回帰が成功し、通常Hostへ復元した。
  実worker完了の調停とAPI送信開始の失効競合はまだ未接続。
- 子process診断の受付順序を変更し、各OS accept直後に接続keyを予約してから
  次の接続を待つようにした。Broker回帰は両接続順で成功。複数Helloの並行読取と
  設定変更の同時発生は製品用の継続ループで検査する。
- Host実行部の期限timer入口が、接続ごとの取消対象要求番号を返す。期限後の
  worker完了は本文を送らず、同じ認証済みpipeで次の要求を受け付けた。
  通常ユーザーの実pipeで確認した。
- 接続切断時にも対象の保留要求番号を返し、遅延完了を旧keyへ配送しない。
  他方のpipeと結果配送は維持した。通常ユーザーの実pipeで確認し、
  全Rustテスト227件。
- OS accept後、Hello完了前にHost設定が変わる場合は旧pipeにReadyを返さず
  閉じ、公開後に接続した別pipeだけが新版のReadyを受けた。実pipeで確認し、
  全Rustテスト228件。
- 結果frameの書込に失敗した接続だけを退役させ、生存する別pipeは結果を受けた。
  政策元が読取不能なら実行部全体を終端する。通常ユーザーの実pipe回帰を追加し、
  全Rustテスト229件。
- 500msを超える結果書込期限は要求を消費せず拒否し、適正な期限で同じ要求の
  結果を配送できた。通常ユーザーの実pipeで確認し、全Rustテスト230件。
  `sumibi-host/spike`構成の全テストとClippy、登録不要の
  `tools/Test-Spike.ps1 -SkipDesktop`も成功し、通常TIPとHostへ復元した。
- Host実行部がConvert受付ごとにBroker起動ID・接続key・要求番号・政策版を持つ
  `JobTicket`と専用Stopイベントを発行する。明示Cancel、期限、失効、切断、
  不正frame、実行部終了時に該当ticketを取消し、別接続のticketを維持する。
  結果が正常に配送されたticketは取消さず台帳から外す。実workerとtransportの
  Stop待機への接続は次段階。
- `FrameInbox`をHostと診断Brokerで共用し、分割frameと同一Readに連結された
  後続frameを保持する。HostのHello後に連結された要求、active接続の不正本文に
  よる当該pipeのみの退役を実pipeで確認した。
- 最大8本の`ReadBatch`で各pipeのoverlapped Readを先に開始し、完了と開始失敗を
  接続別に回収する。実pipeで遅い接続と開始失敗の独立性を確認した。当時の子process
  2接続Host診断はHelloとConvertの双方を並行受信し、接続順・要求到着順を変えて
  成功。全Rustテスト237件、Broker回帰、Clippyが成功し、通常Hostへ復元した。
  製品Hostの継続accept/read/worker調停ループは引き続き必要。
- Host政策元に送信開始guardを追加。`manual::convert_ordered`と診断loopback
  HTTPは、資格情報headerと本文の書込全体を同じguard内で行う。
  TCP接続後・最初のbyte前の失効ではserver受信量0を実ソケットで確認。
  送信後の失効では結果を破棄し、同一byte列の資格情報差替えも旧識別子では
  拒否した。旧snapshot専用送信経路は単体テスト構成に限定。
  全Rustテスト241件とClippyが成功（[ADR 0064](adr/0064-outbound-send-start-order.md)）。
  製品HTTPS adapter、workerのI/O取消、設定UI完了待機は未接続。
- `JobTicket`を`convert_ordered`の送信許可にも結び付けた。接続が先に退役した
  ticketではHost同意が残っていても書込を0回に抑え、送信が先に始まった場合は
  切断の確定が有限な書込終了を待つ。取消イベントは待機中のworkerへ直ちに通知。
  実pipeを使うHostの競合検査を追加し、全Rustテスト242件。
- Hostに接続別のキュー済みframeだけを処理する`poll_active_message`を追加。
  片方が分割frameの途中でも他方のConvertを先に受理し、後続chunkで元の
  frameも受理する実pipe検査を通した。部分frameは10秒で退役し、
  byte未到着のアイドル接続は維持する。子process診断は失効後に再許可し、
  同じ接続の単調増加した次要求をこの入口で読み、結果を返した。
  全Rustテスト244件、Broker回帰が成功し、通常Hostへ復元した。
- Helloの分割受信を`poll_reserved_hello`で扱い、2本とも読取途中にHost政策が
  変わった場合は両pipeを閉じてから公開receiptを完了する。旧Readyは送らず、
  次接続は新版Readyで始まる。Convertも分割chunk間に失効した旧本文を拒否し、
  生存する別pipeだけに失効通知を届けた。通常ユーザーの実pipeで確認し、
  全Rustテスト246件。製品のacceptとworker結果を含む継続調停はまだ必要。
- OS受付時の接続key予約をFanoutにも記録し、先のHelloが分割されていても後の
  Helloを先に有効化できるようにした。退役した予約keyは再利用せず、activeと
  Hello待ちを合わせて最大8本に制限する。設定変更はReadyの有界書込とHost側で
  順序付け、変更が先なら旧Readyを拒否する。
- `poll_once`は公開、期限切れ、Hello、active frameを1回ずつ進める調停入口。
  同一pipe内にCancelと次要求が連結されても、別pipeの完成要求を間に処理する。
  子process診断の再許可後要求もこの入口を使う。OS acceptと実worker完了を含む
  製品の常駐ループはまだ接続していない。全Rustテスト250件、Clippy、
  Broker子process回帰が成功し、既定Hostへ復元した。
- EXE側の`BrokerWorkers`は最大8 workerと8完了を保持し、`JobTicket`のStopを
  workerへ渡す。取消された完了は破棄し、成功と固定エラーは同じ
  `WorkerCompletion::publish`から元のBroker起動ID・接続key・要求番号の検査へ
  戻す。子process診断は失効したworkerを回収し、再許可後の要求もworker完了と
  `poll_once`を交互に進める。通常ユーザーの実pipe→worker→loopback HTTP→
  元のpipeの成功結果と、次要求の固定エラーを確認した。実HTTPS adapter、
  永続的な設定・資格情報、製品Hostの継続acceptループは未接続。
  全Rustテスト252件、Clippy、Broker子process回帰が成功し、既定Hostへ復元した。
- `JobTicket`にBrokerがConvertを受理した時点の期限を固定し、workerが後の期限を
  指定して本文送信を延ばすことを拒否する。期限後のworker投入も拒否する。
  診断HTTPの応答読取は最大50msごとに取消を見直し、失効で待機workerを回収する。
  実pipe・worker・loopback HTTPの連結検査で、失効後に旧結果を配送せず、
  他の要求を受けられる接続を維持した。TCP接続中と書込中の取消は各有限期限で
  境界を定める。製品HTTPS adapterにも取消待機が必要。全Rustテスト255件、
  Clippy、Broker子process回帰が成功し、既定Hostへ復元した。
- 設定操作が返した版を`acknowledge_version`へ渡すと、政策公開を完了してから
  `ExactOrClosed`または`Superseded`を返す。2本への通知とHello待ちpipeの切断、
  失効後の再許可をまとめた新版への置換、未来の不正な版の拒否を実pipeで確認。
  全Rustテスト256件とClippyが成功。製品設定UIからこの入口を呼ぶ経路と、
  完了後の表示更新はまだ未接続。
- worker完了時刻だけでなく、Hostが結果をpipeへ書き始める現在時刻も元の要求期限と
  比較する。期限内完了をHostが遅れて回収した場合、結果を送らずticketを取消し、
  同じpipeで次要求を受ける。版を偽った完了は接続を退役させる。全Rustテスト257件、
  Clippy、実pipeのBroker回帰が成功。
- `BrokerWorkLoop::advance`が政策公開、worker完了、次のpipe frameを一つの所有側
  入口で順序付ける。worker枠が取消済み仕事で埋まる場合は新要求へ固定の
  `Unavailable`を返し、受け付けた要求を宙に浮かせない。待機中の完了より失効通知を
  先に配送する実pipe検査を追加し、子Broker診断の再許可後の要求もこの入口へ接続。
  全Rustテスト260件と実pipeのBroker回帰が成功。OSの継続acceptと製品送信workerは
  まだ接続していない。
- 容量超過を`Unavailable`で返した後、取消済みworkerを回収して枠が空けば、
  同じ認証pipeで次の要求を受け、結果を返せることも実pipeで確認した。
- 認証pipeから`BrokerWorkLoop`で受けた本文をloopback HTTP workerへ渡し、元のpipeへ
  結果と固定エラーを戻す結合検査に切り替えた。HTTP応答待機中の政策失効も同じ
  所有側ループで先に通知し、workerを取消す経路で確認した。
- 別の接続が分割Helloの途中でも、所有側ループが先の接続のworker結果を返し、
  後からHelloを完成させて最新版のReadyを送れる実pipe検査を追加した。
  全Rustテスト261件が成功。
- OS認証済みpipeを所有側ループへ渡す`accept`入口を追加した。接続key予約の前に
  未公開のHost政策を配送し、新接続はその版でReadyになる。子Broker診断の2接続の
  受付もこの入口へ接続し、全Rustテスト262件、Clippy、実pipeのBroker回帰が成功。
- `BrokerWorkLoop::advance_manual`で認証済み要求からHostの`convert_ordered` workerを
  作る入口を追加した。Runtimeとworkerの政策元が同じインスタンスかを確認し、
  worker内で現行の宛先を取得する。実pipe→loopback HTTP→元のpipeの結果と、
  HTTP応答待機中の失効をこの入口で検査した。全Rustテスト263件とClippyが成功。
  製品HTTPS adapterは未接続。
- 同じ認証pipeの次要求を実際のloopback HTTP 503へ送り、workerが固定の
  `Unavailable`だけを元のpipeへ返すことを検査した。2要求とも送信に同じ資格情報と
  元本文を使い、結果成功の後も接続が有効なことを確認した。
- 設定先を別のloopback HTTP宛先へ変更し、最新版の政策通知を配送してから
  次要求を送る検査を追加した。workerが旧宛先を使わず、新しいパスに送って
  結果を元のpipeへ返すことを確認した。全Rustテスト264件とClippyが成功。
- `BrokerWorkLoop::acknowledge_version`は政策通知・切断のreceiptに加え、
  取消済みworkerの実際の終了を有限期限で待つ。政策通知が先に済んでいた場合も待機し、
  HTTP応答待機workerの取消後に同じpipeで再許可の次要求を受けることを確認した。
  workerが期限内に終わらなければ設定完了を成功とせずTimeoutを返す。
  全Rustテスト265件と診断featureのClippyが成功。
- `AcceptTask`でOSの接続待ちを有限期限の専用threadへ移し、所有側からは非同期に
  結果を取得する。結果は固定された親processとdesktop tokenを照合した後のpipe
  だけを含む。取消が受付完了と競合してもpipeを公開しない。子Brokerの診断2接続を
  この受付へ接続し、2本目のOS accept中に1本目のHelloとReadyを完了する段階的な
  子process検査を追加した。さらに受付待ち中の政策失効・再許可、worker結果、
  後続接続の最新版Readyを実pipeで確認した。全Rustテスト269件、Broker回帰、
  Clippyが成功し、既定Hostへ復元した。製品Hostの共有発見と長寿命の受付は未接続。
- `BrokerWorkLoop::advance_with_accept`と`advance_manual_with_accept`で、受付結果、
  政策公開、worker結果、既存pipeのframeを所有側の1ステップへまとめた。OS受付の
  期限切れは既存接続を退役させず、後続の変換結果を返す。先に接続したpipeが
  待機中でも後の受付完了を先に取り出し、未公開政策を新pipeのkey予約より先に
  完了する。診断HTTPの宛先変更後の送信も受付待ち中に通した。子BrokerのHelloも
  この入口へ接続し、2本目を待ちながら1本目のReadyを返す。受付取消・期限切れ・
  instance差替えでOS枠が戻る実pipe検査を追加した。全Rustテスト275件とBroker
  回帰が成功。製品TIPの信頼できる発見と長寿命Host起動は引き続き必要。
- `advance_manual_with_accept`は別のHost政策元を受けた場合、接続keyと受付taskを
  消費する前に拒否する。正しい政策元で同じ認証pipeを後から受け付けられる。
  設定変更の`acknowledge_version`が接続済みだが未予約のOS pipeより先に完了した
  場合、後続のHelloには受領済み最新版のReadyだけを返す。実pipeで確認した。
  全Rustテスト276件。
- `AcceptTask`の認証を別の実processで確認した。起動時に固定した子processが
  pipeへ接続すれば認証済みendpointだけを渡す。同じlogon SIDを持つ現在のprocessが
  その子process向けの受付へ接続しても`WrongPeer`で拒否し、未認証pipeは所有側へ
  渡さない。これは共有Hostに異なるTIPを接続する製品発見の代用ではない。
  全Rustテスト279件。
- 同じ`BrokerPolicyRuntime`へ親processと別の子processをそれぞれ固定して接続し、
  所有側の受付・Hello・Convert・worker結果まで実pipeで通した。子processが
  Readyと結果を受けて終了した後も、親processの既存接続は次の要求を処理できる。
  子fixtureは標準Fileから接続するため、client側のBroker認証をこの検査だけで
  証明するものではない。製品TIPの共有発見・所有TSFへの配送は未接続。
  全Rustテスト281件。
- 子Broker診断の初回2要求も`BrokerWorkLoop::advance`へ移した。後から届く
  要求の受理を待ってから結果workerを完了させ、所有側で結果を配送する。
  政策通知も同じ入口で公開し、失効時は同じ所有側poolの取消workerを回収して
  `acknowledge_version`が版と実際のworker終了を確認してから再許可後の次要求へ継続。
  以前の診断専用`ReadBatch`読取は除き、`ReadBatch`自体の並行I/Oは実pipe単体検査で
  維持する。Broker実子process回帰と`Test-Spike.ps1 -SkipDesktop`が成功し、
  既定HostとパッシブTIPへ復元した。
- 設定変更が連続して取消workerを回収すると、既に退役したworkerの完了が8件の
  channelを埋め、次の取消workerを送信待ちのまま残す問題を修正した。回収時に
  退役済み完了を破棄し、生存中の別接続の完了だけを次の所有側stepへ保持する。
  9件連続の取消回収と、同時に残る生存結果の配送を実pipeで検査した。
- 共有pipe groupを2件専用から最大8件へ一般化し、各instanceを同じACL・
  process全体のendpoint枠で制限した。`BrokerPolicyRuntime::begin_replacement_accept`は
  生存中の認証済み接続から次の受付instanceを作り、呼出し側が信頼経路で固定した
  processだけを受け付ける。3接続を所有側ループで有効化し、3件目の要求結果を
  配送後、1件目を閉じて4件目を受け直す実pipe検査が成功した。
  唯一のclient processが終了しても、認証済みserver handleを保持している間は
  同じgroup内で次の受付を作り直せることも別processの実pipeで確認した。
  別processに固定した受付へ同じユーザーの誤ったprocessが接続した場合、拒否後も
  既存接続を維持して差替え受付を作れる。全Rustテスト287件。TIP processの共有発見と
  製品Hostの常駐起動はまだ別途必要。
- 受付側のpeer認証が終わっていても、所有側が期限後に結果を回収した場合は
  接続を公開せずTimeoutで閉じる。有限期限の直後に接続結果を回収する実pipe検査を
  追加し、全Rustテスト288件が成功した。
- `AcceptTask::cancel_and_reap`で取消後のOS受付workerの退出を有限期限で確認できる。
  `BrokerWorkLoop::stop_and_reap_with_accepts`は全受付を先に取消し、変換workerも
  回収してからpipe名を再利用可能にする。未終了なら失敗を返し、taskを残して
  再試行できる。2受付と進行中の変換workerを同時停止する実pipe検査を追加した。
  全Rustテスト290件。
- Broker起動中の接続履歴64件を使い切ったら、新しいOS受付と接続key予約を
  `Exhausted`で止める。既存の最後の接続は要求と結果配送を継続できる。
  64接続を実pipeで順に有効化した検査で、65件目の予約を拒否し、最後の接続が
  結果を受け取ることを確認した。全Rustテスト291件。新しいBroker起動IDでの
  製品側ローテーションはまだ未接続。

## 次の受入作業

製品Brokerへ統合する責務と順序は
[ADR 0062](adr/0062-product-broker-integration-sequence.md)に記録した。

1. `BrokerWorkLoop`と`BrokerPolicyRuntime`を製品Hostの継続するOS acceptへ接続する。
   共有groupの3接続と差替え受付まで実pipeで確認した。長寿命の製品受付、設定UIの操作完了と
   所有TSFでの結果適用前再検査も製品経路で実証する。
2. 配置されたBrokerの信頼できる発見・署名・更新・起動を実装する。
3. 一般アプリへ製品TIPのキー配送と所有対象の失効を接続し、入力先ごとの保護情報を肯定的に検証する。
4. Notepad等の実アプリで実打鍵、1回Undo、別欄・同長別本文・読取不能の拒否を確認する。

機械全体のTSF登録は今回行っていない。恒久登録に進む前の条件は
[ADR 0045](adr/0045-machine-registration-boundary.md)に記録している。
