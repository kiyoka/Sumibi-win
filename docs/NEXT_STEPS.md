# Issue #1後の実装順序

2026-10-23更新。最小試作の根拠は[受入記録](ISSUE_1_ACCEPTANCE.md)。
各段階は直前の安全条件を満たしてから進める。未実装を本Issueの既存機能として扱わない。

## 1. Core・認証付きIPC・製品用入力基盤

登録済み: [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4)。

最初の段階として、版1の単一結果JSON、1接続1保留のclient状態、世代・取消・期限・切断・
再接続とCore模擬接続を実装した（[契約と検証範囲](IPC_PROTOCOL.md)）。
続いてsumibi-ipcに、固定process handleによるOS相互検査、合計8 endpoint、非同期pipeと
期限・取消を追加し、専用親子processで検証した（[ADR 0003](adr/0003-pinned-desktop-peers.md)）。
さらに単発Host EXE・容量4のworker・所有TSFへの結果配送を接続し、専用ACP storeで19ケースを検証した
（[ADR 0004](adr/0004-one-shot-broker-and-owner-edit.md)）。
実DLLのキー・Core・TSF編集から診断用EXE bridgeへ接続し、17ケースと非同期デモの6ケースも検証した
（[ADR 0005](adr/0005-dll-key-broker-bridge.md)）。
製品Brokerの発見・常駐dispatch、一般アプリの入力基盤は未完了で、受入条件全体を満たした意味ではない。
続いてscope/status/compartmentの保護検査を実装し、本文取得・source送信前・結果適用時に検査する。
状態を変更して戻した場合も拒否し、実DLL/TSFで39ケースを確認した
（[ADR 0006](adr/0006-input-protection-metadata.md)）。一般アプリでの判定は引き続き未検証。
次に他サービスのcompositionを同じcontextで検出して拒否し、通知による取消を追加した。
実DLL/TSFで16ケースと専用デモの空composition回帰を確認した
（[ADR 0007](adr/0007-foreign-composition-boundary.md)）。空compositionの履歴取消には所有ホストの協力が必要。
打鍵ごとの追跡compositionと一般アプリへの統合は引き続き未実装。
続いてF6プレビューの選択範囲を自己所有のcompositionとして保持する基盤を追加し、
開始拒否・確定・取消・強制終了・別compositionへの非適用を18ケースで確認した
（[ADR 0008](adr/0008-owned-selection-composition.md)）。これは既存選択の寿命管理であり、
通常打鍵の追跡・範囲拡張や製品InputSessionの完了ではない。
さらにCoreに所有ホストの実入力範囲を記録する基盤を追加し、tracking-demoから実DLL・非同期Brokerへ接続した。
移動・編集・保護状態変更等による失効、入力範囲だけの変換・1回Undo・取消・送信前再照合を25ケースで確認した
（[ADR 0009](adr/0009-cooperating-host-input-run.md)）。通常入力はホストで一度だけ反映する。
次に実DLLが同じcompositionを入力ごとに伸縮する専用経路を追加し、29ケースで確認した。
Backspaceの縮小、既存本文の保持、拒否時の一度だけの入力反映、同一COM identity、1回Undo、
失効・開始拒否・強制終了後の別object・送信前照合を扱う（[ADR 0010](adr/0010-cooperating-host-composition-growth.md)）。
ホストの本文反映とメタデータ更新は別処理であり、次の入力基盤の課題は原子的な本文入力と
compositionの取引、一般アプリ内での同じ失効・寿命保証と製品キー配送である。
さらに、外部通知で次の打鍵を待たず所有ループを起こし、追跡・下線を解除する仕組みを追加した。
status変更後の復元、cleanup拒否からの回復、有限event、終了後の通知も20ケースで確認した
（[ADR 0011](adr/0011-owner-loop-input-invalidation.md)）。本文の原子的な入力取引は引き続き未完了。
続いて所有ホスト向けの同期入力取引を追加した。本文挿入・caret・composition更新を同じwrite cookieで行い、
書込前拒否と書込後の失敗を区別し、自動再入力を防ぐ22ケースを検証する
（[ADR 0012](adr/0012-synchronous-input-transaction.md)）。rollbackや入力救済の完成ではない。
次は削除を含む製品キー処理、待機容量と未確認入力の救済、一般アプリでの通知・寿命・Undo保証を進める。
続いて記録末尾のBackspaceも同じ同期writeへ接続した。repeatと補助面文字、全削除時のcomposition終了、
1回Undo、書込後の再削除禁止、保護復元と追跡外へのfallbackを27ケースで検証した
（[ADR 0013](adr/0013-synchronous-tracked-backspace.md)）。Coreの計画3件を追加しworkspaceは50件。
次は前方Delete等の製品キー配送、未確認入力の保持・救済と待機容量、一般アプリでの入力・寿命・Undoを進める。
続いて未確認入力の救済を追加した。診断editorの1件・最大1024 UTF-16単位を最大10分保持し、
明示確認・コピー・破棄、保護変更での本文除去、容量超過時の通常入力、期限によるowner wakeを25ケースで確認した
（[ADR 0014](adr/0014-bounded-input-recovery.md)）。Core救済7件を加えworkspaceは57件。
次は製品のプロセス全体の待機容量・キー配送、一般アプリ内での入力・寿命・Undoと常駐Brokerを進める。
単発Brokerの容量予約を補強し、完了済みjob・取り出した結果の保持中も合計4件へ数える。
容量6ケースと実DLLキー19ケース、同時予約・所有者先行破棄を確認しworkspaceは59件
（[ADR 0015](adr/0015-broker-admission-through-delivery.md)）。製品の遅延キー待機行列や常駐dispatchは引き続き未実装。
次に同じ認証済み子processを連続要求で再利用するworker専用Channelを追加した。実子processの14項目と
既定Host拒否2項目、serverの順序/policy検査を確認しworkspaceは61件
（[ADR 0016](adr/0016-authenticated-reusable-broker-channel.md)）。次はEXE workerのdispatchと所有STAへの
結果配送をこの経路へ接続し、キー消費前の容量・取消・対象失効を維持する。常駐共有探索と一般アプリへの統合も残る。
続いてEXE再利用workerと取得前Permitを実装し、既存Bridgeと入力デモへ接続した。
worker 11項目・実DLLキー25ケースと全回帰を確認しworkspaceは63件
（[ADR 0017](adr/0017-reusable-worker-and-owner-delivery.md)）。次は一般アプリの入力adapter・所有compositionの
寿命・保護情報・標準Undo、常駐共有探索と製品の配置/認証を進める。

続いてManualEditの選択変換からITfRangeACP依存を除き、generic保持rangeと明示的なCoreの長さstampを使う。
送信前の選択/保護検査と観測した失敗の永久失効を追加し、12ケースと全回帰、workspace 65件を確認した
（[ADR 0018](adr/0018-retained-manual-range-and-source-validation.md)）。実入力追跡はACP方式を維持する。
次は非ACP入力先を含む一般アプリの検証と、診断sinkに頼らない所有compositionの寿命、標準Undo、
製品の配置/認証・非同期実行を進める。今回の変更で一般アプリを対応済みとはしない。

次に標準RichEdit自身のTSF contextを使う拒否検証7構成と、本文を持たないstatus診断を追加した。
RichEditはTS_SS_REGIONSを報告し、TS_SS_NOHIDDENTEXTを報告しないため現在は変換を拒否する。
保護条件を維持し、診断と実際のstatus gateを共用した。workspaceは66件
（[ADR 0019](adr/0019-native-richedit-status-diagnostics.md)）。
入力デモのmessage pumpもITfMessagePumpへ接続し、再現したfocus通知待機の失敗を修正した。
一般アプリの範囲・隠し文字・領域境界と欠損metadataをどう肯定的に検証するかが次の課題である。
標準Undoの成立、所有compositionの寿命、配置/認証・非同期配送も残る。

続いて共通の選択編集/Broker待機helperも保持済みITfMessagePumpへ接続し、ManualEdit.cancelを追加した。
所有ホストが境界を検出した時点で本文取得やCOM呼出しなしに旧要求と原文bufferを退役できる。
取消4ケースを加えrange-target-testは16件。自然通知の配送に頼れないfixtureも観測し、
明示取消・実DLL callback試験・実event待機を区別して記録する
（[ADR 0020](adr/0020-owner-cancellation-and-shared-tsf-pump.md)）。一般アプリでの通知履歴・保護情報の保証は残る。
入力ホストにも明示境界を追加し、WM_KILLFOCUSで即時に旧入力を再利用禁止にしてcleanupを所有loopへ送る。
通知試験はowner-focus等の24件へ拡張した。前段階の自然focus待機が再発した結果も記録し、
一般アプリの自然通知だけを保証する意味ではない。

直近では救済の予算を所有者間で共有し、診断EXEの全editorで予約中を含め合計4件に制限した。
共有枠は本文bufferより先に予約し、破棄・期限切れcleanup・所有者Dropまで保持する。
Core 5件追加でworkspace 71件、救済31ケースと同期入力/削除/通知/終了の回帰が成功した
（[ADR 0021](adr/0021-shared-input-recovery-budget.md)）。製品の待機行列・複数入力先UI・プロセス間政策は残る。
前面化で未通過のwindow/input/delayedの3検査は維持し、一般アプリの保護情報・sink寿命・標準Undoを次に進める。

所有windowの非アクティブ化・無効化・モード取消も実WndProcから失効経路へ接続した。
復元後の保留変換・救済コピーを拒否し、Windows標準のモード取消は維持する。
通知30・broker入力11・救済34と関連回帰を確認した（[ADR 0022](adr/0022-native-owner-window-boundaries.md)）。
一般アプリの通知保証・保護情報・composition寿命・標準Undoと製品の配置/認証を引き続き進める。

記録末尾のBackspaceはUnicode 17.0.0の拡張書記素へ進め、結合文字・複合絵文字とrepeatを一括削除する。
通常ホストも同じ分割を使い、追跡外・救済満杯での削除単位を合わせた。記録外の本文へは拡張しない。
Core 5件追加でworkspace 76件、Backspace 37件と関連回帰が成功した
（[ADR 0023](adr/0023-grapheme-backspace-within-recorded-input.md)）。一般アプリと任意位置編集は残る。

専用ホストの左右移動・Shift選択・Deleteも書記素へ接続し、境界表のCore7件と実TSF上のホスト編集12件を追加した。
workspace83件と関連回帰が成功。保留中の編集は旧結果を拒否し、明示選択は拡張しない
（[ADR 0024](adr/0024-grapheme-navigation-in-owned-host.md)）。
製品DLLの前方削除取引・任意位置のcomposition継続、一般アプリの保護情報・寿命・標準Undoは引き続き残る。

専用ホストのBackspace連打を一括編集へ統一した。従来のfallbackでは、明示選択を消した後に
残りrepeatが手前の文字へ進み、Undoも分かれていた。同期取引の書込前拒否・救済枠不足と
旧composition観測経路を同じ削除計画へ接続し、選択外を削除せず1回Undo/Redoで復元する。
grapheme-input-testは10件追加で22件。共有救済4枠満杯の既存検査も1回Undoへ強化した。
Core/workspaceの件数は83件のまま。一般アプリと製品DLLの残範囲は上記を維持する。

明示選択のDeleteを実DLLの同期writeへ接続し、本文・caret・一時composition終了を一度だけ処理する。
削除本文を取得せず、旧追跡を退役し、未確認結果は操作情報だけを救済する。
選択Delete20件を加えて同期入力42件、共有4枠満杯の削除を加えて救済35件と関連回帰が成功した。
workspace83件を維持する（[ADR 0025](adr/0025-synchronous-selected-delete.md)）。
選択のないDLL前方削除・任意位置のcomposition継続と、一般アプリ/製品統合の残範囲を次に進める。

続いてF7準備へcontext付き初期化を追加し、最初の文字入力前でも明示選択を変換できるようにした。
選択の向きを保ち、記録のないcollapsed caretから全文を推測しない。準備は本文を取得せず、
保護境界世代と一度だけのReadyを検査する。初回選択17件を加えcomposition検査は46件。
同期入力42・救済35ほか関連回帰とRust83件が成功した（[ADR 0026](adr/0026-initialize-explicit-selection-with-context.md)）。
一般アプリ/製品統合、選択のないDLL前方削除と任意位置の追跡継続は引き続き残る。

記録終端とcaretを分離し、記録内の通常左右移動と前方Deleteを実DLLの同期処理へ接続した。
同じcompositionを保持し、内部caretのF7も現在の記録全体だけを変換する。境界越えや内部挿入等は退役する。
試行後の失敗では元の移動/削除を重ねず、共有救済は操作情報だけを持つ。
Core8件追加でworkspace91件、同期入力68・救済37と関連回帰が成功した
（[ADR 0027](adr/0027-tracked-caret-and-forward-delete.md)）。
続いて記録内の内部Backspaceも同期writeへ接続し、削除前後に残る記録と同じcompositionを保持する。
Coreの方向付き計画と完了検査を共有し、旧ホスト観測経路は末尾限定のまま維持した。
Core5件追加でworkspace96件、Backspace18件追加で55件、共有満杯の内部削除を加えて救済38件と関連回帰が成功した
（[ADR 0028](adr/0028-tracked-internal-backspace.md)）。
さらに記録内の途中挿入を同期writeで継続し、前後に残る記録と同じcompositionを保持する。
合計容量を本文反映前に検査し、未確認入力の救済は供給した断片だけを扱う。
Core6件追加でworkspace102件、同期入力20件追加で88件、共有満杯と断片コピーを加えて救済40件と関連回帰が成功した
（[ADR 0029](adr/0029-tracked-internal-insertion.md)）。
続いてShift＋左右の記録内選択と、通常左右での選択解除を同じcompositionで継続する。
選択方向を端点とは別に照合し、明示選択F7は選んだ部分だけを変換する。
Core6件追加でworkspace108件、同期入力27件追加で115件、共有満杯の選択を加えて救済41件と関連回帰が成功した
（[ADR 0030](adr/0030-tracked-shift-selection.md)）。
さらに記録内の選択置換/Delete/Backspaceも前後のrunを継続し、同じcompositionを保持する。
選択外へrepeatを広げず、容量は削除分を差し引いて検査する。外部選択変更後は旧runを継続しない。
Core6件追加でworkspace114件、同期入力33件追加で148件、共有満杯3操作と断片コピーを加えて救済45件と関連回帰が成功した
（[ADR 0031](adr/0031-recorded-selection-replacement.md)）。
専用単一行ホストのHome/End・Shift選択も、実contextの文書端が記録内なら同じcompositionを保持する。
文書端は位置だけで照合し、記録外へは通常移動を一度だけ行う。Core6件追加でworkspace120件、
同期入力26件追加で174件、共有満杯の2操作を加えて救済47件と関連回帰・静的検査が成功した
（[ADR 0032](adr/0032-recorded-document-edge-navigation.md)）。
一般アプリの行端/折り返し・入力adapter・通知/保護情報・sink寿命・標準Undoと製品統合は残る。

次に手動変換待機入力の実験的なCoreを追加した。1024イベント・本文16384単位、消費前admit、
FIFO先頭の一度だけの完了、次のConvert要求での停止、Enterの非配送、境界/不明結果の不可逆失効を扱う。
Core12件追加で92件・workspace132件と静的/基本DLL検査が成功した
（[ADR 0033](adr/0033-bounded-core-waiting-input.md)）。
後続の専用adapterは受入れ前に既存EXE共有4枠を確保し、実DLL/workerへの順序と未適用救済を25ケースで検証した。
CoreのDeferred救済2件を加えworkspace134件と関連native回帰が成功した
（[ADR 0034](adr/0034-shared-recovery-waiting-adapter.md)）。
後続のwaiting-demoでは専用受付とF8巡回・1件コピー/破棄、所有window通知を接続した。
配送/確認UIの22ケースと関連回帰が成功した（[ADR 0035](adr/0035-waiting-demo-and-record-recovery-ui.md)）。
さらに通常入力の救済も同じ画面へ接続し、本文除去後も操作情報を確認/破棄できるようにした。
コピー公開前の期限照合を追加し、デモ34・待機adapter26ケースと関連回帰が成功した
（[ADR 0036](adr/0036-unified-demo-recovery-and-copy-expiry.md)）。
F7後の待機初期化失敗で捕捉した要求だけを取消し、重複通知で受入れ済み入力を上書きしない。
デモ45ケースと関連回帰で確認した（[ADR 0037](adr/0037-cancel-failed-waiting-start.md)）。
さらに通常救済だけが残る場合もOwnerが表示/保護を定期確認する。
デモ47ケースで検証した（[ADR 0038](adr/0038-owner-wake-for-ordinary-recovery.md)）。
救済の期限切れ後に通常のF7案内へ戻る経路も、文書結果の確認を促す表示へ変更した
（[ADR 0039](adr/0039-waiting-demo-recovery-loss-warning.md)）。
待機期限の案内と、未受入れ文字/Backspace/Enterの通常経路を補強した。
入力のない期限切れでも原文の選択を解除し、後続の通常入力による置換を防ぐ。
Esc明示取消でも安定した原文の選択を解除する。
容量超過と期限/Esc後の通常入力をデモ57ケースで確認した
（[ADR 0040](adr/0040-waiting-deadline-and-unadmitted-fallback.md)）。
共有救済予算は件数と本文総量の同時予約へ拡張し、診断4件・4096単位は維持した。
製品の全対象にまたがる予算の所有者と容量値は未決のまま
（[ADR 0041](adr/0041-configurable-shared-recovery-budget.md)）。
次は専用デモの対話操作を確認し、製品の待機/救済容量・期限・超過時動作を確定する。
製品キーはまだ接続せず、全体予算/期限/超過時政策と一般アプリの残範囲も維持する。

- 版付きJSONメッセージ、有限接続・未完了集合、ID・世代・取消・切断・重複・再接続を定義。
- fake peerと模擬トランスポートから開始し、別プロセスのユーザー／logon SID／PID／起動世代を照合。
- named pipeの非同期I/Oと停止伝播、偽server・PID再利用・容量超過・終了待機を試験。
- composition、通常入力、入力スコープの機密性判定、読取専用、未知条件での拒否を実装。
- 待機容量を消費前に確保し、超過切替と救済期限を決定。遅延ホストキーは再送しない。
- Notepad等で標準Undo・実打鍵・別欄拒否を確認。AppContainerは別ADRで成立するまで非対応。

受入条件：実APIなしで、正常要求と全失効条件をCore→IPC→所有スレッド編集まで一貫して検証すること。

## 2. 手動変換の縦断実装

登録済み: [Issue #5](https://github.com/kiyoka/Sumibi-win/issues/5)。前提はIssue #4。

- Host EXE、送信先別同意、設定形式版・原子的保存、Credential Managerのキー保存と削除。
- 編集用で採用した書記素分割とUnicode版を送信制限へ接続し、1000書記素とバイト／UTF-16上限を検査。
- 模擬HTTP、転送拒否・再送なし・60秒・応答上限・固定エラー。実通信は明示操作の別検証。
- 初回1候補と選択変換、取消・設定変更・同意取消・応答後の対象再検査。

受入条件：未同意・キー欠損・異なる認可版で送信せず、失敗時に原文を失わないこと。

## 3. 追加候補・ユーザー辞書・ローカル補完

登録済み: [Issue #6](https://github.com/kiyoka/Sumibi-win/issues/6)。前提はIssue #5。

- 候補件数のMac SPECとの差を決定し、現在結果を先頭へ。取得だけでは書かない。
- TSF UIElementのホスト表示指示、非アクティブUI、DPI・複数画面・読み上げ。
- 登録辞書の検証と送信量、ローカル索引の出所・固定入力・ハッシュ・生成手順・LICENSE。
- 中黒→Mozc→Sudachiの補完、重複・順序・本文と注釈の分離、API失敗時の非代替。

受入条件：候補選択時だけ対象を再照合して置換し、1回Undoで切替前へ戻ること。

## 4. Decisions自動変換

登録済み: [Issue #7](https://github.com/kiyoka/Sumibi-win/issues/7)。手動経路成立後。

- 公式API・モデル・料金・回答仕様を実装時に再確認し、独立同意・既定オフ。
- 200ms固定窓、32状態、未送信256、512書記素案、状態IDによる対応付けと厳格な応答検査。
- 送信前の回数永続予約、初期10000回、失敗・取消も計数。回数を料金と混同しない。
- 入力は即時、手動待機行列を使わない。Broker全体の直列化と手動優先、最新世代だけ適用。

受入条件：未許可入力を収集せず、古い判定・変換・認可を破棄し、回数上限を同時要求で超えないこと。

## 5. 公開準備

登録済み: [Issue #8](https://github.com/kiyoka/Sumibi-win/issues/8)。機能と互換範囲の確定後。

登録範囲、署名、配置、Host起動、更新中DLL、旧版復旧、削除と設定保持をADRで決定する。
x86/ARM64、昇格、AppContainer、Terminal等を個別の互換表で扱う。
既定IMEを強制変更せず、導入・更新・削除を実機で確認してから配布する。
