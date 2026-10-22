# ADR 0034: 共有救済枠を先に確保する専用待機adapter

状態：専用ACP store・実DLL・非同期workerの診断fixtureで実装・検証。Issue #4 / draft PR #10。
[ADR 0033](0033-bounded-core-waiting-input.md)のCoreを接続する。通常デモ・製品キーの経路は有効化しない。

## 容量・受入れ・救済

input_waiting_host::Waitingは所有STAでWaitingInput、InputWatch、metadata、COM参照を保持する。
workerへ本文queue・COM参照・cookie・callbackを渡さない。fixtureが意味上の文字/Backspace/Enter/Convertを明示admitする。
admitは実contextの通知・保護・選択を検査し、イベントごとに既存EXE共有BudgetのRecoveryを予約してからCoreへ渡す。
失敗時はその予約を解放し、新しい入力を受け入れない。既存entry・他所有者のticketは変更しない。
entry/救済の管理配列も事前に4件確保し、受入れ後の救済化で新しい本文bufferを確保しない。

Core単独の1024イベント/16384単位に対し、このadapterは他editorの救済も含め全体4件、
1文字イベント1024単位、保持本文合計4096単位を上限とする。予約中・未適用・不明な結果を全て数える。
待機済み入力のDLL実行では予約済みの枠を使い、通常Editorの二重予約を行わない。
成功したqueue先頭のpayloadを除去してから共有枠を返す。Dropもqueueを先に破棄して枠を解放する。
本文を救済bufferへ移す際の一時copy、allocator overhead、文書やBroker結果はこの論理長とは別である。

RecoveryにOutcome::Deferredを追加し、「未適用」を「結果不明」と区別する。
DeferredEnter/DeferredConvertは操作情報だけでコピー不可。本文は受入れ済みの供給断片だけを保持する。
通知/保護検査が不成立なら本文を残さず、復元後も古い入力を適用しない。
停止時は先頭のUncertain/AppliedUntrackedと、未試行tailのDeferredを別々のレコードにする。
救済表示も未適用の文言を用いる。既存のコピー・破棄・10分期限の契約を再利用し、文書へ再適用しない。

コピーは指定レコードだけを明示的に確認し、実contextの保護履歴を前後とClipboard公開直前に再検査する。
失敗なら保持し、保護不成立ならredactする。期限切れのレコードは取得不可。
この段階の複数レコード確認はfixtureから呼ぶadapter APIであり、F8の複数行UIや通常デモへの接続はまだない。

## 実DLL・所有対象・順序

初回の固定変換は従来のF7診断helper→実DLL→EXE worker→認証済み子process→版付きIPC→所有STAへ接続する。
adapterは応答前の入力を本文へ反映せず、所有loopでだけ結果配送とqueue排出を行う。
失効時とDropでは自分の正確な要求IDをCancelし、新しい別要求をCancel(0)で巻き込まない。
取消は待機しない。子processの回収・接続終了は従来どおりEXE worker/fixtureの所有loopで行う。

従来の手動変換は結果を選択した状態で返す。専用ホストは確認済み結果の末尾へ自身のcaretを一度だけ移し、
再検査後にCoreをReadyへ進める。これにより待機文字が変換結果を置換しない。
このcaret操作を一般アプリへ適用せず、手動変換そのものの既存選択契約も変えない。

排出する文字/BackspaceはComposing経由の既存独立ABIで本文・caret・compositionを同期取引する。
本文反映済み/結果不明の後にホストfallbackや再実行を行わず、tailは未適用のまま停止する。
Convertは意味上の診断helper呼出しで次の要求を開始し、新要求完了まで排出しない。
Enterはホストへ配送せず、先頭とtailを救済へ移す。SendInput/PostMessageによるキー再生は行わない。

自己compositionを一般のidle検査へ通すために条件を緩めない。
独立ISpikeInputState::Validate(context,start,end,active_start)を追加し、既存ABI/vtableは変更しない。
実DLL内のTracker.observe、正確な記録範囲、選択/向き、通知/保護世代、自己compositionのidentity/全範囲を照合する。
ready/before/cleanup中や、他composition・未知の向き・不整合は拒否する。本文取得もSetTextも行わない。
共通Controlのowner STA・busy・弱Controller・ObjectLease・失敗時cleanupを使う。
adapterは自身の確認済み入力成功後だけこの能力を使い、idle検査は従来どおり維持する。

fixture専用の8秒期限で排出を停止し、未適用入力を救済へ移す。期限の確認は所有loopのserviceに依存する。
ownerが止まった場合の定刻消去は保証しない。製品の期限・超過時切替・process間予算政策は引き続き未決である。

## 検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
waiting-input-testは25ケース。変換→待機文字→Backspaceで「日本語で」となる順序、複合絵文字の削除、
Convert再待機、Enterの非配送、共有枠不足、4件/4096単位、誤UTF-16/制御文字/過大入力、
private/readonly/外部編集/保護復元、取消/診断期限、本文変更前後の失敗/追跡失敗と未適用tail、
自己状態検査と他composition拒否、明示コピー/コピー失敗/保護後拒否/期限、所有者Dropと新要求非取消を確認した。
正常な変換/文字/削除はそれぞれ1回Undoで戻り、3回Undoで元の本文へ戻る。admitは本文/Undo/adapter本文取得を増やさない。
試行後の失敗は1回の本文試行だけで、再serviceでもtailを実行しない。
各session終了後は共有4枠全ての再予約を確認し、利用者のClipboardは試験用メモリーへ差し替えた。

CoreにDeferredの区別・期限・非コピー操作と共有容量の2件を追加し94件、workspace134件。
Test-Spike.ps1 -SkipDesktopのRust134件・fmt・Clippy 3構成・build・load_smoke・self/edit/keyも成功。
同期入力174・救済47・Backspace55・composition46・通知30・broker入力11・追跡25・close4の関連回帰も成功した。
ログはtarget/issue-4-waiting-adapter-static.logとwaiting-adapter-<検査名>.log（全てtarget/配下）。
native検査はTSF/COM解除とDLLアンロードも確認した。

fixtureは専用storeへの明示helper呼出しで、物理打鍵・自然な修飾キー取得・一般アプリの入力配送の受入には読み替えない。
前面化で未通過のwindow/input/delayedの3検査は維持する。
後続は通常デモの明示モードと複数救済レコードの確認UI、所有window通知との接続、製品の容量/期限政策。
一般アプリの保護情報・composition sink寿命・標準Undo・常駐Broker/配置/認証も残る。
