# 開発環境と現在の実装範囲

Issue #1の到達点と残る境界は[最小試作の受入記録](ISSUE_1_ACCEPTANCE.md)を参照。
まとめて検証する場合は通常ユーザーのPowerShellで `./tools/Test-Spike.ps1` を実行する。
途中失敗で停止し、既定では専用ウィンドウとログオンSID限定pipeも検証する。
画面を使用できない環境では `-SkipDesktop` を明示し、未実行の検査を成功扱いにしない。

Issue #4の型付きIPCとclient状態の純粋Rust検証は
`cargo test -p sumibi-protocol --locked --offline`で実行できる。
現時点はProtocol 22件（frame 6・接続状態8・JSON 4・Core模擬接続4）、IPCの境界・worker 6件、保護判定3件、composition列挙1件を含めworkspace合計40件。
JSON構造・固定エラー・世代・取消・再接続・期限・対象照合を扱い、実APIやOS peer認証は使わない。
形式・制限・残る製品接続は[IPC契約](IPC_PROTOCOL.md)を参照。

通常ユーザーの別プロセス・非同期I/O検証は `target/debug/sumibi-setup.exe ipc-peer-test`。
固定ハンドルへ相手を結び付け、トークン・PID・作成時刻を相互検査した後で模擬結果をCoreへ返す。
停止・期限・偽の相手・drop・容量・handle解放を確認する。起動方式と限界は[ADR 0003](adr/0003-pinned-desktop-peers.md)。
通常のデスクトップmedium integrity限定で、サンドボックスや昇格環境で拒否されてもACLを拡張しない。

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

ジョブ予約はprocess内4件、結果channelは各1件。I/O・子processの回収はworkerで行い、
TSFの所有loopがeventを受けて新しい同期write内でCore.finishとSetTextを行う。
詳細な所有・失効・起動の限定条件は[ADR 0004](adr/0004-one-shot-broker-and-owner-edit.md)を参照。
このbroker-testの編集アダプターはsetupへRustライブラリとしてリンクする。
次節のbroker-key-test/broker-demoは実DLLのキーとCore・編集を通す別検証である。
どちらも一般アプリの入力・標準Undoの確認には読み替えない。

Hostのspike featureは開発診断用で、配布しない。`cargo build --workspace`はHostを既定の拒否構成へ戻す。
`Test-Spike.ps1`は通常実行時だけこの診断Hostをビルドしてbroker-testを実行し、
`-SkipDesktop`ではbroker-testも明示的に省略する。

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

broker-key-testは実DLLの予約キーから17ケース、broker-input-testは描画・Undo・Enter・編集・
同期SC_CLOSE・WM_KILLFOCUS・空composition開始終了を含む8ケースが成功。いずれも自動終了する。前者のcapacity/Esc/Enterは
前面化に依存しない同じDLL callbackで確認し、物理キーボード操作の手動確認は含めない。
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
選択の寿命管理までで、通常打鍵の追跡やcompositionの範囲拡張は未実装。
条件とWindowsのsink参照残留の扱いは[ADR 0008](adr/0008-owned-selection-composition.md)、ログは `target/issue-4-owned-*.log`。

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
| Backspace | 選択部分、または直前のUnicodeスカラーを削除 |
| Ctrl+A | 全文選択 |
| Home / End | 文頭 / 文末へカーソル移動 |
| ← / → | Unicodeスカラー単位で移動。選択中は選択の左端 / 右端へ移動 |
| Shift + ← / → / Home / End | 選択の始点を保持して範囲を伸縮 |
| Delete | 選択部分、または直後のUnicodeスカラーを削除 |
| F6 | この入力欄を明示許可し、選択範囲を下線と自己所有compositionで保持。未選択なら全文を選択 |
| F7 | 固定文字列「日本語」へ変換 |
| Esc | 下線プレビューを取消 |
| Enter | 固定変換せず原文を保持して確認、カーソルを選択末尾へ移動 |
| Ctrl+Z | 直前の入力・削除・固定変換をUndo |
| Ctrl+Y | Undoした入力・削除・固定変換をRedo |

Undo/Redoは本文と選択範囲・選択の向きを一緒に復元する。Undo後に新しく入力・削除・変換すると、
古いRedo履歴を破棄する。カーソル移動とプレビューだけではRedo履歴を破棄しない。

カーソルと選択範囲も描画する。1024 UTF-16単位を上限とする固定幅の1行デモであり、
マウス選択・スクロール・クリップボード・書記素クラスタ単位の削除はまだ扱わない。
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
