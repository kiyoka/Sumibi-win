# ADR 0023: 記録範囲内の拡張書記素Backspace

状態：Coreと専用ホストの実DLL入力で検証。一般アプリの任意位置編集は未対応。
2026-10-22。Issue #4 / draft PR #10。ADR 0013の削除単位を更新する。

専用ホストの左右移動・Shift選択・前方削除と内部caretの扱いは、後続の
[ADR 0024](0024-grapheme-navigation-in-owned-host.md)で更新した。DLLの記録範囲の境界は維持する。

## 決定

記録末尾のBackspaceをUnicode scalar単位から拡張書記素クラスタ単位へ変更する。
例えばeと結合アクセント、肌色修飾付き絵文字、国旗、ZWJ家族絵文字を、記録内では一回で削除する。
挿入イベントごとの分割を保存せず、削除時に現在の記録全体を分割するため、
別イベントで追加された結合文字やregional indicatorの組合せも扱う。

分割は[unicode-segmentation 1.13.3](https://docs.rs/unicode-segmentation/1.13.3/unicode_segmentation/)へ厳密固定する。
Cargo.lockのchecksumも固定し、実際の依存ソースのUNICODE_VERSION=(17,0,0)を確認した。
CoreはBACKSPACE_UNICODE_VERSIONとしてこの版を公開し、受入例と共に検証する。
依存はMIT OR Apache-2.0、MSRVは1.85.0。Unicode新版や依存更新時は再検証する。

suffix_grapheme_boundaryは、呼出し側が渡した最大1024 UTF-16単位から削除開始位置を返す純粋関数である。
不正UTF-16・空・不正count・上限超過・クラスタ数を越えるrepeatはNoneを返す。
UTF-16を最大3072 byteの固定stack scratchへ変換し、拡張書記素の後方iteratorを使う。
BMPの1単位は最大3 byte、surrogate pairの2単位は4 byteなので、この容量内に収まる。
削除計画そのものはheapを確保せず、文書本文やCOMを取得しない。他の入力取引全体の無allocation保証ではない。

InputSession.backspace_rangeは従来のcontext・世代・選択・保護を検査してからこの関数を使う。
countはクラスタ数、返す範囲は従来どおりUTF-16座標であり、単位を混同しない。
計画で記録を消費せず、repeat全体を一度の同期SetTextで削除する。既存のcaret・自己composition・
同じwrite cookie・1回Undo・書込試行後の再削除禁止は維持する。wire版・ABI signatureは変更しない。

## 範囲の境界

記録の開始と終了は硬い境界である。文書の前後を読み足してクラスタを拡張しない。
記録が結合文字・ZWJ・regional indicatorから始まり、既存prefixと画面上で結合し得る場合でも、
DLLはその既存prefixを削除対象へ取り込まない。記録内のクラスタと、文書全体で見える一文字が
常に一致する保証ではない。正規化・見た目による同一視・周囲の文字の推測は追加しない。

専用TextStoreの通常ホストBackspaceも同じ関数を、ホスト自身が所有するcaret左側の本文へ適用する。
追跡外や共有救済枠が満杯でも複合絵文字を分割しない。これはDLLによる既存本文取得とは区別する。
repeatの通常ホスト処理は元の回数だけ行い、DLLの一括writeに入った後はfallbackしない。
選択削除は元の選択を使う。左右移動・前方Delete・任意caret位置の完全な書記素編集は未対応のままである。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- Coreに5件追加し36件、workspace合計76件。結合文字・ZWJ家族・修飾絵文字・国旗・VS・Hangul・
  Indic、イベントをまたぐ結合、repeat/超過の非消費、最大UTF-8 scratch、最大1クラスタ、
  不正UTF-16と開始点の非越境を確認した。Unicodeの全適合試験を独自に再実行した意味ではない。
- backspace-input-testは37ケース。従来27件に結合文字・家族・肌色・国旗・repeat・全削除・
  別イベント結合・書込後失敗・既存base直後の追跡開始・追跡外fallbackの10件を追加した。
  一括1 write、caret・同じcomposition identity/全削除時の終了、1回Undo、adapter本文取得不変を確認した。
- recovery-input-test 34件も成功。共有枠満杯の2回Backspace fixtureを家族絵文字入りへ強化し、
  通常ホストが元の2回だけ処理してprefixを残すことを確認した。Clipboardは試験用メモリーで検査する。
- transaction 22・tracking 25・composing 29・input-notify 30・broker-input 11・close 4も成功した。

Test-Spike.ps1 -SkipDesktopはRust 76件・fmt・各構成Clippy・build・load_smoke・self/editまで成功し、
key-testでmodifiers=[false,false,true]（Shift押下状態）を検出して停止した。
同じ最終機能版のkey-testを新しい診断processで一度単独実行し、成功と解除・DLLアンロードを確認した。
ユーザーのキー状態を解除/注入したり、assertionを弱めたりしていない。一括script全体の成功とは扱わない。
最後にCoreの境界例の期待値を固定値へ明示し、fmtとCore 36件を再確認した。

ログはtarget/issue-4-grapheme-backspace-static.log、grapheme-backspace-key-isolated.log、
grapheme-backspace-core-final.log、grapheme-backspace-backspace-input-test.log、
grapheme-backspace-recovery-input-test.log、grapheme-backspace-transaction-input-test.log、
grapheme-backspace-tracking-input-test.log、grapheme-backspace-composing-input-test.log、
grapheme-backspace-input-notify-test.log、grapheme-backspace-broker-input-test.log、grapheme-backspace-close-test.log
（全てtarget/配下）。成功したnative検査のTSF/COM解除・DLLアンロードを確認した。
前面化で未通過のwindow/input/delayedの3検査は今回も再実行しておらず、未確認のまま。
一般アプリの保護情報・製品sink寿命・標準Undo・製品の待機行列/配置/認証は残る。
