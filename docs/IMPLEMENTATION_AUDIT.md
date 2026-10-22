# Windows版 設計と実装の照合表

2026-10-21 / Windows基準 `91552b0` / [全体設計](TECHNICAL_OVERVIEW.md)。
本表は設計の合意や一般アプリでの対応保証を表さない。専用ホスト、未実装の製品機能、未決判断を区別する。

## 1. 設計・コード・受入条件

| 領域 | 現在のコード・状態 | 次の受入条件 |
| --- | --- | --- |
| OS非依存Core | core/lib.rsのManualConversionとinput.rsの有限InputSession。所有ホストの実入力を1024 UTF-16単位まで記録。recovery.rsに有限救済 | 製品追跡・待機・候補・手動／自動の状態と効果を追加 |
| COMとDLL | tip/com.rs、build.rs。factory・独立lease・所有thread・解除再試行 | 一般アプリにロードされる常設登録と更新時の寿命 |
| TSF編集 | tip/edit.rsとmanual_edit.rs。generic保持range、同期read/write、全文照合、Core gate、IPC結果の一度だけ適用。専用所有contextで検証。[ADR 0018](adr/0018-retained-manual-range-and-source-validation.md) | 非ACP入力先の実検証、製品キーとの統合、一般アプリの対象分類と編集 |
| 保護入力 | protection.rs。scope/status/compartmentの肯定検査、取得・source・適用時の再検査。実DLLで39ケース。statusの固定診断コードと標準RichEditの拒否7構成を追加（[ADR 0019](adr/0019-native-richedit-status-diagnostics.md)） | 一般アプリの実スコープ・独自機密欄・判定不能時の動作確認 |
| 追跡composition | 選択composition 18・所有ホストの入力範囲25・実DLLのcomposition伸縮29・同期入力取引22・Backspace 27・他サービス拒否16ケースを確認 | 前方Delete・待機・製品全体の救済・製品InputSession・一般アプリの他IMEとの競合と寿命 |
| 表示属性 | display.rs / preview.rs。点線、原文保持、属性だけ遅延解除 | 通常アプリでの下線、アクセシビリティ、既存属性との共存 |
| 変更通知 | notifications.rs。本文・選択・scope・compartment・composition属性・フォーカスで失効。空compositionは所有ホストから取消を転送 | 製品追跡・IPC・候補との統合、一般アプリの未通知履歴 |
| キー配送 | keys.rs / input_host.rs。専用windowで実TSF経路 | US/JIS配列・変換キー・Terminal・実打鍵 |
| Undo/Redo | TextStoreに本文・選択・向きの履歴。固定変換含む | 一般アプリの標準履歴で変換が1回のUndoにまとまる |
| 描画 | live_render / render / window_host。通知→WM_APP→Frame | 製品候補UIのUIElement、DPI、高コントラスト、読み上げ |
| 手動待機と救済 | 所有editorの救済1件・1024単位・10分と明示コピー/破棄を25ケースで確認。待機行列は未実装 | 消費前容量確保・順序・超過・対象変更・期限と明示破棄の整合 |
| Protocol / Broker | 有限フレーム・JSON・client状態、固定process相互検査・8 endpoint・overlapped pipe。単発Host・容量4（完了済みjob・受取済み結果も含む）・各1結果。[ADR 0015](adr/0015-broker-admission-through-delivery.md)。実DLLキー・Core・実TSFから診断用EXE bridgeを通す経路も検証。EXE再利用workerへ実DLLキー・所有STA配送を接続し、25ケースを検証（[ADR 0017](adr/0017-reusable-worker-and-owner-delivery.md)） | 製品Broker発見・常駐dispatch、一般アプリ内の配置と入力統合。AppContainerは初期非対応 |
| HTTP | 未実装。固定文字列で検証 | 模擬HTTPで認証・同意・転送拒否・上限・取消・エラー分類 |
| 設定・資格情報 | 未実装。LocalAppData / Credential Manager案 | 形式版・原子的保存・秘密分離・認可取消・破損回復 |
| 追加候補・辞書 | 未実装。Mac移植範囲に目的を記録 | 件数決定・現在候補・出所・索引ハッシュ・LICENSE |
| Decisions | 未実装。Mac契約を参照した後続案 | API再確認、独立同意、固定窓、永続回数予約、手動優先 |
| 導入・更新・配布 | プロセス内登録のみ。spike DLLは配布不可 | 署名、登録範囲、Host起動、更新・削除・旧版復旧 |

## 2. Windowsで実行済みの検証

| 検証 | 基準コミットまでの結果 | 限界 |
| --- | --- | --- |
| cargo test --workspace | Core 7件＋COM 1件が成功 | 製品用InputSession全体のテストではない |
| Protocol単体 | フレーム層6件。分割・不正・EOF・上限 | JSON解釈とpeer認証を含まない。単体合計14件 |
| load_smoke | 実DLLのexports、factory、模擬manager、独立属性Info参照とDLL寿命 | 実TSF登録や入力先アプリではない |
| self-test | 各登録段階の意図的失敗・逆順解除・正常系 | プロセス内登録。インストーラーの試験ではない |
| edit-test | 編集23＋属性3＋Preview 8ケース成功 | 一部の編集基盤はEXEにRustライブラリとしてリンク |
| key-test | 実DLLから固定変換、取消・通知・別context拒否 | 画面なし通常キー配送は診断用コールバックで補う |
| window-test | 実TSF通常キー・focus通知、描画5状態と失敗後回復、破棄・再接続 | 一時的に専用ウィンドウをアクティブにする |
| input-test | 文字・絵文字・移動・選択・Delete・変換・Undo/Redo・終了解除 | 所有HWNDへのPostMessage。Shift選択は処理へ明示状態を渡す |
| delayed-test | 実DLLで正常部分置換・1回Undoと失効条件を11ケース検証 | 有限channelの合成応答。製品Core/IPC/HTTP統合ではない |
| ipc-test | ログオンSID限定DACL・remote拒否フラグ・先取り防止・OS peer PID・フレーム | 通常ユーザーの同一プロセスprobe。別プロセス認証やAppContainerは未検証 |
| fmt / Clippy | workspaceとTIPのspikeなし構成を確認。直近変更のworkspace再確認済み | 一般アプリでの成立を証明しない |

本書作成は文書作業であり、過去の実行記録を本日再実行したと読み替えない。
実機手順と個別の条件は[DEVELOPMENT](DEVELOPMENT.md)、[TSF_SPIKE](TSF_SPIKE.md)を参照。

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

2026-10-22、標準RichEditのnative contextで拒否7構成と本文を持たないstatus診断を追加した。
初回全回帰で既存focus通知待機が再現性をもって失敗し、入力デモの有限/待機loopをITfMessagePumpへ変更。
別文書への実切替を検査するfocus試験と通知20ケース、workspace 66件を含む全回帰が省略なしで成功した。
最終ログはtarget/issue-4-native-edit-regression.log。初回失敗と切り分けは
[ADR 0019](adr/0019-native-richedit-status-diagnostics.md)へ記録した。RichEditの変換/標準Undoは依然未対応。

| 入力先 | 現在の状態 | 最初の確認範囲 |
| --- | --- | --- |
| 専用sumibi-setup | 自動試験成功、物理キーボードは未確認 | 実打鍵・Shift・IME切替 |
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
| String.count・Unicode比較 | 書記素分割依存を未選定。照合はUTF-16完全一致で、正準等価を自動許可しない |
| AppKit / Keychain / 単一プロセス / 公証 | TSF UIElement / Credential Manager案 / 別Broker / Windows署名へ再設計 |

更新時は製品仕様の変更、コードの変更、実機の観測を別々に記録する。
要件を未実装コードに合わせて黙って縮めたり、設計案を実装済みへ変更したりしない。
