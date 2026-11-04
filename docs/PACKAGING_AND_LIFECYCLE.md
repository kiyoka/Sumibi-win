# Sumibi-win パッケージング・導入・更新・削除ガイド

本ドキュメントは、[Issue #8](https://github.com/kiyoka/Sumibi-win/issues/8) および [ADR 0103](adr/0103-windows-compatibility-and-lifecycle.md) に基づき、Windows版 Sumibi の製品ビルド、安全な配置、登録、更新、アンインストール手順を説明します。

---

## 1. ビルド方針（製品パッシブ DLL の生成）

本番配布用の `sumibi_tip.dll` は、開発用バックドア API（`spike`）を**完全にコンパイル除外**した「パッシブ DLL」としてビルドします。

```powershell
# 1. 製品版 パッシブ DLL のビルド (spike 除外)
cargo build --release -p sumibi-tip --no-default-features

# 2. 製品版 Host サービス / 管理 CLI のビルド
cargo build --release -p sumibi-host
cargo build --release -p sumibi-setup
```

### 事前検証 (Preflight Check)
インストーラーは、DLL に開発用エクスポート（`SumibiSpikeBodyReadCount` 等）が存在しないこと、および必要な COM エクスポート（`DllGetClassObject`, `DllCanUnloadNow`）が存在することを厳格に検査します。

---

## 2. ディレクトリ配置とアクセス制御 (ACL)

配置先ディレクトリは、管理者以外の一般ユーザーによる改ざんを完全に防ぐため、保護ディレクトリに配置します。

- **配置基準パス**: `C:\Program Files\Sumibi`
- **バージョン別配置構造**:
  ```
  C:\Program Files\Sumibi\
  ├── versions\
  │   └── 0.1.0\
  │       ├── sumibi_tip.dll       (パッシブ製品 DLL)
  │       └── receipt.json         (配置検証メタデータ)
  ├── bin\
  │   ├── sumibi-host.exe          (Host 常駐プロセス)
  │   └── sumibi-setup.exe         (管理 CLI)
  ```
- **アクセス権限**:
  - `Administrators` / `SYSTEM`: フルコントロール (F)
  - `Users`: 読み取り・実行 (RX) のみ（書き込み・変更禁止）

---

## 3. インストール手順 (Installation)

管理者権限で実行します。

```powershell
# 管理者 PowerShell で実行
.\target\release\sumibi-setup.exe install --dll .\target\release\sumibi_tip.dll --version 0.1.0
```

### 実行内容
1. `C:\Program Files\Sumibi\versions\0.1.0` ディレクトリを作成し、厳格な ACL を設定。
2. DLL をコピーし、SHA-256 チェックサムを記録。
3. `install_journal`（先行書き込みジャーナル）を開始。
4. COM クラス登録（`HKLM\Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}`）。
5. TSF 言語プロファイル登録（日本語 `0x0411`、`bEnabledByDefault: false`）。
6. TSF カテゴリ登録（Keyboard, Display Attribute）。
7. ジャーナルにコミット記録。

> [!NOTE]
> **既定 IME 非強制**: インストールを行っても Windows の既定 IME は変更されません。言語バーや `Win + Space` から「Sumibi」を選択して利用を開始します。

---

## 4. Side-by-Side 更新手順 (Update Without Lock Interruption)

既存のプロセスが旧バージョンの `sumibi_tip.dll` をロードしていても、プロセスを強制終了することなく安全に更新できます。

```powershell
# 新バージョン 0.1.1 への更新
.\target\release\sumibi-setup.exe update --dll .\new_artifacts\sumibi_tip.dll --version 0.1.1
```

### 実行内容
1. 新ディレクトリ `versions\0.1.1` に新 DLL を配置。
2. レジストリ（`InprocServer32` および TSF Profile）の参照先をアトミックに `0.1.1` に切り替え。
3. 新規に起動するアプリケーションは即座に新バージョンを使用します。
4. 既に起動しているアプリケーションはそのまま旧バージョンの DLL で安全に動作を継続し、プロセス終了時に解放されます。

---

## 5. アンインストール手順 (Uninstallation)

```powershell
.\target\release\sumibi-setup.exe uninstall
```

### 実行内容
1. TSF カテゴリおよび言語プロファイルの登録解除。
2. COM クラス登録の解除。
3. ジャーナルの完了記録。
4. ファイル・ディレクトリの削除。
   - 使用中でロックされている DLL がある場合は、再起動時削除（`MoveFileExW`）をスケジュール。
5. **ユーザー設定の保護**: `%LOCALAPPDATA%\Sumibi\settings.json` および資格情報ストアの API キーは保持されます（明示的な `--purge` オプション指定時のみ全削除）。
