# ADR 0095: 一般アプリ向け保護プロファイル（Strict / Standard）と TSF 許容基準の設計

2026-11-03 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) / [draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
状態: 承認済みの保護判定拡張アーキテクチャ設計。

## 状況

[Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) の受入条件には以下が定められている：
- 「Notepad等で実打鍵、1回Undo、別欄・同長別内容・読取不能の非適用」
- 「一般アプリの入力先ごとの保護情報、通知履歴、composition寿命、実キー配送」

現在、専用 TSF テキストストア（`session-key-test`）では自律セッション、変換、STA 非同期ドレイン、1-step Undo が完全に動作している。
しかし、Windows 標準の RichEdit コントロール（`Msftedit.dll`）および Windows 11 のメモ帳（RichEditBox 基盤）は、実測（`dev.ps1 native-edit`）において以下のフラグを報告する：
- `dwDynamicFlags = 0x00000000`
- `dwStaticFlags = 0x00000002` (`TS_SS_REGIONS`)
- `InputScope` は未設定（`VT_EMPTY`）または `IS_DEFAULT` (0)

現行の保護ゲート（`crates/sumibi-tip/src/protection.rs`）は極めて厳格（Fail-Closed）であり、`TS_SS_NOHIDDENTEXT`（0x8）の存在および `IS_TEXT`（1）/ `IS_CHAT`（26）/ `IS_CHAT_WITHOUT_EMOJI`（27）の明示的指定を必須としている。
そのため、標準の RichEdit やメモ帳は `HiddenTextPossible`（`0x80070005: E_ACCESSDENIED`）または InputScope 不一致として安全側に倒して拒絶される。

一方、AI による機密情報漏洩を防ぐため、パスワード欄（`IS_PASSWORD` / `ES_PASSWORD`）、PIN 入力欄、読み取り専用ドキュメント（`ReadOnly`）に対しては、いかなる場合も本文取得・送信を行わない「絶対的防御」を堅持する必要がある。

## 決定

安全性を一切妥協することなく、Notepad 等の一般的なテキストエディタで安全に変換と Undo を利用可能とするため、**二層の保護プロファイル（Protection Profile）アーキテクチャ**を策定する。

### 1. 二層の保護プロファイル設計

| プロファイル | 目的・対象環境 | 静的フラグ要件 | 許可 InputScope | 適用場面 |
|---|---|---|---|---|
| **Strict** (厳格) | 高機密・監査環境、専用検証 | `TS_SS_NOHIDDENTEXT` 必須、`TS_SS_REGIONS` 拒絶 | `IS_TEXT`, `IS_CHAT`, `IS_CHAT_WITHOUT_EMOJI` のみ | 既存の拒絶回帰テスト（`native-edit-test`）、専用検証環境 |
| **Standard** (標準) | 一般ユーザー・日常デスクトップ | `TS_SS_REGIONS` 許容（単一選択時）、`TS_SS_NOHIDDENTEXT` は任意 | `IS_DEFAULT` (0) を追加許容 | Notepad、各種テキストエディタ、一般的な文書作成 |

### 2. 両プロファイル共通の「絶対的防御境界（Inviolable Guards）」

プロファイルに関わらず、以下のいずれかに該当する場合は**無条件で即時拒絶（Fail-Closed）**し、本文テキストを一切読み取らない：

1. **パスワード・機密スコープの絶対拒絶**:
   - `InputScope` に `IS_PASSWORD` (31), `IS_PIN_NUMERIC` (42), `IS_PASSWORD_NUMERIC` (43), `IS_PRIVATE` (32) が 1 つでも含まれる場合。
   - Win32 コントロールのウィンドウスタイルに `ES_PASSWORD` が立っている場合。
   - `SendMessage(hwnd, EM_GETPASSWORDCHAR, 0, 0) != 0` の場合。
   - TSF コンテキストが空（`status` が `TF_E_EMPTYCONTEXT`、または `E_NOINTERFACE`）の場合。
2. **読み取り専用（ReadOnly）の絶対拒絶**:
   - `dwDynamicFlags & TS_SD_READONLY != 0` の場合。
   - Win32 コントロールのウィンドウスタイルに `ES_READONLY` が立っている場合。
3. **未検証・異常動的フラグの絶対拒絶**:
   - `dwDynamicFlags & TS_SD_LOADING != 0`（ロード中）。
   - `dwDynamicFlags & TS_SD_RESERVED != 0`（予約ビット）。
   - 未知の動的フラグビットが立っている場合。
4. **複数選択・一時コンテキストの絶対拒絶**:
   - `dwStaticFlags & TS_SS_DISJOINTSEL != 0`（非連続な複数選択）。
   - `dwStaticFlags & TS_SS_TRANSITORY != 0`（CUAS などの一時コンテキスト）。
5. **空選択・置換文字の絶対拒絶**:
   - テキストが選択されていない（キャレットのみ）場合（`E_ABORT`）。
   - 取得テキストにオブジェクト置換文字（`\u{fffc}`）が含まれる場合。
   - 選択単位数が上限（`MAX_SELECTION_UNITS = 4096` UTF-16 単位）を超える場合。
6. **送信未同意（Unconsented）の絶対遮断**:
   - ユーザーが明示的に送信同意を付与していない場合、ホットキーはスルーされ、TSF 読み取りセッションすら開始しない。

### 3. TSF 判定アルゴリズムの拡張設計

`crates/sumibi-tip/src/protection.rs` において、プロファイルに応じた分類関数を導入する：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectionProfile {
    Strict,
    Standard,
}

pub fn input_status_denial_with_profile(
    status: TS_STATUS,
    profile: ProtectionProfile,
) -> Option<InputStatusDenial> {
    use InputStatusDenial::*;
    ...
    if status.dwDynamicFlags & TS_SD_READONLY != 0 {
        Some(ReadOnly)
    } else if status.dwDynamicFlags & TS_SD_LOADING != 0 {
        Some(Loading)
    } else if status.dwDynamicFlags & TS_SD_RESERVED != 0 {
        Some(ReservedDynamicFlags)
    } else if status.dwDynamicFlags & !known_dynamic != 0 {
        Some(UnknownDynamicFlags)
    } else if status.dwStaticFlags & TS_SS_DISJOINTSEL != 0 {
        Some(MultipleSelections)
    } else if status.dwStaticFlags & TS_SS_TRANSITORY != 0 {
        Some(TransitoryContext)
    } else if status.dwStaticFlags & !supported_static != 0 {
        Some(UnknownStaticFlags)
    } else if profile == ProtectionProfile::Strict && status.dwStaticFlags & TS_SS_NOHIDDENTEXT == 0 {
        Some(HiddenTextPossible)
    } else if profile == ProtectionProfile::Strict && status.dwStaticFlags & TS_SS_REGIONS != 0 {
        Some(MultipleRegions)
    } else {
        None
    }
}
```

InputScope の分類：
- `Strict`: `matches!(*scope, IS_TEXT | IS_CHAT | IS_CHAT_WITHOUT_EMOJI)`
- `Standard`: `matches!(*scope, IS_DEFAULT | IS_TEXT | IS_CHAT | IS_CHAT_WITHOUT_EMOJI)`（かつ機密スコープの非存在を完全検証）

### 4. 設定（`settings.json`）および Host 政策との連携

- 設定 JSON（`%LOCALAPPDATA%\Sumibi\settings.json`）および `HostService` / `PolicyAuthority` の政策に `protection_profile: "standard" | "strict"` を保持（デフォルト: `standard`）。
- TIP セッション（`TipHostSession`）の `SharedSessionState` を通じて、Host から配布された保護プロファイルを STA スレッドにアトミック共有し、ホットキー押下時の判定に適用する。

## 影響と検証

- **既存テストの完全保護**:
  - `native-edit-test` は `ProtectionProfile::Strict` で実行され、7 つの拒絶シナリオ（RichEdit / EDIT の遮断）が 100% 維持・検証される。
- **Notepad 等の一般アプリの開通**:
  - `ProtectionProfile::Standard` において、Notepad やプレーンテキストモードの RichEdit でテキストを選択して Ctrl+J を押すことで、安全に変換要求がディスパッチされ、STA への非同期反映（`WM_SUMIBI_DRAIN`）および 1-step Undo が成立する。
- **機密保護の完全性**:
  - パスワード欄、PIN 入力、読み取り専用ファイル、未同意時の Fail-Closed は両モード共通で絶対に突破されない。
