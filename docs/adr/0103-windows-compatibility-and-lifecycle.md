# ADR 0103: Windows版の互換性マトリクス・配置・登録ライフサイクルと公開準備

- **日付**: 2026-11-04
- **状態**: 採択 (Accepted)
- **対象 Issue**: #8 (Windows版の互換性・署名・導入更新削除を検証して公開準備する)
- **関連 ADR**:
  - [ADR 0001: 実現言語とTSFの構成](0001-language-and-ime.md)
  - [ADR 0045: マシン登録前にDLL配置と更新境界を確定する](0045-machine-registration-boundary.md)
  - [ADR 0066: 登録操作の先行記録と中断後の復旧](0066-registration-journal-and-recovery.md)
  - [ADR 0067: 隔離環境でのネイティブ登録バックエンド](0067-isolated-native-registration-backend.md)
  - [ADR 0094: Host自律再接続・一般アプリ保護診断](0094-host-self-healing-and-app-protection-diagnostics.md)
  - [ADR 0095: 一般アプリ保護プロファイルと受入判定](0095-app-protection-profiles-and-acceptance-criteria.md)
  - [ADR 0098: 手動変換エンドツーエンド統合](0098-manual-conversion-end-to-end-integration.md)
  - [ADR 0101: 候補リスト・辞書補完・1-step Undo](0101-candidate-list-and-dictionary-integration.md)
  - [ADR 0102: Decisions 自動変換・独立同意・固定窓・費用保護設計](0102-decisions-auto-conversion-and-cost-protection.md)

---

## 1. 背景と目的

Issue #1 から Issue #7 までの実装を通じて、Sumibi-win は以下のコア機能を成立させた：
1. **TSF TIP**: 非同期 STA ドレインウィンドウ（`WM_SUMIBI_DRAIN`）、一般アプリ向け二層保護プロファイル（Standard / Strict）、パスワード・ReadOnly・一時コンテキスト等の絶対防御境界（Fail-Closed）。
2. **Host / Broker**: ログオン限定相互認証パイプライン、WinHTTP ネイティブ HTTPS 転送、設定GUI・タスクバー最小化常駐（TrayIcon）。
3. **変換パイプライン**:
   - 手動変換（`Ctrl+J`）
   - 追加候補・ローカル辞書補完（候補一覧ポップアップ、数字選択、1-step Undo）
   - Decisions 自動判定（独立同意、200ms 固定窓、10,000回費用保護上限、手動優先）

これらの機能が完成した段階で、Windows OS への正式な組み込み、安全な配置、更新・アンインストール、および互換性境界を確定し、公開準備を整える必要がある。

---

## 2. 決定事項

### 2.1 互換性マトリクス (Compatibility Matrix)

| 区分 | 対象 / 条件 | 動作仕様 | 根拠・制約 |
| :--- | :--- | :--- | :--- |
| **OS** | Windows 11 x64 (22H2 以降)<br>Windows 10 x64 (21H2 以降) | 完全サポート | TSF 1.0/2.0 API、Windows 資格情報マネージャー、WinHTTP |
| **CPU アーキテクチャ** | x86_64 (AMD64) | ネイティブ対応 | 64-bit プロセス空間へのインジェクション |
| **CPU アーキテクチャ** | ARM64 | x64 エミュレーション下で動作確認 | ネイティブ ARM64 バイナリは将来の別 Issue で提供 |
| **CPU アーキテクチャ** | 32-bit (x86) | 非サポート（明確に拒絶） | 32-bit プロセス空間へのインジェクションは行わない（ADR 0045） |
| **実行権限** | 一般ユーザー権限 | 日常利用（Host常駐、TIP入力） | 設定ファイル（`%LOCALAPPDATA%`）、資格情報ストア |
| **実行権限** | 管理者権限（昇格） | インストール・更新・アンインストール時のみ要求 | `%ProgramFiles%` 配置、`HKLM` 登録 |
| **アプリ: 一般テキスト** | メモ帳 (Notepad), RichEdit, WordPad | 完全サポート (Standard プロファイル) | `TS_SS_REGIONS` 許容、1-step Undo、手動/自動変換 |
| **アプリ: エディタ/ブラウザ** | VS Code, Chrome, Edge | 完全サポート | 通常打鍵即時反映、選択置換、候補ナビゲーション |
| **アプリ: ターミナル** | Windows Terminal, PowerShell | 選択的打鍵反映 | コンソールホストの特性に応じた標準入力 |
| **入力欄: パスワード/PIN** | `IS_PASSWORD`, `ES_PASSWORD`, `EM_GETPASSWORDCHAR` | **完全遮断 (Fail-Closed)** | 本文取得・送信を一切行わない、通常打鍵のみ通過 |
| **入力欄: 読取専用** | `TS_SD_READONLY`, `EM_SETREADONLY` | **完全遮断 (Fail-Closed)** | 変換置換を試みない |
| **入力欄: 一時コンテキスト** | `TS_SS_TRANSITORY` | **完全遮断 (Fail-Closed)** | 一時入力中のキャプチャ禁止 |
| **サンドボックス** | AppContainer (UWP / Windows Store App) | 境界分離 | AppContainer 内からの Host パイプ接続は個別分離（一般化しない） |

---

### 2.2 配置ディレクトリとアクセス制御 (Directory Layout & ACLs)

DLL が任意のプロセスにロードされる TSF の特性上、一般ユーザーによる書き換えが可能なディレクトリ（作業ツリーなど）を登録先にすることは厳禁である（DLL 植え込み・乗っ取り攻撃の防止）。

- **インストール基準パス**: `%ProgramFiles%\Sumibi`
- **バージョン別 Side-by-Side 構造**:
  ```
  %ProgramFiles%\Sumibi\
  ├── versions\
  │   ├── 0.1.0\
  │   │   ├── sumibi_tip.dll       (パッシブ製品ビルド・非spike)
  │   │   ├── sumibi_tip.manifest  (SHA-256 & 署名情報)
  │   │   └── receipt.json         (配置領収証)
  │   └── 0.1.1\
  │       ├── sumibi_tip.dll
  │       └── ...
  ├── bin\
  │   ├── sumibi-host.exe          (Host 常駐サービス / トレイ UI)
  │   └── sumibi-setup.exe         (管理 CLI / インストーラー)
  ├── var\
  │   └── journal\
  │       └── registration.journal (先行書き込み登録ジャーナル)
  └── current -> versions\0.1.0    (アクティブバージョンへのシンボリックリンクまたはパス解決)
  ```
- **ACL 要件**:
  - 所有者: `NT AUTHORITY\SYSTEM` または `BUILTIN\Administrators`
  - `BUILTIN\Administrators`: フルコントロール (F)
  - `NT AUTHORITY\SYSTEM`: フルコントロール (F)
  - `BUILTIN\Users`: 読み取りと実行 (RX) のみ。**書き込み・変更権限は一切付与しない**。

---

### 2.3 TSF 登録と「既定 IME 非強制」原則 (TSF Registration)

- **CLSID**: `CLSID_SUMIBI_TIP` (`{E927D4AB-2EC9-4DD3-A921-A4620B95A410}`)
- **Profile GUID**: `{42e4ca10-7cae-4a74-bf57-d497c5667a2c}`
- **言語 ID**: `0x0411` (日本語)
- **登録 API**:
  - `ITfInputProcessorProfileMgr::RegisterProfile`
  - `bEnabledByDefault`: **`false`**
  - Flags: `0`（製品登録。開発用 `TF_RP_LOCALPROCESS` や `TF_RP_HIDDENINSETTINGUI` は付けない）
- **カテゴリ登録**:
  - `GUID_TFCAT_TIP_KEYBOARD`
  - `GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER`
- **既定 IME 非強制の原則**:
  - インストール時に Windows の既定 IME（Microsoft IME 等）を強制的に上書き変更しない。
  - ユーザーが言語設定（または `Win + Space`）で Sumibi を明示的に選択して初めてアクティブになる。

---

### 2.4 使用中 DLL を強制置換しない Side-by-Side 更新機構 (Update & Rollback)

Windows ではプロセスにロードされている DLL ファイルは OS によって共有ロックされ、削除や上書きができない。他社製 IME でよく見られる「インストーラーが使用中プロセスを強制終了する」アプローチは、ユーザーの編集データ損失を引き起こすため採用しない。

1. **新バージョンの配置**:
   - 新バージョンは別ディレクトリ `%ProgramFiles%\Sumibi\versions\<new_version>\` に配置する。
   - 既存のプロセスが旧バージョンの DLL を使用していても一切干渉しない。
2. **アトミックなレジストリ切り替え**:
   - `InprocServer32` および TSF Profile の参照先 DLL パスを、先行ジャーナル（WAL: Write-Ahead Log）を用いてアトミックに新バージョンパスへ更新する。
   - 新規に起動したプロセスは新バージョンの DLL をロードする。
   - 既存プロセスはそのまま旧バージョンの DLL で正常動作を継続し、プロセス終了時に解放される。
3. **失敗時の自動ロールバック**:
   - レジストリ更新またはプロファイル検証に失敗した場合は、ジャーナルに基づいて直前の正常バージョンに即時ロールバックする。
4. **旧バージョンのクリーンアップ**:
   - 次回起動時またはアンインストール時に、どのプロセスからもロックされていない旧バージョンディレクトリを安全に削除する。

---

### 2.5 削除（アンインストール）と設定保護 (Uninstall)

- **アンインストール手順**:
  1. TSF カテゴリ登録の解除 (`UnregisterCategory`)
  2. TSF 言語プロファイルの解除 (`UnregisterProfile`)
  3. COM クラス登録の解除 (`HKLM\Software\Classes\CLSID\{...}`)
  4. ジャーナルの完了記録
  5. 実行ファイルおよび DLL の削除（使用中でロックされている場合は次回再起動時削除 `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` を予約）
- **ユーザー設定の保護**:
  - `%LOCALAPPDATA%\Sumibi\settings.json` および資格情報ストアの API キーは、ユーザーの資産としてアンインストール時にも既定で保持する（明示的なクリーン削除オプション指定時のみ削除可能）。

---

### 2.6 製品用パッシブ DLL の厳格な分離 (Non-Spike Release Build)

- 開発用機能である `sumibi_tip::spike`（`ISpikeInput`, `ISpikeBroker`, `SumibiSpikeBodyReadCount` 等のバックドア的検査インターフェース）は、製品ビルドから完全にコンパイル除外する。
- **ビルド条件**:
  ```powershell
  cargo build --release -p sumibi-tip --no-default-features
  ```
- インストーラー（`sumibi-setup`）は、DLL 読み込み検査時に `SumibiSpikeBodyReadCount` エクスポートが存在する場合はインストールを拒絶する（Fail-Closed）。

---

### 2.7 プライバシー・ライセンス・再現可能なビルド

- **プライバシー**:
  - オプトイン原則: 初期状態では API キー未設定かつ手動/自動変換ともに未同意（Fail-Closed）。
  - 送信前の厳格な保護入力検査（パスワード、PIN、ReadOnly、一時入力の完全遮断）。
  - 秘密鍵（API キー）はプロセス間通信で TIP 側には一切渡さず、Host 内にのみ封じ込める。
- **ライセンス**:
  - Apache-2.0 / MIT デュアルライセンス。
  - 第三者ライセンス一覧（`cargo about` / `THIRD_PARTY_LICENSES.md`）の同梱。
- **再現可能なビルド**:
  - `Cargo.lock` による全依存クレートのバージョン固定。
  - MSVC ツールチェーンによる決定論的リンク。

---

## 3. 受入判定

1. 既定 IME を強制変更しないこと。
2. 開発用 spike API を含む DLL を製品登録できないこと。
3. 一般アプリ（Notepad / RichEdit 等）での安全な動作（手動変換、候補選択、自動判定、1-step Undo）が実機検証できること。
4. インストール・更新・アンインストールの各段階でジャーナルが正常に機能し、不整合を残さないこと。
