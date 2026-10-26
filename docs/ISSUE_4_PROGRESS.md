# Issue #4 の受入進捗

2026-10-26 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) /
[draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
ここでいう成功は、明記した診断Host・専用TSF入力欄・合成本文での結果に限る。
製品の一般アプリ対応や実API利用を意味しない。

| 受入条件 | 現在の根拠 | 残る条件 |
| --- | --- | --- |
| 版付きJSON、ID・世代・取消・切断・重複・再接続、有限容量 | `sumibi-protocol`の`ClientSession`、`sumibi-ipc`の単発Jobと再利用Channel/Worker。要求・接続・結果の容量、期限、古い返信と未発行の未来IDを検査 | 製品Brokerの接続・設定変更元を接続する |
| 別プロセスの相互認証と偽server/PID再利用/終了待機 | 起動時の固定process handle、user/logon SID・session・integrity・PID・生成時刻とpipe peer IDを照合。実子processと偽peer診断を実行 | 配置済みBrokerの信頼できる発見、署名・更新・保護された起動経路 |
| 非同期pipe、バックプレッシャー、所有TSFスレッドへの結果配送 | 有限I/Oと取消、要求前Permit、EXE workerから所有STAへの非同期結果配送。診断Brokerの同一親processからの2接続にも失効と退役を配送 | 製品TIPと一般アプリへの配送・policy通知の全接続への公開順序 |
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

## 次の受入作業

製品Brokerへ統合する責務と順序は
[ADR 0062](adr/0062-product-broker-integration-sequence.md)に記録した。

1. 同じ製品Host内の設定・認可変更元を常駐Brokerの実I/Oへ接続する。複数clientへの通知失敗時の切断、設定UIの操作完了、送信開始と失効の順序、所有TSFでの適用前再検査を実証する。
2. 配置されたBrokerの信頼できる発見・署名・更新・起動を実装する。
3. 一般アプリへ製品TIPのキー配送と所有対象の失効を接続し、入力先ごとの保護情報を肯定的に検証する。
4. Notepad等の実アプリで実打鍵、1回Undo、別欄・同長別本文・読取不能の拒否を確認する。

機械全体のTSF登録は今回行っていない。恒久登録に進む前の条件は
[ADR 0045](adr/0045-machine-registration-boundary.md)に記録している。
