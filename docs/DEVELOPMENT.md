# 開発環境と現在の実装範囲

Issue #1の到達点と残る境界は[最小試作の受入記録](ISSUE_1_ACCEPTANCE.md)を参照。
まとめて検証する場合は通常ユーザーのPowerShellで `./tools/Test-Spike.ps1` を実行する。
途中失敗で停止し、既定では専用ウィンドウとログオンSID限定pipeも検証する。
画面を使用できない環境では `-SkipDesktop` を明示し、未実行の検査を成功扱いにしない。

Issue #4の型付きIPCとclient状態の純粋Rust検証は
`cargo test -p sumibi-protocol --locked --offline`で実行できる。
現時点はCore 62件（応答照合9・入力範囲追跡/削除/caret計画34・書記素移動/削除境界7・救済12）、Protocol 22件（frame 6・接続状態8・JSON 4・Core模擬接続4）、IPCの境界・worker/server 12件、TIP 6件を含めworkspace合計102件。
JSON構造・固定エラー・世代・取消・再接続・期限・対象照合を扱い、実APIやOS peer認証は使わない。
形式・制限・残る製品接続は[IPC契約](IPC_PROTOCOL.md)を参照。

通常ユーザーの別プロセス・非同期I/O検証は `target/debug/sumibi-setup.exe ipc-peer-test`。
固定ハンドルへ相手を結び付け、トークン・PID・作成時刻を相互検査した後で模擬結果をCoreへ返す。
停止・期限・偽の相手・drop・容量・handle解放を確認する。起動方式と限界は[ADR 0003](adr/0003-pinned-desktop-peers.md)。
通常のデスクトップmedium integrity限定で、サンドボックスや昇格環境で拒否されてもACLを拡張しない。

## Windows標準RichEditの拒否条件を検証する

```powershell
cargo build --workspace --locked --offline
./target/debug/sumibi-setup.exe native-edit-test --dll ./target/debug/sumibi_tip.dll
```

専用のRichEdit子windowを作り、実際のnative TSF contextで7構成の拒否を確認する。
通常/リッチテキスト・private/既定scope・password・readonly/復元を扱う。
現在のRichEditはTS_SS_NOHIDDENTEXTを報告せず、変換は未対応。
診断input_status_denialは本文を受け取らず、statusの最初の拒否理由だけを返す。
Noneでもscope等の許可は意味しない。保護条件・JSONは維持する。
adapter本文取得0・元本文不変・native Undoなしを確認し、変換/Undoの成功試験とは区別する。
観測したOS・各HRESULT・寿命と制限は[ADR 0019](adr/0019-native-richedit-status-diagnostics.md)を参照。
ログはtarget/issue-4-native-edit.log、全回帰はtarget/issue-4-native-edit-regression.log。
入力デモの有限pumpとblocking loopはITfMessagePumpを通す。
従来のuser32だけのpumpではfocus通知待機が再現性をもって失敗し、TSF wrapperへの変更後は20件成功した。
focus試験は事前に用意した別文書への実切替を確認し、待機時間や失効の検査を緩和していない。
2026-10-22、通常ユーザーのWindows 11 x64 build 26300 / Rust 1.99.1でworkspace 66件を含む
Test-Spike.ps1全回帰が省略なしで成功。TSF/COM解除・DLLアンロード、診断process残留なしも確認した。

## 選択変換の保持rangeを検証する

```powershell
cargo build --workspace --features sumibi-host/spike --locked --offline
./target/debug/sumibi-setup.exe range-target-test --dll ./target/debug/sumibi_tip.dll
```

ManualEditはITfRangeACPの絶対座標を要求せず、保持rangeの端点・向き・本文を照合する。
2026-10-22、通常ユーザーのWindows 11 x64 build 26300 / Rust 1.99.1で16ケース成功。
送信前の選択/保護状態の検査と、拒否後に状態を戻しても再利用できないことも確認した。
fixtureは専用ACP storeで、非ACPアプリの互換性は未検証。実入力追跡は従来のACP方式のまま。
workspace 65件を含むTest-Spike.ps1の全検査も省略なしで成功した。
ログはtarget/issue-4-retained-range.logとtarget/issue-4-retained-range-regression.log。
詳細と本文cacheの制限は[ADR 0018](adr/0018-retained-manual-range-and-source-validation.md)を参照。
追加4ケースは所有ホストのManualEdit.cancelを扱う。原文返却前後・重複取消・新旧要求の分離を確認し、
旧要求の拒否で本文取得・SetText・Undoを増やさない。新要求だけを正常に適用し1回Undoで戻す。
共通helperも保持したITfMessagePumpを使うが、全focus履歴の自然通知を保証する変更ではない。
最新の個別ログはtarget/issue-4-owner-boundary-final-range-target-test.log、全回帰試行はtarget/issue-4-owner-boundary-regression.log。
所有・通信取消との分担と検証の境界は[ADR 0020](adr/0020-owner-cancellation-and-shared-tsf-pump.md)を参照。
ADR 0020の最終版ではRust 66件・fmt・各構成Clippy・build・load_smoke・self/edit/keyと範囲16・通知24が成功。
window/input/delayedの3検査は、所有windowの前面化が拒否されたため未通過。
入力と焦点のassertionは維持し、直接DLL callbackの成功へ読み替えない。
通常の対話desktopでTest-Spike.ps1を省略なしで再実行する必要がある。
制約を除いた静的検査ログはtarget/issue-4-owner-boundary-static.log。

## 同期入力取引を試す

通常ユーザーのPowerShellで、リポジトリのフォルダから実行する。

```powershell
cargo build --workspace --locked --offline
cargo build -p sumibi-host --features spike --locked --offline
./target/debug/sumibi-setup.exe transaction-input-test --dll ./target/debug/sumibi_tip.dll
./target/debug/sumibi-setup.exe backspace-input-test --dll ./target/debug/sumibi_tip.dll
./target/debug/sumibi-setup.exe transaction-demo --dll ./target/debug/sumibi_tip.dll
```

デモは専用の空の合成入力欄を開く。通常文字を入力すると実DLLが本文・caret・composition・下線を
同じ同期write cookieで更新する。F7で入力範囲を固定の「日本語」へ変換し、Ctrl+Zで戻せる。
Escは追跡を解除する。Backspaceは記録内を同じwrite内で削除・縮小し、全削除でcompositionを終了する。
repeatは記録内の拡張書記素クラスタ単位で数え、追跡範囲を越える要求は全体を元のホスト操作へ返す。
通常の左右移動が記録内なら同じcompositionを保持し、移動先からの前方Deleteも実DLLで処理する。
内部Backspaceも削除前後に残った記録を同じcompositionで保持する。
途中への文字挿入も前後の記録を保持し、合計1024 UTF-16単位以内なら同じcompositionを広げる。
境界越え、Shift/Home/End、任意選択への移動は追跡を退役して通常ホストへ戻す。
内部caretのF7も現在の記録全体だけを変換する。移動は本文/Undoを増やさず、削除は1回Undoで戻せる。
unicode-segmentation 1.13.3 / Unicode 17.0.0に固定し、通常ホストのBackspaceも同じ分割を使う。
記録外の既存本文へクラスタを広げず、正規化もしない。
非空の明示選択をDeleteすると、実DLLが同じwriteで削除・caret・一時composition終了を処理する。
repeatでも選択を一度だけ削除し、削除本文を取得・救済コピーしない。追跡できない前方DeleteとUndoは既存ホスト操作を使う。
終了は×ボタン。既定IME・ユーザー文書・HTTP・資格情報は使用しない。

書き込み前拒否ではホストが元の入力を一度だけ反映する。書き込みを試みた後の失敗では、
診断タイトルに確認を表示し、再入力・rollbackとF7を停止する。Esc/Enter/移動で停止を解除できるが、
未確認入力を自動復旧する機能ではない。入力結果を確認する必要がある。
一般アプリ向けのキー消費・待機・救済を有効にする段階ではない。

2026-10-22 / Windows 11 x64 build 26300 / medium / Rust 1.99.1で22ケース、
TSF/COM解除とDLLアンロード検査が成功。ログはtarget/issue-4-input-transaction.log。
Test-Spike.ps1を省略なしで実行し、workspace 47件、Clippyの3構成、既存の全native検査も成功した。
全回帰ログはtarget/issue-4-input-transaction-regression.log。
条件とdispositionの契約は[ADR 0012](adr/0012-synchronous-input-transaction.md)を参照。
Backspaceの27ケースも成功。ログはtarget/issue-4-backspace-transaction.log。範囲とfallbackの契約は
[ADR 0013](adr/0013-synchronous-tracked-backspace.md)を参照。
最終版はTest-Spike.ps1を省略なしで実行し、workspace 50件・Clippy 3構成と既存native検査も成功した。
全回帰ログはtarget/issue-4-backspace-transaction-regression.log。
書記素追加時のbackspace-input-testは10件を加え37ケースが成功。Core 5件追加でworkspaceは76件。
選択Delete追加時はtransaction-input-testが42ケース、recovery-input-testが35件成功した。
削除後の新規追跡、保留変換の失効、書込後の再削除禁止、救済満杯での1回Undo/Redoも検査する。
Rust83件と関連回帰・検証範囲・ログは[ADR 0025](adr/0025-synchronous-selected-delete.md)を参照。
記録内の移動/前方Delete追加時は26件を加えて同期入力68件、共有満杯時の移動/削除を加えて救済37件。
Rust91件と関連回帰が成功した。操作規則と検証範囲は[ADR 0027](adr/0027-tracked-caret-and-forward-delete.md)を参照。
内部Backspace追加時は18件を加えて55件、共有満杯の内部削除を加えて救済38件、Core5件追加でRust96件。
同期入力68・書記素ホスト22・composition46・通知30・追跡25・broker入力11・close4と静的検査も成功した。
契約とログは[ADR 0028](adr/0028-tracked-internal-backspace.md)を参照。
最新は途中挿入20件を加えて同期入力88件、共有満杯と断片コピーを加えて救済40件、Core6件追加でRust102件。
Backspace55・書記素ホスト22・composition46・通知30・追跡25・broker入力11・close4と静的検査が成功した。
契約と最大長の描画制限は[ADR 0029](adr/0029-tracked-internal-insertion.md)を参照。以下は以前の段階の検証結果である。
fmt・Clippy 3構成・build・基本DLL検査も成功した。一括scriptはkey-testのShift押下状態で停止し、
その後の単独key-testが成功した。script全体の成功とは区別する。
救済34・同期入力22・追跡25・composition伸縮29・通知30・broker入力11・close 4も再確認した。
前面化で未通過のwindow/input/delayedは引き続き未確認。最新ログと境界の条件は
[ADR 0023](adr/0023-grapheme-backspace-within-recorded-input.md)を参照。

## 専用ホストの書記素移動・選択・削除を検証する

専用editorの書記素移動・Shift選択・ホストDeleteは次で検証できる。

```powershell
cargo build --workspace --features sumibi-host/spike --locked --offline
./target/debug/sumibi-setup.exe grapheme-input-test --dll ./target/debug/sumibi_tip.dll
```

実TSF contextでホストの編集処理を呼ぶ22ケースが成功。Core境界7件を含めworkspaceは83件。
Backspaceのrepeatは、DLLが書込前に拒否した場合や救済枠不足でも一度のホスト編集にする。
非空の明示選択は一度だけ削除し、collapsed caretでは指定数の書記素を文書端まで削除する。
同期取引と旧composition観測経路の両方で1回Undo/Redoを検査する。通常キー配送の検査ではなく、
追加ケースはEditor.inputへWM_CHAR相当の引数を直接渡す。
Test-Spike.ps1 -SkipDesktopと、関連するBackspace37・救済34・同期入力22・追跡25・composition29・通知30・broker入力11・close4も成功。
前面化が必要なwindow/input/delayedは未確認のままで、通常キー配送の成功に読み替えない。
操作規則とログは[ADR 0024](adr/0024-grapheme-navigation-in-owned-host.md)を参照。

## 未確認入力の救済を試す

リポジトリのフォルダで、通常ユーザーのPowerShellから実行する。

```powershell
cargo build --workspace --locked --offline
cargo build -p sumibi-host --features spike --locked --offline
./target/debug/sumibi-setup.exe recovery-input-test --dll ./target/debug/sumibi_tip.dll
./target/debug/sumibi-setup.exe recovery-demo --dll ./target/debug/sumibi_tip.dll
```

recovery-demoは合成文字列の入力失敗を作り、空の入力欄を表示する。F8で救済の確認画面を開く。
確認画面のコピー欄、または画面を開いた状態のF10で実クリップボードへ全文をコピーできる。
コピー後も文書へ自動的には挿入しない。F9または破棄欄で救済を消し、×は画面だけ閉じる。
Escは入力追跡の停止を解除するが、救済は保持する。満杯の間の新規入力は通常ホスト経路で一度だけ処理する。

保持はeditorごとに1件・最大1024 UTF-16単位・失敗確定から最大10分。診断EXE全体で予約中も含め合計4件。
満杯なら他所有者のデータを上書きせず、DLL取引に入る前に元の通常ホスト入力へ返す。開閉やコピー失敗で延長しない。
保持期限後は所有ループで画面と救済を解除する。終了時にも破棄し、ディスクへ保存しない。
保護状態の変更・復元やfocus喪失では本文を取り除き、操作情報だけを残す。Backspaceも操作情報だけで、コピーはできない。
既に明示コピーしたOSクリップボードは救済期限で自動消去しない。

recovery-input-testは実DLLの34ケースを自動検証する。コピー先は試験用メモリーで、利用者のクリップボードは変更しない。
共有予算のCore 5件を追加し、workspaceは71件。Test-Spike.ps1 -SkipDesktopと、救済31・同期入力22・Backspace 27・通知24・close 4が成功。
今回、省略なし全回帰は再実行していない。前面化条件で未通過のwindow/input/delayedは引き続き未確認。
ログはtarget/issue-4-shared-recovery-static.logとtarget/issue-4-shared-recovery-native-final.log。
基本契約は[ADR 0014](adr/0014-bounded-input-recovery.md)、共有容量・所有者と制限は[ADR 0021](adr/0021-shared-input-recovery-budget.md)を参照。
追加3件は無効化・非アクティブ化・モード取消と復元後のコピー拒否を検証する。
確認画面で予約した旧コピーも本文除去後は実行せず、操作情報だけを元の期限まで保持する。
最新ログはtarget/issue-4-window-boundary-final-recovery-input-test.log。

## 別Host EXEから実TSFまでの検証

通常ユーザーのPowerShellで実行する。既定のworkspaceビルドで作るHostは変換を許可しないため、
固定結果を使う診断時だけ、続けてHostのspike featureを指定する。

```powershell
cargo build --workspace --locked --offline
cargo build -p sumibi-host --features spike --locked --offline
./target/debug/sumibi-setup.exe broker-test --dll ./target/debug/sumibi_tip.dll
```

broker-testは専用の合成ACP入力欄を作り、自動終了する。ユーザー文書・HTTP・資格情報は使用しない。
2026-10-21、Windows 11 x64 / build 26300 / medium / Rust 1.99.1で19ケース、TSF/COM解除と
DLLアンロード検査が成功。合成データのみのログは `target/issue-4-broker-verification.log`。
正常部分置換とUndo、取消・終了・旧要求、変更後の原文／選択／フォーカス復元、設定・認可、
ロック／読取拒否、期限・ID・版不一致、Readyによる送信前拒否を確認した。

ジョブ予約はprocess内4件、結果channelは各1件。完了済みjob・受取済み結果も予約を保持する。
worker・job・結果の全所有者の解放後に枠を返す（[ADR 0015](adr/0015-broker-admission-through-delivery.md)）。
broker-testに容量6ケースを追加し、正常・拒否結果の保持中も4件の上限が効くことを確認した。
I/O・子processの回収はworkerで行い、
TSFの所有loopがeventを受けて新しい同期write内でCore.finishとSetTextを行う。
詳細な所有・失効・起動の限定条件は[ADR 0004](adr/0004-one-shot-broker-and-owner-edit.md)を参照。
このbroker-testの編集アダプターはsetupへRustライブラリとしてリンクする。
次節のbroker-key-test/broker-demoは実DLLのキーとCore・編集を通す別検証である。
どちらも一般アプリの入力・標準Undoの確認には読み替えない。

Hostのspike featureは開発診断用で、配布しない。`cargo build --workspace`はHostを既定の拒否構成へ戻す。
`Test-Spike.ps1`は通常実行時だけこの診断Hostをビルドしてbroker-testを実行し、
`-SkipDesktop`ではbroker-testも明示的に省略する。

## Broker接続の再利用を検証する

通常ユーザーのPowerShellで、リポジトリのフォルダから実行する。

```powershell
cargo build --workspace --locked --offline
./target/debug/sumibi-setup.exe broker-channel-denied-test
cargo build -p sumibi-host --features spike --locked --offline
./target/debug/sumibi-setup.exe broker-channel-test
./target/debug/sumibi-setup.exe broker-worker-test
```

denied-testは通常Hostの拒否2項目、channel-testは実子processで14項目を自動検証する。専用workerで同じ
起動processへ連続変換し、Reply保持中の拒否、取消、期限、64要求上限、共通4枠、子と親handleの解放を確認する。
原文は合成文字列だけで、ユーザー文書・HTTP・既定IMEを使用しない。
worker-testは11項目を確認する。既存の入力デモも同じEXE workerからBrokerを再利用する。
Channelはblockingのworker専用APIで、TSF/window callbackから直接呼ばない。
所有STAからは非blockingのWorker facadeを使い、起動・通信・子回収はworkerへ限定する
（[ADR 0017](adr/0017-reusable-worker-and-owner-delivery.md)）。製品の常駐共有探索・一般アプリ統合は未完了。
Channel単独導入時のTest-Spike.ps1全回帰はworkspace 61件を含め成功。ログはtarget/issue-4-broker-channel-regression.log。

## 実DLLのキーから非同期変換を試す

上記のworkspaceビルドとHostのspikeビルドを行った後で実行する。

```powershell
./target/debug/sumibi-setup.exe broker-key-test --dll ./target/debug/sumibi_tip.dll
./target/debug/sumibi-setup.exe broker-input-test --dll ./target/debug/sumibi_tip.dll
./target/debug/sumibi-setup.exe broker-demo --dll ./target/debug/sumibi_tip.dll
```

broker-demoは手動操作用。文字を入力し、F7で選択部分（未選択なら全文）を固定の「日本語」に変換する。
約400msの待機中も操作でき、Escで取消、Enterで原文を1回だけ確定、Ctrl+ZでUndoする。
×で閉じると許可取消・worker停止・TSF/COM解除を行う。このデモでF6の下線プレビューは使用しない。
本文は専用入力欄と別Hostのメモリーだけで扱い、外部APIへ送信・保存しない。

broker-key-testは実DLLの予約キーから25ケース、broker-input-testは描画・Undo・Enter・編集・
同期SC_CLOSE・WM_KILLFOCUS・空composition開始終了・無効化・非アクティブ化・モード取消を含む11ケースが成功。いずれも自動終了する。前者のcapacity/Esc/Enterは
前面化に依存しない同じDLL callbackで確認し、物理キーボード操作の手動確認は含めない。
容量検査には未起動予約に加え、完了済みjob・受取済み結果を保持した満杯と、解放後の同じ入力欄での再利用を含む。
さらに同じ子での連続変換・古いCancel・配送中の再入拒否・2回目の取消/本文復元・取消後の明示再開も確認する。
今回の最終全回帰はworkspace 63件と再利用worker 11項目を含め成功。ログはtarget/issue-4-reusable-worker-regression.log。
容量予約補強時の全回帰はworkspace 59件を含め成功。ログはtarget/issue-4-job-admission-regression.log。
同日の通常ユーザー/Windows 11 x64 build 26300/Rust 1.99.1で解除とDLLアンロードも確認した。
ログは `target/issue-4-dll-broker-key-checks.log`、`target/issue-4-dll-broker-input-checks.log`。

通信workerは診断用COM bridgeを提供するsetup EXEに置き、DLLは対象の保持と適用を担当する。
一般アプリへの同じbridgeの存在は仮定しない。責務・再入・IDの寿命は
[ADR 0005](adr/0005-dll-key-broker-bridge.md)を参照。従来のdemoは従来の固定変換経路のまま。

## 入力保護の検証

workspaceとspike Hostを上記手順でビルドした後、通常ユーザーのPowerShellで実行する。

```powershell
./target/debug/sumibi-setup.exe protection-test --dll ./target/debug/sumibi_tip.dll
```

通常scope、password/PIN/private、未設定・不正・上限超過、status、compartment、変更後の復元、
通知なしでの送信前再検査を合成文書で扱う。実DLL/TSFの39ケースが成功した。
初めから保護／不明な27ケースではDLLの本文取得呼出しとBroker job開始が0であることを確認する。
MSCTF内部の本文照会は別に計測し、OSを含む一切の読取を防止したとは扱わない。
条件・制限・診断counterと正式な状態通知の扱いは[ADR 0006](adr/0006-input-protection-metadata.md)を参照。
一般アプリの保護欄を全て検出できるという保証はまだない。

2026-10-21、前段階cd1dc2fの手動デモについて利用者から概ね動作したとの報告を得た。
個別キー・保護入力の手動合格記録ではなく、上記の自動検証とは分けて扱う。

## 他サービスのcompositionの検証

workspaceとspike Hostをビルドした後で実行する。

```powershell
./target/debug/sumibi-setup.exe composition-test --dll ./target/debug/sumibi_tip.dll
```

2026-10-22、実DLL/TSFの16ケースが成功。同じcontextの空・別範囲・複数compositionも拒否し、
取得・source・適用時に再確認する。他サービスのcompositionの本文を取得・終了・引継ぎしない。
非空の開始／終了はproperty通知で取消し、空compositionの履歴は所有ホストの通知で補う。
ホストの協力がない一般アプリで、通知されない空compositionの履歴を検出する保証は未成立。
専用デモのEditorでも同じ所有者通知を接続し、broker-input-testに回帰ケースを追加した。
fixtureの不正scopeを返す時期、実OS内の読取、手動未検証範囲は[ADR 0007](adr/0007-foreign-composition-boundary.md)を参照。
ログは `target/issue-4-composition-*.log`。

## 自分の選択compositionの検証

```powershell
./target/debug/sumibi-setup.exe owned-composition-test --dll ./target/debug/sumibi_tip.dll
```

workspaceビルドの後で実行する。2026-10-22、実DLLの16ケースと直接アダプター2ケースが成功。
F6プレビューは既存選択の下線とcompositionを保持し、F7・Esc・Enter等で自分のcompositionを終了する。
ホスト開始拒否・強制終了・拒否後の再開始・別compositionとの競合・通常成功の1回Undoも確認する。
専用ホストがEXE内の共有通知サービスを提供する構成であり、一般アプリへ同じ存在を仮定しない。
このF6経路は選択の寿命管理まで。通常入力の追跡・composition更新は後続節の専用経路で試す。
条件とWindowsのsink参照残留の扱いは[ADR 0008](adr/0008-owned-selection-composition.md)、ログは `target/issue-4-owned-*.log`。

## 通常入力の範囲だけを変換する

リポジトリのルート（README.mdやCargo.tomlがあるフォルダー）で実行する。

```powershell
cargo build --workspace --locked --offline
cargo build -p sumibi-host --features spike --locked --offline
./target/debug/sumibi-setup.exe tracking-demo --dll ./target/debug/sumibi_tip.dll
```

文字を入力し、そのままF7を押すと直前の連続入力を固定の「日本語」へ変換する。
カーソル移動・削除・外部編集・focus喪失等で追跡を終えたら、未選択のF7は何も変換しない。
新しく入力した分は新しい追跡対象になり、既存prefix/suffixを含めない。
明示的なShift選択やCtrl+Aの範囲も変換できる。F6は使用しない。
待機中Escで取消、Enterで原文確定、Ctrl+Zで変換前へ1回Undo、×で終了する。
従来のbroker-demoは未選択時に全文変換するので、入力範囲の動作確認にはtracking-demoを使う。

自動検証は次のコマンド。2026-10-22のWindows 11 x64 build 26300 / medium / Rust 1.99.1で
25ケースと解除・DLLアンロードが成功した。同日にTest-Spike.ps1の全検査も省略なしで成功している。

```powershell
./target/debug/sumibi-setup.exe tracking-input-test --dll ./target/debug/sumibi_tip.dll
```

通常入力はホストで一度だけ反映し、追跡できないときも再送・取消しない。
Coreの記録上限は1024 UTF-16単位。保護メタデータを入力前後と送信前に検査する。
専用ホストの協力が前提で、打鍵ごとのcomposition拡張や一般アプリの製品InputSessionではない。
詳細は[ADR 0009](adr/0009-cooperating-host-input-run.md)、ログは `target/issue-4-tracking-*.log`。

## 入力compositionの伸縮を試す

リポジトリのルートから、通常ユーザーのPowerShellで実行する。

```powershell
cargo build --workspace --locked --offline
cargo build -p sumibi-host --features spike --locked --offline
./target/debug/sumibi-setup.exe composing-demo --dll ./target/debug/sumibi_tip.dll
```

入力すると、その範囲に下線が付き、同じcompositionが入力に合わせて伸びる。
Backspaceで範囲を縮め、末尾まで消すと下線とcompositionを終了する。
F7でその範囲または明示選択を固定の「日本語」へ非同期変換し、Ctrl+Zで1回Undoする。
composing/transactionホストの初回選択も、文字入力で追跡を作る前にF7を開始できる。
明示選択の向きを維持し、記録のないcollapsed caretからは全文を推測しない。
Escは追跡/待機を取消し、Enterは原文確定をホストへ通す。移動・focus喪失・×終了でも後始末する。
F6は使用しない。本文はホストで一度だけ反映し、composition操作失敗を理由に再送しない。

自動検証は次のコマンド。専用ホスト29ケースで、実DLLの所有CLSIDと同一objectの伸縮、
開始拒否、失効、別composition非干渉、取消、1回Undo、送信前検査、同期SC_CLOSEを確認する。
最新は初回選択の17ケースを追加し、合計46件が成功。初期化・逆向き選択・最大範囲・sink欠損、
各種拒否・取消・履歴復元とReadyの一度だけの使用を検査する。Rust83件・関連回帰と範囲は
[ADR 0026](adr/0026-initialize-explicit-selection-with-context.md)を参照。
2026-10-22 / Windows 11 x64 build 26300 / medium / Rust 1.99.1で、29ケースと
Test-Spike.ps1の全検査が省略なしで成功。workspace 47件、解除とDLLアンロードも確認した。

```powershell
./target/debug/sumibi-setup.exe composing-input-test --dll ./target/debug/sumibi_tip.dll
```

ホストの本文反映と、その後のcompositionのメタデータ更新は別の同期処理である。
一般アプリの原子的なTSF入力取引・標準Undoの完成とは扱わない。外部通知による失効は
所有ループで次の打鍵を待たず検出・後始末する。通知callback中には書かない。
条件・寿命は[ADR 0010](adr/0010-cooperating-host-composition-growth.md)、ログは `target/issue-4-composing-*.log`。

## 入力追跡の外部通知を検証する

上記のworkspaceとspike Hostをビルドした後、通常ユーザーのPowerShellで実行する。

```powershell
./target/debug/sumibi-setup.exe input-notify-test --dll ./target/debug/sumibi_tip.dll
```

専用ホスト30ケース。composing-demoの所有ループはメタデータのeventも待ち、外部の本文・選択・
scope・compartment・status・focusやcompositionの変更で古い追跡を解除する。
通知自体では編集せず、本文を読まず、次の打鍵やF7も必要としない。
変更後に元へ戻しても旧追跡を復活させない。変換待ちのreadonly復元も適用を拒否する。

readonly/lockによるcleanup拒否は最大1件を保持する。同じ世代の連続wakeではwriteを再試行せず、
状態回復の通知や明示操作で再試行する。一般アプリで同期writeが拒否された場合の即時表示解除は未保証。
eventの切断/再接続、終了後通知、signal失敗でも通常入力を失わないことも確認する。
2026-10-22 / Windows 11 x64 build 26300 / medium / Rust 1.99.1で20ケースが成功。
Test-Spike.ps1も省略なしで成功し、workspace 47件、全構成Clippy、既存TSF・Broker・IPC検査、
TSF/COM解除・DLLアンロードを再確認した。
詳細は[ADR 0011](adr/0011-owner-loop-input-invalidation.md)、ログは `target/issue-4-input-notify*.log`。
現在のfocusは所有ホストが境界を転送するowner-focusで検証し、自然通知だけを保証しない。
WM_KILLFOCUSで旧入力を即時に再利用禁止にし、本文取得・編集を行わずeventへ集約する。
owner loopでcleanupし、focus復元・古い送信前checker・cleanup拒否/回復後も旧入力を再利用しない。
追加した4件を含む24ケースの結果と前段階のfocus失敗の記録は
[ADR 0020](adr/0020-owner-cancellation-and-shared-tsf-pump.md)を参照。
続いてWndProcからWM_ACTIVATEAPP(FALSE)・WM_ENABLE(FALSE)・WM_CANCELMODEも同じ失効経路へ接続した。
正のactivation/enableは正常な追跡を取り消さず、古い権利を復活させない。
境界callbackがhandled/errorを返してもDefWindowProcを通し、標準のmouse capture解除を維持する。
今回のRust 71件・fmt・Clippy・基本DLL/TSF検査と、通知30・broker入力11・救済34、
追跡25・composition伸縮29・同期入力22・Backspace 27・close 4が成功。
前面化で未通過のwindow/input/delayedは維持し、今回、省略なし全回帰は再実行していない。
詳細と最新ログは[ADR 0022](adr/0022-native-owner-window-boundaries.md)を参照。

## 初期環境の記録（2026-10-20）

- Windowsの作業フォルダーで開発。ブランチは `codex/issue-1-windows-design`。
- Visual Studio 2022 Community、MSVCディレクトリ `14.44.35207`、Windows SDK Includeディレクトリ `10.0.26100.0` を確認。
- 同日、Visual Studio本体を `D:\Microsoft Visual Studio\2022\Community` に再インストール。構成のエクスポート前後でcomponent一覧が一致し、全8件のテストと実DLLロード検証も再成功。キャッシュ先は `D:\VisualStudioCache`。共有部品とWindows SDKの一部はCドライブに残る。
- Rust 1.99.1 (`58cf8f9c3`)、Cargo 1.99.1、`x86_64-pc-windows-msvc` を確認。CoreとTIP DLLのコンパイル・リンクに成功。
- 起動済みシェルのPATHにはRustが反映されていなかったため、検証は `C:\Users\kiyok\.cargo\bin\cargo.exe` を明示して実行した。

新しいPowerShellから次を実行する。PATH未反映ならCargoの絶対パスを使う。

```powershell
rustc --version --verbose
cargo --version
rustup show active-toolchain
cargo test --workspace
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --locked
cargo run -p sumibi-tip --example load_smoke --locked -- target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked -- plan --dll target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked -- self-test --dll target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked -- edit-test --dll target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked -- key-test --dll target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked -- delayed-test --dll target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked -- close-test --dll target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked -- ipc-test
cargo run -p sumibi-setup --locked -- ipc-peer-test
```

ターゲットは `x86_64-pc-windows-msvc`。検証済みのRust 1.99.1を `rust-toolchain.toml` に固定し、
Cargo生成の `Cargo.lock` を管理する。Coreに外部依存はない。
OneDrive上のincremental cacheでハードリンク警告が出る場合は、そのシェルで
`$env:CARGO_INCREMENTAL = '0'` を指定できる。今回の最終検証はこの設定で行った。

## 今回の実装

Cargo workspaceに `sumibi-core` を作成。unsafeを禁止し、手動変換1件の応答適用可否を
OS非依存で判断する小さな基盤を実装した。本文を保持する型にDebugを実装しない。

- 未完了要求の上書きを拒否。
- 取消後・完了後の応答を拒否。別の古い応答は現在の要求を消費しない。
- context・範囲・選択・UTF-16原文・入力世代・設定版・認可版・保護条件を完全照合。
- 保護入力、空原文、不正UTF-16、空の変換結果を拒否。
- 対応する応答は検証失敗でも消費し、再適用しない。

Coreの7個のテストに加え、TIPのCOM寿命・異常系テストを実行する。
2026-10-20のWindows x64検証では全8件が成功し、fmtチェックとClippy（警告をエラー扱い）も成功した。
実DLLのロード検証用 `load_smoke` は、登録せず専用プロセスでエクスポート関数を呼び、
クラスファクトリー／TIP生成、模擬マネージャーへのキーsink登録・解除、参照解放、
アンロード判定とFreeLibraryを確認する。実際のWindows TSFによる有効化成功とは区別する。

## TIPの実装範囲と依存

`sumibi-tip` は `cdylib` として `target/debug/sumibi_tip.dll` を生成する。
`DllGetClassObject`、`DllCanUnloadNow`、IClassFactory、ITfTextInputProcessorExの骨格を実装。
COM参照とサーバーロックを数え、未知CLSID／IID、集約要求、null引数を拒否する。
Rustパニックを起こし得る自前のCOM処理はABI内で固定HRESULTへ変換する。
DllMainでの初期化や通信は追加していない。COMの入力ポインターの有効性は呼出し側の契約とする。

Activate/ActivateExは所有スレッドでキーsinkを登録する。キーsinkはTIPから独立したCOMオブジェクトで、
TIPへの循環参照を作らない。すべてのキー／preserved keyを未消費で返し、本文の読取・変更をしない。
二重有効化とライフサイクル処理中の再入を拒否する。解除失敗時は状態を保持して再試行できる。
Deactivate忘れの場合も同じスレッド上で解除を試み、残ったsinkは自身の参照が解放されるまでDLLを保持する。

`sumibi-setup` のプロセス内登録を用いた経路では、実TSFによる有効化・前面キーsink確認・解除が成功した。
登録後の各段階で意図的に失敗させる巻き戻し検証も成功。既定IMEやセッション全体の設定を変更しない。
常設インストールや入力先アプリでの実打鍵は未検証。詳細は[TSFスパイク](TSF_SPIKE.md)を参照。

選択範囲の編集基盤として `sumibi-tip::edit` を追加した。読取セッションでスナップショットを作り、
別の書込セッションでフォーカス・選択・UTF-16原文・ポリシー世代を再検査して一度だけ置換する。
同期セッションが拒否された場合は遅延再試行しない。戻り値だけでなくコールバック実行も確認する。
`edit-test` は専用のメモリー上のITextStoreACPを実TSFへ接続して23ケースを検証し、全件成功した。
`edit-test` の編集アダプターは検証EXEにRustライブラリとしてリンクされ、実TSF経由で動作する。
追加した `key-test` は実DLLの予約キーコールバックから編集を呼び、専用ホストの1回のUndoも確認する。
下線の表示属性プロバイダーも追加し、ロードexampleで実DLLの列挙・参照寿命を検証した。
`edit-test` には属性の部分範囲への付与・読戻し・解除、許可取消、ロック拒否の3ケースを追加した。
さらに開発用Previewでセッション間の下線属性保持・取消・確定を実装し、8ケースを追加した。
実DLLのBeginPreviewと予約キー確定、取消／許可取消時のロック拒否からの再試行も専用ホストで成功した。
原文・選択変更の実TSF通知でプレビューを無効化し、下線解除だけを非同期セッションへ送る。
古い解除要求が新しいプレビューへ作用しないよう世代を検査する。本文置換やキー再送は遅延実行しない。
Esc取消／Enter原文通過とフォーカス通知の処理は、検証用APIから実DLLのコールバックを直接呼んで確認した。
画面を持たない専用ホストでは通常キー配送とOnSetFocus通知が発生しなかった。
追加した `window-test` は一時ウィンドウを表示し、AssociateFocusとGetWndで文書を関連付ける。
この構成ではTestKeyDown/KeyDownからの通常キー配送と、実ウィンドウのフォーカス移動通知が成功した。
実行例: `cargo run -p sumibi-setup --locked -- window-test --dll target/debug/sumibi_tip.dll`。
対話デスクトップを使用し、検証中は短時間だけウィンドウがアクティブになる。CIの画面なしテストには含めない。
検証ホストの本文／下線描画も追加した。TSFの本文・属性を同期読取し、実DLLから取得した点線定義を
メモリー上のDIBへ描いてWM_PAINTで表示する。原文・プレビュー・取消・確定・Undoの5状態を確認した。
`target/window-test/` に同じ描画バッファのBMPを保存する。選択範囲外の下線画素が0であることも検査する。
ビューをITfTextEditSinkへ接続し、本文／属性変更の通知から表示を自動更新する。
通知ではWM_APPの更新要求を予約するだけにし、後のメッセージ処理で同期readセッションと描画バッファ更新を行う。
連続通知は集約し、読取失敗時は古い表示を消す。接続変更・終了後の予約は識別子で拒否する。
自動更新、読取失敗からの復帰、要求の集約、再接続とウィンドウ終了の検証が成功した。
専用デモの入力メッセージ処理も追加した。起動と操作は次節を参照。
通常アプリの入力スコープの機密性判定、実打鍵、Notepad等の実アプリでの表示・Undo・互換性は未実装／未検証。

## 入力デモの起動

リポジトリの作業ディレクトリで実行する。

```powershell
cargo build --workspace --locked --offline
cargo run -p sumibi-setup --locked --offline -- input-test --dll target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked --offline -- demo --dll target/debug/sumibi_tip.dll
```

`input-test` は自動で終了する。`demo` は入力できる1行の検証ウィンドウを開き、閉じると登録も解除する。
入力した本文はこの専用ウィンドウ内のメモリーだけで扱い、デモの本文を保存・送信しない。

| 操作 | 動作 |
| --- | --- |
| 文字入力 | 選択部分を置換、またはカーソル位置へ挿入 |
| Backspace | 明示選択、または書記素を削除。内部caretは含まれる書記素全体を対象。ホストのrepeatは選択を広げず1回Undo（Unicode 17.0.0） |
| Ctrl+A | 全文選択 |
| Home / End | 文頭 / 文末へカーソル移動 |
| ← / → | 書記素単位で移動。transaction-demoの記録内なら実DLLで追跡を保持。選択中は該当方向へcollapseして残りrepeat分を移動 |
| Shift + ← / → / Home / End | 選択の始点を保持して範囲を伸縮 |
| Delete | 明示選択、または直後の書記素を削除。transaction-demoの明示選択/記録内は実DLLの同期取引。内部caretは含まれる書記素全体を対象 |
| F6 | この入力欄を明示許可し、選択範囲を下線と自己所有compositionで保持。未選択なら全文を選択 |
| F7 | 固定文字列「日本語」へ変換 |
| Esc | 下線プレビューを取消 |
| Enter | 固定変換せず原文を保持して確認、カーソルを選択末尾へ移動 |
| Ctrl+Z | 直前の入力・削除・固定変換をUndo |
| Ctrl+Y | Undoした入力・削除・固定変換をRedo |

Undo/Redoは本文と選択範囲・選択の向きを一緒に復元する。Undo後に新しく入力・削除・変換すると、
古いRedo履歴を破棄する。カーソル移動とプレビューだけではRedo履歴を破棄しない。

カーソルと選択範囲も描画する。1024 UTF-16単位を上限とする固定幅の1行デモであり、
マウス選択・スクロール・通常editorのクリップボード操作はまだ扱わない。
専用ホストは左右移動・Shift選択・Backspace/Deleteを書記素単位で扱う。明示部分選択は勝手に拡張しない。
製品DLLの前方削除取引・任意位置でのcomposition継続・一般アプリの同じ動作は未対応。
自動検証では所有ウィンドウへテスト用メッセージを送る。実キーボード操作の手動確認はまだ行っていない。

2026-10-21、利用者のデモ試用で閉じるボタンによる停止が報告された。
修正後はWM_CLOSEで終了要求とwakeメッセージを記録し、外側のメッセージループでTSFを解除してから破棄する。
後始末が失敗してもウィンドウを残さず、失敗自体は記録する。`close-test`は通常・下線中・許可取消後・
後始末エラーの4ケースを、別threadからの同期SC_CLOSEと実GetMessageループで検証する。
その後の「×を押しても何も起こらない」報告について、STATICクラスが×の位置を
HTTRANSPARENTと判定する問題を再現した。専用クラスとDefWindowProcへ変更し、
各終了ケースで×の座標がHTCLOSEになることも検証する。ウィンドウはタスクバーとAlt+Tabに表示する。
2026-10-21、利用者から修正版の×で閉じられたと確認を得た。同じ実行は終了コード0で、
TSF/COM解除・DLLアンロード検証も成功。試用報告だけを個別キー操作の合格記録にしない。

## 検証用ビルドと依存

キー接続は `sumibi-tip` の開発用feature `spike` に限定する。`sumibi-setup` がこのfeatureを有効化するため、
workspaceビルドのDLLには検証用COMインターフェースが含まれる。このDLLを常設IMEとして配布しない。
`cargo build -p sumibi-tip --no-default-features` では検証用の許可APIやキー編集経路を含めない。
両構成でClippyを警告なしで通した。TIPの変換経路は、専用ホストが明示許可したcontextだけを対象にする。
入力デモの描画は、ホスト自身が所有する入力欄の本文を読む。

| 直接依存 | 固定版 | License | 公開MSRV |
| --- | --- | --- | --- |
| windows | 0.62.2 | MIT OR Apache-2.0 | 1.82 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 | 1.82 |

Microsoftのwindows-rsによる生成バインディングとCOM実装マクロを使用する。
`windows-core` の直接依存は `implement` マクロの生成コードに必要。
使用APIに対応するfeaturesだけを有効化する。依存の公開MSRVを本プロジェクトで検証したわけではない。
unsafeはCOM境界とロード検証用exampleに限定し、各呼出しに寿命とポインターの根拠を記載する。
登録検証EXEの `sumibi-setup/src/probe.rs` もOS境界としてunsafeを使用する。Coreには導入しない。
依存の更新継続性・配布サイズ・第三者ライセンス同梱は公開準備で再評価する。

根拠: [windows-rs](https://github.com/microsoft/windows-rs)、
[DllGetClassObject](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-dllgetclassobject)。

## アダプターとの契約と残作業

Coreは置換を実行しない。将来のTSFアダプターは、所有スレッドの書込編集セッション内で
対象全文を再取得し、`finish` に渡して、成功した本文を同じ編集セッション内で適用する。
返された本文を後の編集セッションへ持ち越さない。COM参照やcookieはCoreへ渡さない。
context・範囲の識別子と要求IDの一意性はアダプターの責務。再接続でも要求IDを再利用しない。
フォーカス変更、入力変更、切断、Esc、同意取消時は `cancel` を呼ぶ。

専用ホストで実APIなしの登録・表示・キー配送・期限付き遅延・Undoを検証した。
一般アプリでの打鍵・composition・標準Undo・保護入力判定の完了を意味しない。
Coreの待機入力行列・候補、TIPの通常入力との統合、製品用の常駐Host・発見は未実装。
型付きJSON・固定process認証・overlapped pipeと単発Hostを追加し、専用ACP storeへの配送まで検証した。
形式と異常系は[IPCフレーム層](IPC_PROTOCOL.md)を参照。
次の製品用実装は[後続作業](NEXT_STEPS.md)に分割する。実API通信はまだ導入しない。
現行構成と残る実装ゲートは[全体設計](TECHNICAL_OVERVIEW.md)、[実装照合表](IMPLEMENTATION_AUDIT.md)を参照。
