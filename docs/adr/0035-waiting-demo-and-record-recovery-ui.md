# ADR 0035: 待機入力デモと複数救済レコードの確認

状態：専用単一行ACPデモ・実DLL・非同期workerで実装・検証。Issue #4 / draft PR #10。
[ADR 0034](0034-shared-recovery-waiting-adapter.md)のadapterを明示的なwaiting-demoへ接続する。
既存transaction-demoの入力仕様と、製品キー・一般アプリの未対応範囲は維持する。

## デモと入力受付

waiting-demoは文字入力後のF7で固定の「日本語」へ非同期変換する。
応答待ちの文字・Backspace・F7・EnterはCoreの意味上のイベントへ渡し、admit成功後だけ消費する。
本文反映・IPC結果配送・救済UIの操作は所有loopで行い、WndProcでworkerの完了を待たない。
文字は従来のWM_CHARデコード後に供給する。補助面文字も既存の有限なhigh-surrogate状態とlowの組から扱い、
未完のsurrogateは境界で破棄する。rawキーや修飾キー状態をqueueへ保存しない。

Backspace・Enter・F7のkeydownを消費した場合、Window.route_messageはTranslateMessageへ進まず、
二重のWM_CHARを生成しない。Convertの排出ではbusy状態で従来F7 helperを使い、新Sessionを重ねて作らない。
新たな変換が始まったらtailはその結果を待つ。Enterはホストへ送らず、未適用の救済に移す。
Escは要求を取消して先の受入れ済み入力を救済へ移す。移動/Delete/Ctrl/Alt修飾操作は待機を止めてから従来経路へ返す。

受入れ前に既存EXE共有Recovery枠を予約する。待機と他editorの救済を合わせ最大4件、本文4096 UTF-16単位。
このデモの待機は最大8秒。満杯では新イベントを受け入れず、旧要求を取消し、既に受入れた入力を保存した後に
今回の元の入力だけを従来ホスト経路へ渡す。受入れ済み入力や試行後の失敗を再生しない。
8秒と4件の制限はデモ表示/起動案内にも示す。製品の期限・超過切替・process間政策を決定した意味ではない。

## 救済の確認

F8で最初の救済を開き、さらにF8で残るレコードを順に巡回する。
確認windowのcaptionに「現在位置/件数」を表示し、期限や破棄で件数が減った場合も更新する。
コピー可能な供給本文、未適用Enter等の操作情報、結果不明/追跡失敗を既存Viewで表示する。
F9/破棄ボタンは表示中の1件だけを破棄し、F10/コピーボタンはその1件の全文だけを明示コピーする。
確認windowを×で閉じても保持を続ける。画面を閉じた状態のF10はコピーしない。
未確認の救済が残っている間の新しいF7は、既存レコードを上書きせず確認画面へ案内する。

recordのRecovery ticketは各独立slotで同値になり得るため、UI操作をticketだけに結び付けない。
Keyは一意Session ID・固定entry index・そのslotのticketを組にし、選択を変えても旧操作を別の行へ適用しない。
旧Sessionの操作、期限切れ、破棄済みレコードは無効とする。コピー/破棄の受付は有限のscalar通知だけで、
所有loopが正確なKey・期限・保護履歴を再検査する。

コピー失敗では本文と期限を保持する。コピーの公開直前に、context保護検査とOwnerの境界epochを両方照合する。
Clipboard処理中に境界やcloseが再入しても公開せず、payloadをredactし、古いプレビューを閉じる。
救済は最大10分で取得不能となり、所有loopのcleanupで容量を返す。定刻の物理メモリー消去を保証しない。
全レコードの明示確認が完了した場合だけ入力失敗状態を解除し、期限切れだけで未確認失敗を再利用可能にしない。

## 所有・通知・終了

OwnerはRefCellからSession/Viewを取り出してからCOM/native/UIを呼び、外部呼出し中に借用を保持しない。
busy中のwindow境界はscalar flagとepochで直ちに再利用/公開を止め、所有loopがqueue取消・本文除去を行う。
focus喪失・非アクティブ化・無効化・モード取消を既存のowner通知から接続し、状態復元後も旧要求を再利用しない。
閉じる操作はclosedを不可逆に設定し、busy処理から戻ってもSessionやViewを復元しない。
取消/Dropは正確な要求IDを使い、worker停止待ちは従来の所有loopに限定する。

Broker待ちではcompletion/input-signal handleを待ち、待機期限/救済期限も定期確認する。
排出可能なtailがあるときだけ次の所有loopへ即座に進める。IPC待ちでbusy spinしない。
一般アプリの未通知の履歴を検出する保証や、物理的なフォーカス切替の受入には読み替えない。

## 検証と残る範囲

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。
waiting-demo-testは22ケース。デモと同じroute_message/WndProcへ合成MSGを渡し、
FIFO文字/Backspace・Enter非配送・容量不足の今回入力1回・UTF-16補助面・再変換・診断期限・Esc、
救済巡回/1件コピー/1件破棄/コピー失敗/×後保持、旧行/旧Session/期限切れ操作の拒否、
Clipboard公開中の境界/close、busy中close、focusと3種のwindow境界を確認した。
コピー/破棄は実際のViewボタンも使い、Clipboardは試験用メモリーへ差し替える。利用者のClipboardは変更しない。
確認後は共有4枠全ての再予約、TSF/COM解除、DLLアンロードも検証した。

待機adapter25・同期入力174・救済47・Backspace55・書記素ホスト22・composition46・通知30・
broker入力11・追跡25・close4も成功した。Rustは134件のまま。
Test-Spike.ps1 -SkipDesktopでRust134件・fmt・Clippy 3構成・build・load_smoke・self/edit/keyが成功。
waiting-demo-testを省略なしscriptと省略表示にも追加した。
ログはtarget/issue-4-waiting-demo-static.logとwaiting-demo-<検査名>.log（全てtarget/配下）。

合成MSGの経路検証であり、物理打鍵・自然な修飾キー取得・一般アプリのキー配送の完了ではない。
前面化で未通過のwindow/input/delayedの3検査は維持し、全回帰成功へ読み替えない。
次は専用デモの対話操作を確認し、製品の待機/救済容量・期限・超過時動作を確定する。
一般アプリの保護情報・composition sink寿命・標準Undo、常駐Broker・配置/認証も残る。
