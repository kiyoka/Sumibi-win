# ADR 0007: 他サービスのcompositionへの非干渉

2026-10-22 / Issue #4 / PR #10。入力保護検査（[ADR 0006](0006-input-protection-metadata.md)）に追加。
自分の追跡compositionを作る実装ではなく、選択変換が他サービスの未確定入力を扱わないための境界である。

## 判定と所有

`tip/composition.rs`は同じcontextのITfContextCompositionを取得し、EnumCompositionsから1件だけNextする。
対象選択と交差する範囲だけを調べず、同じcontext内に1件でも存在すれば拒否する。
空範囲のcomposition、離れた範囲、複数compositionも同じ扱い。
別contextのcompositionだけを理由に現在の入力先を拒否しない。

EnumCompositionsは正確なS_OKと非null出力が必要。
Nextは正確なS_FALSE・取得0・null出力の組合せだけを「存在なし」とする。
S_OKの空出力、S_FALSEの不整合出力、エラー、未対応は拒否する。
失敗時の部分初期化COM出力も解放し、列挙を繰り返さない。
viewの本文・範囲・ownerは取得せず、TakeOwnership・EndComposition・TerminateCompositionは呼ばない。

保護検査でscope等の形を検証した後、composition検査を行い、残りの外部メタデータ照会後にも再確認する。
既存の本文取得前後、送信用sourceを返す前、適用時の検査がこの条件も通る。
存在する場合はDLL自身のGetTextとBroker job開始を行わず、変換キーを通過させる。
OS内の属性照会等まで無読取にできるという保証はADR 0006同様にしない。

## 変更通知と空composition

ControllerのOnEndEditはGUID_PROP_COMPOSINGの更新も監視し、保留要求を取消する。
非空compositionの開始・終了だけで本文と選択が元のままでも、旧要求を復活させない。
ManualEditの既存guardも編集通知を受けた時点で永久に失効する。

実機では空compositionの開始・終了でOnEndEditが届かず、編集トランザクションsinkでも検出できなかった。
この場合を通常の編集通知で捕捉したと主張しない。
専用TextStoreは所有者としてITfContextOwnerCompositionSinkを実装し、開始・更新・終了で
明示登録されたISpikeBroker.Cancel(0)へメタデータだけを転送する。
同じ接続をbroker-demoのEditorでも行い、終了時に解除する。
取消はDLL ticketを先に破棄してEXE workerへ停止を通知し、本文編集や待機をしない。
転送時はRefCellの借用を解放し、相手のcompositionは許可する。取消失敗でもそれを拒否・終了させない。
新しいCOM ABIや外部プロセスのCOM参照は導入しない。

所有ホストの協力がなくても、適用時にまだ存在する空compositionは再検査で拒否できる。
一方、通知がなく、次の検査より前に空compositionが開始・終了した履歴は、列挙だけでは復元できない。
一般アプリのowner sinkを横取りせず、この履歴の保証は一般アプリでは未成立として残す。
所有者側からの通知を提供する専用ホストの検証を、一般アプリ対応へ読み替えない。

## 回帰fixtureの境界

所有者sinkを追加した検証で、初期化中から不正なnull/countのInputScopeを返すfixtureが
textinputframework.dll 10.0.26100.9711内で0xc0000005になった。OSのApplication記録でモジュールを確認した。
protection-testは正常なscopeでウィンドウ・contextを初期化し、実DLLの同期probeの直前に不正値へ切り替え、
解除前に正常値へ戻す。壊したmetadataをOSの非同期初期化へ公開する時間を除くためであり、
DLLの拒否条件を緩める変更ではない。
本文照会中の変更ケースでは、初期化時のTSFキャッシュを正式なOnTextChangeで無効にしてから検査し、
ホスト本文照会で保護指定を変えるhookが実際に実行されたことも確認する。
null・空・過大count・部分初期化エラーの4ケースは、その同期probeでGetInputScopesが実際に呼ばれたことも確認する。
OS内部の本文コピー数は実行ごとの観測に留め、固定の合否値にしない。

## 検証

2026-10-22 / Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- workspace 40件。追加1件はraw COM列挙のHRESULT・count・出力有無32組合せ、1回だけのNext、部分出力解放を扱う。
- composition-test 16ケース。通常成功、同一contextの全域・別範囲・空・複数の初期拒否、
  開始前に終了したcomposition、別context、待機中の非空開始／開始終了、所有者通知による空開始／開始終了、
  所有者転送なしの空compositionの適用時拒否、source前の開始／開始終了、適用前とプレビュー前の拒否。
  初期拒否4ケースは実DLLのGetText呼出し0・Broker job開始0と、compositionが存続することを確認する。
  非空待機2ケースはowner転送を無効にし、DLLのproperty通知だけで取消することを分けて検証する。
- broker-input-test 8ケース。従来7件に、デモと同じEditorでF7待機中の空composition開始・終了を追加。
- protection-test 39、broker-key-test 17、broker-test 19、close-test 4と、fmt・Clippy・ビルド・
  load_smoke・self/edit/key検査、解除・DLLアンロード・診断子processの回収を確認。
- ログはtarget/issue-4-composition-*.log。物理キー操作、他IMEの実製品との同時操作、Notepad等の一般アプリは未検証。

自分のcomposition・通常入力追跡・製品Broker・一般アプリでの寿命とUndo、HTTP・同意・設定・資格情報は残る。
Issue #4全体を完了として扱わない。

根拠：[EnumCompositions](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontextcomposition-enumcompositions)、
[Next](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-ienumitfcompositionview-next)、
[StartComposition](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontextcomposition-startcomposition)、
[Compositions](https://learn.microsoft.com/en-us/windows/win32/tsf/compositions)、
[ITfContextOwnerCompositionSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfcontextownercompositionsink)。
