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

2026-10-30: `broker-http-key-test`を追加し、別process Hostが明示同意と送信順序を
確認したうえで、専用TSF入力をローカル模擬HTTPへ渡す経路を用意した。
模擬HTTPの分割要求・不正資格情報・不正本文の単体検査とclippyは成功した。
通常のWindows環境で利用者が`tools/Test-BrokerHttpKey.ps1`を実行し、
`process-http: OK`、COM/TSFの登録解除とDLL解放、およびパッシブTIPへの復元を確認した。
合成入力から別process Hostの同意・送信順序、localhostの応答、TSF編集と1回Undoまで通った。
この診断は合成入力とlocalhostのみを使い、製品HTTPSや実APIを実装しない。
診断HTTPの送信は部分書込のたびに残り期限を計算し直し、期限切れ後は
残りを送らない。これにより送信中のpolicy guardを期限内に解放できるようにした
（OSの書込からの復帰遅延は別途あり得る）。製品HTTPSへの適用は今後必要。

Mac版の固定コミットを基準に、初回変換用のChat Completions互換JSON生成と
応答からの候補抽出を追加した。モデルは明示指定し、原文の書記素数を1000以下、
IPC側のUTF-16上限も守る。応答は完全な1候補JSON、完了理由stop、拒否なしに限る。
Mac版が許す通常文字列や部分JSONは現段階では受け入れない。これは通信を行わない
純粋な処理であり、辞書・追加候補・製品HTTPS・同意UIとの接続は未実装。
この初回変換JSONを診断用localhost HTTPへ接続し、既存の同意・期限・取消の
送信境界を通して模擬応答から1候補を取得する試験を追加した。同意取消後の拒否、
不完全応答と外部HTTPSの拒否も確認した。接続先パスの補完はtransport内部では行わない。
設定時にChat Completions用の完全なパスを生成する入口を追加し、補完後の完全な
送信先を同意と送信の両方に使う。クエリ・フラグメント・親移動などは拒否する。
補完パスへのlocalhost送信と、補完前の別パスを使った同意の拒否を検査した。
診断用のHost policyへモデル名を追加し、モデル変更と設定版更新を同じロック下で行う。
診断用の初回変換入口は要求版と同じsnapshotからモデルを取得し、送信前後も版を再検査する。
localhost通信中のモデル変更による旧結果の破棄、古い版の拒否、不正なモデル名や
版の枯渇時に設定を変更しないことを確認した。モデル設定の永続化・製品Hostや
実サービスとの接続は残る。
BrokerBootからも診断用Chat処理を呼べるようにし、同一processの認証済みpipeから
設定済みモデルを含むlocalhost模擬HTTP要求と候補結果までを往復確認した。
この検査は別processや一般アプリ、製品HTTPSへの対応を意味しない。
さらに別processのfixtureを起動handleで固定・認証し、同じBroker経路から
localhost模擬Chatへ送信して、候補を子processへ返すことを確認した。
これは一般アプリの製品TIPや製品HTTPSの確認ではない。
全体テストの並列実行では既存IPC試験が一度失敗し、共有ロックのpoisonが連鎖した。
最初の失敗原因は未特定だが、IPC単独と全体の直列再実行、および後日の並列再実行は成功した。
後続の並列回帰でhandoff準備のタイムアウト確認が再度失敗した。
イベント直後でも結果mutexが解放前なら非ブロッキングsnapshotはNoneになり得るため、
この試験を期限内にsnapshot公開を待つ形に修正した。製品の非ブロッキング処理は維持した。
修正後は対象試験を5回連続で実行し、全体の並列回帰も成功した。
Host専用のWindows Credential Managerアクセスを追加した。固定名のgeneric資格情報を
現在ユーザー・このPC向けに保存、読取、削除し、不正な文字やサイズは拒否する。
HostのCredentialへの変換入口も追加した。読取や保存だけでは同意を与えない。
別名・セッション限定の偽キーで保存、更新、読取、削除を実機確認し、試験後削除した。
Hostの保存・削除・再読取をpolicy更新と直列化する入口も追加した。保存失敗時も
旧キーと同意を破棄し、Brokerへの通知が必要なpolicy版とエラーを返す。送信中の
guardとの順序、失敗時の旧許可の無効化と再同意の必要性を試験した。この同期処理は
設定workerで使い、TSF callbackでは呼ばない。資格情報変更の結果は更新元の
policyに結び付け、同じBrokerだけが通知・取消workerの終了を確認できる。
模擬保存失敗後の実pipeへの禁止通知、別policyの拒否、期限切れからの再試行を確認した。
再読取を設定workerへ移す入口を追加し、同じpolicyの同時再読取を1件に制限した。
このworkerを保存・削除にも拡張し、worker経由の3操作を同一policyにつき同時1件に
制限した。不正なキーの保存失敗時に旧キー・同意を破棄することも確認した。
workerの完了を非ブロッキングで取得し、同一policyのBrokerで通知完了を待てる。
模擬読取失敗をworkerから実pipeへの通知まで確認した。taskの破棄は進行中のOS読取を
取消さないため、Broker側は引き続きpolicy変更を配信する必要がある。
製品UI・起動時読取からこの完了待ちまでの接続は未実装で、実キーの登録は行っていない。
接続先とモデルを保存する版付きJSONを追加した。完全なChat送信先を保存し、
キー・入力本文・送信同意は含めない。読取時はサイズ、版、未知・重複項目と送信先を
検証し、復元後のpolicyはキーなし・同意なしとする。同じディレクトリの新規一時
ファイルを書き込み・flushしてから置換し、一時ファイルを後始末する。保存と同じ
policyの接続先・モデル更新を直列化し、保存失敗時は稼働中の設定を維持する。
送信先変更時は旧同意を解除し、モデルのみの変更でも旧設定版の結果を無効化する。
保存後の設定版を同一Brokerで通知完了する入口と実pipe試験を追加した。配置先の選定、
停電時の永続性保証、製品UIと常駐起動処理への接続は残る。
保存・初回作成は固定の空sidecarファイルをOSロックし、競合時は待機せず拒否する。
設定編集用には読込時の内容と一致する場合だけ保存・policy更新する入口を追加した。
別processのロック競合、解放後の保存、古い編集内容の拒否、異常なlock内容を
修復せず拒否することを検査した。lockファイルは通常処理で削除せず再利用する。
この調整は協調する保存処理が対象で、無条件保存入口は古い編集内容の比較を行わない。
設定保存を専用workerから実行する入口を追加し、資格情報操作と合わせて同じpolicyの
workerを同時1件に制限した。完了結果は非ブロッキングで取得し、同一Brokerへの
通知完了を確認できる。送信guard待ち、二重起動拒否、古い編集の拒否と実pipe通知を検査した。
taskを破棄しても開始済みの書込は取り消さず、Brokerはpolicy変更の配信を継続する。
保存済み設定を検証してから資格情報を読み、送信未同意のHostを生成する起動準備処理を
追加した。破損設定ではキーを読まず、キーの存在・欠損・読取失敗の各状態でも送信を
許可しない。`sumibi-host startup-check <absolute-settings-path>`から呼び出せる。
このコマンドは起動準備の読取検査で、待受や変換要求を開始しない。製品常駐起動と
UIからの再同意は残る。
設定の既定配置を現在ユーザーのKnown Folder LocalAppData配下の
`Sumibi\settings.json`とし、`startup-check`はパス省略時にこの場所を確認する。
`settings-init`でHTTPS接続先・モデルを新規作成する入口を追加した。既定配置先または
明示した絶対パスへ作成し、正常・破損のどちらの既存ファイルも上書きしない。
試験用ファイルの新規作成と二重実行時の保持を実行ファイルで検証した。
`settings-window`で作成済み設定のホスト名・モデル・パスを編集する単独画面を追加した。
保存はworkerと競合検査を使い、次回起動用の設定として保存する。試験用ファイルで
画面表示、モデル変更の保存、空モデルの拒否、×での終了をWindows画面上で確認した。
さらにAPIキーの伏せ字入力と明示保存を追加し、既存の資格情報workerへ接続した。
空白・改行・非ASCII・長さ超過を保存前に拒否し、保存開始後は入力欄を空にする。
保存済みキーは読み取って表示しない。画面の表示確認と入力検査の単体試験を行い、
画面確認では保存を実行せず、既存の実キーを変更していない。
キー削除ボタンと「いいえ」を既定とする確認を追加し、既存の削除workerへ接続した。
画面上は配置確認のみ行い、現在ユーザーの保存済みキーは削除していない。
保存・削除の進行中は設定保存とキー操作を無効にし、終了操作も完了を待つ。
送信同意・稼働中の製品Brokerへの接続と常駐起動は引き続き未接続。
製品HTTPSへの前段として、native WinHTTPの非同期要求準備と最終callbackまでの
context保持を追加した。TLS既定・転送等の抑止・失効確認を設定し、要求枠は8件。
準備・超過拒否・明示closeとDrop後の解放を試験した。
外部サービスへの通信は行っていない。詳細は[ADR 0070](adr/0070-native-https-request-lifetime.md)。
続いて初期本文のnative送信完了・固定エラー分類を追加し、Hostの送信guardから呼ぶ
入口を用意した。localhostの合成本文送信と、TLS待機の取消・期限切れ・最終context
解放を試験した。続いて応答受信を追加し、HTTP status・JSON media type・header重複を
検査する。本文64 KiBの上限、長さ照合、chunked処理、受信中の取消・同一期限と最終
context解放をlocalhostで試験した。
初回変換adapterをnative送受信につなぎ、要求の設定版からモデルを取得して既存の
JSON本文を生成し、既存parserで候補を抽出する。送信前後の同意・設定版・キー同一性と
取消は既存のordered変換境界で検査する。BrokerBootの専用入口は同一authorityを要求し、
有限worker内で呼ぶ。模擬交換による成功・途中変更の拒否と、実pipeから平文HTTP拒否・
localhost TLS接続失敗の固定エラー配送を確認した。正常TLS経由の候補取得、外部API、
常駐製品Hostからの起動は未確認・未接続である。
詳細は[ADR 0071](adr/0071-native-chat-policy-transport.md)を参照。
同意UI向けに非ブロッキングの版付き表示情報と許可・取消workerを追加した。
表示後の接続先・モデル・同一文字列を含むキー置換・認可変更は古い許可を拒否する。
設定保存・資格情報操作と同じ1件枠を使い、結果のreceiptは同じauthorityのBrokerだけが
配信完了を確認できる。実pipeで許可・取消の通知、取消workerの終了、異なるauthorityの
拒否と期限切れ後の再試行を検査した。実キーや送信同意をこのPCに設定していない。
この段階では同意backendまでを実装した。
詳細は[ADR 0072](adr/0072-version-bound-consent-actions.md)を参照。
続いて`sumibi-host host-window [absolute-settings-path]`へ送信管理画面を追加した。
起動workerで設定とキーを読み、同意offerの接続先・モデル・キー有無を表示する。
明示確認後の許可と取消を専用ownerへ渡し、Broker配信確認後に操作完了とする。
終了要求は操作queueと独立し、未処理の許可を捨て、進行中操作を待った後に取消し、
非公開Brokerの終了を確認する。合成キーで起動中・許可待機中の終了、古い/別authorityの
offer拒否を試験した。試験用設定の実画面でキー欠損時の許可無効化と×での正常終了を
確認した。画面上での許可・キー変更や外部送信は行っていない。
この入口はpeerを公開・受付せず、変換dispatchも行わない。安全なTIP発見・接続、
自動常駐と、正常TLS経由の実変換成功は残る。
詳細は[ADR 0073](adr/0073-host-consent-window-lifetime.md)を参照。
Hostのowner threadが同じログオン内の単一起動mutexを保持するようにした。
枠取得は設定・キー読取より前で、別設定パスでも2つ目は起動枠使用中として拒否する。
終了操作と取消・Broker資源の終了を待って枠を解放し、再起動は未同意から始める。
別processの拒否、正常解放、異常終了後の回収、名前の型衝突と取得失敗時の枠回復を
ランダムな試験用名で検査した。Host寿命試験は注入した独立枠を使い、利用者が
起動中のHostと競合しない。同一起動枠はHostの認証・発見には使わない。
別途、試験用設定の実画面で2つ目の起動枠使用中表示と許可の無効化、最初のHostが
継続すること、×で正常終了後に同じ設定で再起動できることを確認した。
詳細は[ADR 0074](adr/0074-logon-scoped-host-instance.md)を参照。
公開HTTPSの明示検査を追加し、Postman EchoでnativeのTLS POST・JSON受信と最終context
解放を確認した。Hostの実native交換も通し、echo JSONは変換候補にせず拒否した。
期限切れ・自己署名のbadssl test証明書は、タイムアウト等ではなく証明書検証失敗として
拒否できた。即時エラーとcallbackエラーの分類も合わせた。通信は固定の合成入力・
ダミーキーだけで、実キーや証明書storeは変更していない。
`tools/Test-NativeHttps.ps1`に3件の再実行手順をまとめ、通常テストではignoredとした。
これは正常TLSの通信検証で、実変換APIからの候補取得や一般アプリ対応の完了ではない。
詳細は[ADR 0075](adr/0075-public-https-validation.md)を参照。
根拠: [CredWriteW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew)、
[CREDENTIALW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw)、
[CredReadW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credreadw)。
参照: [Mac版クライアント](https://github.com/kiyoka/Sumibi-mac/blob/553c69021eda7a8a5944d8748357f3790cc2f45c/Sources/SumibiCore/OpenAICompatibleClient.swift)、
[プロンプト](https://github.com/kiyoka/Sumibi-mac/blob/553c69021eda7a8a5944d8748357f3790cc2f45c/Sources/SumibiCore/Conversion.swift)。

2026-10-30: Host署名確認の前段として、WindowsのオフラインAuthenticode署名検査を
追加した。署名済み`vswhere.exe`と未署名テスト実行ファイル、相対パス・UNCの拒否を
このPCで確認した。発行者の特定、親ACL、実行中イメージとの同一性、オンライン
失効確認は実装していないため、この検査結果はHost起動や発見の許可に使わない。
詳細は[ADR 0069](adr/0069-offline-authenticode-inspection.md)を参照。
署名検証後の署名者証明書SHA-256取得と指定指紋の照合を追加し、別手段で得た
`vswhere.exe`の証明書指紋と一致すること、異なる指紋を拒否することを確認した。
製品の指紋配布・更新、保護配置、実行中イメージ照合と失効方針は引き続き必要。
固定済みprocessの実行イメージパスを照会する入口と異常バッファの拒否を追加した。
パスから実行中イメージの同一性や署名を推定しない。通常のWindows環境で利用者が
実process試験を実行し、自processと別processのパス取得、および終了後の拒否を確認した。
署名を検査した同じファイルを保持するAPIを追加し、一時コピーで保持中の書込・削除拒否、
保持中の起動と解除後の書込を確認した。保護された親と実行中イメージの照合は残る。
保持中の署名済みファイルと候補のfile IDを照合し、hard linkは一致、同じ内容の
別コピーは不一致とする。IDを取得できない場合やゼロIDは拒否する。
ファイルIDの一致だけでは実行中イメージや親ディレクトリの安全性を証明しない。
保持中ファイルのACLをhandleから読み、開発用の厳格な所有者・権限構成と照合する
入口を追加した。不正ACLの単体試験と、Dドライブに保護配置された開発用DLLの
読取検査が成功した。親の保護、製品署名の信頼と実行中イメージの照合は残る。
開発用の保護ディレクトリをhandleで開き、所有者・ACL・継承設定を検査する入口を
追加した。DドライブのSumibiルート、開発版、登録状態の3ディレクトリで成功した。
ドライブrootと`Program Files`の開発用ACL規則も追加し、Dドライブのrootから
保護配置DLLまで各handleを同時保持した読取試験で確認した。階層全体の製品認証、
製品署名と実行中イメージの同一性の確認は残る。
開発用の固定配置をrootから対象ファイルまでまとめて開き、ACL検査中のhandleを保持する
入口を追加した。署名照合時はACL検査側と署名検査側のfile IDも照合する。
Dドライブの開発版で階層検査と未信頼の自己署名拒否を確認した。信頼された製品Hostで
署名照合が成功するケース、依存物・実行中processの確認は未実装である。
開発版TIPの`staging.json`を保持し、内容、パス、版名とDLLのSHA-256を照合する
入口を追加した。標準テスト値、不一致の拒否とDドライブの実ファイル・記録を確認した。
記録内の署名者指紋の実証明書との照合は別の診断入口で行い、信頼は別の署名検査に委ねる。余分な配置物、
依存DLL、製品Host、実行中processの確認は残る。
署名検証済みファイルを保持したまま、停止状態の子を明示的な実行ファイルパスで作成する
診断を追加した。返されたprocess handleを固定し、観測した画像パスの現在のfile IDと
保持中ファイルを照合する。実行再開や認証用process tokenの取出し口は公開しない。
子は作成時から専用jobへ所属させ、job handleの最後のcloseで終了する。
明示終了時は子の終了まで待ち、終了確認失敗時もfile/配置/job/枠を早く解放しない。
署名済み一時コピーで照合・書込拒否・別コピー拒否・最大4件・正常終了・Drop終了・
job終了を確認した。`tools/Test-SuspendedImageProbe.ps1`で4件を明示実行できる。
保護配置を保持したHost向け入口も追加したが、この実機の成功検査は署名済みツールの
一時コピーによるもの。実行中の画像section、依存DLLや製品署名方針の証明にはしない。
詳細は[ADR 0076](adr/0076-suspended-signed-image-probe.md)を参照。

保持中のHostファイルをx64 PE32+として読む限定profileと、静的import名の一覧取得を追加した。
DLL・別architecture・delay import/CLR、範囲外や重複sectionを子作成前に拒否する。
保護配置Hostの停止診断へ接続し、parser 8件と実Hostビルドの1件を確認した。
実ビルドには大文字・小文字が異なる同じDLLのdescriptorがあり、重複をまとめて
17件の名前を取得した。重複分もdescriptor上限へ数える。
この一覧は依存物の信頼を証明せず、loader全体や動的依存の検証は残る。
追加後のworkspace testは482件成功、公開HTTPS試験3件は明示実行用としてignored。
関連3crateの全target Clippyとfmt検査も成功した。OneDrive上のincremental cacheの
hard link警告は継続しているが、コードのClippy指摘は残っていない。
詳細と再現手順は[ADR 0077](adr/0077-host-image-metadata.md)を参照。

停止起動診断へ、親の環境を継承しない4変数の明示的環境を追加した。
Windows APIから得たsystem directoryだけをPATHへ設定し、環境注入と長さ超過を拒否する。
子作成時にリモート/Low label画像の拒否とSystem32優先を設定し、3項目を
所有process handleから読み戻す。設定・確認に失敗すれば診断を拒否して終了処理を行う。
署名済みの停止中の子を含む6件が成功した。環境は生成bufferで検査し、子のメモリーを
読み出す検査やDLL実ロードは行っていない。依存実体の信頼・製品起動と発見は残る。
追加後のworkspace testは484件成功、明示実行用の公開HTTPS試験3件はignored。
関連3crateの全target Clippy、fmt検査も成功した。
詳細は[ADR 0078](adr/0078-suspended-launch-environment.md)を参照。

静的importをDLL名から関数名・序数まで展開し、未boundのlookup/IATを最後のNULまで
照合するようにした。予約bit、範囲外・終端欠落・bound形式を拒否し、descriptorごと
4096件・全体8192件の上限は重複分も数える。関数名の大文字・小文字は区別する。
parser 13件と実Hostビルド1件が成功し、17種類のDLLに対する165件の正規化済み参照を取得した。
依存DLLの実体・署名、exportとの照合、API set/forwarderや動的依存の解決は残る。
追加後のworkspace testは489件成功、明示実行用の公開HTTPS試験3件はignored。
関連3crateの全target Clippyとfmt検査も成功した。
詳細は[ADR 0079](adr/0079-static-import-symbols.md)を参照。

DLL公開表の読取と、関数名・序数の照合を追加した。未公開、画像内RVAと未解決forwarderを
区別し、alias・欠番・dataを扱う。名前表の整列、各表の範囲、slot/名前65536件と
全文字列4MiBの上限を検査する。保持中の署名検査済みfileから読む入口も用意した。
DLL parser 8件と、実HostのKernel32参照89件の照合が成功した。85件は画像内RVA、
4件は未解決forwarderで、未公開は0件。コードのload・実行、forwarderの解決は行っていない。
全依存の配置・署名・実際のbind先の信頼、API set/forwarderや動的依存は引き続き残る。
追加後のworkspace testは498件成功、明示実行用の公開HTTPS試験3件はignored。
関連3crateの全target Clippyとfmt検査も成功した。
詳細は[ADR 0080](adr/0080-dll-export-metadata.md)を参照。

明示したDLL公開表内だけで、名前・序数のforwarderを最大16段追跡する処理を追加した。
module名は正規化し、slot単位でalias・名前/序数切替の循環も検出する。moduleやsymbolの
欠落・不正形式・上限超過を拒否し、API setは未解決として同名DLLへの置換を拒否する。
合成DLLの7件と、固定した3つのsystem DLLを保持する実ファイル検査が成功した。
今回のKernel32参照89件は最終RVAまで到達し、前回未解決の4件はいずれも1段の転送だった。
これはmetadataの照合であり、署名・配置や実loaderのbind先を認証するものではない。
API set、全依存DLLの信頼、動的依存、製品起動・発見は残る。
全体回帰では既存テストの通知直後のsnapshot取得に競合が見つかり、非ブロッキングの
契約に合わせて検査側の待機・終了確認を修正した。製品workerの動作は変更していない。
再検査のworkspace testは506件成功、明示実行用の公開HTTPS試験3件はignored。
関連3crateの全target Clippyとfmt検査も成功した。
詳細は[ADR 0081](adr/0081-bounded-export-forwarders.md)を参照。

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
  結果を受け取ることを確認した。上限前から待機していたOS受付が後で完了した場合も
  所有側は`AdmissionRejected`でその接続だけを閉じ、既存接続の処理を続ける。
  全Rustテスト291件。新しいBroker起動IDでの
  製品側ローテーションはまだ未接続。
- `SharedAnchor`はBroker自身を固定peerとする最初のpipe instanceを占有し、
  実clientが0件になってもpipe名と`FILE_FLAG_FIRST_PIPE_INSTANCE`の所有を維持する。
  自己接続handleは本文I/Oに使わず、実client用instanceは各々別の固定processを
  認証する。実pipeで0件の期間を3回またぐ受付と、所有側ループのkey 1→2および
  2件目の結果配送を確認した。最大8 instanceのうち1件をアンカーに使うため、
  同時に受け付ける実clientは最大7件。全Rustテスト293件。製品Hostの常駐経路と
  信頼できるTIP発見は未接続。
- Hostの`begin_anchor_accept`はこのアンカー受付をBrokerの64接続履歴上限と
  同じ入口で制限する。上限前に始めた受付は所有側が完了時に拒否し、上限後の
  新規受付はOS instanceを作る前に拒否する。実pipeで確認した。
- 診断用子Brokerの2接続起動も`SharedAnchor`の3 instance groupへ切り替え、
  最初のinstanceを子Broker自身が保持する。親からの2接続、結果、政策変更、
  取消を含む`Test-Broker.ps1`が成功し、既定Hostへ復元した。
- アンカー1件と実受付7件でprocess全体の8 endpoint枠が満ち、8件目の実受付は
  `Capacity`で拒否される。待機受付を取消・回収した後は同じアンカーから再び
  受付を作れる。停止結合検査もアンカー付き2受付と進行中workerに変更し、
  すべて回収後に同名pipeを新規作成できることを確認した。全Rustテスト294件。
- 実clientが0件の間に設定同意を失効・再許可し、それぞれの版の完了receiptを
  所有側で受けた後、次のクライアントが最新版のReadyと結果を受け取ることを
  アンカー付き実pipeで確認した。
- `BrokerWorkLoop::retire_for_restart`はRuntimeの認証済みpipeを先に退役させ、
  旧jobを取消してから、待機受付と実workerを有限期限で回収する。成功後に
  アンカーと旧clientを解放すれば同名pipeを再作成でき、旧worker結果は
  `Retired`として拒否する。終了時の取消は送信mutexを待たずに通知し、
  送信中workerが期限までに退出しない場合は失敗を返して再試行できる。
  実pipeと送信競合で確認した。全Rustテスト296件、Clippyと`Test-Broker.ps1`成功。
  製品Hostの自動ローテーションは未接続。
- 政策元とBrokerの状態が矛盾してRuntimeが終端する場合も、送信mutexの解放を
  待たずに全jobへ取消を通知する。送信中のmutexを保持したまま終端処理が返り、
  workerの取消と接続の退役が確定する回帰検査を追加した。
- アンカー付き共有待受で、固定した子processとは異なる同ユーザーprocessの
  接続を`WrongPeer`で拒否した後も最初のinstanceを保持し、正しいpeer向けの
  次の受付と本文I/Oが通ることを実pipeで確認した。待機受付の期限切れ後も
  同じアンカーから再受付できることを追加確認した。全Rustテスト298件。
- `BrokerBoot`でランダムなpipe名・起動ID、アンカー、Runtime、受付とworkerの
  所有を一つの寿命にまとめた。0接続期間を挟む2回の受付・結果を同じ起動IDで
  検査し、履歴残数の減少と退役後の同名pipe再作成を確認した。異なる政策元を
  受付前に拒否しても、正しい元で同じ受付を続けられる。製品Hostの常駐起動と
  TIP発見は未接続。待機受付だけが残る場合も退役を完了してからアンカーを
  解放できることを確認した。全Rustテスト301件。
- `BrokerBoot`の所有APIで、実client0件の間の失効・再許可を受領してから
  後続のReadyへ最新版を渡した。認証付きpipeのConvertを診断loopback HTTPへ
  一度だけ送り、結果を同じpipeへ返す縦断も確認した。製品HTTPS adapterではない。
  全Rustテスト303件。
- 親が起動した別processを、接続より先に起動handleから`PinnedProcess`として固定し、
  `BrokerBoot`のアンカー付き共有待受でHello・Convert・結果まで通した。子process
  の退出後に旧Bootを退役させ、アンカー解放前は同名pipeを新規作成できず、解放後は
  作成できることも確認した。子fixtureは標準Fileで接続するためclient側のBroker
  認証を示さず、製品TIPの発見・配置の代替でもない。全Rustテスト304件。
- `BrokerBoot::rotate`で待機受付を回収してから旧アンカーを解放し、新しい
  起動IDとpipe名で受付・Hello・Readyを完了した。workerが期限内に退出しない
  場合は旧Bootを返して新規作成を保留し、退出後の再試行で切替えた。保留中の
  旧接続は再開せず、新しい名前の配布は製品Host側に残る。全Rustテスト306件。
- 実pipeで接続履歴64件を使い切り、新規受付を`Exhausted`で止めることを確認。
  切替後は履歴予算と接続keyを初期化して新しいclientのReadyを返した。
  切替直前に変更された政策も新Bootが最新版として取り込む。全Rustテスト307件。
- 切替の呼出し時点で期限切れなら旧Bootを退役させず、同じpipe名と起動IDで
  引き続き受付・Hello・Readyを通す。旧Bootを失わずに再試行できる。
  全Rustテスト308件。登録不要のBroker回帰と`Test-Spike.ps1 -SkipDesktop`も成功し、
  既定HostとパッシブTIPへ復元した。
- 通常終了も`BrokerBoot::shutdown`でBootの所有権と回収結果を一緒に返す。
  期限切れでは稼働中のBoot、回収未完了では退役中のBootを保持して再試行できる。
  待機受付の回収後だけアンカーを解放する実pipe回帰を確認した。
- IPC受付の最大60秒を超える期限は、Bootの開始前に拒否する。終了・切替では
  稼働中の旧Bootを`InvalidDeadline`で返して、退役途中の受付を残さない。
- 退役未完了で旧Bootを保持する場合でも、旧clientのpipe読取が`Disconnected`に
  なることを実pipeで確認した。旧本文を接続中と誤認して再開させない。
- 履歴上限後の新Bootに旧起動IDのHelloを送り、拒否と接続keyの消費を確認した。
  次の正しいHelloは別keyでReadyまで進む。旧Bootの再接続を許可しない。

## 次の受入作業

製品Brokerへ統合する責務と順序は
[ADR 0062](adr/0062-product-broker-integration-sequence.md)、共有待受の所有と寿命は
[ADR 0065](adr/0065-authenticated-shared-broker-listener.md)に記録した。

1. `BrokerWorkLoop`と`BrokerPolicyRuntime`を製品Hostの継続するOS acceptへ接続する。
   共有groupの3接続と差替え受付まで実pipeで確認した。長寿命の製品受付、設定UIの操作完了と
   所有TSFでの結果適用前再検査も製品経路で実証する。
   受付が連続しても既存接続のframeを後回しにし続けないよう、Host所有ループの
   受付と既存接続処理を交互に進める。3本の実pipeで、2本目の受付後に1本目の
   Convertと3本目の受付が同時に待つ場合、Convertを先にworkerへ渡すことを確認した。
   通常・manual両入口で同じ調停を使う。製品Host EXEからの継続起動は未接続。
   待機受付も接続履歴と同時instance枠に数え、OS instance作成前に超過を拒否する。
   2 instanceの実pipeで待機中・接続中の超過拒否と接続終了後の枠回復、
   履歴最後の1件を待機中に重複確保できないことを確認した。
   Hostがpeerの固定前に受付可否を確認する入口を追加し、待機期限切れ後も
   履歴を消費せず、同じBootで再受付してReadyを返すことを実pipeで確認した。
   同時待機する受付をBoot IDと受付番号で識別し、成功・期限切れの通知を
   対応する受付へ返す。後から開始した受付が先に失敗する実pipe検査を追加した。
   受付IDを指定した個別取消も追加し、取消完了まで枠を保持する。2本の待機受付の
   一方を取り消した後、もう一方が接続できることを実pipeで確認した。
   OS認証済みでHello待ちの接続も残り履歴の予約に数えるよう修正した。
   履歴最後の接続がHello待ちの間、新たな受付を拒否する実pipe検査を追加した。
   履歴の最後の枠が待機中に占有される場合は一時的な`Capacity`とし、
   期限切れ後に受付が回復することと、実際に履歴を使い切った場合の`Exhausted`を区別した。
   異なるprocessを同時に受け付けるため、固定peerごとに単一instanceの新しい
   pipe名を用意する入口を追加した。親と別の子processで同じBrokerへの受付と
   子の変換結果を確認した。製品の名前配布とclient側のHost認証は未接続。
   続く結合検査では、起動した2子へHost handleを個別に渡して双方向のOS認証、
   Hello・変換・結果を確認した。誤Host・誤clientの拒否と新しいpipeでの再受付も検査した。
   このhandle受渡しは試験専用で、一般アプリの発見経路は未実装。
   固定済み共有Hostに接続するChannelと非同期workerを追加し、2子processで
   一方の終了後も他方の変換が成立すること、取消と旧政策版の非送信を確認した。
   製品の信頼できる発見・政策版の取得・TIP配送は未接続。
   準備済みworkerが変換前にReadyの政策版を所有側へ通知できるようにした。
   準備中の予約拒否、停止、失敗、期限切れと、Ready後に取り消された同意の
   本文非送信を確認した。待機中の変更もworkerが検知し、接続を退役して
   所有側をcompletion eventで起こす。製品TIPの所有TSFへの配送は未接続。
   診断TIPと準備済みworkerを結合し、Readyの政策版を本文取得前に受け取る。
   同一processの試験用Brokerと専用TSFホストで、変換と1回Undo、取得前および
   処理中の同意取消、本文・選択・フォーカスを変更して戻した場合の破棄を確認した。
   これは合成入力と固定結果を使う診断であり、製品のHost発見や一般アプリ接続ではない。
   さらに別processの試験用Hostを起動し、親子の固定process handleによる相互認証後、
   専用TSFホストの変換・1回Undoを確認した。Host終了後は入力を取得せず拒否する。
   handleの受渡しは診断用の親子間だけで、製品の発見経路ではない。
   workerの予約番号は、並行呼出しで事前検査後に新しい番号が確保・解放された
   場合も、スロット獲得後に再確認して古い番号を拒否する。
   固定済みpeerへの接続先配布APIを追加し、配送失敗・期限切れ・panicで
   該当受付を取消す。既に接続済みの場合、別受付がある場合と、取消後の
   枠回復を実pipeで確認した。配布経路の認証と製品での利用は残る。
   認証済み専用pipeによる期限付き配布を追加した。受付peerとpipe peerの
   不一致は受付作成前に拒否し、受信側もHostの固定handleを照合する。
   別processへの配布とその後の変換、不正形式と取消を確認した。
   その専用pipeを信頼できる手段で発見・確立する製品経路は残る。
   専用pipeの受信から認証・Readyまでをworkerで行う入口を追加し、準備中の
   予約拒否、不正形式、別Host、期限切れ、停止を確認した。別process Hostと
   専用TSFホストでもこの経路で変換・UndoとHost終了後の拒否を確認した。
   診断bridgeを準備中に接続し、Ready前の変換キーが本文を読まず通過すること、
   Ready後の変換・Undoと準備中のHost終了後の拒否を別process試験で確認した。
   bootstrap pipeの接続待ちもworkerへ移し、接続前のキー通過、接続後の
   変換・Undo、誤peer、停止、期限切れと接続前のHost終了を確認した。
   専用serverの作成もworkerへ移し、成功をeventで通知してから名前を配布する。
   名前の衝突・停止後の解放と、別process Host・専用TSFでの変換を確認した。
2. 配置されたBrokerの信頼できる発見・署名・更新・起動を実装する。
3. 一般アプリへ製品TIPのキー配送と所有対象の失効を接続し、入力先ごとの保護情報を肯定的に検証する。
4. Notepad等の実アプリで実打鍵、1回Undo、別欄・同長別本文・読取不能の拒否を確認する。

機械全体のTSF登録は今回行っていない。恒久登録に進む前の条件は
[ADR 0045](adr/0045-machine-registration-boundary.md)に記録している。
2026-10-28にパッシブDLLの読取専用事前検査と現在ユーザー限定の登録・解除を
再確認した。ワークスペースのDLLは未署名で作業ユーザーに書込権限がある。
保護配置した別コピーは自己署名で未信頼のため、HKLMへは登録していない。
2026-10-29には[ADR 0067](adr/0067-isolated-native-registration-backend.md)の
低整合性レジストリ切替えで実COM/TSF APIを取引journalへ接続した。
Windows 11 x64の通常ユーザーから47ケースが成功し、途中終了後の別プロセス復旧、
無効profile、カテゴリの双方向binding、未知項目での変更停止を確認した。
通常/Host spikeの各Rust 347件と`Test-Spike.ps1 -SkipDesktop`も成功した。
試験用GUIDと隔離されたkeyだけを扱い、製品の恒久登録・有効化は行っていない。
続いて[ADR 0068](adr/0068-fixed-registration-journal-store.md)で、固定ロックを
journalの寿命へ結び付け、解除済みjournalの退避と次回取引を追加した。
退避済み履歴も読取専用で完全性・`Removed`段階・ファイル名との取引ID一致を確認し、
不完全な末尾や誤名を修復せず新規取引を止める。
保護state候補の親ACLを読み、利用者のUAC承認後に空のstateを作成した。
両PowerShellで所有者・ACL・内容を再検証し、通常ユーザーの書込はOSに拒否された。
state内の実journalファイル作成と管理者権限の復旧試験は次段階に残る。
管理者Shellから直接実行する実ファイルACL検査scriptを追加した。
通常ユーザーでは作成前に拒否され、stateは空のままである。
専用HKCU登録fixtureも固定storeを使い、中断後の別プロセス復旧と
別プロセスからのロック重複取得拒否を含む46ケースが成功した。
隔離した実COM/TSF API fixtureも試験IDごとの低整合性固定storeへ移し、
47ケースを再実行した。製品の保護stateや実HKLMは変更していない。
退避履歴と固定storeの再開・解除結合試験後は通常/Host spikeの各Rust 353件、
メモリー内ACL117ケース、模擬登録46ケース、
隔離した実API47ケースが成功した。画面操作を伴う回帰は今回省略した。

## Issue #4 の受入条件との照合

| 条件 | 今回までの確認 | 残る製品受入 |
| --- | --- | --- |
| 版付きJSON、ID・世代・取消・切断・再接続、容量 | ProtocolとCore、実pipe、子Brokerの診断で検査 | 製品の長寿命Brokerでの受付と更新 |
| 別processのSID・PID・起動世代、偽serverと終了 | 固定したkernel process handle、作成時刻、相互peer照合、誤peer・終了後再受付を検査 | 配置済みTIPを信頼経路で発見・固定する仕組み |
| 非同期pipe、バックプレッシャー、所有STA配送 | 有限I/Oとworker、専用TSFホストへの一度だけの配送を検査 | 一般アプリの所有TSFでの結果適用 |
| composition、通常キー、スコープと保護 | 専用ホストの実DLL、Core、保護拒否を検査 | 一般アプリの正の保護判定と実キー配送 |
| 待機容量、超過切替、救済期限 | 共有予算と専用waiting-demoで検査 | 製品の容量・期限政策と複数入力先UI |
| Notepad等の実打鍵、1回Undo、別欄・同長別本文・読取不能拒否 | 専用ホストでは実打鍵とUndo、対象変更や読取不能の拒否を検査 | Notepad等の一般アプリで同じ受入を実施 |

実APIなしのCore→IPC→編集は専用診断ホストで縦断確認した。AppContainerは拒否し、
pipe ACLはlogon SIDに限定する。Hostの診断出力は固定した結果分類とケース名で、
入力本文・キーを出力しない。機械登録と製品Hostの常駐起動は未実施なので、
Issue #4とPR #10は引き続き未完了・draftとして扱う。

開発用配置の追加診断として、保持中のDLLとreceiptを対象にWinTrustが報告する
署名者証明書のSHA-1指紋を記録と照合する。自己署名の実DLLでは一致と不一致の
拒否を確認し、Windowsの信頼検査は引き続き失敗する。これは開発用の整合性診断で、
署名の信頼、製品の起動許可、実行中processの認証を提供しない。

2026-11-01:
1. [ADR 0082](adr/0082-system-module-lease-catalog.md) に基づき、保持された実DLL lease による静的依存エクスポート解決を実装した。
   System32配下の実DLLを排他読取・reparse point拒否で開いて保持する `SystemModuleLease` と、有限forwarder追跡を行う `SystemModuleCatalog` を用意し、
   Hostの非API set静的依存13モジュール（158シンボル、winhttp.dll / ws2_32.dll を含む）が最終RVAまで完全解決されることを検証した。
2. [ADR 0083](adr/0083-api-set-boundary.md) に基づき、API setの調査（Microsoft公式仕様およびPEB ApiSetMap）を実施し、
   contract名からの推測や内部構造体走査を禁止し、カタログ上では `ApiSetUnresolved` 拒否を維持する境界を確定した。
3. [ADR 0084](adr/0084-host-control-broker-dispatch.md) に基づき、`HostControl` ワーカーループに peer 受付チャネル（`try_accept_peer` / `try_accept_private_peer`）と
   変換ディスパッチ（`advance_native_chat` / `advance_diagnostic_chat`）を統合した。
   同意前拒否 → 同意付与後の接続・許可 → 同意取消後の失効拒否 → 正常終了のライフサイクルを結合テストで検証した。
4. [ADR 0085](adr/0085-logon-bootstrap-discovery.md) に基づき、独立したプロセス同士（常駐Hostと一般アプリ内TIP）が相互に発見・認証・ハンドオフを行う製品接続プロトコルを実装した。
   - `PinnedProcess::from_peer_pid`: パイプ接続後にカーネルから取得したピア PID を同一ログオンセッション・同一デスクトップ・Medium Integrity（0x2000）・非昇格・非AppContainerの条件で固定・検証。
   - `BootstrapServer` / `Pipe::connect_bootstrap`: ログオン限定パイプ `\\.\pipe\Sumibi.Bootstrap.v1.{logon_sid}` による相互発見と PID 照合・同一性検証。
   - `discovery::discover_offer` / `serve_offer`: 認証済みブートストラップパイプを通じた一回限りプライベートパイプの handoff 転送。
   - `Worker::prepare_discovered` / `Worker::prepare_discovery`: TSF STA スレッドを一切ブロックせず、バックグラウンド Worker 内で bootstrap 接続 → Host 相互認証 → handoff 受領 → プライベートパイプ接続 → Ready 政策取得までを自律完了。
   - `HostControl::serve_bootstrap_handoff`: HostControl ワーカーループと bootstrap 待受の統合結合テスト（発見接続 → 政策 Ready → 予約 → 同意取消 → 予約拒否 → クローズ）を検証。
   - 全体テスト 519 passed / 3 ignored（全件合格）。
5. ログオン限定 Discovery 待受と TSF キー入力・変換ライフサイクルの統合検証:
   - `crates/sumibi-setup/src/broker_discovery_fixture.rs`: ログオン限定ブートストラップ待受を立て、TSF クライアント側 Worker からの自律 Discovery 接続・ハンドオフ・Ready 政策取得・変換要求の処理を行う統合フィクスチャを実装。
   - `discovery_fixture_serves_discovery_and_converts` 結合テストを追加し、Discovery 経由での接続・ポリシー承認・予約・要求変換（"nihongo" → "日本語"）・完了結果受領・正常シャットダウンの全シーケンスを実証。
   - `crates/sumibi-setup/src/broker_keys.rs`: 実 DLL のキー入力検査（`exercise_broker_keys`）に `"process-discovery"` ケースを追加。Discovery 経由で取得した Worker を Bridge に装着し、実 DLL 経由での変換ディスパッチ・プロセス同一性照合・テキスト適用・1回 Undo のパイプラインを統合。
   - 全体テスト: **全 520 passed / 3 ignored（全件合格）**。
   - `cargo clippy`: 警告ゼロ。
   - `cargo fmt --check`: 差分なし。
   - `Test-HostImage.ps1`: 全 33 テスト合格。
   - `Test-SuspendedImageProbe.ps1`: 全 6 テスト合格。

2026-11-02:
1. [ADR 0086](adr/0086-host-service-discovery-loop.md) に基づき、常駐 Host サービス待受ループと停止調停（`HostService`）を実装した。
   - `HostService` (`crates/sumibi-host/src/host_service.rs`):
     - 同期的に `BootstrapServer::create` で第1インスタンスを作成し、起動直後の接続競合（`ERROR_FILE_NOT_FOUND`）を完全に排除。
     - バックグラウンド待受スレッド（`sumibi-discovery-listener`）で、`additional()` により常に次の待受インスタンスを先行確保しながら接続を受け付ける連続待受ループを実装。
     - クライアント接続時に `HostControl::try_accept_private_peer` で private pipe を発行し、`handoff::send` で安全に引き渡し。
     - `HostControl` の worker ハンドルを `Mutex<Option<JoinHandle>>` に変更し、`try_finish` を `&self` で安全に呼べるようにするとともに、`HostService::stop` において Discovery 待受スレッドとワーカースレッドの双方を完全にドレイン終了（Graceful Teardown）させる仕組みを統合。
     - 単体テスト `host_service_starts_accepts_discovery_and_shuts_down`（Discovery 接続 → 同意付与 → Ready 政策取得 → 予約 → 同意取消 → 予約拒否 → ドレイン終了）および `host_service_starts_with_invalid_settings_path_reports_failed` を追加。
2. CLI 常駐ランナー（`sumibi-host run [settings-path]`）の実装:
   - `crates/sumibi-host/src/host_runner.rs`:
     - Windows の `SetConsoleCtrlHandler` による Ctrl+C シグナルハンドラを導入。
     - 初期起動フェーズの監視、稼働中のエラー検知、および終了シグナル受信時の安全な停止調停（`service.stop()`）を実装。
   - `crates/sumibi-host/src/consent_window.rs`:
     - 送信管理ウィンドウ（UI）起動中も `HostService` を通じてバックグラウンド Discovery 待受ループを同時稼働するよう更新。
   - `crates/sumibi-host/src/main.rs`:
     - `run` / `host-run` コマンドを追加。
   - `crates/sumibi-host/tests/startup_cli.rs`:
     - `host_run_rejects_extra_arguments`, `host_run_fails_with_invalid_settings` 統合テストを追加。
3. 全体検証:
   - `cargo test --workspace`: **526 passed, 0 failed, 4 ignored（全件合格）**。
   - `cargo test --workspace --features sumibi-host/spike`: **526 passed, 0 failed, 4 ignored（全件合格）**。
   - `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: 警告ゼロ。
   - `cargo fmt --all --check`: 差分なし。
   - `Test-HostImage.ps1`: 全 37 テスト合格。
   - `Test-SuspendedImageProbe.ps1`: 全 6 テスト合格。

4. [ADR 0087](adr/0087-tip-host-session-lifecycle.md) に基づき、一般アプリ内 TIP の自律 Host 発見・セッション維持と安全なキー変換結合（`TipHostSession`）を実装した。
   - `crates/sumibi-tip/src/session.rs`:
     - `SharedSessionState`: アトミックな `connected`, `allowed`, `settings`, `authorization` 状態共有。ロック不要・TSF STA スレッドへの完全な非ブロッキング保証。
     - `TipHostSession`: TSF ライフサイクル（`start` / `close`）と連動するマネージャ。
     - ホットキー（Ctrl+J: `COMMAND_CONVERT`）を `ITfKeystrokeMgr::PreserveKey` で登録・解除。
     - バックグラウンドワーカー（`sumibi-tip-session`）による自律的な Discovery 接続試行、未同意（`PolicyDenied`）時のバックオフ再試行、同意付与時の政策同期、同意失効時のセッション切断・再接続待機（Self-Healing Connection）。
     - 変換ホットキー押下時（`on_preserved_key`）の絶対的 Fail-Closed 保護：未同意時や未接続時は即座に `Ok(false.into())` を返し、キーをスルー。入力本文の取得や外部送信を構造的に遮断。
     - 単体テスト `shared_session_state_reflects_atomic_updates`, `session_on_preserved_key_denies_when_not_ready`, `session_autonomously_discovers_host_and_updates_policy`（未同意時のキー拒否 → 同意付与後の自律発見・接続・Ready 反映 → 同意取消後の失効反映 → 安全なクローズ）を検証。
   - `crates/sumibi-tip/src/keys.rs` & `com.rs`:
     - `Active` および `KeySink` に `TipHostSession` を統合。
     - `Activate` 時に `TipHostSession::start` を起動し、`OnPreservedKey` で `session.on_preserved_key` へディスパッチ。
     - `Deactivate` 時に `active.revoke()` から `session.close()` を呼び出し、安全にドレイン終了。
5. [ADR 0088](adr/0088-sta-async-drain-window.md) に基づき、メッセージ専用ウィンドウによる STA への非同期完了通知とテキスト自動適用（`DrainWindow` / `AsyncDrain`）を実装した。
   - `crates/sumibi-tip/src/session.rs`:
     - `DrainSink`: TSF 所有 STA スレッド上に固定される完了ドレインレシーバ（`owner`, `manager`, `client`, `event_receiver`, `pending_ticket`）。
     - `DrainWindow`: 親に `HWND_MESSAGE` を指定したメッセージ専用ウィンドウを STA スレッド上に生成。非表示でタスクバーや UI に影響を与えず、モーダルループ中もメッセージが破棄されない。
     - `WM_SUMIBI_DRAIN` (`WM_APP + 42`): バックグラウンドワーカーから `PostMessageW` で非同期に STA スレッドを起こし、`WindowProc` 経由で自動的に `sink.drain()` を実行してテキストを置換適用。
     - `session.close()` 時に `GWLP_USERDATA` のクリアと `DestroyWindow` を直列化し、破棄後の不正参照を構造的に防止。
     - 単体テスト `drain_window_posts_and_processes_message_on_sta_thread`（別スレッドからの `PostMessageW` 送信 → STA メッセージループ起床 → `WindowProc` での `sink.drain()` 自動実行とイベント消費）を検証。
6. [ADR 0089](adr/0089-end-to-end-session-key-conversion.md) に基づき、実 DLL（`sumibi_tip.dll`）と外部常駐 Host（`HostService`）のエンドツーエンド自律セッション結合検証（`sumibi-setup session-key-test`）を実装・完全成立させた。
   - `crates/sumibi-tip/src/spike.rs` & `keys.rs`:
     - `has_grant` によるディスパッチ調停（明示的テスト grant なし時は実機同様に `TipHostSession` へディスパッチ）。
     - `KeySink` への `session: Weak<TipHostSession>` 引き継ぎ。
     - `SharedSessionState` の初期 `input` revision を `1` に設定し、`ManualConversion` gate の前提条件を満たすよう整合。
   - `crates/sumibi-host/src/diagnostic_http.rs`:
     - `prepare_body` において、`Destination` のループバック検証（`address.is_loopback()`）を維持したまま `Scheme::Https`（`ConsentOffer::can_grant()` 要件）を許容し、E2E のモック HTTP サーバー結合を実現。
   - `crates/sumibi-setup/src/session_keys.rs`:
     - 全 5 シナリオを網羅するエンドツーエンド検証 Harness を新設。
     - シナリオ 1: 未同意時の Fail-Closed（ホットキー即時拒否、`body_reads == 0`）。
     - シナリオ 2: 自律発見・接続、ホットキー受付、バックグラウンド変換（"nihongo" → "日本語"）、`WM_SUMIBI_DRAIN` による STA 非同期テキスト置換（"prefix 日本語 suffix"）。
     - シナリオ 3: TextStore の 1-step Undo（"prefix nihongo suffix" への完全復元）。
     - シナリオ 4: 同意失効時の自律検知と Fail-Closed（失効後は即時拒絶、`body_reads` 増加なし）。
     - シナリオ 5: 終了処理、COM/TSF プロファイルの登録解除と `DllCanUnloadNow` クリーンアップ。
7. 開発・検証統合ランナー（`dev.ps1` / `tools/dev.ps1` / `Makefile`）の導入:
   - 頻出するビルド・テスト・検証コマンドを 1 つの PowerShell スクリプト（`.\dev.ps1 <task>`）に集約し、Antigravity / エージェント環境における権限許可の問い合わせ回数を最小限（プレフィックス承認で 1 回）に抑制。
   - サポートタスク: `session-test`, `build`, `build-tip`, `test`, `test-all`, `check`, `fmt`, `verify`, `ci`, `help`。
8. [ADR 0090](adr/0090-default-gui-host-entry.md) に基づき、製品 Host エントリー（`sumibi-host.exe`）の引数なしデフォルト GUI 起動と設定自動誘導を実装した。
   - `crates/sumibi-host/src/main.rs`:
     - 引数なし（`args.is_empty()`）起動時の自動ルーティング：設定ファイル存在時は `consent_window::run`（送信管理画面＋常駐待受）、未作成時は `settings_window::run`（初期設定画面）を自動起動。
     - `--help` / `-h` / `help` によるコマンドラインヘルプ表示を標準装備。
   - `crates/sumibi-host/src/consent_window.rs`:
     - 待受案内文を更新し、バックグラウンドで一般アプリからの変換要求を待受中であることを明示。
   - `crates/sumibi-host/tests/startup_cli.rs`:
     - `host_cli_help_displays_usage_and_commands` テストを追加・検証。
9. 全体検証:
   - `powershell -ExecutionPolicy Bypass -File .\dev.ps1 test-all`: **全件合格**。
     - standard workspace tests: 532 passed, 0 failed, 4 ignored。
     - workspace tests with sumibi-host/spike: 532 passed, 0 failed, 4 ignored。
     - `Test-HostImage.ps1`: 全 37 テスト合格。
     - `Test-SuspendedImageProbe.ps1`: 全 6 テスト合格。
     - `session-key-test`: 全 5 シナリオ完全合格。
   - `powershell -ExecutionPolicy Bypass -File .\dev.ps1 check`: 警告・フォーマット差分ゼロ。
