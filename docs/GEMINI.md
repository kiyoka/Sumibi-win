# Gemini API による日本語変換

Issue #22。macOS 版と同じ Google の OpenAI 互換 API を利用する。
設定画面で OpenAI／Google Gemini／その他（互換 API）を選択できる。
プロバイダーは保存された接続先から判定し、既存のカスタム接続先やモデルを
画面を開くだけで上書きすることはない。

## 設定

1. Sumibi の設定画面で「Google Gemini」を選ぶ。
2. 次の接続先とモデルを確認し、「接続先とモデルを保存」を押す。
3. Google AI Studio で取得した Gemini API キーを入力し、「Gemini API キーをこのPCに保存」を押す。
4. Host を終了して開き直し、送信先とモデルを確認して手動送信を許可する。
5. メモ帳で Sumibi を選び、`nihongo` を入力して Ctrl + J を押す。

| 項目 | 値 |
| --- | --- |
| ホスト名 | `generativelanguage.googleapis.com` |
| パス | `/v1beta/openai` |
| 保存後の完全パス | `/v1beta/openai/chat/completions` |
| モデル初期値 | `gemini-3.6-flash`（編集可能） |
| 接続 | HTTPS、443 番ポート |

モデルの利用可否・制限は Google 側のアカウントと提供状況による。
Google Gemini を選ぶと、モデル欄の一覧に `gemini-4-argon`、
`gemini-3.6-flash` の順で表示される。Argon は精度重視。
3.6 Flash は速度と費用を重視した既定値。モデル欄には任意の ID も入力できる。
[Sumibi のベンチマーク記事](https://zenn.dev/kiyoka/articles/japanese-input-method-sumibi-35)
で報告された 3.8 Flash の説明・候補列挙の付加と応答時間を考慮し、
初期値には 3.6 Flash を採用する。保存済みのモデルは自動変更しない。
手動変換、選択範囲の変換、追加候補は既存の Chat Completions 経路を利用する。
Decisions 自動変換の送信許可と予算は手動変換と独立しており、
Google を選ぶだけで自動変換を許可することはない。

Google の標準有料枠の料金（2026-11-07 確認、USD／100万トークン）。

| モデル | 現行の入力／出力 | 改定日 | 改定後の入力／出力 |
| --- | --- | --- | --- |
| Gemini 4 Argon | $2.00／$10.00 | 2026-12-01 | $4.00／$20.00 |
| Gemini 3.6 Flash | $0.75／$3.75 | 2027-01-01 | $1.50／$7.50 |

出力料金には思考トークンが含まれる。Windows 版の Decisions 予算は送信件数を
数える方式で、トークン単価を使った費用計算は行わない。実際の請求条件は
[Google Gemini API の公式料金表](https://ai.google.dev/gemini-api/docs/pricing)を参照。

## キーと既存設定

キーは Windows 資格情報マネージャーに、スキーム・ホスト・ポート・完全パスごとに
保存する。モデルだけの変更では同じ接続先のキーを使える。
設定 JSON、ログ、TIP への IPC にキーは書かない。
未保存の接続先ではキーの入力・保存・削除を無効にする。
「API キーを削除」は画面の保存済み接続先のキーだけを削除する。
接続先の変更時はメモリー上の旧キーと同意を破棄する。
モデルの変更時も手動・自動の同意を失効させ、古い応答は適用しない。
別サービスのキーを自動的に引き継ぐ処理はない。

旧 version 1 設定は、Host 起動時または設定画面を開く際に version 2 へ移行する。
元の設定を検証し、編集用のプロセス間ロックを取得してから、共通保存されていた
旧キーを元の接続先へ移す。移行先にキーがある場合は上書きしない。
保存に成功した後に旧キーを削除し、設定の版を更新する。
失敗時は設定を version 1 のまま残して起動・編集を止め、再試行可能にする。
ホスト、ポート、パス、モデル、保護設定は保持し、送信同意は復元しない。
version 2 を読めない旧ビルドへ戻す場合は、旧形式の設定とキーの再登録が必要になる。

## 検証

自動テストでは Google のベースパス・完全パス・末尾スラッシュ、モデルを含む
初回／追加候補リクエスト、互換応答、接続先別のキー識別、切替時のキー失効、
旧設定の移行失敗・再試行・一度だけの移行、設定画面の復元を確認する。
既存の WinHTTP テストで Bearer 認証、リダイレクト拒否、HTTP エラー、
タイムアウトを検証する。CI に実 API キーは不要。

2026-11-06、実機で Google Gemini の接続先と API キーを保存し、Host を
起動して手動送信を許可した（初期値を 3.6 Flash に変更する前の確認）。
ユーザー操作により、メモ帳で `nihongo` →
Ctrl + J が「日本語」だけになることを確認した。
Antigravity の空のチャット入力欄でも、同じ操作で変換できることを
ユーザー操作により確認した。

その後、保存済みモデルを `gemini-3.6-flash` に変更して Host を再起動し、
Antigravity と ChatGPT アプリの両方で変換できたことをユーザーが確認した。

以下は引き続き実機確認が必要。

- メモ帳で範囲選択、追加候補、取消・再変換。
- 無効キー、利用不可モデル、429、タイムアウト時に原文が残ること。
- OpenAI → Google → OpenAI の切替と再起動後のモデル・キーの分離。
- 必要に応じて、独立した同意の下で Decisions 自動変換。

Antigravity の入力欄との互換性（Issue #16）では、Undo に Ctrl + Z が
2 回必要な制限が残る。今回の Gemini 確認は空欄からの初回変換を対象とする。

## macOS 版との対応

| 項目 | macOS 版 | Windows 版 |
| --- | --- | --- |
| 選択肢 | OpenAI / Google / その他 | OpenAI / Google Gemini / その他 |
| Google の接続先 | `/v1beta/openai` | 同じベースパスを完全パスへ補完 |
| モデル | プロバイダー設定で指定 | プリセットを入力後、自由に編集可能 |
| 設定の反映 | macOS 側の設定機構 | 保存後に Host を再起動。接続先やモデルを変えた場合は送信を再許可 |

参照：

- [Google の OpenAI 互換 API](https://ai.google.dev/gemini-api/docs/openai)
- [macOS 版のプロバイダー設定](https://github.com/kiyoka/Sumibi-mac/blob/main/Sources/SumibiCore/ProviderConfiguration.swift)
