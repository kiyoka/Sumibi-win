# ADR 0105: Windows 向けワンクリックインストーラーと配布パッケージ設計

- **日付**: 2026-11-05
- **状態**: 採択 (Accepted)
- **対象**: Windows 向け配布インストーラーの作成
- **関連 ADR**:
  - [ADR 0045: マシン登録前にDLL配置と更新境界を確定する](0045-machine-registration-boundary.md)
  - [ADR 0103: Windows版の互換性マトリクス・配置・登録ライフサイクルと公開準備](0103-windows-compatibility-and-lifecycle.md)
  - [ADR 0104: キャレット直前打鍵記録による手動変換（方式B）とTSFキーイベント統合](0104-preceding-run-record-manual-conversion.md)

---

## 1. 背景と課題

Issue #8 において、TSF COM 登録、パッシブ打鍵記録による直前変換（`Ctrl+J`）、二層保護プロファイル、および `sumibi-setup.exe` による CLI 登録管理が成立した。

しかし、一般のエンドユーザーが GitHub Releases 等からダウンロードして直感的にインストール・利用開始するためには、手動でのコマンドライン操作や開発スクリプト実行を必要とせず、Windows 標準の操作性（ダブルクリックで UAC 昇格し自動完了する）を備えた配布用インストーラーパッケージが必要である。

---

## 2. 決定事項

### 2.1 配布形態（ポータブル・ワンクリック自己完結パッケージ）

外部の重厚なインストーラーフレームワーク（WiX や Inno Setup 等）に依存せず、Windows 標準環境（PowerShell 5.1+ / cmd.exe）のみで完結する自己完結型パッケージ（ZIP）を採用する。

- **アーカイブ名**: `Sumibi-v<Version>-windows-x64.zip`
- **チェックサム**: `SHA256SUMS.txt` による改ざん防止
- **パッケージ内部構造**:
  ```
  Sumibi-v0.1.0-windows-x64/
  ├── Install.cmd        (ダブルクリックで UAC 昇格し Install.ps1 を実行)
  ├── Install.ps1        (インストーラー本体: 配置・権限・登録・スタートアップ)
  ├── Uninstall.cmd      (ダブルクリックで UAC 昇格し Uninstall.ps1 を実行)
  ├── Uninstall.ps1      (アンインストーラー本体: 登録解除・停止・ファイル削除)
  ├── README.txt         (インストール・使い方・アンインストールガイド)
  └── bin/
      ├── sumibi_tip.dll (パッシブ本番リリース DLL / spike API 排除)
      ├── sumibi-host.exe(常駐サービス・GUI 設定マネージャー)
      └── sumibi-setup.exe(管理 CLI / TSF & COM 登録エンジン)
  ```

### 2.2 インストール処理フロー (`Install.ps1`)

1. **管理者権限の自動検証**: 非昇格時は `Install.cmd` 経由で UAC ダイアログを自動ポップアップ。
2. **実行中プロセスの安全停止**: `sumibi-host` が起動中の場合は安全に終了。
3. **配置と厳格なアクセス制御 (ACL)**:
   - 配置先: `C:\Program Files\Sumibi\bin\` および `versions\0.1.0\`
   - `Administrators` / `SYSTEM`: フルコントロール
   - `Users`: 読み取り・実行 (RX) のみ
   - `ALL APPLICATION PACKAGES` (`*S-1-15-2-1`): 読み取り・実行 (RX) を付与し、Windows 11 メモ帳等の AppContainer アプリからの安全なロードを保証。
4. **TSF / COM 登録**:
   - `sumibi-setup.exe install --dll <targetDll> --scope system` を実行。
   - `HKLM` COM クラス登録、TSF カテゴリ登録、および日本語プロファイル（`0x0411`）の自動有効化。
5. **スタートアップ自動常駐の登録**:
   - `HKLM\Software\Microsoft\Windows\CurrentVersion\Run` に `"C:\Program Files\Sumibi\bin\sumibi-host.exe" host-window --minimized` を登録。
6. **スタートメニューのショートカット作成**:
   - `C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Sumibi` に作成：
     - `Sumibi Settings` (API キー・モデル設定画面を開く)
     - `Sumibi Background Host` (トレイ常駐サービスを開く)
     - `Uninstall Sumibi` (アンインストーラーを起動)
7. **初回設定への誘導**:
   - インストール完了時、API キー設定画面（`settings-window`）の起動を選択可能。

### 2.3 ビルド順序と安全境界の保証 (`New-DistributionPackage.ps1`)

ワークスペース内の依存関係解決によって `sumibi-setup`（開発依存に `features = ["spike"]` を含む）が `sumibi_tip.dll` を上書きするリスクを完全に排除するため、以下のビルド順序を厳格に強制する：

1. **Step 1**: 先に `sumibi-host` および `sumibi-setup` を `--release` でビルド。
2. **Step 2**: その**後**に `sumibi-tip` を `--no-default-features --release` でビルド（パッシブ DLL を確定）。
3. **Step 3**: パッケージステージング直後に `sumibi-setup plan-system-dev --dll ...` を実行し、開発用エクスポートが存在しないことを自動検証。

---

## 3. 利点と効果

- **ユーザーフレンドリー**: `Install.cmd` をダブルクリックするだけで、管理者昇格・配置・IME 登録・スタートアップ設定が数秒で完了。
- **Fail-Closed 安全性の保証**: パッシブ性自動検査により、本番パッケージに開発用バックドアが混入する可能性を排除。
- **クリーンな削除**: `Uninstall.cmd` により、TSF 登録・スタートアップ・ショートカット・ファイルが完全にクリーンアップされ、個人設定（API キー）は保護。
