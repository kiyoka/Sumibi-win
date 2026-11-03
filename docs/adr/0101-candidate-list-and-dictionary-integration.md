# ADR 0101: 候補リストプレゼンター・キー選択と 1-step Undo 保証

2026-11-04 / [Issue #6](https://github.com/kiyoka/Sumibi-win/issues/6)
状態: 承認済みの開発方針。[ADR 0099](0099-candidate-list-and-dictionary-integration.md) および [ADR 0100](0100-local-dictionary-completion.md) に基づき、TIP 側での候補リスト状態管理、キーナビゲーション、確定置換、および 1-step Undo 仕様を決定する。

## 状況

[ADR 0099](0099-candidate-list-and-dictionary-integration.md) により複数候補の生成とパース、および [ADR 0100](0100-local-dictionary-completion.md) により中黒・Mozc・Sudachi のローカル補完パイプラインが定義された。
残る核心部分は、TIP（`sumibi_tip.dll`）側において、受信した候補リストを安全に提示し、ユーザーのキー操作で選択・確定・取消・Undo を行う状態管理である。

受入条件：
1. 候補取得だけでは本文を自動置換しない。
2. 候補表示中のナビゲーション（上下キー、数字キー直接選択、Enter 確定、Esc 取消）。
3. 確定置換時のみ正確な対象を再照合して置換する。
4. 確定後、1 回の Undo（`Ctrl+Z`）で候補選択前の本文へ戻ること。

## 決定

### 1. 候補セッション状態機械（`CandidateSession`）

TIP の STA スレッド上に候補セッション状態を定義する：

```rust
pub struct CandidateSession {
    /// 候補アイテム一覧（インデックス 0 は現在結果）
    pub candidates: Vec<CandidateItem>,
    /// 現在フォーカス中のインデックス (0 .. candidates.len() - 1)
    pub selected_index: usize,
    /// 候補表示中フラグ
    pub is_active: bool,
    /// 対象コンテキストおよび世代番号
    pub generation: u64,
}
```

### 2. キー入力ルーティング規則

候補表示中（`is_active == true`）のキーイベント処理：

| キー | 動作 | 備考 |
|---|---|---|
| **Down / Tab** | `selected_index = (selected_index + 1) % len` | 次の候補へ移動（循環） |
| **Up / Shift+Tab** | `selected_index = (selected_index + len - 1) % len` | 前の候補へ移動（循環） |
| **1 〜 9, 0** | 指定番号の候補（1-based）を選択して即時確定 | 1=Index 0, 2=Index 1, ..., 0=Index 9 |
| **Enter / Space** | 現在フォーカス中の候補で即時確定 | テキスト置換を実行し、候補表示を閉じる |
| **Esc** | 候補表示を取消（クローズ） | 本文は置換せず、現在結果を維持 |

### 3. STA 非同期ドレインでの確定置換と 1-step Undo

1. **確定置換**:
   - 選択された候補のテキスト（`item.text`）を取得。出所注釈は本文に一切含めない。
   - `DrainSink` へ確定テキストを渡し、TSF 編集セッション内で対象範囲を安全に置換。
2. **1-step Undo 保証**:
   - 置換直前の本文・選択位置を TextStore の Undo レコードへ単一トランザクションとして保存。
   - 置換直後に `Ctrl+Z` を押した場合、候補選択前の状態へ 1 回で確実に復元。

## 影響と検証

- `session-key-test` に再変換・候補一覧取得・キーナビゲーション・確定・Undo の E2E テストシナリオを追加。
- 実機 Live デモ画面に候補リストのグラフィカル表示（出所タグ付き）を追加し、手動キー操作（数字キー選択・Enter確定・Undo）を検証可能にする。
