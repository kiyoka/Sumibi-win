# ADR 0009: 所有ホストが入力した範囲だけを変換する

2026-10-22 / Issue #4 / PR #10。[ADR 0008](0008-owned-selection-composition.md)の次段階。
専用ホストの通常入力をCoreで追跡し、F7から既存の実DLL・非同期Brokerへ渡す。
打鍵ごとのcomposition開始・範囲拡張、一般アプリ内の製品InputSessionは今回の範囲に含めない。

## 入力を記録する条件

`sumibi-core/input.rs`のInputSessionは、協力するホストが実際に挿入したUTF-16だけを保持する。
カーソル付近の既存本文や単語を検索して補わない。本文を持つ型にDebugを実装せず、永続化しない。
上限は1024 UTF-16単位。空・不正UTF-16・制御文字・世代や座標の異常を拒否し、
上限超過時にも末尾だけを切り出して追跡しない。製品案の1000書記素とは別の技術上限である。

ホストは挿入の前後で、フォーカスcontext・選択・scope/status/compartment・compositionなしを
同期readで確認する。`inspect_input`自体はGetTextを呼ばず、閉じたカーソルも検査できる。
ただしMSCTF内部の本文照会まで防ぐという意味ではない。
入力は従来のホスト処理で一度だけ反映し、成功後にだけ追跡状態を更新する。
前後の保護状態が不一致、ロック拒否、読取専用、判定不明なら追跡を消す。
追跡失敗を理由に入力を取り消す・再送する・元イベントをもう一度通すことはしない。

直前のcontext識別子・通知世代・composition世代・選択が完全一致し、カーソルが直前の末尾にあれば連結する。
それ以外の新しい挿入は、その挿入分だけで新しい範囲を始める。
選択への入力は置き換えた新しい文字だけを追跡し、前後の既存本文を含めない。
ホスト識別子・通知世代はオーバーフローで再利用しない。

## 失効とF7

InputWatchは所有STAのITfTextEditSink・thread manager・compartment通知を購読し、
本文・選択・属性・context/focus・保護状態の通知で世代を進める。通知中に本文を読まない。
空compositionの開始・終了は所有TextStoreの単調counterも照合する。
通知されない変更をこのcounterだけで検出できるとは仮定しない。
ホストの移動・削除・制御操作・不正文字列・focus喪失・終了でも追跡を消す。
元の本文や選択へ戻しただけでは古い追跡を復活させない。

tracking-demoのF7は、現在のカーソルと有効な追跡記録が一致するときだけその範囲を選択する。
追跡は選択変更より前に一度だけ消費する。失敗時に全文選択へ切り替えない。
自動で選択した範囲には印を残し、後のF7でそれを「利用者が明示した選択」として再利用しない。
印は新しい実入力や移動・制御操作で解除し、単なる取消やfocus通知では解除しない。
利用者のShift選択・Ctrl+A等による明示選択は、従来の保護検査を通す選択変換として扱う。

EXE bridgeのStartではworkerを開始する前に一度だけ検査する。
実DLLから渡された原文と記録したUTF-16の完全一致、選択範囲、保護Stamp、通知世代、
composition世代を再照合する。検査失敗時は予約も失効させ、同じ予約のStartを再試行して迂回できない。
検査中の再入Reserve/Startも拒否し、closureのpanicは固定エラーに変換して予約を失効させる。
検査用closureのCOM参照・本文・watchは呼出し中に解放し、workerへ渡さない。
Startへ到達しなかった場合もF7の同期呼出し終了時に検査用closureを解除する。
結果適用は従来のManualEdit/Coreによる対象・原文・保護状態の再検査を引き続き必要とする。

## 操作と範囲

`tracking-demo`は専用1行入力欄。F7で入力範囲または明示選択を固定の「日本語」へ非同期変換する。
Escは待機取消、Enterは原文確定をホストへ一度だけ通す。Ctrl+Zで変換前へ1回Undoできる。
F6は使わず、通常打鍵に下線やcompositionを付けない。従来のbroker-demoの全文選択は変更しない。
外部API・保存・設定・資格情報は使用しない。ビルドと起動は[開発手順](../DEVELOPMENT.md#通常入力の範囲だけを変換する)を参照。

## 検証

2026-10-22 / Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- Coreに5件追加し、workspaceは45件（Core 12、Protocol 22、IPC 6、TIP 5）。
  Unicodeの連結、既存prefixの除外、一度だけの消費、全失効条件、置換、異常入力、正確な上限と超過を確認。
- tracking-input-testの専用ホスト25ケースが成功。空欄、prefix、補助面文字、選択置換、
  明示選択、移動・選択復元・本文復元、空composition、privateと復元・private中の入力、
  readonly・lock拒否・focus、容量と超過、待機中Esc・Enter・追加入力、送信直前の不一致を扱う。
- 正常系は実DLLの予約キー→Core/TSF→別Hostの固定結果を通し、描画と1回Undoでprefix/suffixを含む原文へ戻る。
  bridge直接呼出しの2件では本文不一致後の予約再利用と、検査中のReserve/Start再入を拒否することを確認する。
  範囲・composition・scope変更は実DLLの変換開始経路を使い、worker開始と本文書換えを拒否する。
- Test-Spike.ps1を省略なしで実行し、fmt・3構成Clippy・build・load_smoke・self/edit/key、
  Broker 19・Brokerキー17・Broker入力8・保護39・他composition 16・所有composition 18・追跡25、
  window/input/close/delayed、pipe/2-process peerも成功。解除・DLLアンロードと診断process残留なしを確認。
- ログはtarget/issue-4-tracking-input.logとtarget/issue-4-tracking-regression.log。
  専用ホストへのメッセージと同じ入力callbackによる自動検証であり、物理打鍵・一般アプリの確認ではない。

未完了：打鍵ごとのcomposition取引と範囲拡張、製品キー配送、待機入力・救済、一般アプリの
寿命・実打鍵・標準Undo、製品Brokerの発見・常駐dispatch、HTTP・同意・設定・資格情報。
専用EXEの協力を一般ホストや他IMEへ仮定せず、Issue #4は引き続き未完了とする。

後続の[ADR 0010](0010-cooperating-host-composition-growth.md)で、別のcomposing-demoへ
実DLLによるcomposition伸縮を追加した。このADRのtracking-demo自体はメタデータ更新を行わない。
