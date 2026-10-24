# ADR 0033: 手動変換待機入力の有限Coreモデル

状態：OS非依存Coreのみ実装・検証。Issue #4 / draft PR #10。
製品キー・診断editor・実DLL・Brokerの待機経路にはまだ接続しない。
[全体設計 8.1](../TECHNICAL_OVERVIEW.md#81-待機入力と救済)の提案を、容量・順序・完了の検証へ進める。

## 決定と容量

sumibi-core::waiting_inputにWaitingInputを追加する。入力はデコード済みのUTF-16文字列、
Backspace回数、Enter、Convertという意味上の操作に限定する。仮想キー・scan code・修飾キー状態・
ネイティブイベント・COM参照・cookie・callbackを格納せず、キー再生や文書編集を行わない。

1 queueの上限は1024イベント、本文合計16384 UTF-16単位、1文字イベント1024単位。
文字は非空の正しいUTF-16かつ制御文字を含まないこと、Backspaceは1〜1024回であることを検査する。
Enterは文字列に埋め込めず、独立した意味上の操作にする。書記素削除やrepeatの実行は後続adapterの責任である。

admitは上限・対象・状態を検査し、try_reserve_exactで本文とentryの容量を確保してから成功receiptを返す。
adapterが元の入力を消費してよいのは、この成功後だけ。拒否時は新しい入力を消費せず、既存entryと本文を維持する。
満杯で古い入力を上書きしない。本文合計は論理長であり、allocator overheadや一時copyを含むprocessメモリー上限ではない。

これは単一queueの試験的な上限である。診断EXEのRecovery共有4枠・本文4096単位とは独立し、
製品の全所有者・プロセス間予算や救済先の確保を満たすものではない。
製品の期限/救済/超過時の政策が未決なので、期限による自動破棄・製品キー消費を有効化しない。

## 対象・順序・完了

Targetはcontextと論理入力対象の一意ID、本文世代、設定版、認可版、肯定的なallowed検査結果を持つ。
対象IDは文書・フォーカス・選択の境界で変更し、元へ戻っても再利用しない。OSのrangeや編集cookieではない。
RequestIdもadapterが全体で一意に発行し、同一queue内の後続要求では増加させる。queue再作成後も再利用しない。

| 状態 | 許可する処理 | 次の状態 |
| --- | --- | --- |
| Waiting(request) | 入力の有限保持、同じ要求の確定結果の照合 | Ready、またはRetired |
| Ready | 入力の有限保持、同じ対象への先頭操作の開始 | Delivering、またはRetired |
| Delivering(ticket) | 先頭の借用と、その試行の一度だけの完了 | Ready、Waiting、またはRetired |
| Retired(reason) | 明示的な救済検査と保護境界によるpayload除去 | 再開しない |

settleは現在の要求IDだけを受け入れる。古い応答で新しい待機要求を取消したり入力を排出したりしない。
変換が未適用なら同じTarget、確認済み適用なら本文世代だけが正確に1進んだTargetを必要とする。
nativeの範囲・保護・結果適用の正当性はadapterが独立に検査する。不明な結果は未適用へ読み替えない。

start_nextはFIFOの先頭だけを凍結する。admitのreceiptとは別の新しいticketを発行し、
NotAttempted後に再び開始する場合もticketを更新する。別queue・前試行・重複の完了は現在の先頭を消費しない。
ID枯渇でwrapしない。deliveryは借用だけで、本文を転送・削除せず、発行済みticketだけを受け入れる。

確認済みAppliedは先頭を一度だけ除去し、対象の本文世代を1進める。
AppliedUntrackedは確認済みの先頭だけを除去して停止し、未適用tailを保持する。
Uncertainまたは不正な完了は先頭を含めて停止し、先頭だけが適用済みかもしれないという区別を残す。
停止後は元の操作を再実行しない。NotAttemptedは未試行の先頭を保持してReadyへ戻すが、Coreは自動再試行を実行しない。

Convertを開始したadapterが新しい要求IDを返したら、Convertだけを除去してWaitingへ移る。
新しい要求が完了するまでtailの排出を停止する。Convertを本文編集の成功として完了することはできない。
EnterはDeliveringにならず、先頭とtailを保持してManualEnterで停止する。
遅延Enterをホストへ送信する機能や、通常Enterのその場の通過を変更する接続はない。

## 境界と救済の限界

対象の不一致は不可逆に停止する。context/論理対象の交換は保守的にfocus境界として扱い、
focus喪失・保護変更・allowed不成立・設定/認可版変更では保持payloadを除去する。
境界中に完了が戻っても再開しない。状態復元や後続のcancelでも失効を解除しない。
本文世代だけの不一致、明示取消、ManualEnter、結果不明は自動適用不能な救済状態として保持できる。
このCoreには通知監視がないため、adapterは観測した保護履歴を即時retireへ転送する必要がある。

Recoveryは理由・件数・本文単位数・不明な先頭の有無・redactedだけを公開する。
retainedは明示確認のための借用で、adapterは表示/コピー前にも保護履歴を再検査する。
既存の1件救済UIへの暗黙の切詰めや移送はしない。copy、永続化、IPC送信、自動再入力、timerは実装しない。
本文を持つ型にDebug/Cloneを実装せず、payload除去はVecの破棄であり物理メモリーのゼロ化保証ではない。

## 検証と後続

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
Coreに12件追加して92件、workspace合計132件。
既存ManualConversionと組み合わせた純粋adapter模擬では、「nihongo」の変換応答→待機「です」→Backspaceを
順に一度だけ反映し、「日本語で」となること、重複応答/完了が採用されないことを確認した。
native編集・実キー・実IPCの検証には読み替えない。

ほかに本文/イベント各上限と解放後の再利用、不正UTF-16/制御文字/count、供給bufferの独立保持、
Convertによる再待機と古い応答、Enterの停止、不明/適用後追跡失敗の先頭とtailの区別、
前試行/別所有者のticket拒否、対象/設定/認可/保護の復元後も失効、配送中の保護境界、ID枯渇と不正完了を検証した。
Test-Spike.ps1 -SkipDesktopでRust132件・fmt・Clippy 3構成・build・load_smoke・self/edit/keyが成功。
基本DLL検査ではTSF/COM解除とDLLアンロードも確認。ログはtarget/issue-4-waiting-input-static.log。

前段階の同期入力174・救済47等のnative拡張検査は今回再実行していない。
前面化で未通過のwindow/input/delayedの3検査、一般アプリ・物理打鍵の制限も維持する。
次は専用adapterで取得前の共有容量・救済先・要求取消との整合を証明し、実DLL/workerで順序を検証する。
製品キー接続、全体予算と期限政策、一般アプリの保護情報・composition寿命・標準Undo・配置/認証は残る。

後続の[ADR 0034](0034-shared-recovery-waiting-adapter.md)で、既存共有救済枠を先に確保する専用adapterを実DLL/workerへ接続した。
上記件数と未接続範囲は本ADRの実装時点の記録であり、通常デモ・製品キーは引き続き未接続である。
