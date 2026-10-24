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
