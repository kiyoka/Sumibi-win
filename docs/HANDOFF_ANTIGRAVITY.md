# Sumibi-win — Antigravityへの作業引継ぎ

作成日: 2026-11-01（Asia/Tokyo）

## 1. 最初に把握すること

現在の主作業は **Issue #4の実装継続**。直近は製品Hostの安全な発見・起動に向け、
署名・保護配置の診断、PEの静的依存、DLL公開表と転送参照の検査を進めていた。

**Issue #4は未完了。一般アプリで使えるIMEや製品Hostの完成とは扱わない。**
専用TSFホスト・合成入力・固定process handleを使う診断の成功と、製品の受入は区別する。
API set、全依存DLLの信頼、実際のloaderのbind先、安全なHost発見、一般アプリへの接続が残る。

本書は作成時点のスナップショット。再開時はソース、Git状態、利用環境を確認すること。
この引継ぎ書自体は作成時に未コミット・未pushであり、GitHubからcloneしただけでは含まれない。

## 2. リポジトリと作業環境

| 項目 | 値 |
| --- | --- |
| Repository | https://github.com/kiyoka/Sumibi-win |
| 作業ディレクトリ | `C:\Users\kiyok\OneDrive\ドキュメント\ChatGPT\Sumibi-win` |
| 現在のbranch | `codex/issue-4-protocol-session` |
| upstream | `origin/codex/issue-4-protocol-session` |
| origin | `https://github.com/kiyoka/Sumibi-win.git` |
| 引継ぎ基準HEAD | `cef6141` |
| Shell / target | PowerShell / `x86_64-pc-windows-msvc` |
| Rust / Cargo | ともに1.99.1（引継ぎ書作成時に確認） |
| toolchain | `rust-toolchain.toml`で`1.99.1-x86_64-pc-windows-msvc`を指定 |

直前のユーザー依頼で未pushの30コミットをpush済み。
引継ぎ書を作る前の作業ツリーはcleanで、ローカルHEADとorigin追跡refが一致していた。
GitHubのIssue/PR状態は今回APIで再照会していない。リポジトリ資料上の対象は
[Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4)と[draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
完了・merge済みと推定しないこと。

このCodex環境では`.git`の書込みや一部のWindows実process/pipe試験にsandbox外実行が必要だった。
これはGitHub認証の欠損とは別問題。直前のCLIによるpushは成功している。
別エージェント環境の権限・認証が同じとは限らないため、必要な時に確認する。

## 3. 先に読む資料

1. [ISSUE_4_PROGRESS.md](ISSUE_4_PROGRESS.md) — 受入条件、確認範囲、残る製品受入。
2. [DEVELOPMENT.md](DEVELOPMENT.md) — 実行コマンド、登録・診断の範囲。
3. [TECHNICAL_OVERVIEW.md](TECHNICAL_OVERVIEW.md) / [IMPLEMENTATION_AUDIT.md](IMPLEMENTATION_AUDIT.md) — 全体構成と実装照合。
4. [IPC_PROTOCOL.md](IPC_PROTOCOL.md) — 版・ID・政策・取消・接続の契約。
5. [NEXT_STEPS.md](NEXT_STEPS.md) — 全体の後続作業。長い履歴を含むため、最新ソースと進捗資料も併読する。
6. [ADR 0045](adr/0045-machine-registration-boundary.md) — 機械登録前の境界。
7. [ADR 0069](adr/0069-offline-authenticode-inspection.md)から[ADR 0081](adr/0081-bounded-export-forwarders.md) — 直近の署名・通信・Host・依存検査。

進捗資料の古い日付や件数は当時の記録。最新の全体テスト基準は533件（+4 ignored）。

## 4. 現在の実装を探す場所

| crate / ファイル | 役割 |
| --- | --- |
| `crates/sumibi-core` | 入力・変換・失効・待機・救済のCore。TSF/COMを持ち込まない |
| `crates/sumibi-protocol` | JSONメッセージ、版、要求ID、政策とsession状態 |
| `crates/sumibi-ipc` | 固定process認証、非同期pipe、worker、bootstrap、配置・画像診断 |
| `crates/sumibi-host` | 政策所有、Broker、設定・同意UI、初回Chat変換 |
| `crates/sumibi-tip` | TSF DLL。一般アプリ向けの製品入力経路は未完了 |
| `crates/sumibi-setup` | 専用TSFホスト、登録・復旧・Broker等の診断CLI |
| `crates/sumibi-credentials` | Windows資格情報マネージャーとの境界 |
| `crates/sumibi-winhttp` | WindowsネイティブHTTPSと非同期I/Oの寿命管理 |

直近の主要ファイル:

- [authenticode.rs](../crates/sumibi-ipc/src/authenticode.rs): オフライン署名検査と`VerifiedFile`。
- [development_placement.rs](../crates/sumibi-ipc/src/development_placement.rs): 保護配置の階層・file保持。
- [launch_probe.rs](../crates/sumibi-ipc/src/launch_probe.rs) / [launch_environment.rs](../crates/sumibi-ipc/src/launch_environment.rs): 停止起動診断と限定した子環境。
- [host_image.rs](../crates/sumibi-ipc/src/host_image.rs) / [host_imports.rs](../crates/sumibi-ipc/src/host_imports.rs): x64 PEと静的import。
- [dll_exports.rs](../crates/sumibi-ipc/src/dll_exports.rs): 公開名・序数・RVA・未解決forwarder。
- [export_resolver.rs](../crates/sumibi-ipc/src/export_resolver.rs): 明示した公開表間の有限forwarder追跡。
- [dependency_exports.rs](../crates/sumibi-host/tests/dependency_exports.rs): 実Hostと保持したsystem DLLの読取検査。
- [host_control.rs](../crates/sumibi-host/src/host_control.rs), [host_service.rs](../crates/sumibi-host/src/host_service.rs), [host_runner.rs](../crates/sumibi-host/src/host_runner.rs), [broker_boot.rs](../crates/sumibi-host/src/broker_boot.rs), [policy.rs](../crates/sumibi-host/src/policy.rs): Host寿命・ログオン限定Discovery待受ループ・停止調停・政策所有。
- [session.rs](../crates/sumibi-tip/src/session.rs), [keys.rs](../crates/sumibi-tip/src/keys.rs), [com.rs](../crates/sumibi-tip/src/com.rs): 一般アプリ内TIPの自律Host発見・セッション維持（TipHostSession）、非ブロッキング状態共有、安全なホットキー結合。
- [consent_task.rs](../crates/sumibi-host/src/consent_task.rs), [consent_window.rs](../crates/sumibi-host/src/consent_window.rs): 表示版へ結び付いた同意操作と常駐待受統合。
- [first_conversion.rs](../crates/sumibi-host/src/first_conversion.rs): 初回Chat変換の形式・応答検査。

## 5. 実装済みの範囲と、成功を広げて解釈してはいけない点

### Core / IPC / 専用TSF

要求ID・世代・取消・切断・再接続、有限容量、期限、固定kernel process handleによる
相互peer照合、所有STAへの非同期配送を実装し、実pipe・子process・専用TSFホストで検証している。
合成入力から別process Broker、localhost HTTP、TSF編集と1回Undoまでの専用診断もある。
このhandle受渡しや専用入力先を、一般アプリ向けの信頼できるHost発見経路と取り違えない。

### 通信・設定・同意

Windows資格情報の固定targetは`Sumibi-win/API-key/v1`。設定JSONにはendpoint/modelを保存し、
キーや永続同意は保存しない。キーの現在の有無・値は今回再確認していない。
Hostは送信未同意で開始し、同意操作は表示した設定版・認可版・接続先等へ結び付ける。
設定変更・資格情報変更・取消・期限切れを送信前と結果受入時に検査する。

ネイティブWinHTTPと初回Chat応答処理は実装済み。公開HTTPSでは合成本文・ダミーキーを使う
正常TLS送受信、不正応答、期限切れ・自己署名証明書拒否の検査がある。
**実キーによる実provider変換と一般アプリへの結果適用は製品受入未確認。**

`startup-check`、`settings-init`、`settings-window`、`host-window`、`run`（`host-run`）がある。
`run` は常駐サービスとしてコンソールシグナル（Ctrl+C）による停止調停とDiscovery待受ループを実行する。
`host-window` は送信管理画面とともにバックグラウンドDiscovery待受ループを実行する。
`settings-window`は単独編集画面で、変更は次回Host起動用。
`host-window`は同意と寿命の管理画面で、現在peerを公開・受付せず変換dispatchも行わない。
ログオン単位の単一起動枠はあるが、自動常駐・tray・製品発見は未実装。

### 署名・配置・停止起動

`VerifiedFile`は署名を検査した同じfileを、書込・削除を共有せず保持する。
Windowsのオフライン信頼検査だけではSumibiの正規署名者を認証しない。
署名者指紋の選定・保護・ローテーション、オンライン失効方針は製品設計に残る。

停止起動診断は`CREATE_SUSPENDED`の子を作り、再開せず終了させる。
作成時からkill-on-close jobへ所属させ、最大4件。終了未確認ならfile/配置/job/枠を保持する。
子環境はOS API由来の4変数のみ。リモート/Low label画像拒否とSystem32優先を設定し、
有効状態を子processから読み戻す。成功fixtureは署名済み`vswhere.exe`の一時コピー。
**画像パスの現在のfile ID照合は、実際にmapされた画像sectionのfile ID証明ではない。**
停止診断を再開するAPIや、これだけで製品Hostを信頼する経路は作っていない。

### 静的依存と公開表・転送追跡

今回のdebug Hostでは17種類のDLLと、重複除去後165件の静的参照を取得した。
DLL名は小文字化して統合し、関数名の大小文字は区別する。
Host profileはx64 PE32+に限定し、DLL取り違え、delay/CLR/bound import、不正範囲等を拒否する。
importはdescriptorごと4096件・全体8192件、関数名512byteまで。重複分も容量へ数える。

DLL公開表ではalias・序数・欠番・data・forwarderを区別する。
slot/名前65536件、全文字列4MiB等の上限があり、RVAは呼出可能性を保証しない。

`ExportCatalog`は呼出元が登録したmetadataだけを借用し、ファイルを自動取得しない。
128 module・16段まで追跡し、module名とslotで循環を検出する。
`MODULE.Symbol` / `MODULE.#ordinal`の限定形式で、追加dot等の曖昧な転送は拒否する。
`api-` / `ext-`は`ApiSetUnresolved`。同名の物理DLLとしてcatalogへ登録することも拒否する。

固定して開いたKernel32・KernelBase・Ntdllのmetadataで、HostのKernel32参照89件が
最終RVAへ到達した。うち4件は1段の転送。これはそのOS/debug buildの観測で、
全依存・署名・保護配置・実loaderのbind先の信頼を証明する結果ではない。

## 6. 次に進める作業

### 直近の継続候補: API setと依存DLLの信頼

1. [ADR 0081](adr/0081-bounded-export-forwarders.md)と`export_resolver.rs`の境界を把握する。
2. API setのOSに対応した解決方法を一次資料で調べ、必要な証拠・対応範囲・失敗時の扱いを設計する。
   API setを単なるファイル名と見なしたり、contract名からKernelBase等を推測したりしない。
   現時点では方式を決定していない。信頼できる方法を確立するまでは未解決の拒否を維持する。
3. 明示したmoduleと、保持した実DLLの同一性・署名・親配置の保護を結び付ける。
   metadata catalogを信頼の証明として使わず、必要なfile/配置のleaseを寿命中保持する。
4. Kernel32だけでなくHostの全静的依存・転送先を検査する。動的依存・manifest・検索順・
   更新時の条件も別に定義し、単に名前や公開関数が存在するだけで許可しない。

参考一次資料:

- [Windows API sets](https://learn.microsoft.com/en-us/windows/win32/apiindex/windows-apisets)
- [API set loader operation](https://learn.microsoft.com/en-us/windows/win32/apiindex/api-set-loader-operation)
- [PE Format](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)

### Issue #4全体で残る製品経路

- 製品署名者方針、信頼された保護配置Hostでの成功fixture、更新・復旧モデル。
- 実際にmapされた画像の同一性と依存物の条件を満たすHost起動・発見。
- 起動したHost、TIP processの固定と、認証されたbootstrap/pipe配布の製品接続。
- Host所有ループへの受付・dispatch・設定/資格情報更新通知の接続。
- 一般アプリの所有TSFへのキー配送・失効通知・肯定的な保護情報検査。
- Notepad等での実打鍵、1回Undo、別欄・同長別本文・読取不能時の拒否。
- 配置・署名・更新条件を満たした上での機械登録と登録取引の管理者側実ファイル検証。

診断の追加だけを最終ゴールにしないこと。製品経路の各ゲートを具体化し、
未達条件を説明できる状態で一般アプリ受入へ進める。

## 7. 検証の再現

すべてリポジトリrootで実行する。

```powershell
cd 'C:\Users\kiyok\OneDrive\ドキュメント\ChatGPT\Sumibi-win'
git status --short --branch
git log -3 --oneline
rustup show active-toolchain

# 通常の全体回帰。引継ぎ時の最終成功記録: 506 passed / 3 ignored
cargo test --workspace --locked --offline

# 関連crateの全targetを検査
cargo clippy -p sumibi-ipc -p sumibi-host -p sumibi-setup --all-targets --locked --offline -- -D warnings
cargo fmt --all -- --check

# PE/import/export/転送と実DLLの読取検査。コードをloadしない
powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\Test-HostImage.ps1

# 通常desktop tokenと署名済みvswhere fixtureが必要。停止中の子を作り、終了させる
powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\Test-SuspendedImageProbe.ps1
```

`Test-HostImage.ps1`はHost parser 13件、DLL公開表parser 8件、転送追跡7件、
実Host/実DLLの3件を実行する。通常のWindows x64で検証する。
一部のnative試験はAppContainer/elevated tokenではスキップするので、Rustの成功件数だけで
実process試験が成立したと判断しない。停止起動scriptはfixture/token不足のスキップを失敗扱いにする。

公開HTTPSを明示的に再確認する場合:

```powershell
# Internet通信あり。合成本文とダミーキーのみを送る
powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\Test-NativeHttps.ps1
```

この3件は通常の全体回帰ではignored。以前の明示実行では成功したが、最終506件の回帰では
再実行していない。`--offline`はCargo依存取得の抑止であり、プログラムの通信を禁止する指定ではない。

最新回帰の出力は、このworkspaceの`target/export-resolver-workspace-test.log`に残る。
失敗時の記録は`target/export-resolver-workspace-failure.log`。`target`はGit管理外なので、
別PCへのcloneには付いてこない。件数は各`test result`行を集計し、手で推計しないこと。

### 直近で修正した非同期テストの競合

`9d207d6`を参照。準備eventが通知済みでも、producerがstate lockを保持していれば、
非ブロッキングの`prepared_policy()`は一時的に`None`等になり得る。
成功側は元の期限内で`None`だけを再観測する。失敗側は通知後の入力拒否を確認し、
workerの終了を確認してから保存された正確なerrorを検査する。
通知済み・lock保持中の条件を決定的な単体検査にも追加した。
製品workerをブロッキングへ変更したり、エラー判定を広げて成功扱いにしたりした修正ではない。

## 8. 作業上の制約と注意

- TSF所有STA/callbackで署名・ディスク・ネットワークの待機を行わない。
- Ready前や保護条件不明の入力本文を取得・送信しない。未知条件は拒否する。
- 同意取消・政策変更・対象/選択/フォーカスの変更履歴を弱めない。変化して戻っても失効する条件を維持する。
- pipe名、PID、同じSID、同じファイル名や署名だけをHost認証にしない。固定したkernel handleと既存のpeer検査を維持する。
- 実キー・本文・資格情報をログや引継ぎ資料へ出さない。診断は合成入力・ダミーキーを使う。
- 実キーの変更、証明書storeの信頼追加、恒久HKLM/TSF登録、既定IME変更を、診断成功から自動的に実行しない。
- `spike` DLLを製品の常設IMEとして登録しない。workspaceビルドはSetup経由でspikeを含み得る。
  passive DLLが必要な検証では、資料の順序に従い`sumibi-tip --no-default-features`のビルドを最後に行い再検査する。
- ユーザーはWSLを必要としており、C:空き容量が少ないという経緯がある。削除・移動・大量の別構成ビルドを安易に行わない。
- OneDrive上のincremental cacheでhard link失敗→copyの警告が出る。既知の環境警告で、コードのClippy指摘とは区別する。
- 通常の読取・実装・必要な検証は進める。UACや実打鍵が必要になったら、具体的な成果物・操作・影響を用意して現在の操作可否を確認する。
- 当時の利用者希望は開発用自己署名と`D:\Program Files\Sumibi`への保護配置。
  過去の資料では保護配置TIPは自己署名未信頼で、製品Hostの信頼済み成功fixtureではない。
  この書類作成時には配置・資格情報・登録の実状態を再検査していない。
- 直前のpush依頼は既存30コミットまで完了した。この引継ぎ書や以後の新規変更をpushする際は、その時点の依頼範囲を確認する。

## 9. 直近コミット一覧

| Commit | 内容 |
| --- | --- |
| (今回) | 一般アプリ実機手動検証手順書（MANUAL_APP_VERIFICATION.md）の全面改訂とランブック整備 |
| `c195534` | ADR 0097: 設定画面（settings_window）への保護プロファイル選択 UI の追加 |
| `e724370` | ADR 0096: 保護プロファイル設定（settings.json）と Host/TIP 政策動的同期 |
| `5b35331` | ADR 0095: 一般アプリ向け二層保護プロファイル（Strict / Standard）と TSF 許容基準 |
| `d7ec411` | ADR 0094: Host 障害復旧（Self-Healing 再接続）の検証と一般アプリ TSF 保護診断の強化 |
| `94262f5` | ADR 0093: 製品 Host のバックグラウンド常駐起動オプション（`--minimized` / `--tray`） |
| `ea408dd` | ADR 0092: 常駐 Host のシステムトレイ（通知領域）統合と最小化常駐 |
| `d118bdc` | ADR 0091: 一般アプリ実機検証手順と非機密 TSF 保護診断の整備 |
| `1062e3c` | ADR 0090: 製品 Host エントリーのデフォルト GUI 起動と設定自動誘導 |
| `af76aca` | ADR 0089: 自律セッション（TipHostSession）によるエンドツーエンドTSF変換・Undo検証と開発統合ランナー |
| `b176593` | ADR 0088: メッセージ専用ウィンドウによる STA 非同期完了通知とテキスト自動適用 (AsyncDrain) |
| `cef6141` | ADR 0087: 一般アプリ内 TIP の自律 Host 発見・セッション維持と安全なキー変換結合 (TipHostSession) |
| `18763a7` | ADR 0086: 常駐 Host のログオン限定待受ループと停止調停 (HostService, host run CLI) |
| `a4fcf37` | ADR 0085: ログオン限定ブートストラップ待受による Host / TIP 相互発見・ハンドオフプロトコル |
| `7cf2295` | ADR 0083, ADR 0084: HostControl ワーカーループへの peer 受付と変換ディスパッチ統合、API set 境界定義 |
| `6bc5dce` | ADR 0082: 保持された実 DLL lease による静的依存エクスポート解決 (SystemModuleLease / SystemModuleCatalog) |
| `adba3c4` | 明示したDLL metadataの有限forwarder追跡。Kernel32の89参照を最終RVAへ照合 |
| `9d207d6` | 準備通知直後の非ブロッキングsnapshotに関する既存テスト競合を修正 |
| `a0931eb` | DLL公開表の限定読取とKernel32公開参照の照合 |
| `3e30115` | 静的importの関数名・序数、未bound thunk tableの検査 |
| `1cb9ff3` | 限定した子環境と画像読込mitigationの読み戻し |
| `5e6b59e` | Hostのx64 PEと静的DLL名を停止診断前に検査 |
| `2a97c42` | 署名済みファイルの停止起動・job終了・有限保持 |
| `81147d8` | 公開HTTPS検査と証明書エラー分類 |
| `7ce46d7` | ログオン単位のHost単一起動 |
| `b96d8a2` | 明示的なHost同意画面と終了調停 |

## 10. Antigravityへの開始指示例

```text
docs/HANDOFF_ANTIGRAVITY.md と参照資料を読み、作業を引き継いでください。
基準は codex/issue-4-protocol-session の adba3c4 です。
現在のGit状態と実装を確認し、Issue #4の残タスクを進めてください。
直近の候補はAPI setと全依存DLLの信頼された解決です。
metadata検査の成功を製品の起動許可や一般アプリ受入完了とは扱わず、既存の拒否条件を維持してください。
必要な検証を行い、変更点・結果・残る条件を日本語で報告してください。
```
