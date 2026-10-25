# ADR 0018: 選択変換の保持rangeと送信前の失効検査

2026-10-22 / Issue #4 / PR #10。基準は66bad0e。
専用ホストで実装・検証済み。一般アプリの対応完了は意味しない。

## 背景と判断

ManualEditはITfRangeを所有STAに保持し、端点と原文を再照合してから置換していたが、
Coreへ渡すTargetのためだけにITfRangeACP.GetExtentも要求していた。
この追加要件を除き、保持したgeneric rangeを選択変換の基準にする。

[ITfRange](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfrange)は
本文取得・context・端点比較を扱う。
[ITfRangeACP](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfrangeacp)は
ACP方式の入力先で使う追加interfaceであり、generic rangeの照合に絶対座標を持ち込まない。
実入力の追跡・挿入・末尾削除は従来のACP方式のままである。

## Coreの契約

Target.selectionを明示的なSelection型へ変更する。

| 種類 | 意味と検査 |
| --- | --- |
| Acp(start, end) | 従来の協調adapterの絶対座標。非負・順序の検査を維持 |
| RetainedRange { utf16_units } | 所有adapterがnative端点・向きを照合した範囲の長さ。文書座標ではない |

RetainedRangeではcontext/rangeの不透明IDが非ゼロで、長さが有効な非空UTF-16本文と一致する必要がある。
IDは同じ要求のcapture/finishだけに用い、別要求へ再利用しない。
Core.finishは選択モデルを含むTarget全体を完全一致で検査し、同じ長さでもAcpへ切り替わった結果を拒否する。
COM参照・端点・cookieをCoreやwireへ渡さない。JSON版1には変更しない。

## 所有adapterの検査

1. captureは通知購読を開始し、同期readで選択・保護状態・有限本文を取得する。
   保持したITfRangeは従来どおり所有STA内だけに置く。
2. sourceは新しい同期readで現在のselectionを取得し、contextのIUnknown identity、
   TF_SELECTIONSTYLE全体、IsEqualStart/IsEqualEndを照合する。
   保護Stamp・focus・policy・通知失効・期限も検査し、成功時だけ既取得本文を返す。
   source自身では本文を再取得しない。
3. sourceでlock・選択・保護・policy等の失敗を観測したらGuardを永久に失効させる。
   状態を元へ戻してもsource/finishを再利用できず、新しい明示要求が必要になる。
   別threadでは、apartment内のRc/Cellを操作する前に従来どおり拒否する。
4. finishは新しい同期write内でgeneric端点・向き・contextと全文、保護情報を再検査する。
   外部COM後の最終検査、Core.finish、一度のSetTextを同じcookie内で行う。
   通知を受けた変更・復元による失効も従来どおり維持する。

端点検査はedit.rsのchecked_selectionをsourceとvalidate_snapshotで共用する。
本文は1024 UTF-16単位までで、prefixの絶対位置を長さに混ぜない。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- Coreに2テストを追加。補助面文字を含む長さ4、一度だけの適用、選択モデル変更、
  長さ不一致・ゼロID・不正UTF-16を確認。workspaceは65件（Core 26・Protocol 22・IPC 12・TIP 5）。
- range-target-testは実MSCTFと専用ACP store上で12ケース成功。
  正常・補助面文字・4096単位のprefix、送信前の選択/向き/部分選択・lock・policy・private拒否、
  拒否後の復元による再利用禁止、適用前の選択/向き/本文変更を確認する。
  正常は一度のSetTextと一度のUndoでprefix/suffixを含む元文書へ戻る。
- sourceを繰り返してもadapterのGetText counterは増加しない。
  送信前失敗・復元後のsource/finishは本文取得・SetText・Undoを増やさない。
  このcounterはsetupへリンクしたTIPライブラリのもので、ロードDLLのcounterとは別である。
- tools/Test-Spike.ps1へ新検査を組み込み、省略なしの全回帰が成功。
  fmt・各構成Clippy・build・load_smoke、既存の実DLLキー25・入力取引22・Backspace 27・
  通知20・救済25・再利用worker 11項目を含むnative/pipe/peer、解除・DLLアンロードを確認。
  ログはtarget/issue-4-retained-range.logとtarget/issue-4-retained-range-regression.log。

## 検証の境界と次の段階

fixtureは引き続きITextStoreACPであり、実際の非ACPアプリやITextStoreAnchorの互換性は未検証。
今回は明示的なITfRangeACP要件の除去と、同じadapterの選択失効を検査した段階である。
一般アプリの保護情報、所有compositionのsink寿命、標準Undo、非同期配送と配置/認証は残る。

fixtureの送信前選択・向き変更は通知なしで現在値を返すが、一般アプリの未通知履歴を保証しない。
本文変更は正式なOnTextChangeを使う。MSCTFのcacheがあるため、storeのmemoryだけを通知なしで変更しても
再取得本文の検証にはならないことを試験作成時に確認した。
[OnTextChangeの契約](https://learn.microsoft.com/en-us/windows/win32/api/textstor/nf-textstor-itextstoreacpsink-ontextchange)に従い、
host側変更を通知し、TSFによるSetTextを同じ通知で重複報告しない。
sourceの追加検査もmetadataのみであり、通知がない任意の本文変更の検出とは扱わない。
OS内部の本文参照とadapterのcounterの区別は[ADR 0006](0006-input-protection-metadata.md)を維持する。

## 2026-10-25: 選択中のオブジェクト置換文字

`ITfRange::GetText`で取得した選択本文に`U+FFFC`が含まれる場合、
通常の文字列としてBrokerへ送る前にcaptureを拒否する。
Coreの要求開始・結果判定、版1の送受信本文、直接/所有TSF置換も同じ文字を拒否する。
これは文字として読める本文だけを扱う現在の範囲に合わせた判断であり、
埋め込みオブジェクトを変換対象に一般化しない。
専用TSF storeの`edit-test`に固定合成の`a\u{fffc}b`と
置換結果`日本\u{fffc}語`を追加し、captureまたは適用前の拒否、
本文不変、Undo履歴なし、COM/TSF解除を確認した。
実TSFの選択編集は25ケースとなった。一般アプリのオブジェクト表現は未検証。
認証済み実子Brokerの`broker-channel-test`でも同文字の原文を送信前に
`InvalidArgument`として拒否し、接続・要求番号を保った次の正常要求を確認した。
単発Jobと再利用Workerの共通`Request::validate`もProtocolの`valid_text`を使い、
スレッドや子Brokerの起動前に同じ原文を拒否する。

## 2026-10-25: Core単独の手動要求検査

OS非依存の`ManualConversion`も、要求ID・対象ID・入力/設定/認可版のゼロ、
原文の1024 UTF-16単位超過を開始前に拒否する。ACPと保持rangeのどちらも
不透明なcontext/range IDを必要とする。結果も1024単位を超えたら一度だけ
失効し、返却本文を作らない。TIP/Protocolと同じ現在の有限上限をCoreでも
守るための検査であり、製品の最終的な送信文字数の決定ではない。
Core 98テストでゼロ値、原文超過、補助面文字を含む結果超過を確認した。

## 2026-10-25: 適用時のTSF client照合

`ManualEdit`がcapture時に保持したclient IDと、finishを呼ぶ側のclient IDが
異なる場合、write sessionより前に拒否する。`range-target-test`の実TSF
17ケース目で別clientを指定し、本文読み直し・SetText・Undo履歴がないことと
COM/TSF解除を確認した。さらに同じ経路で結果中の`U+FFFC`と1024 UTF-16単位を
超える補助面文字列も拒否し、実TSFの`range-target-test`は19ケースとなった。
