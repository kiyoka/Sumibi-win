# Issue #4 の受入進捗

2026-10-26 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) /
[draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
ここでいう成功は、明記した診断Host・専用TSF入力欄・合成本文での結果に限る。
製品の一般アプリ対応や実API利用を意味しない。

| 受入条件 | 現在の根拠 | 残る条件 |
| --- | --- | --- |
| 版付きJSON、ID・世代・取消・切断・重複・再接続、有限容量 | `sumibi-protocol`の`ClientSession`、`sumibi-ipc`の単発Jobと再利用Channel/Worker。要求・接続・結果の容量、期限、古い返信と未発行の未来IDを検査 | 製品Brokerの接続・設定変更元を接続する |
| 別プロセスの相互認証と偽server/PID再利用/終了待機 | 起動時の固定process handle、user/logon SID・session・integrity・PID・生成時刻とpipe peer IDを照合。実子processと偽peer診断を実行 | 配置済みBrokerの信頼できる発見、署名・更新・保護された起動経路 |
| 非同期pipe、バックプレッシャー、所有TSFスレッドへの結果配送 | 有限I/Oと取消、要求前Permit、EXE workerから所有STAへの非同期結果配送。診断Brokerの2接続と、Host政策実行部から同一processの実pipe2本への通知・失敗隔離を確認 | 製品Hostのaccept/read/worker結果ループへの接続、異なるTIP processと一般アプリへの配送 |
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
  全Rustテスト221件が成功（[ADR 0063](adr/0063-host-policy-runtime.md)）。
  製品Host EXEの主ループと異なるTIP processへの接続は未実装。

## 次の受入作業

製品Brokerへ統合する責務と順序は
[ADR 0062](adr/0062-product-broker-integration-sequence.md)に記録した。

1. `BrokerPolicyRuntime`を製品Hostのaccept/read/worker結果ループへ接続する。OS accept時点のkey発行、異なる到着順のHello、設定UIの操作完了、送信開始と失効の順序、所有TSFでの適用前再検査を実証する。
2. 配置されたBrokerの信頼できる発見・署名・更新・起動を実装する。
3. 一般アプリへ製品TIPのキー配送と所有対象の失効を接続し、入力先ごとの保護情報を肯定的に検証する。
4. Notepad等の実アプリで実打鍵、1回Undo、別欄・同長別本文・読取不能の拒否を確認する。

機械全体のTSF登録は今回行っていない。恒久登録に進む前の条件は
[ADR 0045](adr/0045-machine-registration-boundary.md)に記録している。
