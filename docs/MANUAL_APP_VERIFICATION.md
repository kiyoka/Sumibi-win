# Sumibi-win 一般アプリ・実機手動検証手順書（Manual App Verification Runbook）

文書版: 2026-11-03 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) / [draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)
関連 ADR: [ADR 0091](adr/0091-manual-app-verification-and-diagnostics.md), [ADR 0092](adr/0092-system-tray-and-background-minimize.md), [ADR 0093](adr/0093-background-minimized-host-startup.md), [ADR 0094](adr/0094-host-self-healing-and-app-protection-diagnostics.md), [ADR 0095](adr/0095-app-protection-profiles-and-acceptance-criteria.md), [ADR 0096](adr/0096-protection-profile-configuration-and-dynamic-propagation.md), [ADR 0097](adr/0097-settings-window-protection-profile-ui.md)

本手順書は、Windows 実機デスクトップ環境において、常駐 Host（`sumibi-host.exe`）と TSF TIP（`sumibi_tip.dll`）を連携させ、一般アプリ（Notepad / メモ帳、RichEdit アプリ等）での動作、保護プロファイル切り替え、および保護判定（Fail-Closed / Inviolable Guards）を安全に手動検証するための公式ランブックです。

---

## 1. 原則と安全基準

1. **作業ツリー外への恒久登録禁止**:
   - [ADR 0045](adr/0045-machine-registration-boundary.md) に則り、`HKLM` やシステム共有領域への変更は一切行いません。
   - 登録は現在のユーザー（`HKEY_CURRENT_USER`）限定のマーカー付きキーのみを用い、検証終了後は必ず解除（`unregister-dev`）します。
2. **機密性の厳格な保護**:
   - デバッグ出力（`OutputDebugStringW`）には、固定の識別子・エラーコード（HRESULT）・非機密フラグ（動的/静的フラグのビットマスク値）のみを出力し、ドキュメントの本文テキストや入力文字は一切含めません。
   - 送信未同意時や保護条件不一致時は、入力本文を取得・送信せず即時拒絶（Fail-Closed）します。
3. **合成入力と安全なテストデータ**:
   - 機密文書や個人データを含む実ファイルではなく、テスト用の安全な文字列（例: `nihongo`）を使用してください。

---

## 2. 二層保護プロファイルと絶対的防御境界（アーキテクチャ概要）

[ADR 0095](adr/0095-app-protection-profiles-and-acceptance-criteria.md) 〜 [ADR 0097](adr/0097-settings-window-protection-profile-ui.md) に基づき、Sumibi-win は二層の保護プロファイルを提供します：

| プロファイル | 既定 | 許容される条件 | 主な対象アプリ | 挙動 |
|---|---|---|---|---|
| **Standard (標準)** | **Yes** | `TS_SS_REGIONS` (0x2), `IS_DEFAULT` (0), `IS_TEXT` 等 | Notepad（Windows 11 メモ帳）、RichEdit コントロール、一般エディタ | 選択テキストの取得・変換・自動置換（Undo 可能）を許可 |
| **Strict (厳格)** | No | `TS_SS_NOHIDDENTEXT` (0x8) 必須、`TS_SS_REGIONS` 拒絶、明示的 `IS_TEXT`/`IS_CHAT` 必須 | 高機密監査環境、厳格検証ストア | 一般アプリを `HiddenTextPossible`（0x80070005）で安全に拒絶 |

### 両プロファイル共通の「絶対的防御境界（Inviolable Guards）」
どちらのプロファイルを選択していても、以下の機密領域・異常状態では**無条件で即座に拒絶（Fail-Closed）**され、本文テキストの取得や外部送信は一切行われません：
1. **パスワード・PIN・Private**: `IS_PASSWORD` (31), `IS_PIN_NUMERIC` (42), `IS_PRIVATE` (32), `ES_PASSWORD`, `EM_GETPASSWORDCHAR != 0`, 空コンテキスト（`TF_E_EMPTYCONTEXT` / `E_NOINTERFACE`）。
2. **読み取り専用（ReadOnly）**: `TS_SD_READONLY` (0x1), `ES_READONLY`。
3. **複数選択・一時コンテキスト**: `TS_SS_DISJOINTSEL` (0x1), `TS_SS_TRANSITORY` (0x4)。
4. **空選択・置換文字**: 未選択（キャレットのみ: `E_ABORT`）、オブジェクト置換文字（`\u{fffc}`）、4096 単位超。
5. **送信未同意（Unconsented）**: Host 画面で同意が付与されていない場合、ホットキーは完全無視（スルー）されます。

---

## 3. 検証の事前準備

### ステップ 3.1: ビルド
PowerShell コンソールを開き、最新のバイナリをビルドします：
```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 build
powershell -ExecutionPolicy Bypass -File .\dev.ps1 build-tip
```

### ステップ 3.2: 開発用 COM 登録
現在ユーザー（HKCU）に `sumibi_tip.dll` の COM クラスを登録します：
```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 register-dev
```
> 期待される出力: `Current-user COM activation: OK (C:\dev\Sumibi-win\target\debug\sumibi_tip.dll)`

登録状態の確認：
```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 verify-com
```

### ステップ 3.3: デバッグログ監視の準備（推奨）
[Sysinternals DebugView](https://learn.microsoft.com/en-us/sysinternals/downloads/debugview) を起動するか、Visual Studio / WinDbg 等をアタッチして、`[Sumibi-TIP]` プレフィックスのログを監視します。
> **出力例（非機密ログ）**:
> `[Sumibi-TIP] Preserved key status: dyn=0x00000000, stat=0x00000002, denial=None`
> `[Sumibi-TIP] Preserved key compartments: kbd_disabled=-1, empty_ctx=-1`

---

## 4. 自動化された実機コンポーネント検証（Fast Verification）

実際のエディタを手動操作する前に、Windows 標準コンポーネントに対する自動検証を実行します。

### ステップ 4.1: Native Edit 自動検証（RichEdit & EDIT）
Windows 標準の `Msftedit.dll`（RichEdit）および `EDIT` コントロールを実機プロセス内に直接生成し、TSF 保護判定を網羅的に検証します：
```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 native-edit
```
> **期待される結果**:
> - `Native RichEdit Strict Profile`: 7 拒絶シナリオ（plain-text, rich-text, private, default-scope, password, readonly, readonly-restored）すべて安全拒絶。
> - `Native RichEdit Standard Profile`: 4 シナリオ完全合格（plain-text 置換成功、default-scope 置換成功、password 絶対拒絶、readonly 絶対拒絶）。
> - `Native EDIT`: 空コンテキストによる安全拒絶。

### ステップ 4.2: 自律セッション・キー変換・非同期ドレイン・Undo 検証
```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 session-test
```
> **期待される結果**: 全 5 シナリオ（未同意 Fail-Closed、自律発見・非同期ドレイン置換、1-step Undo、失効 Fail-Closed、クリーンアップ）が完全合格。

---

## 5. インタラクティブ手動検証手順（Desktop Verification）

### ステップ 5.1: Host サービスの起動と設定確認

1. **Host の起動**:
   別コンソールから Host を起動します：
   ```powershell
   .\target\debug\sumibi-host.exe
   ```
   （バックグラウンド最小化で起動したい場合は `.\target\debug\sumibi-host.exe --minimized` を指定します）

2. **設定画面での確認**:
   - 初回起動時またはトレイアイコン右クリック →「設定画面を開く」を選択します。
   - ホスト名、モデル、パスが適切に入力されていることを確認します（例: `localhost`, `mock-model`, `/v1`）。
   - **「保護設定」ラジオボタン**を確認します：
     - `◎ 標準 (Standard: Notepad等一般アプリ対応)`（既定値）
     - `○ 厳格 (Strict: 高機密監査環境)`
   - 必要に応じて API キーを入力して「キーを保存」を押します。
   - 「設定を保存」を押して設定を保存します。

3. **送信管理画面での同意付与**:
   - トレイアイコンをダブルクリック、または右クリック →「送信管理画面を開く」を選択します。
   - 「外部送信に同意する」チェックボックスを**オン**にします。
   - 画面が「一般アプリからの変換要求を待受中」となることを確認します。

---

### ステップ 5.2: Standard プロファイル（既定）での実機打鍵と変換・Undo

1. **Notepad（メモ帳）または RichEdit アプリ（WordPad 等）を起動**:
   - Windows 11 の標準「メモ帳」を新しく起動します。
2. **テスト文字列の入力と選択**:
   - テキストエリアに `nihongo` と入力します。
   - マウスドラッグまたは `Shift + Home` / `Shift + 矢印` で `nihongo` を全選択（反転表示）します。
3. **ホットキー押下**:
   - **Ctrl + J** を押下します。
4. **観測と確認**:
   - **期待される挙動**: 選択された `nihongo` が、設定されたモックまたは変換先からの結果（例: `日本語`）に自動置換されます。
   - **デバッグログの観測**:
     ```text
     [Sumibi-TIP] Preserved key status: dyn=0x00000000, stat=0x00000002, denial=None
     [Sumibi-TIP] Drain applied text for request_id=...
     ```
5. **1-step Undo の確認**:
   - **Ctrl + Z** を押下します。
   - **期待される挙動**: 置換された `日本語` が直前の `nihongo` へ復元されます。

---

### ステップ 5.3: Strict プロファイルへの動的切り替えと安全拒絶

1. **設定の切り替え**:
   - トレイアイコン右クリック →「設定画面を開く」。
   - 「保護設定」で **`◎ 厳格 (Strict: 高機密監査環境)`** を選択します。
   - 「設定を保存」をクリックします（アプリの再起動は不要です。Host から TIP へ動的に政策が同期されます）。
2. **Notepad での再打鍵**:
   - メモ帳で `nihongo` を選択した状態で **Ctrl + J** を押下します。
3. **観測と確認**:
   - **期待される挙動**: メモ帳のテキストは**一切変化しません**（不変）。
   - **デバッグログの観測**:
     ```text
     [Sumibi-TIP] Preserved key status: dyn=0x00000000, stat=0x00000002, denial=Some(HiddenTextPossible)
     [Sumibi-TIP] Preserved key capture rejected: HRESULT 0x80070005
     ```
   - Notepad は `TS_SS_NOHIDDENTEXT` を持たないため、Strict プロファイルの安全ポリシーに従って Fail-Closed（拒絶）されたことが確認できます。
4. **Standard への復帰**:
   - 設定画面で再度「標準 (Standard)」を選択して「設定を保存」することで、直ちに Notepad での変換が再度有効になります。

---

### ステップ 5.4: 絶対的防御境界（Inviolable Guards）の確認

プロファイル設定に関わらず、以下の機密・保護コンテキストでは絶対にテキストが取得・送信されないことを確認します：

1. **パスワード欄の絶対拒絶**:
   - 任意のアプリ（ブラウザのログイン画面、Windows 資格情報ダイアログ、パスワード入力欄等）のパスワード欄にフォーカスを当てます。
   - 文字列を入力・選択して **Ctrl + J** を押下します。
   - **観測**: テキストは置換されず、デバッグログに空コンテキストまたはパスワードスコープ拒絶が記録されます。
2. **読み取り専用（ReadOnly）の絶対拒絶**:
   - メモ帳等でファイルを「読み取り専用」として開くか、ReadOnly なビュー（ヘルプ画面やプレビュー画面）のテキストを選択します。
   - **Ctrl + J** を押下します。
   - **観測**: `denial=Some(ReadOnly)`（`TS_SD_READONLY`）により即座に拒絶され、テキストは不変です。
3. **空選択の絶対拒絶**:
   - テキストを選択せず、キャレット（点滅カーソル）のみの状態で **Ctrl + J** を押下します。
   - **観測**: 空選択（`E_ABORT`）として即座に拒絶され、送信処理は一切開始されません。
4. **送信未同意（Unconsented）の絶対遮断**:
   - Host の送信管理画面で「外部送信に同意する」チェックボックスを**オフ**（未同意）にします。
   - Notepad でテキストを選択して **Ctrl + J** を押下します。
   - **観測**: ホットキーはスルーされ、TSF 編集セッションすら開始されません（デバッグログ: `Preserved key ignored: host session is not ready`）。

---

### ステップ 5.5: システムトレイと常駐動作の確認

1. **最小化とトレイ格納**:
   - 送信管理画面の「×」閉じるボタン、または「−」最小化ボタンをクリックします。
   - ウィンドウがタスクバーから消え、タスクバー通知領域（システムトレイ）に Sumibi アイコンが存在することを確認します。
   - トレイ格納中もバックグラウンドで待受が継続し、Ctrl + J の変換が正常に動作することを確認します。
2. **トレイメニュー**:
   - トレイアイコンをダブルクリック → 送信管理画面が再表示される。
   - トレイアイコンを右クリック → ポップアップメニュー（「送信管理画面を開く」「設定画面を開く」「Host を終了」）が表示され、各項目が正常に動作する。

---

## 6. 検証後のクリーンアップ

検証が完了したら、開発用登録を解除してシステムを完全にクリーンな初期状態に戻します：

```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 unregister-dev
```
> **期待される出力**:
> `Current-user development COM registration removed: C:\dev\Sumibi-win\target\debug\sumibi_tip.dll`

Host プロセスの終了：
- システムトレイアイコンを右クリックし、「Host を終了」を選択します（または管理画面の終了操作）。
- バックグラウンドプロセスが安全に停止し、一時リソースが解放されたことを確認します。

これで実機検証手順は完了です。
