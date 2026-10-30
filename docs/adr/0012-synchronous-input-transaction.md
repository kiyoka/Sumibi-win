# ADR 0012: 所有ホストでの同期入力取引

状態：診断経路の実装と専用ホストでの検証。製品採用・一般アプリ対応の完了ではない。
2026-10-22。Issue #4 / draft PR #10。

## 背景

ADR 0010のBeforeInput/AfterInputは、ホストが本文を反映した後に別の編集セッションで
compositionを更新する。入力を一度だけ反映できるが、本文・caret・下線を同じwrite cookieで
処理する製品用入力基盤にはなっていない。

TSFの同期編集もデータベースのrollbackではない。SetTextが失敗を返しても、壊れた／中断した
storeが本文の一部を変更した可能性を排除できない。HRESULTだけを根拠に元のキーを再送すると
二重入力になり得るため、書き込み開始を成功とは独立して記録する。

## 決定と処理順

既存ISpikeInputのvtableを変更せず、同じ所有STAの独立ISpikeInputWriter.Insertを追加する。
引数はcontext、UTF-16 pointer/length、disposition出力だけ。cookie・Rust allocationを越境させず、
workerへ渡さない。所有threadと再入を検査した後で入力を参照する。

1. 正しいUTF-16・1〜1024単位を確認する。制御文字・埋込みobject U+FFFCを拒否する。
   Coreの追跡容量、座標・世代の上限も書き込み前に検査する。
2. 読取メタデータでfocus・選択・scope/status/compartment・通知世代・所有identityを観測する。
   外部変更があれば古い記録を失効させ、自分のmetadataだけを片付けてから新しい範囲を観測する。
3. 一度だけTF_ES_SYNC | TF_ES_READWRITEを要求し、そのcookie内で対象を再照合する。
   新規時は選択範囲（空caretを含む）に自分のcompositionを開始する。開始拒否・sink欠損なら
   本文書き込みへ進まない。継続時は同一COM identityだけを認め、caretが追跡末尾にあることを確認する。
4. scope/status/compartment、選択、通知世代、他の表示属性を再検査する。
   書き込みを試みる状態を先に記録し、供給された入力だけを選択range.SetTextへ一度渡す。
   raw HRESULTの厳密なS_OKだけを成功とする。既存本文のGetTextは呼ばない。
5. 同じwrite cookie内でcaretを末尾へ移し、同じcompositionを伸ばし、自己所有の下線だけを更新する。
   既存のBefore/After経路とcookie内metadata処理を共有する。追跡済み本文の書き直しは行わない。
6. cookieを解放した後にも保護履歴と安定したメタデータを確認し、Core記録を確定する。
   正常な自己OnEndEditを、status・compartment・focus変更の履歴から区別する。
   OnEndEditではINPUTSCOPE更新の有無だけを最大1範囲問い合わせ、不明なら保護境界扱いにする。
   値や本文を読まず、変更後に元へ戻されたscopeも自己入力の通知として許可しない。

通常のcapture・source・変換結果適用には従来通りcompositionなしの保護gateを使う。
自己composition中の本文読取は許可しない。今回の書込は、所有する範囲と実入力だけを扱う独立した経路である。
F7では自分のcompositionを終了してから、従来のsource照合・非同期Broker・適用gateへ進む。

## 結果と再入力の契約

| disposition | 意味 | 呼出し元の処理 |
| --- | --- | --- |
| 0 / NOT_ATTEMPTED | SetTextへ到達していない | 元のホスト入力をその場で一度だけ処理できる。追跡は付けない |
| 1 / APPLIED | 本文・caret・composition更新と追跡を確認した | ホストから追加挿入しない |
| 2 / APPLIED_UNTRACKED | SetTextはS_OKだが後続確認が失敗した | 本文を再入力・rollbackしない。追跡を失効させる |
| 3 / UNCERTAIN | SetTextを試み、S_OKを確認できなかった | 変更済みの可能性があるため再入力・rollbackしない |

HRESULTの失敗だけで0を推測しない。出力を受け取れない場合も再入力を許可しない。
dispositionのCellはpanic境界の外に置き、SetTextへ入る直前に3、そのS_OK直後に2を記録する。
後続の失敗で0へ戻さない。遅延同期callbackは従来のactive gateで拒否する。

2/3ではDLLと専用ホストが変換を停止し、診断画面に入力結果の確認を表示する。
次の実入力はホストで一度だけ処理し、未確認の変更を自動的に追跡へ採用しない。
Esc/Enter/移動等の明示的なuser boundaryで停止状態を解除できる。
通知PollやCancel(false)では解除せず、別compositionを引き継がない。
後始末が拒否された場合の保持と再試行はADR 0011の有限cleanupを使う。

未確認入力の救済UI・保持容量・期限は未実装である。UNCERTAINでは実際に文字が挿入されていない
場合もあり、入力欠落が解消されたという意味ではない。製品経路として有効化しない。

## 検証と範囲

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
transaction-input-testはロードした実DLLで22ケースを実行する。
正常な伸長・選択置換・repeat・UTF-16・Backspace互換・Undo・F7変換と1回Undo、
lock拒否・private・composition開始拒否・sink欠損、SetText前／変更後の失敗、caret失敗、
status/scope/compartmentの変更・復元、他composition、外部選択変更、強制終了、close、
不正入力、1024単位の上限と超過拒否を扱う。

TextStoreのwrite-lock番号により本文とcompositionが同じwrite内で処理されたことを確認する。
失敗時の本文・Undo・SetText呼出し回数を検査し、書き込みを試みた後は自動再入力しないこと、
失敗後のF7が送信しないこと、Esc後の新規入力だけで追跡を再開できることを確認する。
ロード済み実DLLのSumibiSpikeBodyReadCountは、入力だけでは増えない。
MSCTF内部のstore照会まで防いだという意味ではない。

ログ：target/issue-4-input-transaction.log、target/issue-4-input-transaction-regression.log。
Test-Spike.ps1を省略なしで実行し、workspace 47件、fmt・Clippyの3構成、既存native検査とpipe/peer、
TSF/COM解除・DLLアンロードも成功した。診断processの残留なしを確認した。
起動手順は[開発手順](../DEVELOPMENT.md#同期入力取引を試す)。

transaction-demoは所有する合成ACP欄だけを使う。通常文字の挿入を今回の経路へ送り、
Backspace/削除/Undoは既存ホスト操作との互換検査にとどまる。
物理打鍵、Notepad等の一般アプリ、製品キー消費、非同期write待機、救済、常駐Broker、
HTTP・同意・設定・資格情報は未完了であり、Issue #4は閉じない。

根拠：[ITfRange.SetText](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfrange-settext)、
[Microsoft SampleIMEのcomposition編集](https://github.com/microsoft/Windows-classic-samples/blob/main/Samples/IME/cpp/SampleIME/Composition.cpp)、
[ITfEditRecord.GetTextAndPropertyUpdates](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfeditrecord-gettextandpropertyupdates)。

後続：[ADR 0013](0013-synchronous-tracked-backspace.md)で、transaction-demoの記録末尾Backspaceも
同じwrite cookieへ接続した。前方Delete・待機・救済・一般アプリ統合は引き続き未完了。
