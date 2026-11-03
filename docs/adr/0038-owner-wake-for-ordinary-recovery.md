# ADR 0038: 待機デモの通常救済を定期確認する

状態：専用waiting-demo・実DLLで実装・検証。Issue #4 / draft PR #10。
[ADR 0036](0036-unified-demo-recovery-and-copy-expiry.md)の単一確認画面の待機方法を補強する。

## 問題と変更

waiting-demoではOwnerが待機入力と通常入力の救済を同じViewに表示する。
通常入力のRescue自身はViewを持たないため、従来のRescue.wait_msは確認画面が閉じているものとして
最長で救済期限まで待てた。その間はOwnerの残り時間表示と保護状態の確認が更新されない。

Ownerの待機時間を、通常救済だけが残る場合にも決める。
確認画面があるときは最大1000msで残り時間を更新し、画面が閉じている間は最大250msで
保護状態を再検査する。待機Queueや待機レコードがある場合の既存の250ms確認を維持する。
記録がなくなれば不要な周期起床を止める。救済の期限管理は既存Rescueに任せる。

保護が読取専用に変わると、次の所有loopで供給本文を除去する。
本文を除去した操作情報はF8で確認でき、F10ではClipboardに書かず、F9で破棄できる。
本文の取得許可を広げず、他のデモのRescue表示には変更を加えない。
これは協調する専用ACPホストの検査であり、一般アプリの未通知変更を全て検出する保証ではない。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
waiting-demo-testは45件から47件へ拡張した。
通常救済だけの表示・非表示で待機上限を確認し、非表示中の読取専用化から本文除去、
操作情報の再表示、コピー拒否、明示破棄まで実DLL/TSFと試験用Clipboardで確認した。
本文の再入力、利用者のClipboard書込、物理キー注入は行わない。
Rust134件・fmt・Clippy 3構成・build・load_smoke・self/edit/keyも成功した。
ログはtarget/issue-4-recovery-wake-*.log。

同期入力174件は直前の[ADR 0037](0037-cancel-failed-waiting-start.md)に記録した単独再実行で成功し、
初回連続実行の一時的な失敗の原因は引き続き未確定である。
前面化で未通過のwindow/input/delayedの3検査も残る。
