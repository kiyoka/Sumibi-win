# ADR 0025: 明示選択Deleteの同期取引

状態：診断DLLと専用ACPホストで実装・検証。選択のない前方削除や一般アプリ対応は未完了。
2026-10-22。Issue #4 / draft PR #10。ADR 0012・0013・0024の後続。

## 決定

専用transaction-demoの非空選択に対するDeleteを、実DLLの同期writeへ接続する。
従来はホストが直接削除していた。文字挿入と共通のreplace経路を使い、選択・保護情報を
書込前後に照合し、本文・caret・一時compositionの開始/終了を同じwrite cookieで処理する。
旧入力の追跡・compositionは事前に退役させる。そのmetadata cleanupは別cookieになる場合がある。

独立したISpikeSelectionEraser::DeleteSelection(context, disposition)を追加する。
既存のInput/Writer/Eraser ABIは変更しない。owner STA・再入禁止・context identity・
Controller寿命・ObjectLeaseは既存Controlと共通で、workerへCOMや本文を渡さない。

対象は明示された非空選択で、最大1024 UTF-16単位。選択のない呼出しは書込前に拒否する。
APIにrepeat引数はなく、ホストへ届いたrepeatは選択範囲を一度だけ削除する。
結合アクセントだけを明示選択した場合も範囲を広げない。書記素から前方範囲を推測せず、
既存本文のGetTextも行わない。範囲が空の通常Deleteは従来のホスト書記素削除に任せる。

## 同期処理と失敗

1. 旧入力run・準備済み原文を破棄し、自己compositionのcleanupを完了する。
2. focused context・選択位置・肯定的なscope/status/compartment・他compositionを検査する。
3. 同期write内で再照合し、選択上に自己compositionを開始する。開始拒否・sink欠損は本文書込前に止める。
4. 外部COM呼出し後の選択・保護・通知世代・属性を再照合し、SetTextを空文字で一度だけ呼ぶ。
5. caretを元の選択先頭へ置き、今回の自己compositionだけを終了する。再照合後に結果を公開する。

削除後は空のInputSessionとし、既存prefix/suffixを新しい変換元として採用しない。
次の明示入力は新しいrunを開始できる。削除した本文を記録・復元・自動再送しない。
保護検査は既存の厳格なgateを使い、一般アプリの欠損metadataを許可する変更はない。
診断EXEの共有composition sinkは引き続き必要で、製品sink寿命の解決には読み替えない。

| disposition | ホストの扱い |
| --- | --- |
| NOT_ATTEMPTED | 元の削除を一度だけ通常ホストで処理してよい |
| APPLIED | 本文・caret・composition終了と完了後検査が成立 |
| APPLIED_UNTRACKED | SetText成功後に検証/終了が失敗。再削除せず確認へ進む |
| UNCERTAIN | SetText試行直前から失敗。変更前/変更後の失敗を区別できず、再削除しない |

HRESULTだけをfallbackの許可に使わない。試行後の失敗はControlの書込停止と既存の明示境界へ接続する。
Enter/Esc等を後から再送せず、rollbackもしない。

## 容量と救済

ホストはDLL呼出し前にRecoveryのOperation::DeleteSelectionを予約する。
全EXE共有4枠・所有者ごと1枠は既存と共通で、満杯ならDLL本文書込へ入らず通常ホストへ返す。
削除のレコードは操作情報だけを保持し、本文用の容量を確保しない。
不正な非空payloadをfinishへ渡しても保存せず、確認UIは「選択範囲の削除」を表示する。
削除した本文は取得・コピーしない。copyable=falseとし、確認/明示破棄・期限・所有者終了の既存規則を使う。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
transaction-input-testに選択Deleteの20件を追加し、既存22件と合計42件が成功した。

- 通常選択、repeat=1024の一度だけの削除、結合アクセントだけの選択、最大1024単位。
- 旧追跡の退役と削除後の新規入力、変換保留中の削除で旧結果を拒否。
- 空選択・readonly・他composition・null引数の書込前拒否。
- private・lock拒否・owner拒否・sink欠損・救済slot予約中の一度だけのホストfallback。
- SetTextの変更前/変更後失敗、caret失敗、status/scope/compartmentの変更後復元で再削除しない。
- 1回Undo/Redoによる本文と選択の復元、同じwrite cookie、自己compositionと下線の終了。
- 未確認削除は操作情報だけを救済し、コピー要求でも試験用Clipboardを呼ばない。

全ケースでDLLのadapter本文取得counterは削除によって増えない。
保留ケースはF7の原文取得を行った後を基準にし、旧結果の配送時にも増えないことを確認する。
OS内部の全照会を計測する主張ではない。テストはEditorの操作helper/診断ABIを直接呼び、
物理キーや通常TSFキー配送の受入には読み替えない。
保留ケースの初回は、実入力前の新しいControlにはTrackerがないためF7が開始されず停止した。
fixtureを実DLL入力で初期化し、F7が選択した範囲をUndoの基準にして再確認した。
新しいcontextの既存選択をF7だけで初期化する一般入力adapterは、この検査で実装したものではない。

recovery-input-testへ共有4枠満杯の選択Deleteを追加し、合計35件が成功した。
本文書込をDLLへ渡さず、選択だけを削除して1回Undo/Redoし、既存救済ticketを変更しない。
Coreは既存の削除payload拒否検査を両方の削除操作へ広げ、workspace83件（Core43件）を維持する。
Test-Spike.ps1 -SkipDesktopも成功し、Rust83件・fmt・各構成Clippy・build・load_smoke・self/edit/keyを確認した。
Backspace37・書記素ホスト22・composition29・通知30・追跡25・broker入力11・close4も成功した。
成功したnative検査はTSF/COM解除とDLLアンロードを確認している。

ログはtarget/issue-4-selection-delete-static.logとselection-delete-<検査名>.log（すべてtarget/配下）。
保留fixture初回のログはtarget/issue-4-selection-delete-pending-initial.log。
前面化で未通過のwindow/input/delayedの3検査と省略なし全回帰は再確認していない。
選択のないDLL前方削除・任意位置のcomposition継続・製品待機・一般アプリの保護情報/寿命/標準Undoは残る。
