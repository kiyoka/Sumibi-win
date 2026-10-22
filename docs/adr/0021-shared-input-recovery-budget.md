# ADR 0021: 所有者間で共有する救済枠

状態：診断EXEの共有予算と実DLL入力で検証済み。製品の待機行列や一般アプリへの統合は未完了。
2026-10-22。Issue #4 / draft PR #10。ADR 0014の追加。

## 背景

従来のRecoveryはeditorごとに1件・1024 UTF-16単位・10分を制限していた。
editorが増えると保持件数も増えるため、入力を消費する前に所有者をまたぐ予算も必要になる。

## 決定

CoreのBudgetを複数のRecoveryへ渡し、予約中・確認待ちを合わせて4件までに制限する。
診断EXEはOnceLockで唯一のBudgetを持ち、全Rescueの生成時に共有する。Budgetには本文・COM・callbackを置かない。
別STAからの予約も同じatomic counterで照合する。枠が戻るまで待機したり、他所有者の本文を調べたりしない。
Coreの単独Defaultは独立予算であり、ライブラリのDefaultだけでプロセス全体を制限する意味ではない。
別EXE・別DLLに独立リンクされたCoreや複数プロセスをまたぐ予算は提供しない。

- 各editorは従来どおり1件。共有4件は診断経路の保守的な上限で、製品案の16384単位とは区別する。
- 共有枠をbuffer確保とDLL呼出しより前に予約する。buffer確保に失敗しても局所PermitのDropで返す。
- 複製できないPermitをRecordが所有する。本文bufferの破棄後に枠を返す。
- NOT_ATTEMPTED/APPLIED、明示破棄、コピー成功、期限切れcleanup、clear、所有者Dropで解放する。
- 本文のredact、確認画面の開閉、コピー失敗は枠を解放しない。操作警告を保持する間も上限に数える。
- 満杯時は既存の書込前fallbackを使い、元のホスト入力をその場で処理する。古い救済を上書きしない。
  Backspaceのrepeatはホスト側の元の回数だけ処理し、本文書込を試みた入力は再送しない。

救済レコードが保持する挿入本文の論理上限は合計4096 UTF-16単位である。
allocatorの余剰容量、確認画面・明示コピーの一時buffer、文書本文、Brokerの結果bufferは別であり、
プロセス全メモリーが8192 byte以下という保証ではない。Brokerの4枠とも独立の予算である。

## 期限と所有者

期限は従来どおり失敗確定から最大10分。期限切れの取得・コピーは拒否する。
実際の枠とbufferの解放はその所有者のexpire/clear/Dropが行う。別所有者が期限切れを横取りしない。
所有ループが止まれば枠は戻らず、新規の入力取引は消費前にfallbackする。
所有ループ停止中に期限どおりのメモリー消去を保証するものではない。
製品の待機入力、所有者一覧、複数入力先のUI、プロセスをまたぐ政策は引き続き未実装。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

Coreへ5件を追加し救済12件、Core 31件、workspace合計71件。
予約中・確認待ち・redactの合算、各解放経路、重複解放拒否、満杯/不正/ID枯渇による非消費、
budget handleの先行破棄と別予算の分離、12 thread同時予約からの4件上限と全Drop後の再利用を確認した。

recovery-input-testは既存25件と共有予算6件の31ケースが成功。
追加fixtureは同じ専用contextに別々のRescueを作り、補助所有者はコピー不能な操作情報だけを保持する。
複数の実アプリを操作する試験とは区別する。満杯時の挿入と2回Backspaceは実DLLのSetTextへ入らず、
host履歴が元の操作分だけ増える。破棄・期限切れ・close・Drop後は別所有者の失敗入力を保持でき、
本文再取得・Clipboard呼出し・過去入力の再送がないことも確認した。
初回の新Backspace試験ではhostの2回削除を1履歴と数えた期待値が失敗したため、元のrepeat分の2履歴へ修正した。
実装の入力回数は変更していない。初回ログはtarget/issue-4-shared-recovery-native.log。

Test-Spike.ps1 -SkipDesktopで71件・fmt・各構成Clippy・build・load_smoke・self/edit/keyが成功。
加えてtransaction 22・Backspace 27・input-notify 24・close 4の実DLL回帰が成功し、解除・DLLアンロードを確認した。
ログはtarget/issue-4-shared-recovery-static.log、shared-recovery-native-final.log、
shared-recovery-transaction-input-test.log、shared-recovery-backspace-input-test.log、
shared-recovery-input-notify-test.log、shared-recovery-close-test.log（全てtarget/配下）。
今回、省略なしの全回帰は再実行していない。前段階で前面化を拒否されたwindow/input/delayedの3検査は未通過のまま。
一般アプリ・標準Undo・製品sink寿命も未解決で、Issue #4とPRのdraftを維持する。
