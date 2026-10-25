# Windows版 設計と実装の照合表

2026-10-21 / Windows基準 `91552b0` / [全体設計](TECHNICAL_OVERVIEW.md)。
本表は設計の合意や一般アプリでの対応保証を表さない。専用ホスト、未実装の製品機能、未決判断を区別する。

## 1. 設計・コード・受入条件

| 領域 | 現在のコード・状態 | 次の受入条件 |
| --- | --- | --- |
| OS非依存Core | core/lib.rsのManualConversionとinput.rsの有限InputSession。所有ホストの実入力を1024 UTF-16単位まで記録。recovery.rsに有限救済とDeferred。waiting_input.rsの待機FIFOを専用adapter/デモへ接続 | 製品キーと容量/期限、追跡・候補・手動／自動の統合 |
| COMとDLL | tip/com.rs、build.rs。factory・独立lease・所有thread・解除再試行 | 一般アプリにロードされる常設登録と更新時の寿命 |
| TSF編集 | tip/edit.rsとmanual_edit.rs。generic保持range、同期read/write、全文照合、Core gate、IPC結果の一度だけ適用。専用所有contextで検証。[ADR 0018](adr/0018-retained-manual-range-and-source-validation.md) | 非ACP入力先の実検証、製品キーとの統合、一般アプリの対象分類と編集 |
| 保護入力 | protection.rs。scope/status/compartmentの肯定検査、取得・source・適用時の再検査。実DLLで39ケース。statusの固定診断コードと標準RichEditの拒否7構成を追加（[ADR 0019](adr/0019-native-richedit-status-diagnostics.md)） | 一般アプリの実スコープ・独自機密欄・判定不能時の動作確認 |
| 追跡composition | 選択composition 18・所有ホストの入力範囲25・実DLLのcomposition伸縮/初回選択46・同期入力取引174（選択Delete20・記録内移動/前方Delete26・内部挿入20・Shift選択27・記録内選択編集33・文書端移動/選択26を含む）・書記素Backspace 55（内部caret18を含む）・他サービス拒否16ケースを確認 | 一般アプリの行端移動・待機・製品全体の救済・製品InputSession・他IMEとの競合と寿命 |
| 表示属性 | display.rs / preview.rs。点線、原文保持、属性だけ遅延解除 | 通常アプリでの下線、アクセシビリティ、既存属性との共存 |
| 変更通知 | notifications.rs。本文・選択・scope・compartment・composition属性・フォーカスで失効。空compositionは所有ホストから取消を転送 | 製品追跡・IPC・候補との統合、一般アプリの未通知履歴 |
| キー配送 | keys.rs / input_host.rs。専用windowで実TSF経路。待機デモの文字・F7・Enter・F8を利用者が実打鍵 | US/JIS配列・変換キー・Terminal・一般アプリの実打鍵 |
| Undo/Redo | TextStoreに本文・選択・向きの履歴。固定変換含む | 一般アプリの標準履歴で変換が1回のUndoにまとまる |
| 描画 | live_render / render / window_host。通知→WM_APP→Frame | 製品候補UIのUIElement、DPI、高コントラスト、読み上げ |
| 手動待機と救済 | 所有editor救済48。待機Coreの件数/本文量を設定可能とし、専用adapterへ4枠・4096単位を明示。2026-10-24、通常ユーザーの対話desktopで実DLL通常版adapter26・通常版waiting-demo59ケース、修飾付き変換キーの非消費を含む現行対話版61ケースが成功。AppContainer起動は入力受付前に拒否。共有救済を取得前に予約し、待機/通常救済を1画面へ接続（[ADR 0044](adr/0044-configurable-waiting-queue-limits.md)） | 一般アプリでの製品キー・容量/超過/期限/明示破棄とプロセス間政策。変換キーの実打鍵は未確認 |
| Protocol / Broker | 有限フレーム・JSON・client状態、固定process相互検査・8 endpoint・overlapped pipe。単発Host・容量4（完了済みjob・受取済み結果も含む）・各1結果。[ADR 0015](adr/0015-broker-admission-through-delivery.md)。実DLLキー・Core・実TSFから診断用EXE bridgeを通す経路も検証。EXE再利用workerへ実DLLキー・所有STA配送を接続し、25ケースを検証（[ADR 0017](adr/0017-reusable-worker-and-owner-delivery.md)）。認証済みpipe上の進行中/待機中失効と再同意後の旧結果破棄を診断Hostで検証（[ADR 0046](adr/0046-inflight-policy-revocation.md)）。Broker側で要求受付時・結果返却前の政策版を再検査し、受付直前の失効を実子processで確認（[ADR 0050](adr/0050-broker-side-policy-gate.md)）。許可・読取状態の復元後も旧結果を拒否する実子Broker/TSF縦断検査を追加（[ADR 0051](adr/0051-restored-protection-and-consent-broker-regression.md)）。再許可後に届く旧要求も実子Brokerの受付で拒否（[ADR 0052](adr/0052-reconsent-before-broker-acceptance.md)）。完成済み結果は直接Channel読取時と所有側配送時の両方で期限を再検査（[ADR 0053](adr/0053-expire-completed-broker-results-at-delivery.md)）。単発Jobの完成後明示取消でも旧本文を拒否（[ADR 0054](adr/0054-one-shot-completion-cancellation.md)） | 製品Broker発見・常駐dispatch、設定/認可変更元と全接続への実配送、一般アプリ内の配置と入力統合。AppContainerは初期非対応 |
| HTTP | 製品通信は未実装。Host内の手動送信ゲートで接続先識別子・同意・版・資格情報・期限を送信前/結果返却前に検査する（[ADR 0047](adr/0047-host-manual-send-gate.md)）。ローカル模擬HTTPで送信形式・転送拒否・応答上限・期限・失敗分類を検証（[ADR 0048](adr/0048-diagnostic-loopback-http.md)）。固定文字列の診断Brokerを維持 | 製品API契約、非同期WinHTTPの取消/TLS/プロキシ、設定/資格情報と同意UIへの接続 |
| 設定・資格情報 | Host内メモリーの一貫した設定・認可スナップショットを追加。接続先・キー変更、同意付与・取消で版と送信可否を更新。IPC用の秘密を含まない政策値と変更カーソルを追加し、型付きProtocolで取消と旧結果拒否を検証（[ADR 0049](adr/0049-host-in-memory-policy-authority.md)）。保存・UI・Credential Manager・実IPC配送は未実装 | 形式版・原子的保存・秘密分離・認可取消の実配送・破損回復 |
| 追加候補・辞書 | 未実装。Mac移植範囲に目的を記録 | 件数決定・現在候補・出所・索引ハッシュ・LICENSE |
| Decisions | 未実装。Mac契約を参照した後続案 | API再確認、独立同意、固定窓、永続回数予約、手動優先 |
| 導入・更新・配布 | 現ユーザーの開発用COM登録・別プロセス生成確認・解除とプロセス内TSF検査が成功。パッシブDLL限定の読取専用マシン登録事前確認を追加。HKLM/恒久TSF登録コードは配置先ACL・署名・更新設計の後に実装する。spike DLLは配布不可 | 保護配置、署名、マシン登録実証、Host起動、更新・削除・旧版復旧 |

## 2. Windowsで実行済みの検証

| 検証 | 基準コミットまでの結果 | 限界 |
| --- | --- | --- |
| cargo test --workspace | Core 7件＋COM 1件が成功 | 製品用InputSession全体のテストではない |
| Protocol単体 | フレーム層6件。分割・不正・EOF・上限 | JSON解釈とpeer認証を含まない。単体合計14件 |
| load_smoke | 実DLLのexports、factory、模擬manager、独立属性Info参照とDLL寿命 | 実TSF登録や入力先アプリではない |
| self-test | 各登録段階の意図的失敗・逆順解除・正常系 | プロセス内登録。インストーラーの試験ではない |
| edit-test | 編集23＋属性3＋Preview 8ケース成功 | 一部の編集基盤はEXEにRustライブラリとしてリンク |
| key-test | 実DLLから固定変換、取消・通知・別context拒否 | 画面なし通常キー配送は診断用コールバックで補う |
| window-test | 2026-10-23、通常版と対話クリック用コマンドが対話desktopで成功。実TSF通常キー・focus通知、描画5状態と失敗後回復、破棄・再接続を確認 | 専用診断windowの自動キー検査。物理打鍵や一般アプリの保証ではない |
| input-test | 2026-10-23に対話desktopで成功。文字・絵文字・移動・選択・Delete・変換・Undo/Redo・終了解除 | 所有HWNDへのPostMessage。Shift選択は処理へ明示状態を渡す |
| delayed-test | 2026-10-23に対話desktopで成功。実DLLで正常部分置換・1回Undoと失効条件を11ケース検証 | 有限channelの合成応答。製品Core/IPC/HTTP統合ではない |
| ipc-test | ログオンSID限定DACL・remote拒否フラグ・先取り防止・OS peer PID・フレーム | 通常ユーザーの同一プロセスprobe。別プロセス認証やAppContainerは未検証 |
| fmt / Clippy | workspaceとTIPのspikeなし構成を確認。直近変更のworkspace再確認済み | 一般アプリでの成立を証明しない |

本書作成は文書作業であり、過去の実行記録を本日再実行したと読み替えない。
実機手順と個別の条件は[DEVELOPMENT](DEVELOPMENT.md)、[TSF_SPIKE](TSF_SPIKE.md)を参照。

2026-10-24、利用者が専用`waiting-demo`へ`nihongo`→F7→直後に`abc`を実打鍵し、
「日本語abc」を確認した。×終了後のTSF/COM解除とDLLアンロードも成功した。
同日の省略なし`tools/Test-Spike.ps1`はRust 138件、fmt、Clippy 3構成、
build、実DLL/デスクトップ/別プロセスIPCを完走した
（`target/issue-4-full-regression-2026-10-24.log`）。
最初の一括回帰は新規IPCテストと既存の共有4枠テストの並列競合で停止し、
テスト用ロックを共用した後に20回のIPC単体反復と省略なし一括回帰が通った。
この実打鍵は専用ACPデモであり、Notepad等の一般アプリを受け入れた記録ではない。

同日、対話用の固定結果を2秒に延ばした`waiting-demo`で利用者が`nihongo`→F7→
直後にEnter→F8を実打鍵し、救済画面が開くことを確認した。終了時の診断値は
待機開始1・イベント提示1・受入1・開始失敗なし。救済残件数0は終了時点の値であり、
開画面の否定には使わない。後続の計測は実行中の最大件数と画面作成回数を保持する。
同じ版の`waiting-demo-test`通常版59ケースとTSF/COM解除・DLLアンロードが成功した。
変換キーを専用windowのF7と同じ開始・待機中再変換へ接続した直後、合成`VK_CONVERT`を含むクリック待ち付き対話版60ケースが一度成功し、TSF/COM解除・DLLアンロードも確認した。後続の表示変更版では既存`start-retained`の保持件数が2件の想定に対し4件となる失敗を一度観測した。受付件数の診断を追加し、変換キーの受付を待機デモに限定した。各windowの前面化待ちも追加した現行版は、対話版60ケースとTSF/COM解除・DLLアンロードが成功（`target/issue-4-convert-key-foreground-per-case.log`）。件数不一致は今回再現せず、物理的な変換キーと一般アプリでの配送は未確認。
さらに待機中の変換キー判定へ`LPARAM`のAlt状態を渡し、修飾付きキーを受理しないことを開始前・待機中の両方で検査した。対話版61ケースとTSF/COM解除・DLLアンロードが成功（`target/issue-4-convert-key-alt-native-final.log`）。実キーボードと一般アプリでの配送は未確認。

Issue #4の追加分（2026-10-21）は[IPC_PROTOCOL](IPC_PROTOCOL.md)に記録する。
Protocol 22件のうち16件が追加のJSON・session・Core模擬接続であり、基準コミット当時の14件と混同しない。
製品TSF配送や別プロセス認証を新たに成功とする更新ではない。

さらにsumibi-ipcを追加した段階では、単体合計34件と、専用親子processの相互検査・停止等14確認が成功。
固定ハンドルの相手だけへ接続する検証で、製品Brokerの起動・発見・TSF編集の完成ではない。
詳細と別ユーザー・実PID再利用等の未実行範囲は[ADR 0003](adr/0003-pinned-desktop-peers.md)を参照。

次に単発Host・有限worker・Core→所有TSFの実配送を追加。workspaceは36件、broker-testは
専用ACP storeで19ケースと解除・DLLアンロードが成功した。元へ戻した変更による失効も検証した。
一般アプリ、製品キー、HTTPへの接続の成功ではない。[ADR 0004](adr/0004-one-shot-broker-and-owner-edit.md)を参照。

実DLLのキー・Core・TSF編集を通す段階ではbroker-key-testの17ケース、非同期デモと同じ
入力・終了のbroker-input-test 6ケースが成功した。COMの既存試験に独立control leaseと
所有threadの拒否を追加し、workspace合計は36件のまま。
専用ACP storeと診断用EXE bridgeの範囲であり、一般アプリや常駐Brokerは未接続。
詳細は[ADR 0005](adr/0005-dll-key-broker-bridge.md)を参照。

続いて入力保護を追加。workspaceは39件、protection-testは39ケース、broker-input-testは
WM_KILLFOCUSを含む7ケースへ増えた。初期保護／不明の27ケースでDLL読取・Broker job開始0を確認する。
MSCTF内部の本文照会は別に計測し、OSを含む完全な非読取とは扱わない。
フォーカスの画面なし検査とホストstatus通知の前提も明記した。[ADR 0006](adr/0006-input-protection-metadata.md)を参照。

2026-10-22、同一contextのcompositionの非干渉検査を追加。workspaceは40件、composition-testは16ケース、
broker-input-testは空compositionの所有者通知を含む8ケース。保護39・キー17・Broker19・close4も再確認した。
非空のproperty通知と、空compositionを所有ホストが転送する取消を区別し、一般アプリの履歴保証とはしない。
不正scope fixtureはDLLの同期probeだけで返し、壊した値をWindows初期化へ渡さないよう修正した。
詳細と残る統合は[ADR 0007](adr/0007-foreign-composition-boundary.md)を参照。

同日、選択プレビューに自己所有compositionを追加。owned-composition-testは実DLL 16件と
直接アダプター2件の計18件。開始拒否32回、強制終了、同じ範囲の別object、新旧要求、Undo、DLL解放を確認した。
通常入力追跡の完了ではなく、EXEが提供する診断sinkを必要とする。scope/保護/Brokerに加え、
既存input・delayed・close・windowのUI検証も成功した。[ADR 0008](adr/0008-owned-selection-composition.md)を参照。

同日、所有ホストの実入力範囲をCoreへ記録し、tracking-demoのF7から実DLLと非同期Brokerへ接続した。
Coreの5テスト追加でworkspaceは45件。tracking-input-testの25ケースでは既存本文を含めない変換、
境界・保護・composition・復元後の失効、取消、1回Undo、送信直前の原文・範囲・世代の拒否を確認した。
Test-Spike.ps1を省略なしで実行し、上記の既存検査とpipe/peerを含む全検査が成功した。
打鍵ごとのcomposition拡張・製品InputSession全体・一般アプリの入力取得の完成ではない。
詳細は[ADR 0009](adr/0009-cooperating-host-input-run.md)を参照。

同日、実DLLのISpikeInputとTrackerを追加し、通常入力で同じcompositionを伸縮する診断経路を接続した。
composing-input-testの29ケースと全回帰検査が成功。Coreの末尾削除2件を追加し、workspaceは47件。
ホスト開始拒否等でも本文は一度だけ反映され、強制終了後の別objectを引き継がず、
変換前にはcompositionを終了して通常の本文保護とsource照合を必要とする。
ホストの本文反映とcompositionメタデータ更新は別処理で、製品の原子的なTSF入力取引ではない。
詳細は[ADR 0010](adr/0010-cooperating-host-composition-growth.md)を参照。

続いて通知から所有ループを起こす仕組みとstatus購読を追加。input-notify-testの20ケースでは
次の入力なしの追跡解除、変更/復元後の非復活、cleanup拒否からの回復、wake集約と終了後の非作用、
変換待ちのreadonly復元拒否を確認した。実DLLのcounterでcleanup時の本文取得が増えないことも確認した。
workspaceは47件のままで、Test-Spike.ps1の全検査も省略なしで成功した。
一般アプリの同期write成功や全ての未通知履歴を保証する変更ではない。
詳細は[ADR 0011](adr/0011-owner-loop-input-invalidation.md)を参照。

同日、実DLLの同期入力取引を追加し、本文・caret・compositionを同じwrite cookieで更新した。
transaction-input-testの22ケースで、正常入力・F7/Undo、書込前拒否・書込後の失敗、保護履歴の復元、
再入力禁止と変換停止、強制終了・close・容量を確認した。実DLLの本文読取counterも維持した。
workspace 47件とTest-Spike.ps1の全検査が省略なしで成功。ログはtarget/issue-4-input-transaction-regression.log。
削除の入力取引、未確認入力の救済と一般アプリの製品キー配送は未完了である。
詳細は[ADR 0012](adr/0012-synchronous-input-transaction.md)を参照。

同日、記録末尾のBackspaceを実DLLの同期writeへ接続した。backspace-input-testの27ケースで、
scalar単位の範囲計画・一括repeat・全削除・Undo/Redo・F7、追跡外への返却と失敗後の再削除禁止を確認した。
Coreに計画の3件を追加し、workspaceは50件。挿入と削除で本文書込・確定処理を共有し、
自己compositionだけを縮小・終了する。前方Deleteや任意位置編集・製品キー配送は未完了。
詳細は[ADR 0013](adr/0013-synchronous-tracked-backspace.md)を参照。
Test-Spike.ps1を省略なしで実行し、上記50件と既存native検査も成功した。
ログはtarget/issue-4-backspace-transaction-regression.log。TSF/COM解除・DLLアンロードと診断process残留なしを確認した。

同日、所有editorの未確認入力を1件・1024単位・10分に制限する救済と確認画面を追加した。
recovery-input-testの25ケースで、予約・容量・明示コピー/失敗/破棄・期限・保護失効・古い操作の拒否を確認。
Coreに救済7件を追加してworkspaceは57件。コピー試験は差替え先を使い、実クリップボードを変更しない。
文書本文・Undo・SetText・実DLLの本文取得回数を救済操作で増やさないことも確認した。
詳細は[ADR 0014](adr/0014-bounded-input-recovery.md)を参照。製品の待機行列・全体容量と一般アプリは未完了。
Test-Spike.ps1の全検査を省略なしで実行し、57件と既存native/pipe/peerも成功した。
ログはtarget/issue-4-input-recovery-regression.log。TSF/COM解除・DLLアンロード、診断process残留なしも確認。

同日、ManualEditからITfRangeACPの必須問い合わせを除き、Coreに明示的な保持rangeの長さstampを追加した。
source前には本文を再取得せずcontext・選択端点・向き・保護状態を再検査し、観測した失敗を復元後も拒否する。
range-target-testの12ケースとCore追加2件、workspace 65件を含むTest-Spike.ps1全回帰が省略なしで成功。
ログはtarget/issue-4-retained-range-regression.log。解除・DLLアンロードを確認した。
fixtureは依然ACP方式で、非ACPアプリの互換性や未通知本文変更の検出は未検証。
詳細は[ADR 0018](adr/0018-retained-manual-range-and-source-validation.md)を参照。

## 3. アプリ互換表の記録形式

記録内の通常左右移動と前方Deleteを実DLLへ接続した。Core8件追加でworkspace91件。
同期入力は26件追加で68件、救済は共有満杯時の追跡済み移動/削除を加えて37件。
本文取得なしの移動/削除、内部caretから記録全体のF7、同一composition、境界越え・失敗時の非再操作を確認した。
既存関連回帰も成功。一般アプリと実キー検証には読み替えない（[ADR 0027](adr/0027-tracked-caret-and-forward-delete.md)）。

続いて内部caretからのBackspaceも実DLLの同期削除へ接続した。削除前後の記録と同じcompositionを保持し、
Coreは前後方向の計画と完了検査を共有する。5件追加でCore56・workspace96件。
Backspaceは18件追加で55件、共有満杯の内部削除を加えて救済38件。
同期入力68・書記素ホスト22・composition46・通知30・追跡25・broker入力11・close4と静的検査が成功した。
本文取得不変、1回Undo/Redo、全repeat境界、書込後の非再操作と操作情報だけの救済を確認した。
一般アプリ・物理キー・未通過のwindow/input/delayedの3検査は残る（[ADR 0028](adr/0028-tracked-internal-backspace.md)）。

記録内の途中挿入も既存Writerで継続し、供給文字の前後と同じcompositionを保持する。
Core6件追加で62件・workspace102件。同期入力20件追加で88件、救済2件追加で40件と関連回帰・静的検査が成功。
合計容量・最大長の実TSF範囲・1回Undo/Redo・書込後の非再入力・供給断片だけの救済コピーを確認した。
最大長の下線描画は固定幅rendererの制約で未確認。一般アプリと製品統合の残範囲は維持する
（[ADR 0029](adr/0029-tracked-internal-insertion.md)）。

記録内のShift＋左右の選択と通常左右での解除も、同じcompositionで追跡を継続する。
端点に加えてactive endpointを観測・cookie内・完了・Ready消費で照合し、未通知の方向変更も拒否する。
Core6件追加で68件・workspace108件、同期入力27件追加で115件、共有満杯の選択を加えて救済41件。
本文/Undo/SetTextを増やさない選択、選択F7とUndoの方向復元、解除後の入力、失敗後の非再選択を確認した。
関連回帰と静的検査も成功。選択置換/削除後の前後の追跡は残る（[ADR 0030](adr/0030-tracked-shift-selection.md)）。

続いて記録内の選択置換/Delete/Backspaceも前後のrunと同じcompositionを保持する。
Core6件追加で74件・workspace114件、同期入力33件追加で148件、共有満杯3操作と断片コピーを加えて救済45件。
選択外へ広げないrepeat、削除分を差し引いた最大容量、1回Undoでの本文・範囲・向き復元、
置換/削除後のF7と書込後の非再操作、外部選択変更後の非採用を確認した。
関連回帰と静的検査も成功。最大長の描画・一般アプリ・実打鍵の制限は維持する
（[ADR 0031](adr/0031-recorded-selection-replacement.md)）。

専用単一行ホストの文書端が記録内にあれば、Home/End・Shift選択も同じcompositionで継続する。
位置情報だけで実contextの末尾を前後照合し、記録外への移動は記録端に置き換えない。
Core6件追加で80件・workspace120件、同期入力26件追加で174件、共有満杯の2操作を加えて救済47件。
選択counterでDLL1回/ホスト0回と、書込前fallbackのDLL0回/ホスト1回を区別し、失敗後の非再操作も確認した。
関連回帰と静的検査も成功。一般アプリの行端・物理打鍵・未通過のdesktop3検査は維持する
（[ADR 0032](adr/0032-recorded-document-edge-navigation.md)）。

次に手動変換待機入力のOS非依存Coreを追加した。1024イベント/本文16384単位を消費前に確保し、
FIFO・試行ticket・Convertによる再待機・Enterの非配送・不明結果の不可逆停止を12件で確認した。
Core92件・workspace132件とTest-Spike.ps1 -SkipDesktopが成功。native拡張検査は前段階の結果を維持する。
製品キー/診断editorへの接続、共有予算/救済先とnativeの順序検証は未完了
（[ADR 0033](adr/0033-bounded-core-waiting-input.md)）。

専用Waiting adapterでは取得前に既存共有救済枠を確保し、実DLL/workerでFIFOと再変換・取消を接続した。
待機入力25ケースと関連native回帰が成功。Deferred救済2件を追加してCore94件・workspace134件。
独立した自己状態検査ABI、確認済み変換後のcaret、失敗後の未適用tail、正確な要求IDのDrop取消を検証した。
通常デモのキー受付・複数確認UI・一般アプリには未接続（[ADR 0034](adr/0034-shared-recovery-waiting-adapter.md)）。

waiting-demoを追加し、専用のroute_message/WndProc経路と複数救済の巡回/1件確認を接続した。
合成MSGと実Viewボタンの22ケースで順序・非再送・window境界・旧操作拒否・コピー公開前取消を確認した。
関連native回帰とRust134件・静的/基本DLL検査も成功。物理打鍵や一般アプリの受入とは区別する
（[ADR 0035](adr/0035-waiting-demo-and-record-recovery-ui.md)）。

待機と通常入力の救済を1画面へ統合し、Source付きKeyで同値ticketを区別する。
保護による本文除去後も操作情報を確認/破棄でき、Clipboard公開前の期限も検査する。
デモ34・待機adapter26ケースと、同期入力174・救済47・通知30・close4が成功。
Rust134件・静的/基本DLL検査も維持した（[ADR 0036](adr/0036-unified-demo-recovery-and-copy-expiry.md)）。

F7要求の開始後にWaiting初期化が失敗した場合、捕捉した要求IDだけを取消す。
保護拒否/失敗/境界/終了/取消、新しい要求との入れ替わり、同一要求の重複通知、
救済の上書き拒否と明示再試行を加え、waiting-demoは45ケースになった。
待機adapter26・救済47・同期入力174・通知30・close4とRust134件・静的/基本DLL検査も確認した
（[ADR 0037](adr/0037-cancel-failed-waiting-start.md)）。

待機デモで通常救済だけが残る場合もOwnerが定期起床し、表示中の残り時間と、
非表示中の保護状態を更新する。読取専用化からの本文除去・操作情報の確認/破棄を追加し、
デモ47ケースを実DLLで確認した（[ADR 0038](adr/0038-owner-wake-for-ordinary-recovery.md)）。

待機/通常救済の期限切れで記録がなくなった後も、未確認入力の文書確認案内を維持する。
デモ47ケースとRust134件・静的検査を確認した
（[ADR 0039](adr/0039-waiting-demo-recovery-loss-warning.md)）。

待機期限の表示と、期限直後/容量超過で未受入れの文字・Backspace・Enterの通常経路を補強した。
原文を選択したまま通常入力が置換しないよう、安定した専用対象では先にcaretを末尾へ移す。
入力なしで所有ループが期限を検出する場合も、安定した原文の選択を解除する。
後続の通常入力による保護履歴変更では救済本文を除去し、操作記録は維持する。
Esc明示取消でも安定した原文の選択を解除し、後続の文字が原文全体を置換しないようにした。
デモ57ケースで本文・Undo・救済の状態を確認した
（[ADR 0040](adr/0040-waiting-deadline-and-unadmitted-fallback.md)）。

2026-10-22、context付きF7準備を追加し、文字入力前の明示選択を変換できるようにした。
初回選択17件を加えてcomposing-input-testは46件。逆向き選択と1回Undo、sink欠損時の成功、
記録なしcaret/保護欄/lock等の拒否、取消・履歴復元・Readyの一度だけの使用を確認した。
同期入力42・救済35ほか関連回帰とRust83件も成功。非ACP/一般アプリ互換や通常キー配送の受入とは分ける。
詳細は[ADR 0026](adr/0026-initialize-explicit-selection-with-context.md)を参照。

2026-10-22、明示選択のDeleteを実DLLの同期writeへ接続した。旧runは退役し、削除本文を取得しない。
選択Delete20件を加えtransaction-input-testは42件、共有予算不足の選択削除を加えrecovery-input-testは35件。
未確認削除は操作情報だけでコピー不可。Rust83件・静的検査・既存の関連回帰が成功した。
物理キーと一般アプリの受入とは分ける。詳細は[ADR 0025](adr/0025-synchronous-selected-delete.md)を参照。

2026-10-22、専用ホストのBackspace repeatで選択外の文字まで消す問題を再現し、一度の削除計画/編集へ修正した。
同期取引のfallback・救済枠不足・旧composition観測経路の検査10件を加え、grapheme-input-testは22件。
共有4枠満杯の救済検査も1回Undo/Redoによる本文/選択復元を確認する。Core/workspaceは83件を維持する。
今回のログと受入範囲は[ADR 0024の追補](adr/0024-grapheme-navigation-in-owned-host.md#追補backspace連打の一括編集)を参照。

2026-10-22、Coreに書記素境界の7件を追加し、workspaceは83件。専用ホストの移動/Shift選択/Deleteへ接続した。
grapheme-input-testの12ケースはホストの編集処理と実TSF通知・旧結果拒否・Undo/Redoを確認し、通常キー配送とは分ける。
Test-Spike.ps1 -SkipDesktopとBackspace37・救済34・同期入力22・追跡25・composition29・通知30・broker入力11・close4が成功。
前面化で未確認の3検査と一般アプリ対応は維持する。詳細とログは[ADR 0024](adr/0024-grapheme-navigation-in-owned-host.md)を参照。

2026-10-22、記録末尾と専用ホストのBackspaceを拡張書記素単位へ変更した。
unicode-segmentation 1.13.3 / Unicode 17.0.0を固定し、Core 5件追加でworkspaceは76件。
Backspace 37・救済34・同期入力22・追跡25・composition伸縮29・通知30・broker入力11・close 4が成功。
一括scriptはkey-testのShift押下状態で停止した。Rust/fmt/Clippy/build等は成功し、key-test単独実行も成功。
一括成功には数えず、前面化で未通過のwindow/input/delayedも維持する。
開始点と終了点は硬い境界であり、前後本文を読み足す一般アプリの書記素編集ではない。
詳細とログは[ADR 0023](adr/0023-grapheme-backspace-within-recorded-input.md)を参照。

2026-10-22、所有WndProcで無効化・非アクティブ化・モード取消を失効へ接続した。
通知30・broker入力11・救済34、追跡25・composition伸縮29・同期入力22・Backspace 27・close 4が成功。
Rust 71件とfmt・各構成Clippy・基本DLL/TSF検査も成功した。省略なし全回帰は今回は再実行していない。
WM_CANCELMODEの標準capture解除を、callbackがhandled/errorを返す場合も確認した。
実EnableWindowと、所有HWNDへの診断通知を区別し、物理的な他アプリ切替の試験とはしない。
前面化で未通過の3検査と一般アプリ対応は残る。詳細とログは[ADR 0022](adr/0022-native-owner-window-boundaries.md)を参照。

2026-10-22、Core Budgetと診断EXEで所有者間の救済予約を合計4件に制限した。
Coreへ同時予約と解放等の5件を追加し、workspace 71件・fmt・各構成Clippy・基本DLL/TSF検査が成功。
実DLLの救済31・同期入力22・Backspace 27・通知24・close 4も成功した。
今回、省略なしの全回帰は再実行しておらず、前面化で未通過の3検査は維持する。
複数の実アプリではなく、別々の救済ownerと専用ACP storeでの検証である。
詳細とログは[ADR 0021](adr/0021-shared-input-recovery-budget.md)を参照。

2026-10-22、所有ホストのManualEdit.cancelと入力境界の即時禁止・所有loop cleanupを追加した。
共通TSF pumpを選択編集/Broker待機等にも接続した。範囲16・通知24とRust 66件、各構成Clippy等が成功。
全回帰試行はwindow-testで前面化できず停止し、window/input/delayedは最終版でも前面化拒否を明示して未通過。
全回帰成功には含めず、close 4件とpipe/peerの独立成功と区別する。
自然focus待機の再発、所有者転送への変更、最新ログは[ADR 0020](adr/0020-owner-cancellation-and-shared-tsf-pump.md)を参照。

2026-10-22、標準RichEditのnative contextで拒否7構成と本文を持たないstatus診断を追加した。
初回全回帰で既存focus通知待機が再現性をもって失敗し、入力デモの有限/待機loopをITfMessagePumpへ変更。
別文書への実切替を検査するfocus試験と通知20ケース、workspace 66件を含む全回帰が省略なしで成功した。
最終ログはtarget/issue-4-native-edit-regression.log。初回失敗と切り分けは
[ADR 0019](adr/0019-native-richedit-status-diagnostics.md)へ記録した。RichEditの変換/標準Undoは依然未対応。

| 入力先 | 現在の状態 | 最初の確認範囲 |
| --- | --- | --- |
| 専用sumibi-setup | 自動試験成功。2026-10-23にtransaction-demoで利用者が`nihongo`入力、F7変換、Ctrl+Z復元、×終了を目視確認 | Shift選択・IME切替、別デモの実打鍵 |
| 専用window内の標準RichEdit | native TSFの拒否7構成を自動確認。隠し文字なしの保証がなく変換は未対応 | 肯定的な保護情報・領域境界の扱いを決定してから変換/標準Undo |
| Notepad | 未検証 | 通常入力・選択・Enter/Esc・1回Undo・読取専用 |
| VS Code | 未検証 | editorと検索欄、複数文書、キー競合 |
| Chromium系 | 未検証 | input / textarea / contenteditable、長文、パスワード |
| Windows Terminal | 未検証 | Ctrl+J、行実行、折返し、TUI、ホスト側Undo |
| AppContainer / 昇格アプリ | 未検証・初期対応に含めない | IPC権限・UI・セッション分離の成立を先に調査 |

各結果にはコミット、OSビルド、アプリ版、CPU、権限、入力欄種別、キー配列、手動／自動を付ける。
ある欄の成功を同じアプリ全体へ一般化しない。Macの既知不具合はケースとして参照するが、Windowsの不具合と断定しない。

## 4. Mac資料との対応と未決事項

Mac参照はmain `553c69021eda7a8a5944d8748357f3790cc2f45c`。資料内のmain/PR/作業版の区別を維持する。

| Macの資料・論点 | Windows側の参照先・差 |
| --- | --- |
| TECHNICAL_OVERVIEW：全体図・送信境界・手動／自動 | [全体設計](TECHNICAL_OVERVIEW.md)。現在の検証と将来の製品構成を別図にする |
| INPUT_CONTROLLER_ARCHITECTURE：Runtime共有と寿命 | [所有資料](INPUT_CONTROLLER_ARCHITECTURE.md)。TSF thread/context単位。0.3秒判定を移植しない |
| SPEC：製品要件 | Windowsの確定仕様と同一ではない。未実装・未決を全体設計と本表で表示 |
| WINDOWS_HANDOFF：共通契約・Windows再設計 | [MAC_PARITY](MAC_PARITY.md)。固定コミットから独立実装 |
| 追加候補12件と追加10件要件の差 | Windowsも未決。辞書追加枠10件と混同しない |
| 待機中の制御キー・容量 | 遅延再送なし案。超過時の安全な切替と救済期限を決定するまで有効化しない |
| String.count・Unicode比較 | 編集用の書記素分割をUnicode 17.0.0に固定。送信上限への接続は未実装。照合はUTF-16完全一致で、正準等価を自動許可しない |
| AppKit / Keychain / 単一プロセス / 公証 | TSF UIElement / Credential Manager案 / 別Broker / Windows署名へ再設計 |

更新時は製品仕様の変更、コードの変更、実機の観測を別々に記録する。
要件を未実装コードに合わせて黙って縮めたり、設計案を実装済みへ変更したりしない。
