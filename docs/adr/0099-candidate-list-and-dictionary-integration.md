# ADR 0099: 再変換（追加候補）プロトコル・複数候補抽出および候補リスト設計

2026-11-04 / [Issue #6](https://github.com/kiyoka/Sumibi-win/issues/6)
状態: 承認済みの開発方針。[Issue #5](https://github.com/kiyoka/Sumibi-win/issues/5)（[PR #11](https://github.com/kiyoka/Sumibi-win/pull/11) / [ADR 0098](0098-manual-conversion-end-to-end-integration.md)）の手動変換縦断統合を基礎とし、再変換時の追加候補取得・抽出・提示仕様を決定する。

## 状況

[ADR 0098](0098-manual-conversion-end-to-end-integration.md) により、Unicode 17.0.0 準拠の 1,000 書記素上限、WinHTTP 通信、Windows 資格情報マネージャー、STA 非同期ドレイン（`DrainWindow`）および 1-step Undo を備えた初回手動変換の縦断パイプラインが完全成立した。
実機実 API（OpenAI `gpt-4o-mini`）での手動打鍵検証も成功している。

[Issue #6](https://github.com/kiyoka/Sumibi-win/issues/6) および [Mac版移植表](file:///c:/dev/Sumibi-win/docs/MAC_PARITY.md) における次の要件は、初回変換（1候補即時確定）に続く「**再変換（追加候補の要求・取得・選択）**」の実装である。
主な設計上の課題と境界条件は以下の通り：

1. **候補件数の仕様差の決定**:
   - Mac 実装の最大 12 件（現在結果＋最大 11 件）と SPEC 案（追加 10 件）の整理。
   - 現在結果（第1候補）を常に先頭（インデックス 0）に保持し、追加取得した候補（最大 10 件）を第 2 候補以降に配置して合計最大 11 件とする。
2. **取得時非置換原則**:
   - 初回変換とは異なり、追加候補は「取得しただけでは本文を自動置換しない」。
   - ユーザーが候補選択 UI（TSF UIElement / 非アクティブ候補ウィンドウ）で明示的に選択（数字キー `1`〜`9`, `0` または 上下キー＋Enter）したときのみテキストを置換する。
   - 取消操作（Esc キーまたはフォーカス喪失）時は、現在結果のまま候補表示を閉じ、本文は一切変更しない。
3. **1-step Undo の保証**:
   - 候補を選択して置換した後、`Ctrl+Z`（Undo）を押した場合は、候補選択前の状態（初回変換結果、または元の原文）へ確実に 1 回で復元できること。
4. **安全制約の堅持**:
   - Unicode 17.0.0 準拠、総合 1,000 書記素上限（`MAX_TOTAL_GRAPHEMES`）、制御文字排除、UTF-16 1,024 単位制限を適用。
   - 注釈（読みや品詞、出所等）は本文へ混入させず、候補文字列のみを安全に置換対象とする。

## 決定

### 1. 変換種別（`ConversionKind`）の導入

手動変換リクエストに変換種別を定義する：

- `ConversionKind::Initial`: 初回変換。1 候補のみを生成・抽出し、STA 非同期ドレインで即座に本文を確定置換する（既存動作）。
- `ConversionKind::Additional`: 再変換。元原文 `source`、現在結果 `current_result`、および `user_dictionary` をもとに追加候補を要求する。

### 2. 追加候補プロンプトとパース契約

1. **追加候補用プロンプト**:
   - システムプロンプト末尾（`SYSTEM_PROMPT_SUFFIX`）を追加候補用（`ADDITIONAL_SYSTEM_PROMPT_SUFFIX`）として分岐：
     ```text
     文脈に合う自然な日本語の変換候補を複数件（最大10件）作ってください。{"candidates":["候補1","候補2",...]}というJSONだけを返してください。
     JSON以外の説明やMarkdownのコードフェンスは返さないでください。
     ```
   - ユーザープロンプトには、`source`（元原文）と `current_result`（現在の変換結果）を明確に区分して含める。
2. **複数候補レスポンスパーサー（`response_candidates`）**:
   - 受信した JSON から `candidates: Vec<String>` を抽出。
   - 各候補文字列のトリム、制御文字排除、空文字除外、最大 1,024 UTF-16 単位検証を実施。
   - `current_result` と完全一致する重複候補を自動排除。
   - 最大件数（追加最大 10 件）で制限。
   - 返却リストは、先頭に `current_result`（第1候補）を確実に保持し、その後に LLM 追加候補を連結（合計最大 11 件）。

### 3. IPC メッセージとセッション状態の拡張

1. **型付きプロトコルの拡張**:
   - `Request` に `ConversionKind`（または `current_result` の有無）を含める。
   - `Response` に `candidates: Vec<String>` を含め、単一候補時（`candidates.len() == 1`）と複数候補時を統一的かつ安全に表現。
2. **TIP 側の候補保持と選択状態機械**:
   - `CandidateSession`: 受信した候補一覧、選択中インデックス（0〜N-1）、元の選択範囲、世代番号を保持。
   - 選択確定イベント（キー・クリック）で `DrainSink` へ確定指示をポストし、安全置換。

## 影響と検証

- 既存の初回変換（`session-key-test`、全 8 シナリオ）の動作は 100% 後方互換を維持。
- 追加候補の生成・抽出・重複排除を単体テストで網羅検証。
- TSF テストハーネスおよび Live デモで、再変換による候補一覧取得と選択置換、Undo 復元を検証可能にする。
