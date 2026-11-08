# Sumibi-win 互換性マトリクスと保護入力仕様

本ドキュメントは、[Issue #8](https://github.com/kiyoka/Sumibi-win/issues/8) および [ADR 0103](adr/0103-windows-compatibility-and-lifecycle.md) に基づき、Windows版 Sumibi の OS・CPU・権限・アプリケーション種別・入力欄種別における対応範囲と保護動作を定義したものです。

---

## 1. 環境・アーキテクチャ互換性

| 項目 | サポート状況 | 詳細・前提条件 |
| :--- | :---: | :--- |
| **Windows 11 (64-bit)** | ✅ 完全対応 | 22H2 以降推奨。TSF 1.0 / 2.0、WinHTTP、資格情報ストア完備 |
| **Windows 10 (64-bit)** | ✅ 完全対応 | 21H2 以降。標準 TSF アーキテクチャで動作 |
| **x86_64 (AMD64)** | ✅ 完全対応 | ネイティブ 64-bit DLL インジェクション |
| **ARM64 (WoA)** | ⚠️ エミュレーション対応 | x64 エミュレーション下で動作（ネイティブ ARM64 版は将来対応） |
| **x86 (32-bit)** | ❌ 非対応 | 32-bit プロセス空間への DLL ロードはセキュリティ境界上除外 |
| **一般ユーザー権限** | ✅ 完全対応 | 日常の入力、Host 常駐、設定変更、資格情報保存 |
| **管理者権限 (UAC)** | ✅ インストール時のみ要求 | `%ProgramFiles%` への配置および `HKLM` レジストリ登録 |

---

## 2. アプリケーション別互換性表

| アプリケーション | プロファイル | 変換動作 | Undo (Ctrl+Z) | 備考 |
| :--- | :---: | :---: | :---: | :--- |
| **Windows メモ帳 (Notepad)** | Standard | ✅ 正常動作 | ✅ 1-step Undo | `TS_SS_REGIONS` 許容により自然な範囲変換が可能 |
| **RichEdit コントロール** | Standard | ✅ 正常動作 | ✅ 1-step Undo | WordPad、各種インストーラー、ダイアログ内エディタ等 |
| **Antigravity チャット入力欄** | Standard | 直前入力・選択範囲とも変換成功 | 直前入力は2回必要 | [PR #21 の実機確認](https://github.com/kiyoka/Sumibi-win/pull/21)。2026-11-08 に Antigravity で再度変換成功の報告あり。再試験した入力欄と Undo は未確認 |
| **Visual Studio Code** | Standard | 実機で変換成功 | 未確認 | 2026-11-08 にユーザー確認。エディタ／チャットの内訳と選択範囲の結果は未確認 |
| **Google Chrome アドレスバー** | Standard | 修正版で変換成功 | 未確認 | 2026-11-08 に `IS_SEARCH` を許容する修正版でユーザー確認。選択範囲は未確認 |
| **Google Chrome Web フォーム** | Standard | 実機で変換成功 | 未確認 | 2026-11-08 にユーザー確認 |
| **Google Chrome X の投稿欄** | Standard | 実機で変換成功 | 未確認 | 2026-11-08 にユーザー確認 |
| **Microsoft Edge アドレスバー** | Standard | 直前入力の変換・置換に成功 | 未確認 | 2026-11-08 に `IS_SEARCH` を許容する修正版で確認。選択範囲は未確認 |
| **Microsoft Edge Web フォーム** | Standard | 実機で変換成功 | 未確認 | 2026-11-08 に Edge を開き直して再試験し、変換・置換を確認 |
| **Windows Terminal** | Standard | Ctrl + J が改行になり、変換できず | — | 2026-11-08 にユーザー確認。Sumibi の変換キーが届くかは未調査 |
| **PowerShell（Windows Terminal 以外のホスト）** | Standard | 未確認 | 未確認 | コンソールホストを区別して検証する |
| **Windows エクスプローラー** | Standard | ✅ 正常動作 | ✅ 1-step Undo | ファイル名変更、アドレスバー入力 |

---

[Issue #34](https://github.com/kiyoka/Sumibi-win/issues/34) の診断では、Edge の
アドレスバーが検索入力スコープ `IS_SEARCH` を返し、入力範囲の取得前に
`E_ACCESSDENIED` で停止していた。Standard では `IS_SEARCH` を許容する。
Strict や機密スコープが混ざる入力欄では拒否する。

## 3. 入力欄種別と絶対防御境界 (Inviolable Guards)

Sumibi は、ユーザーのプライバシーと機密情報の漏洩を完全に防ぐため、以下の入力欄において「本文取得・外部送信」を**数学的・決定論的に完全遮断（Fail-Closed）**します。

| 入力欄種別 | 検出フラグ / メタデータ | 動作 |
| :--- | :--- | :--- |
| **パスワード / PIN 入力欄** | `TS_SS_PASSWORD`, `ES_PASSWORD`, `EM_GETPASSWORDCHAR` | **完全遮断**: 変換キーは無視され、通常打鍵のみを直接ドキュメントへ通します。本文の取得や外部送信は一切行いません。 |
| **読取専用 (ReadOnly)** | `TS_SD_READONLY`, `EM_SETREADONLY` | **完全遮断**: 変換置換を試みません。 |
| **短命の入力コンテキスト** | `TS_SS_TRANSITORY` | Strictでは拒否。Standardでは `TS_SS_NOHIDDENTEXT` も存在する場合のみ通常の保護検査へ進む。スコープ・合成中・フォーカス・原文一致・範囲の再検証は省略しない。 |
| **複数・不連続選択** | `TS_SS_DISJOINTSEL` | **完全遮断**: 不連続な選択範囲に対する誤置換を防止します。 |
| **他社製 IME の合成中** | 他 IME のアクティブな Composition 存在 | **完全遮断**: 他 IME の入力を横取り・破壊しません。 |

---

## 4. 変換機能の動作マトリクス

| 機能 | 起動トリガー | 動作条件 | キャンセル・破棄条件 |
| :--- | :--- | :--- | :--- |
| **手動変換 (Manual)** | `Ctrl + J` | ・有効な API キー設定済み<br>・手動同意（`manual_consent`）承認済み<br>・テキスト選択中、または普通にローマ字打鍵直後（キャレット直前の打鍵記録と原文の完全一致検証による自動選択変換 [ADR 0104](adr/0104-preceding-run-record-manual-conversion.md)） | 矢印キー移動、Esc キー、フォーカス喪失、記録不一致 |
| **追加候補リスト** | 手動変換後、候補リスト表示 | ・複数候補が存在する場合<br>・矢印キー/Tab で選択、数字キーで直結 | Esc キーで閉じる、1-step Undo で直前に復元 |
| **Decisions 自動変換** | 200ms タイピング休止 | ・有効な API キー設定済み<br>・独立同意（`auto_consent`）承認済み<br>・残余予算あり（10,000回上限内）<br>・判定閾値 0.5 以上 | **手動変換発火（最優先）**、通常打鍵継続、フォーカス喪失 |
