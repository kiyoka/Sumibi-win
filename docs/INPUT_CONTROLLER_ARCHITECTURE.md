# Windows入力経路の責務と状態の寿命

2026-10-21。実装基準 `91552b0`。[全体設計](TECHNICAL_OVERVIEW.md)の所有関係を詳述する。
現行の検証用Controllerと、未実装の製品InputSessionを区別する。

## 1. 所有者と終了条件（現在の実装）

| 所有者・型 | 保持するもの | 終了・失効 |
| --- | --- | --- |
| setupのApartment・Library | COM STA、ロード済みDLL | 全COM参照解放とアンロード可能判定後にDLL解放、最後にSTA終了 |
| Tip / Active | 所有ThreadId、manager、clientId、キー登録、Controller、Provider登録 | Deactivateで許可失効と登録解除。失敗時は状態を残して再試行 |
| Controller | manager、許可context、EditPolicy、Preview、通知購読、解除世代、再入ガード | revoke・フォーカス喪失で書込権限を先に失効 |
| Delayed / Controllerの検証要求 | 弱Controller、単調時計期限、単調増加ID、保留ID | 取消・無効化で失効。旧応答で新要求を消費せず、取消後の選択再取得を禁止 |
| キーsink・Provider・Control | Controllerへの弱参照と独立のObjectLease | Controllerが消えたら操作しない。外部COM参照が残ればDLLの寿命は維持 |
| Observer / Subscription | 弱Controller、Sourceと購読cookie | closeでUnadvise。旧通知でも失効した対象へ書かない |
| SelectionSnapshot | context・range・選択style・原文・取得時policy・所有thread | 操作中に再検査。本文を持つためDebugを実装しない |
| Preview | snapshot、atom、activeフラグ、manager/client、OwnedComposition | cancel/commitで自分のcompositionを終了。解除拒否時は再試行情報を維持 |
| OwnedComposition | 自分で作成したCOM object、所有thread、寿命phase | identityの消失で強制終了を検出。別objectへ付替えず、workerへ渡さない |
| Session | one-shot処理、実行結果、所有thread、ObjectLease | コールバック一度。同期要求終了後の遅延本文操作は拒否 |
| TextStore | 合成本文、選択、sink、lock、Undo/Redo、検証用HWND | 専用文書終了。一般アプリの本文所有者ではない |
| Window / View | HWND、Frame、入力callback、refresh識別子、旧WndProc | WM_NCDESTROYでactiveを落とし、旧予約を無効化 |
| Binding | context、client、購読cookie、Scene、弱Viewへのrefresh handle | closeでactiveを落とし、表示を消し、購読解除 |
| Association | manager、文書、旧関連付け、弱View | 生存中のHWNDだけ関連付けを戻す。破棄後の同じHWND値へ触れない |
| Core InputSession / TrackedInput | 所有ホストの実入力UTF-16、範囲、context識別、通知世代と選択 | 不一致・境界・上限超過で消去。takeで一度だけ消費。Debug・永続化なし |
| setup Tracking / InputWatch | 所有STAのmanager/client、メタデータ通知購読、Core記録、自動選択の印 | 挿入前後とF7の検査。通知世代はラップしない。workerへ参照を渡さない |
| bridge InputCheck | 次のStartに限定する原文・範囲・保護状態の検査closure | 同期呼出し中に消費・解放。失敗時は予約も失効。F7終了・shutdownでも解除 |
| DLL Input Control / Tracker | 弱Controllerと独立lease、InputWatch、Core記録、自己所有composition、下線、1件の入力前状態とReady | 所有STAと再入ガード。Cancelで失効、cleanup拒否は最大1件保持。Readyはsource検査で一度だけ消費 |
| Input Monitor / EXE Signal | DLL側はwake先、EXE側はmanual-reset eventとactive/ownerだけ | 通知はsignalだけ。所有ループがPollし、終了時にsignalを無効化。本文・target・DLL callbackをeventへ持たせない |

## 2. 強参照と弱参照

実線は保持、破線は弱参照またはコールバック接続を示す。呼出し順の図ではない。

```mermaid
flowchart LR
    tip["Tip"] --> active["Active"]
    active --> ctl["Rc Controller"]
    active --> provider["Provider登録"]
    ctl --> preview["Preview・SelectionSnapshot"]
    ctl --> sub["通知購読・cookie"]
    preview --> context["ITfContext・ITfRange"]
    ctl --> manager["ITfThreadMgr"]
    keys["TSFが保持するキーsink"] -.-> ctl
    provider -.-> ctl
    control["外部が保持するControl"] -.-> ctl
    observer["TSFが保持するObserver"] -.-> ctl
    cleanup["非同期の属性解除Session"] -.-> ctl
    window["Window"] --> view["Rc View"]
    view --> callback["入力callback"]
    callback --> editor["Rc Editor・TextStore"]
    binding["Binding"] --> state["描画State・context"]
    view -.-> state
    state -.-> view
```

キーsink、通知sink、独立表示属性Info・Enumerator、編集SessionはそれぞれDLLのObjectLeaseを保持する。
Tipの参照が0でも、他のCOMオブジェクトやserver lockが残ればDllCanUnloadNowはS_FALSEになる。
Controllerへの弱参照とDLLロード寿命の計数は別の責務である。
図のViewとStateは互いを強参照しない。キューのメッセージにRcの生ポインターを積まない。
WndProcのuserdataはWindowが保持するViewを参照し、コールバック中の再入破棄に備えて一時的な強参照を確保する。

## 3. 所有スレッドと再入

TSF/COMの操作は生成時のスレッドに限定し、別スレッドは固定HRESULTで拒否する。
単発Brokerの検証経路はCOM参照を移動せず、要求IDと結果をeventで所有スレッドへ通知してそこで編集要求を行う。
専用所有contextのManualEdit/workerの起点は[ADR 0004](adr/0004-one-shot-broker-and-owner-edit.md)を参照。
現在のManualEditはgeneric ITfRangeを保持し、source前にcontext・端点・向きと保護状態を本文の再取得なしで検査する。
観測した失敗は復元後も失効し、finishは新しいwriteで全文も再照合する。
Coreへは絶対座標と区別した長さstampだけを渡す（[ADR 0018](adr/0018-retained-manual-range-and-source-validation.md)）。
実入力追跡はACP方式のままで、非ACPアプリの互換性は未検証。
実DLLのキー・Core・編集から診断用EXE bridgeを通す非同期経路も追加した
（[ADR 0005](adr/0005-dll-key-broker-bridge.md)）。workerはDLLへの参照やcallbackを保持しない。
単発ジョブの予約はworker・job・結果が共有し、最後の解放までprocess内4件に数える。
結果を取り出してjobを外した後のCOM再入でも、Completionが枠を保持する
（[ADR 0015](adr/0015-broker-admission-through-delivery.md)）。
worker専用Channelは同じ認証済み子とpipeを再利用し、Replyを保持している間は次要求を拒否する。
取消・通信失敗・期限で接続を閉じ、古い要求を再送しない。Channel/Replyは共通4枠を最後のDropまで保持する
（[ADR 0016](adr/0016-authenticated-reusable-broker-channel.md)）。connect/close/Dropにも待機があり、
Channelを既存のTSF callbackから直接呼ばず、EXE Workerの容量1 Command/Completionとeventで接続する。
Bridgeは取得前Permitを結果配送の最後まで保持し、配送中のReserve/drain/shutdown再入を拒否する。
起動・通信・子回収とChannelのDropはworkerへ限定し、facadeのDropでは待たない
（[ADR 0017](adr/0017-reusable-worker-and-owner-delivery.md)）。
製品InputSessionと一般アプリ内でのBrokerClientの配置は未実装。
さらにsourceを返す前と編集時に保護メタデータを再検査し、scope/compartmentの変更で要求を失効させる。
scopeのStampと通知、MSCTF内部読取との境界は[ADR 0006](adr/0006-input-protection-metadata.md)を参照。
同一contextのcompositionも拒否する。GUID_PROP_COMPOSINGで失効させ、空compositionの通知は
所有するTextStoreから既存Cancelへ転送する。一般ホストにこの協力を仮定しない
（[ADR 0007](adr/0007-foreign-composition-boundary.md)）。所有者callbackのCOM参照は終了時に解除し、workerには渡さない。
選択プレビューは自分のcompositionを開始し、終了後に従来の本文検査を行う。
診断ホストの共有EXE sinkにはDLL参照を持たせず、開始拒否でOS側に参照が残ってもDLLをpinしない。
強制終了のidentity検査と契約・制限は[ADR 0008](adr/0008-owned-selection-composition.md)を参照。
所有ホストの入力範囲を追跡するTrackingも追加した。通常入力はホストが一度だけ反映し、
挿入前後のメタデータ・通知世代をCoreへ渡す。F7の自動選択は一度だけ使い、
Startの同期検査で実DLLから渡された原文と照合してから全COM参照を手放す。
専用EXEが入力を観測できる前提であり、一般アプリの入力取得ではない
（[ADR 0009](adr/0009-cooperating-host-input-run.md)）。
composing-demoでは実DLLの独立ISpikeInputが同じcompositionの伸縮を担当する。
通常の本文取得はidleを必要とし、メタデータ照会だけが保持中の正確な1 objectを許容する。
生の通知世代とCoreの入力世代を分け、外部通知による不一致で旧記録を破棄する。
ホストの本文反映後にメタデータを更新する限定経路であり、原子的な本文入力取引は未完了
（[ADR 0010](adr/0010-cooperating-host-composition-growth.md)）。
InputWatch/ManualEdit Guardはstatus通知も購読する。外部通知をEXEのeventに集約し、
次の打鍵を待たず所有ループのPollで追跡を失効・cleanupする。
失敗直後の通知世代を保持し、同じ世代の再signalだけではwriteを再試行しない。
参照と終了後通知の扱いは[ADR 0011](adr/0011-owner-loop-input-invalidation.md)を参照。
transaction-demoでは独立ISpikeInputWriterが供給された文字を一度だけ挿入し、caretとcompositionを
同じ同期write cookieで更新する。scope/status/compartment/focusの履歴を自己OnEndEditと分けて検査する。
SetTextを試みた後の失敗ではHRESULTから再入力を推測せず、変換を停止して明示boundaryを待つ。
既存本文取得のidle gateは維持する。待機・救済・一般アプリ統合の完成ではない
（[ADR 0012](adr/0012-synchronous-input-transaction.md)）。
transaction-demoのBackspaceは、Coreの記録末尾だけから削除範囲を計算し、実DLLの独立
ISpikeInputEraserで削除・caret・composition縮小を同じwriteに接続する。全削除では自己objectを終了する。
本文書込と確定処理は挿入と共有する。旧世代・追跡外・選択・他composition等では一部を先に消さず、
書込前に元のホスト処理へ返す。試行後の失敗では再削除せず、明示boundaryまで変換を停止する。
前方Delete・任意位置編集・待機・救済は未完了（[ADR 0013](adr/0013-synchronous-tracked-backspace.md)）。
所有editorの救済はCoreの1件・最大1024単位のslotを取引前に予約し、未確認結果だけを有限期限で保持する。
UI callbackはticket付き操作のみを通知し、所有ループが期限・保護履歴を検査して明示コピー／破棄する。
TSF対象・cookieを救済へ保存せず、workerやIPCにも渡さない。本文書込や遅延キー再送は行わない。
保護変更やfocus喪失ではpayloadを除去する。一般アプリや製品の待機行列ではない
（[ADR 0014](adr/0014-bounded-input-recovery.md)）。
Controllerのbusyガードにより操作中の再入を拒否し、自分の書込で生じる通知を外部編集と誤認しない。
TSFを呼ぶ前にRefCellの借用を解放し、通知やウィンドウ破棄が再入しても借用違反を起こさないようにする。

本文置換と属性解除の延期は区別する。属性解除は弱参照・世代一致を検査した上で非同期化できる。
本文置換は同期スパイクの完了条件を満たす場合だけ実行する。製品の非同期編集は別の期限付き要求として設計する。
同じ処理の再試行と、単なる読取確認も分ける。確認失敗を理由にSetTextを再実行しない。

## 4. 失効と後始末の順序

1. 許可・要求・プレビューを論理的に失効させ、本文書込を禁止する。
2. 候補などの表示を非表示にし、可能なら自分の属性だけ解除する。
3. ロック拒否なら解除だけを世代付きで予約し、古い予約が新対象に作用しないようにする。
4. 通知・Provider・予約キー・キーsinkを解除する。必要なcookieと参照は失敗時に保持する。
5. 文書とプロファイル・factoryを逆順に解除し、独立COM参照の解放を確認する。
6. DLLのアンロード可能判定が成立してからFreeLibraryを行う。

解除が失敗したときに権限を復活させない。Dropによる最善努力の後始末だけで、成功を保証したと記録しない。
setupは明示解除の結果を検査し、DLLを保持すべき場合は強制解放を避ける。
表示側もcloseで先にactiveを落とし、予約済みWM_APPの後着を無害にする。
HWNDが同じ整数でも別Viewなら別対象と扱う。

## 5. 製品InputSessionとBrokerの寿命（設計案）

| スコープ | 所有する状態 | 他スコープへ渡さないもの |
| --- | --- | --- |
| TSFスレッド | adapter、manager、client、入力先識別、所有スレッドへの結果配送 | 生COM参照・cookieをIPCやworkerへ渡さない |
| contextの接続世代 | 追跡、選択、原文アンカー、候補、保留要求、入力世代 | 別contextへ旧候補や救済文字を自動移行しない |
| 要求 | 一意ID、設定・認可版、取得原文、期限、結果の消費状態 | 接続し直したBrokerへ未完了要求を再利用しない |
| Broker接続 | nonce、起動世代、認証済みpeer、有限の未完了集合 | peerの本文を別接続の要求へ対応付けない |
| Broker全体 | 設定、資格情報、辞書、累積判定回数、自動処理の排他 | 資格情報をDLLや候補UIへ配布しない |

MacのInputRuntime.sharedは短命コントローラー再生成への対策だが、Windowsでプロセス共有の
単一入力状態を採用する根拠にしない。TSFスレッドとcontextの寿命を実測して管理する。
IUnknownの同一性と接続世代を組み合わせ、ポインター値の再利用や同じ原文の別欄を区別する。
対象変更・無効化・設定取消・IPC切断で要求を取消し、結果の配送先を検索して別欄へ適用しない。

## 6. 検証と残る確認

実装根拠は[com.rs](../crates/sumibi-tip/src/com.rs)、[spike.rs](../crates/sumibi-tip/src/spike.rs)、
[notifications.rs](../crates/sumibi-tip/src/notifications.rs)、[window_host.rs](../crates/sumibi-setup/src/window_host.rs)、
[live_render.rs](../crates/sumibi-setup/src/live_render.rs)。
COM単体試験とload_smokeは独立sink/Info参照によるアンロード阻止、再入・解除失敗を確認する。
window-testは旧属性解除、新描画Binding、破棄後予約、実フォーカス通知を確認する。
専用ACP storeでは非同期IPC後の所有thread編集、取消・終了・変更後の復元と失効をbroker-testで確認した。
一般アプリでのcomposition寿命、非同期IPCとの再入、終了直前の外部編集は未検証。
これらを実装する際は専用ホストで異常系を追加し、対象アプリの手動結果を別に記録する。
