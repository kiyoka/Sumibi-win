# ADR 0024: 所有ホストの書記素移動・選択・前方削除

状態：Coreと専用ホストの編集処理を実TSF上で検証。製品DLLの前方Deleteや一般アプリ対応は未完了。
2026-10-22。Issue #4 / draft PR #10。ADR 0023の後続。

## 決定

専用editorの左右移動・Shift選択・前方Deleteを、Backspaceと同じ拡張書記素単位にする。
従来は左右キーで複合絵文字や結合文字の途中へ移動でき、Deleteで一部だけを残す場合があった。
分割は従来どおりunicode-segmentation 1.13.3 / Unicode 17.0.0に固定する。
[依存のgraphemes仕様](https://docs.rs/unicode-segmentation/1.13.3/unicode_segmentation/trait.UnicodeSegmentation.html#tymethod.graphemes)のextended=trueを使う。

Coreのgrapheme::Boundariesは、呼出し側が渡した最大1024 UTF-16単位から境界の位置だけを作る。
本文・COM・cookie・grantを保持せず、Debug/Clone/serializationも提供しない。
位置はUTF-16単位で、UTF-8のbyte offsetではない。scalar境界のbitsetも作り、surrogate pairの途中は拒否する。
最大1025個のu16境界と有限bitset、既存の最大3072 byte UTF-8 scratchを使い、heapを確保しない。
空文書も境界0だけで表す。不正UTF-16・上限超過・不正位置・countは拒否する。
これは編集許可ではなく、本文変更後には作り直す必要がある。

| 操作 | 専用ホストの規則 |
| --- | --- |
| 左右移動 | 書記素境界間を移動し、repeatは文書端で止める |
| 書記素内のcaretから左右移動 | 一回目でその書記素の移動方向側の端へ寄せる |
| Shift移動 | anchorを維持し、caretだけ移動する。向きが逆転してもanchorを変えない |
| 選択中の通常左右移動 | 一回目で該当方向の選択端へ寄せ、残りrepeat分だけ移動する |
| collapsed caretのDelete/ホストBackspace | 書記素を削除する。内部caretなら、まずその書記素全体を対象にする |
| 非空の明示選択の削除 | 有効なUTF-16選択をそのまま一度だけ削除。repeatで広げない |
| Home/End | 従来どおり文書端。Shift時はanchorを維持 |

プログラムで結合アクセントだけを明示選択した場合などは、その範囲を尊重する。
選択を正規化したり、見た目を理由に別の文字を巻き込んだりしない。

## 所有と既存の入力取引

TextStoreは自身の本文から操作ごとに新しい境界表を作り、RefCellの借用を解放してから
選択/本文を変更し、正式なOnSelectionChange/OnTextChangeを送る。
選択変更後の追跡と古い変換権は既存の通知経路で失効する。
ホストのDelete repeatは一度のuser_editと1回Undoにまとめ、readonly/lock中は変更しない。

DLLは新たに既存本文を取得せず、Coreの記録末尾BackspaceはADR 0023の硬い記録境界を維持する。
ホストは本文を所有するため全体を分割できるが、一般アプリの文書をDLLが読めることとは区別する。
製品DLLの前方削除取引、任意位置でのcomposition継続、製品キー配送、一般アプリの標準Undoは未実装。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

Coreに7件を追加して43件、workspace合計83件。
空文書・端・repeat、結合文字/ZWJ/修飾/国旗/VSの内側からの移動と削除、明示選択の非拡大、
surrogate内部/不正座標/不正countの拒否、1024単位の最大境界表と最大1クラスタ、RIの組合せを検証した。
元のsuffix Backspaceも同じUTF-16変換helperを使い、既存Core検査を維持する。

新しいgrapheme-input-testは専用の実TSF contextで12ケース。
左右移動、Shiftでのanchorと向き、選択collapseとrepeat、複数Delete、内部caretでの両方向削除、
明示部分選択、文書端/Home/End、readonly、lock拒否、変換保留中の移動/削除による旧結果拒否を確認した。
編集は1回Undo/Redoで復元し、DLLのSetTextとadapter本文取得counterを増やさない。
保留試験はホスト操作の正式な通知で失効させ、明示Cancelや直接DLL通知callbackで代替しない。
初回はF7が原文を範囲選択する前提を見落とし、左キーで先頭へcollapseしたためfixture期待値が失敗した。
意図した末尾編集になるよう右キーで選択を末尾へcollapseしてから左/Deleteを行い、最終12件が成功した。
初回ログはtarget/issue-4-grapheme-editor-native.log。

この検査は通常キーから呼ばれるホストの編集メソッドを直接呼び、TSF通知・失効・描画まで確認する。
物理キー・TSF通常キー配送の受入には読み替えず、前面化で未通過のwindow/input/delayedの3検査は維持する。
Test-Spike.ps1に新コマンドを追加し、-SkipDesktopの省略表示にも含めた。

最終版のTest-Spike.ps1 -SkipDesktopでRust83件・fmt・各構成Clippy・build・load_smoke・self/edit/keyが成功。
加えてBackspace37・救済34・同期入力22・追跡25・composition伸縮29・通知30・broker入力11・close4を再確認した。
ログはtarget/issue-4-grapheme-editor-static.log、grapheme-editor-native-final.logと、
grapheme-editor-<検査名>.log（全てtarget/配下）。native検査のTSF/COM解除・DLLアンロードを確認した。
省略なし全回帰と一般アプリの保護情報・sink寿命・標準Undo・製品配置/認証は残る。
