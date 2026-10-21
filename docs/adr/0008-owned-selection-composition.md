# ADR 0008: 所有する選択compositionの寿命

2026-10-22 / Issue #4 / PR #10。[ADR 0007](0007-foreign-composition-boundary.md)の非干渉条件を保つ。
既存の選択を未確定範囲として保持する開発用の基盤。通常打鍵の追跡・範囲拡張・製品InputSessionは未実装。

## 操作と本文の扱い

F6プレビューは、既存の原文と範囲を検査して下線を付け、その範囲でStartCompositionする。
compositionが既にあるcontextは拒否し、同じ原文だからと他サービスの範囲を引き継がない。
この開始操作自体は本文を変更せず、Undoの本文履歴も増やさない。

F7または期限内の診断応答による確定では、所有オブジェクトが存続することを確認してEndCompositionする。
その後、同じ同期write内で従来の原文・範囲・選択・保護状態・policyを再照合する。
別compositionが残っていれば従来のidle gateが拒否する。自己所有を理由に本文取得条件を緩めない。
成功したSetTextの後には失敗し得る処理を置かず、同期write失敗後の本文操作を非同期に再試行しない。

Esc・Enter・許可取消・外部編集・フォーカス喪失では原文を維持し、自分のcompositionと下線だけを解除する。
Enterの通過は従来どおりホストへ一度だけ。broker-demoはF6を使わず、非同期の選択変換を維持する。

## 状態と照合

`owned_composition.rs`は所有thread、New/Live/Ended/Terminated、StartCompositionの戻り値だけを保持する。
COM参照・範囲・cookieをworkerへ渡さず、Debugも実装しない。
StartCompositionの正確なS_OKと非null出力を別々に検査し、部分出力は保持して自分の後始末だけを行う。

終了前にITfCompositionViewのIUnknown identityを列挙値と照合する。原文や同じ座標による推測はしない。
列挙は最大16回、各1件。正確なS_OK/1/非nullかS_FALSE/0/nullだけを受け入れる。
失敗・不整合・上限超過を「終了済み」に変換しない。
一致オブジェクトがなく列挙が正常終了したらTerminatedとし、古い確定を永久に拒否する。
同じ原文・同じ範囲に別compositionが作られてもそれを取得・終了・置換しない。

後始末はメタデータだけを扱い、保護状態やフォーカスが変わっていても保持済みの自分の範囲・atomだけを消す。
終了やwrite lockが拒否された場合は所有情報を残す。自分のcomposition終了後に属性解除が失敗した場合も、
本文を確定せず、残った属性の後始末だけを再試行する。Dropは同期のbest effortで、本文を予約しない。
後始末不能時の一般アプリでの復旧UIや製品寿命は引き続き未実装。

## ホストの通知サービスとWindowsでの観測

このWindowsでは、ホストが開始拒否してS_OK/nullが返った後、DLLに置いたITfCompositionSinkが解放されず、
DLL leaseが残ることを再現した。sinkをnullにするとE_INVALIDARGになった。
公式資料のoptional記述からnullが動くと仮定せず、この観測を他OS版の保証にしない。
参照数の手動補正や、生きたcallbackを残したDLLの強制アンロードはしない。

専用ホストはEXE内のITfCompositionSinkを診断STAごとに1個共有し、所有contextの
`GUID_SPIKE_COMPOSITION_SINK` compartmentへIUnknownを置く。DLLはそのサービスを読むだけで、
未知の一般contextを許可状態へ書き換えない。未設定・ITfCompositionSink非対応なら開始を拒否して属性を片付ける。
このGUIDはspike構成だけの契約で、本文送信への同意や製品Brokerの接続設定ではない。

EXEのsinkは本文・context・cookie・DLL interface・DLL callbackを保持せず、通知で編集しない。
DLLは既存のproperty通知と、終了直前の実オブジェクト照合で失効を判断する。
開始拒否時にWindowsがsink参照を残す現象そのものを直したとは扱わない。
同じEXEオブジェクトを共有し、参照が残っても新しい通知オブジェクトやDLL参照を増やさない。
製品では一般ホストにこのサービスの存在を仮定せず、別の寿命設計と互換検証を必要とする。

## 検証

2026-10-22 / Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- owned-composition-testは18ケース。16件は実DLL clientの所有CLSIDを確認し、
  取消・確定・強制終了・新要求と旧応答・許可取消・focus・原文復元・lock/readonly拒否・
  ホスト開始拒否・開始lock拒否・通知サービス欠損/不正・他composition追加・保護状態変更・診断遅延応答を扱う。
- 直接アダプター2件はController通知なしで強制終了し、同じ範囲・同じ原文へ別compositionを作った場合も、
  古い確定が拒否され、別compositionが残ることを確認する。
- ホスト開始拒否は同じsinkで32回行う。解除後の新規開始も成功し、全ケース後にDLLをアンロードできる。
- 通常成功の本文置換は1回だけのUndoで原文へ戻る。取消・開始拒否・metadata操作で本文履歴を増やさない。
- workspaceは40件のまま。fmt・workspace/TIP spikeなし/Host全featureのClippy、build、load_smoke、self/edit/keyが成功。
- 既存composition 16・保護39・broker-key 17・broker-input 8・broker 19・input・delayed 11・close 4・windowの検証も成功。
  focusは前面化に依存しない同じ実DLL callbackを使う場合があり、物理キーや他製品IMEとの手動確認には読み替えない。
- 各プロセスのTSF/COM解除・DLLアンロード、診断processの回収を確認。ログはtarget/issue-4-owned-*.log。

未実装：通常入力を追跡してcompositionを伸縮する処理、製品InputSession、一般アプリでの同じ寿命保証・
標準Undo、製品Brokerの発見・常駐処理、HTTP・同意・設定・資格情報。Issue #4は引き続き未完了。

根拠：[StartComposition](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontextcomposition-startcomposition)、
[EndComposition](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcomposition-endcomposition)、
[TerminateComposition](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontextownercompositionservices-terminatecomposition)、
[OnCompositionTerminated](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcompositionsink-oncompositionterminated)。
