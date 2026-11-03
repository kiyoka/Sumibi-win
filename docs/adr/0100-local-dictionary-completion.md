# ADR 0100: ローカル辞書補完（中黒・Mozc・Sudachi）と出所分離

2026-11-04 / [Issue #6](https://github.com/kiyoka/Sumibi-win/issues/6)
状態: 承認済みの開発方針。[ADR 0099](0099-candidate-list-and-dictionary-integration.md) に基づき、LLM 変換後の追加候補一覧に対するローカル辞書補完仕様を決定する。

## 状況

[Issue #6](https://github.com/kiyoka/Sumibi-win/issues/6) および [Mac版移植表](file:///c:/dev/Sumibi-win/docs/MAC_PARITY.md) では、LLM による追加候補取得に加えて、以下のローカル補完が求められている：
- 中黒（・）区切り補完
- Mozc 由来の記号・特殊文字補完
- Sudachi 由来の同音異義語・標準語彙補完
- 補完候補の出所分離、重複排除、最大件数制限
- API 失敗時のオフライン代替禁止（Fail-Closed）
- 元データのライセンス（Mozc: BSD-3-Clause, Sudachi: Apache-2.0）遵守

## 決定

### 1. 補完パイプラインと順序

候補一覧は以下の厳格な優先順序で統合される：

```text
[1] 現在結果 (Index 0: current_result)
  ↓
[2] LLM 追加候補 (Index 1..N: OpenAI API 生成結果)
  ↓
[3] 中黒（・）補完 (区切り記号の正規化)
  ↓
[4] Mozc 記号補完 (矢印、数学記号、括弧等)
  ↓
[5] Sudachi 同音異義語補完 (代表的漢字表記バリエーション)
```

- **最大件数**: 全体で最大 11 件（現在結果 1 件 ＋ 追加候補最大 10 件）。上限に達した時点で後続の補完は打ち切る。
- **重複排除**: 前段階（現在結果および LLM 候補）に既に存在する表記と完全一致する候補はスキップする。

### 2. 出所（Source Kind）の保持

各候補は出所メタデータを保持する：
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateSource {
    CurrentResult,
    Llm,
    Nakaguro,
    MozcSymbol,
    SudachiHomophone,
}
```
- UI 表示時には、出所に応じたラベルやツールチップを表示可能とする。
- テキスト置換時には、出所メタデータや注釈を一切含めず、純粋な本文テキストのみを置換する。

### 3. API 失敗時のオフライン代替禁止（Fail-Closed 原則）

- LLM API 通信が失敗（ネットワーク断、HTTP 500、タイムアウト、認証エラー等）した場合、**ローカル辞書だけで代替して候補を返さない**。
- API 失敗時は直ちにエラーとして処理を中断し、本文を維持する（Fail-Closed）。
- ローカル辞書は「API 成功時の追加候補の網羅性向上」のためにのみ適用される。

### 4. ライセンスと固定データ

- Mozc 記号データ（BSD 3-Clause）および Sudachi 語彙データ（Apache License 2.0）のライセンス条項を保持し、ドキュメントに明記する。
- 外部ファイルへの依存による実行時 I/O 障害や改ざんを防ぐため、厳選された高頻度語彙・記号テーブルをコンパイル時静的データとして埋め込む。

## 影響と検証

- 追加候補リストが LLM 応答だけでなく、日本語入力で頻出する記号や同音異義語を漏れなくカバーする。
- 単体テストにて、中黒補完、記号変換、同音異義語補完、重複排除、および出所タグ付けを網羅検証する。
