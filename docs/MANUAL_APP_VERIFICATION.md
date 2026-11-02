# Sumibi-win 一般アプリ・実機手動検証手順書

文書版: 2026-11-02 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) / [ADR 0091](adr/0091-manual-app-verification-and-diagnostics.md)

本手順書は、Windows 実機デスクトップ環境において、常駐 Host（`sumibi-host.exe`）と TSF TIP（`sumibi_tip.dll`）を連携させ、一般アプリ（Notepad や各種エディタ）での動作や保護判定（Fail-Closed）を安全に手動検証するためのガイドです。

---

## 1. 原則と安全基準

1. **作業ツリー外への恒久登録禁止**:
   - [ADR 0045](adr/0045-machine-registration-boundary.md) に則り、`HKLM` やシステム共有領域への変更は行いません。
   - 登録は現在のユーザー（`HKEY_CURRENT_USER`）限定のマーカー付きキーのみを用い、検証終了後は必ず解除（`unregister-dev`）します。
2. **機密性の厳格な保護**:
   - デバッグ出力（`OutputDebugStringW`）には、固定の識別子・エラーコード（HRESULT）のみを出力し、ドキュメントの本文テキストや入力文字は一切含めません。
   - 送信未同意時や保護条件不一致時は、入力本文を取得・送信せず即時拒絶（Fail-Closed）します。
3. **合成入力と安全なテストデータ**:
   - 機密文書や個人データを含む実ファイルではなく、テスト用の安全な文字列（例: `nihongo`）を使用してください。

---

## 2. 検証の事前準備

### ステップ 1: ビルド
PowerShell コンソールを開き、最新のバイナリをビルドします：
```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 build
powershell -ExecutionPolicy Bypass -File .\dev.ps1 build-tip
```

### ステップ 2: 開発用 COM 登録
現在ユーザー（HKCU）に `sumibi_tip.dll` の COM クラスを登録します：
```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 register-dev
```
> 出力例: `Current-user COM activation: OK (C:\dev\Sumibi-win\target\debug\sumibi_tip.dll)`

登録状態の確認：
```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 verify-com
```

### ステップ 3: デバッグ出力の監視（推奨）
Visual Studio、WinDbg、または [Sysinternals DebugView](https://learn.microsoft.com/en-us/sysinternals/downloads/debugview) を起動して、`[Sumibi-TIP]` プレフィックスのログを監視します。
PowerShell から簡易的に監視することも可能です。

---

## 3. Host サービスの起動と待受

別の PowerShell ウィンドウまたはエクスプローラーから、Host を起動します：
```powershell
.\target\debug\sumibi-host.exe
```

- **初回起動時**:
  設定ファイル（`%LOCALAPPDATA%\Sumibi\settings.json`）が未作成の場合は初期設定画面が開きます。
  設定完了後、または設定が存在する場合は送信管理画面（`consent_window`）が表示されます。
- **待受状態**:
  画面上に「一般アプリからの変換要求を待受中」と表示され、バックグラウンドで Discovery パイプの待受が行われます。
- **同意の付与**:
  画面上の「外部送信に同意する」チェックボックスをオンにし、同意を付与した状態にします（未同意時は Fail-Closed で一切変換されません）。

---

## 4. 一般アプリでの挙動確認

### 4.1. Fail-Closed（安全な拒否）の確認

1. **Notepad（メモ帳）または RichEdit コントロールを持つアプリ**を起動します。
2. テキスト欄に `nihongo` と入力します。
3. 文字列を選択（反転表示）した状態で、変換ホットキー **Ctrl + J** を押します。
4. **観測結果の確認**:
   - **RichEdit コントロール**:
     - `TS_SS_NOHIDDENTEXT`（隠しテキストなし保証）がないため、保護ゲートにより `HiddenTextPossible`（`0x80070005: E_ACCESSDENIED`）として即座に拒絶されます。
     - デバッグ出力: `[Sumibi-TIP] Preserved key capture rejected: HRESULT 0x80070005`
     - 本文の取得・送信は行われず、エディタの内容は完全に不変に保たれます。
   - **標準 EDIT コントロール**:
     - CUAS による `TS_SS_TRANSITORY` / `0x40000000` フラグにより、`TransitoryContext` / `UnknownDynamicFlags` として拒絶されます。
   - **未選択時**:
     - テキストを選択せずキャレットのみの状態で Ctrl + J を押した場合、空選択として即座に拒絶されます（`0x80004004: E_ABORT`）。

### 4.2. 正常系変換と Undo の確認（専用ハーネス / 適合コンテキスト）
肯定的な保護条件を満たすコンテキスト（`session-key-test` で実証された専用 TextStore 等）において：
1. テキストを選択して Ctrl + J を押下。
2. デバッグ出力: `[Sumibi-TIP] Preserved key conversion dispatched: request_id=1`
3. Host 側で変換完了後、STA に通知され自動置換: `[Sumibi-TIP] Drain applied text for request_id=1`
4. Ctrl + Z を押下することで、直前の状態（元本文）へ完全復元（1-step Undo）。

---

## 5. 検証後のクリーンアップ

検証が完了したら、必ず登録を解除してシステムを元の状態に戻します：

```powershell
powershell -ExecutionPolicy Bypass -File .\dev.ps1 unregister-dev
```
> 出力例: `Current-user development COM registration removed: C:\dev\Sumibi-win\target\debug\sumibi_tip.dll`

- Host ウィンドウを閉じます（またはプロセスを終了します）。
- レジストリ内に残存キーがないことを確認します。
