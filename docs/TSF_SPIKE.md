# TSFスパイクの検証状況

Issue #1の模擬遅延応答11ケースとIPC検証は[受入記録](ISSUE_1_ACCEPTANCE.md)・
[ADR 0002](adr/0002-ipc-boundary-and-desktop-probe.md)を参照。

2026-10-21 / Windows x64 / Rust 1.99.1 / windows-rs 0.62.2

## 今回の成果

- Activate/ActivateExからITfKeystrokeMgrへ独立したITfKeyEventSinkを登録。
- 所有スレッドと処理中フラグを確認。COM呼出し中にRefCellの借用を保持しない。
- Deactivateで解除し、失敗時は同じ状態から再試行。二重解除は成功として扱う。
- TIP解放時も解除を試行。解除失敗でもsinkの参照が残る間はDLLをアンロードしない。
- プレビューがないときは、通常キーのテスト、押下、解放はFALSEを返す。
- 検証用feature有効時だけ、専用ホストが許可した入力欄のCtrl+J予約キーから固定変換を実行できる。
- 通常のキー経路からは本文の収集・編集を行わない。外部送信、キー再送、常設のIME登録も行わない。

## 成功した検証

```powershell
cargo test --workspace --locked --offline
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo build --workspace --locked --offline
cargo run -p sumibi-tip --example load_smoke --locked --offline -- target/debug/sumibi_tip.dll
```

全8テスト成功。TIPテストはDLLのグローバルカウンターを共有するため、複数ケースを1テスト内で直列実行する。
`load_smoke` は別プロセスで実DLLをロードし、制御可能な模擬TSFマネージャーをDLLに渡す。

| ケース | 結果 |
| --- | --- |
| sink登録失敗後の有効化再試行 | 成功 |
| 二重有効化 | E_UNEXPECTEDで拒否 |
| 登録処理中のDeactivate再入 | E_UNEXPECTEDで拒否 |
| sink解除失敗後の解除再試行 | 成功 |
| Deactivate後のActivate／二重Deactivate | 成功 |
| Deactivateを忘れたTIPの解放 | sink解除を確認 |
| TIP解放時のsink解除失敗 | managerに残ったsinkを解放するとDLLも解放可能 |
| Enter・Esc・Backspace・英字の各コールバック | すべて未消費 |
| 最後のオブジェクト／ロックの解放 | DllCanUnloadNowがS_OK |

## 実TSFでの登録・有効化・解除

`sumibi-setup` を追加した。COMファクトリーをCoRegisterClassObjectでプロセス内に登録し、
RegisterProfileのTF_RP_LOCALPROCESSで開発用プロファイルを作成する。
ActivateProfileのTF_IPPMF_FORPROCESSでこのプロセスだけを有効化し、ITfKeystrokeMgr::GetForegroundが
SumibiのCLSIDを返すことを確認する。既定IMEを設定するAPIやユーザー全体の有効化フラグは使用しない。

```powershell
cargo build --workspace --locked
cargo run -p sumibi-setup --locked -- plan --dll target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked -- probe --dll target/debug/sumibi_tip.dll
cargo run -p sumibi-setup --locked -- self-test --dll target/debug/sumibi_tip.dll
```

- `plan` はDLLのパスと操作の範囲を表示するだけで、DLLをロードしない。
- `probe` は実TSFで登録・有効化・解除を実行する。恒久インストールではなく、プロセス終了後は利用できない。
- `self-test` はCOM登録直後、TSFプロファイル登録直後、有効化後に意図的な失敗を起こし、巻き戻しを検証する。最後に正常経路も検証する。

2026-10-20の日本語入力環境でplan、probe、self-testはすべて成功した。
以前の未登録CLSIDへの直接ActivateExで出たE_INVALIDARGは、この登録・有効化経路では再現しなかった。
旧 `load_smoke --tsf` は削除し、実TSF検証をこのツールへ一本化した。
現在の入力言語が日本語以外の環境は未検証で、有効化に失敗する場合がある。ツール自身は入力言語を切り替えない。

登録解除はTF_URP_LOCALPROCESSとCoRevokeClassObjectを用いる。途中失敗時もRAIIで同じ順序の後片付けを行い、
解除エラーは成功扱いせず終了コード1とする。全参照の解放後にDllCanUnloadNowがS_OKであることとFreeLibraryの成功を確認する。
途中の異常で参照が残った場合は、コードのアンロードによる不正参照を避けるためDLLをプロセス終了まで保持する。
ActivateProfileのS_FALSEを成功と誤認しないよう、この呼出しは生のHRESULTも検査する。

これはWindowsアプリでの実打鍵の検証ではない。常設のCOM／TSFインストーラーは別途必要。

## 固定結果の選択範囲編集

```powershell
cargo run -p sumibi-setup --locked -- edit-test --dll target/debug/sumibi_tip.dll
```

専用のメモリー上のITextStoreACPを実TSFのdocument/contextへ接続し、読取・書込セッションを検証する。
ユーザーの入力欄や文書は使わない。`edit-test` の編集ロジックは `sumibi-tip::edit` を検証EXEへRustライブラリとしてリンクして実行する。
DLL内のキー処理から編集を呼ぶ経路は、後述の `key-test` で別に検証する。

読取セッションで選択範囲・原文・ポリシーを記録し、別の書込セッションで照合して一度だけSetTextする。
cookieは各DoEditSessionの間だけ使用する。原文比較はUTF-16単位の完全一致で、近傍検索や正規化はしない。
スナップショットは一度しか使えない型にし、同期アクセス拒否時や要求終了後のコールバックでは編集しない。
コールバックが実行されずS_OKだけが返る異常も成功扱いにしない。

2026-10-20に以下の23ケースが成功した（Cargoの8単体テストとは別）。

| ケース | 結果 |
| --- | --- |
| 原文nihongoを日本語へ置換 | 成功 |
| prefix/suffixを残して文中の選択だけ置換 | 成功 |
| 原文を同じ長さの別内容へ変更 | 拒否、変更後の本文を保持 |
| 選択変更、同じ原文の別入力欄へフォーカス移動 | 拒否、両方の本文を保持 |
| 入力・設定・認可の各世代変更、許可取消 | 拒否 |
| 許可されていない入力、読取専用 | 拒否。許可なしでは本文読取も0回 |
| 読取／書込ロック拒否、書込コールバック欠落 | 拒否、本文を保持 |
| 読取が進まない、不正UTF-16、空選択 | 拒否 |
| 上限1024 UTF-16単位／1025単位 | 前者は成功、後者は拒否 |
| 絵文字・結合文字を含む原文の全文読取 | 成功 |
| 部分読取、Unicode部分読取、サロゲートで分断するホスト | 全文を読めず拒否、本文を保持 |

この環境の実TSFは、意図的に読取を制限したホストに同じブロックを再要求し、残りを取得できなかった。
完全な原文が得られなければ変換を見送り、取得した断片だけを置換しない。
1024単位は試作のリソース上限であり、設計中の送信文字数や書記素クラスタの上限とは別。

EditPolicyのallowedは、将来の入力スコープ判定等を行うアダプターが管理する契約。
今回trueにしているのは専用検証ホストだけ。汎用アプリのパスワード欄を検出済みとは扱わない。
本番のキー経路へ接続する前に、判別不能な入力先を拒否する分類処理が必要。
下線・Enter／Escの実打鍵・Notepad等でのUndoと互換性は後続の検証項目。

## DLL内のキー経路と専用ホストのUndo

```powershell
cargo build --workspace --locked
cargo run -p sumibi-setup --locked -- key-test --dll target/debug/sumibi_tip.dll
```

開発用feature `spike` を追加。通常ビルドでは有効にしないが、検証ツールsumibi-setupの依存指定から有効になる。
workspaceビルドは検証用DLLを生成する。常設IME向けのビルド／配布には使わない。

ITfSourceSingleに独立したITfFunctionProviderを登録し、ホストはGetFunctionProvider/GetFunctionで
検証用ISpikeControlを取得する。GrantContextで許可した現在のcontextだけにCtrl+J予約キーを登録する。
これはパスワード検出APIではなく、専用ホストが持つ明示的な開発用権限である。
未知contextを一般的に許可するヒューリスティックは追加しない。

SimulatePreservedKeyで実TSFからDLL内のOnPreservedKeyを呼び、DLL内の編集セッションを通して固定変換する。
デスクトップへのSendInputや物理キーの再送はしない。成功した変換だけをキー消費として返す。
ProviderとKeySinkはControllerのWeak参照を持ち、解放後に編集を再開しない。
許可取消時は先に本文アクセスを無効化し、予約キーを解除する。有効化解除でもProviderとキーを解除する。

今回、実DLLを使用して以下の6群を確認した。

- 予約キーの登録とDLL内のコールバックからnihongo→日本語へ変換。許可の再設定も成功。
- Enter・Esc・Backspace・英字のTestKeyDown/KeyDown/TestKeyUp/KeyUpは未消費で、本文を変更しない。
- 専用ホストのSetText履歴は1回の変換につき1件。ホストのUndoを1回実行すると原文へ戻る。
- 読取専用では変換せず、キーも消費しない。
- 同じ原文を持つ別contextでは変換せず、両方の本文を保持する。
- 許可取消後は本文読取0回、変更なし。終了後はDLLがアンロード可能。

未知の機能GUID／IIDとnull contextの許可要求も拒否する。
専用ホストのUndo履歴は検証用であり、実アプリのUndo動作を保証する実績ではない。
下線表示、実キーボードでの操作、汎用入力欄の保護判定はまだ未検証／未実装。

## 下線の表示属性基盤

`ITfDisplayAttributeProvider`、`IEnumTfDisplayAttributeInfo`、`ITfDisplayAttributeInfo` をDLLに追加した。
入力文字色・背景色を変更しない細い点線の定義を1種類返す。初期実装の定義は読取専用で、
SetAttributeInfoはE_NOTIMPLを返す。カスタマイズ設定の保存はまだ行わない。
ロード検証exampleで未知GUID・null出力の拒否、列挙のClone/Reset/Skip、終端・部分取得時の
生HRESULT S_FALSEを確認する。Infoが残る間はTIP解放後もDLLをアンロード不可とする。

`edit-test` に3ケースを追加し、この環境の実TSFで成功した。

- 選択部分だけにGUID_PROP_ATTRIBUTEのGUID atomを付与して読み戻し、前後が未設定であることを確認して解除する。
- 許可取消後は付与を拒否する。
- 同期書込ロック拒否時は付与を拒否する。

本文と専用ホストのUndo履歴は変更しない。この3ケースの属性検証は開発用feature `spike` の
`probe_underline` で行う。
属性操作前に編集対象を再検査し、既存の属性runがある場合は上書きせず拒否する。
付与・確認・解除は同じ同期書込セッション内で行うため、待機中に下線を表示し続ける機能ではない。
確認失敗時にも解除を試み、解除が失敗した場合は検証全体を失敗として返す。
表示属性プロバイダーのカテゴリを永続登録しておらず、実アプリが描画する下線は未確認。
`edit-test` の属性操作コードはEXEへRustライブラリとしてリンクされる。
プロバイダーのCOM経路はロードexampleで実DLLを使って確認する。

## 編集セッション間のプレビュー保持

開発用feature `spike` に `edit::preview::Preview` を追加した。
beginで対象と認可世代を記録して下線属性を付与し、編集ロックの終了後も保持する。
cancelは元の範囲のSumibi属性runだけを解除し、本文は読まない。別の表示属性を解除しないよう
値を確認し、列挙されたrunの端点も元の対象範囲へ制限する。
同期書込ロックが拒否された場合は状態を保持し、明示的な解除を再試行できる。
Dropの解除はベストエフォートであり、ロック拒否時の解除完了を保証しない。

commitはフォーカス・選択・原文・認可世代を再検査して下線を解除し、さらに再検査して1回だけ置換する。
失敗後に新しい選択を取り直して古い結果を適用しない。本文の書換成功後に失敗し得る処理を置かない。
原文が変更された場合、属性はTSF自身によって破棄されることもある。

`edit-test` に保持・取消・確定・許可取消・選択変更・原文変更・別contextへの移動・
解除ロック拒否からの再試行・Dropを組み合わせた8ケースを追加し、全件成功した。
確定のみ本文を書き換えてUndoを1件追加する。取消では本文読取数も増えない。

検証用ISpikeControlにBeginPreview/CancelPreview/VerifyPreviewを追加し、実DLLでも確認した。
BeginPreviewは専用ホストが明示的に呼び、プレビュー中の予約キーコマンドは保持済み対象を確定する。
許可取消・Deactivateでは解除を試み、失敗時は編集権限を先に失効させた上で解除用の状態を保持する。
今回のkey-testでは取消と許可取消の両方でロック拒否・解除再試行が成功し、固定変換と1回のUndoも成功した。

これは待機状態と確定の検証であり、LLM通信・待機タイマー・実アプリへの常設登録はまだ行わない。
原文・選択変更の自動取消とEsc・Enterの処理は次節の通り追加した。
実打鍵との接続と通常アプリでの下線描画は未確認。専用ホストの描画は後述する。

## 通知による自動取消とEsc・Enter

開発用の許可設定時、contextのITfTextEditSinkとmanagerのITfThreadMgrEventSinkを登録する。
通知オブジェクトはControllerへのWeak参照だけを持ち、解除時は登録cookieを使う。
原文・選択変更をOnEndEditのメタデータから検出し、確定を直ちに無効化する。
通知のread-only cookieで本文や属性を書き換えず、下線解除だけをTF_ES_ASYNCの書込セッションへ送る。
非同期コールバックはWeak参照・世代・取消状態を検査し、新しいプレビューには作用しない。
解除が拒否されても古い対象の確定を許可せず、明示取消／許可取消で解除を再試行できる。

通常Enterはプレビューを取り消して原文を保持し、OnKeyDownでFALSEを返す。固定変換は実行しない。
Escはプレビューの取消を消費し、その後のEscは通過する。ロック拒否時も取消を優先する。
OnTestKeyDownは状態確認だけを行い、有効なプレビューのEnter/EscでTRUEを返す。
プレビューなし、異なるcontext、修飾キー付き、キー解放は通過する。本文やキーイベントの再送は行わない。

`key-test` で追加確認した項目:

- 実TSFの原文変更・選択変更通知でプレビューを自動取消し、下線を解除する。
- 古い解除要求の後に開始した新しいプレビューの下線を保持する。
- DLLのキーコールバックでEsc取消、Enter原文通過、修飾キー・別contextの通過を確認する。
- Esc時に解除ロックを拒否しても確定を無効化し、後で解除できる。
- DLLのフォーカスコールバックで別contextへの移動を取り消し、許可を失効する。
- 通知や取消の後もUndo履歴を増やさず、終了時にDLLがアンロード可能になる。

この環境では画面を持たないホストのTestKeyDown/KeyDownから通常キーのコールバックが呼ばれず、
SetFocusだけではOnSetFocus通知も発生しなかった。そのため検証用GetFunctionにキー／manager通知sinkの
取得経路を追加し、これらは実DLLの同じ実装を直接呼んでいる。原文・選択変更は実TSFからの通知を使用する。
この画面なし構成では通常キーの配送と実ウィンドウによるフォーカス通知を確認できない。
実ウィンドウを追加した次節の検証では、この2点も成功した。

## 実ウィンドウと通常キー配送

```powershell
cargo build --workspace --locked --offline
cargo run -p sumibi-setup --locked --offline -- window-test --dll target/debug/sumibi_tip.dll
```

`window-test` は、専用ITextStoreACPに対応する短時間のネイティブウィンドウを作成する。
GetWndでそのHWNDを返し、AssociateFocusで文書とウィンドウを関連付ける。
ウィンドウをアクティブにしない構成では通常キーが届かなかったため、この検証では一時的に
検証ウィンドウがアクティブになる。実行には対話デスクトップが必要で、通常のcargo testには含めない。
終了時に関連付けを復元してウィンドウを破棄し、前面が検証ウィンドウのままなら元の前面への復帰を試みる。
既定IMEの変更、キーのデスクトップ注入、他アプリの入力操作は行わない。

この環境で、以下を確認した。

- contextのGetActiveView/GetWndが検証ウィンドウのHWNDを返す。
- プレビュー中のEsc/EnterはITfKeystrokeMgrのTestKeyDown/KeyDownからDLLへ配送される。
  Escは取消を消費し、Enterは原文を保持して未消費として返る。直接DLL呼出しへのフォールバックはしない。
- 別ウィンドウへフォーカスを移すと関連するTSF文書も移り、実際のフォーカス通知でプレビューを取り消す。
  このケースでも通知コールバックを直接呼ぶフォールバックはしない。
- 許可されていない別入力欄では固定変換せず、本文を保持する。
- 予約キーによる固定変換、1回のUndo、原文・選択変更による取消、世代をまたぐ解除保護も成功する。
- ウィンドウとの関連付けの解除、COM/TSF登録の解除、DLLのアンロード可能状態まで成功する。

AssociateFocusは文書への参照を保持しないため、関連付けガードが文書を保持する。
以前の関連付けがない場合のnull出力は正常な結果として扱い、生ABIで受け取って復元する。
SetFocusが返す以前のHWNDもnullになり得るため、結果の成否はGetFocusで新しい対象を確認して判定する。

`window-test` のウィンドウは自動検証用のSTATICを基にしている。本文と下線の描画は次節で説明する。
キーボード編集用の `input-test` と `demo` はさらに後述する。
キーテストはTSFのキーAPIをプログラムから呼び、実キーボードの打鍵を再現しない。
実キーボード操作の手動確認、実アプリでの互換性、入力スコープ分類は残っている。

## 本文と下線の描画

`window-test` に、専用文書の本文と表示属性を読み取って描画する検証を追加した。
同期readセッションで本文とGUID_PROP_ATTRIBUTEの各runを読み、GUID atomをGUIDへ戻して、
実DLLのITfDisplayAttributeProvider/GetDisplayAttributeInfoから表示定義を取得する。
入力はUTF-16の妥当性と1024単位の上限を検査し、属性runの端点も本文内に制限する。
読取cookieは保持せず、遅れて呼ばれたコールバックは結果を生成しない。

表示属性プロバイダーはプロセス内COM登録から直接取得する。カテゴリの永続登録と
ITfDisplayAttributeMgrを経由する汎用アプリ向けの解決経路は、まだ検証していない。
この描画器はSumibiの固定点線属性だけを扱い、他のスタイルや複雑な文字組みを実装しない。

本文をGDIで32-bit DIBへ描き、測定した選択範囲の幅に沿って点線を付ける。
ウィンドウのWM_PAINTはキャッシュ済み画素だけを表示し、COM呼出しや本文読取を行わない。
BeginPaint/EndPaint、メモリーDC・DIB・選択したGDIオブジェクトの寿命を管理し、
ウィンドウコールバックからRustのpanicを外へ出さない。

この環境で次の5状態を確認した。

| 状態 | 本文 | 下線 |
| --- | --- | --- |
| original | prefix nihongo suffix | なし |
| preview | prefix nihongo suffix | nihongoだけに点線 |
| cancelled | prefix nihongo suffix | なし |
| converted | prefix 日本語 suffix | なし |
| undo | prefix nihongo suffix | なし |

各段階で本文・属性run・画素を検査し、WM_PAINTが成功したことを確認する。
プレビューでは点線に黒画素と空白があり、範囲の前後に下線画素がないことを検査する。
取消はUndo履歴を増やさず、確定は1件追加する。同じ描画バッファを
`target/window-test/{original,preview,cancelled,converted,undo}.bmp` に保存する。
previewとconvertedの保存画像も目視確認した。これらはデスクトップのスクリーンショットではない。

描画更新は次節の変更通知に接続した。入力メッセージからの本文編集も後述の専用デモへ追加した。
Notepad等の互換性確認を完了したものとは扱わない。

## 変更通知からの表示自動更新

専用ホストに `live_render::Binding` を追加した。contextのITfTextEditSinkへ登録し、
OnEndEditで本文変更と属性だけの変更の両方を受ける。通知中は編集セッションを要求せず、
対象ウィンドウへWM_APPの更新要求だけをPostMessageで送る。通知cookieは保持しない。
更新要求の処理で本文／属性を同期読取し、描画バッファを差し替えてWM_PAINTを実行する。
WM_PAINT自体は引き続きキャッシュ済み画素だけを扱う。

同じ表示接続の予約を集約し、複数の要求から1回だけ読み直す。
各接続にはプロセス全体で一意の識別子を割り当て、解除・再接続・HWND再利用をまたぐ古い予約を拒否する。
通知オブジェクトと更新コールバックはWeak参照を使い、window callbackは処理中のViewをRcで保持する。
読取後にも接続の有効性を検査し、COM呼出し中の再入によって接続が変わった場合は古い結果を表示しない。
ウィンドウ終了時は接続を無効化し、破棄済みHWNDへ読取要求を送らない。

読取・描画生成が失敗した場合は古い本文／下線のキャッシュを破棄して空白に描き直す。
自動再試行のループは作らず、次の変更通知または明示的な更新要求で復帰する。
通知の解除に失敗した場合も先に読取を停止し、登録cookieを保持して解除を再試行できる。

`window-test` では各段階の手動capture/draw/present呼出しを外し、次を確認した。

- 原文・プレビュー・取消・確定・Undoが通知から自動表示され、本文・属性・画素が期待どおりになる。
- 連続する更新要求は1回へ集約され、読取や描画が自分自身を更新するループを起こさない。
- 読取ロックを拒否すると古い下線を含む表示が消え、次の通知で正常表示へ復帰する。
- 予約済みのまま接続を交換しても、旧予約は新しい表示の読取を実行しない。
- ウィンドウを予約済みのまま終了しても読取せず、その後に作った表示へ作用しない。
- 通知・関連付け・COM/TSF登録を解除し、最後にDLLがアンロード可能になる。

この接続は専用ホストが所有する合成文書だけを対象とし、通常アプリや機密入力欄を自動的に許可しない。

## 入力メッセージと手動デモ

`input-test` と `demo` を追加した。どちらも同じEditorとネイティブのメッセージ処理を使う。
メッセージループはWM_KEYDOWN/UPをまずITfKeystrokeMgrへ渡し、TestがTRUEのときだけKeyDown/Upを呼ぶ。
TSFが消費したキーにはTranslateMessageや入力先の処理を実行せず、二重のWM_CHAR生成を防ぐ。
未消費のキーだけをTranslateMessage/DispatchMessageへ渡し、WM_CHARでホスト自身の本文を変更する。

ホスト編集はTSFロックの外で、UTF-16境界・上限・読取専用状態を検査して行う。
本文と選択を一致させてからOnTextChange/OnSelectionChangeを通知し、表示は既存の自動更新経路へ流す。
UTF-16サロゲート対は両方が揃うまで保留し、不完全な対を本文へ挿入しない。
Backspaceは対を分断せず1スカラーを消す。結合文字を含む書記素クラスタ全体の削除はまだ実装しない。
左右キーとDeleteもサロゲート対を分断しない。Shiftと左右・Home・Endの組合せで、
選択の始点を保持して範囲を伸縮する。選択の向きはTS_SELECTIONSTYLEのaseへ反映し、
Undoでも復元する。選択変更は既存の通知経路でプレビューを取り消す。
Ctrl/Alt付きの移動とShift+Deleteの切り取りは未実装。
WM_CHARの繰返し回数も上限内で反映し、1回の編集としてUndoへ記録する。

F6で専用入力欄を許可してプレビューし、F7で固定変換する。Enterは原文を保持する1行の確認操作で、
改行を挿入しない。TSFの取消処理を通過した後のWM_CHARで確認し、確認回数が1回であることも検査する。
描画にはカーソルと選択背景を追加した。空文書も表示する。

Ctrl+YのRedoもホスト側で処理する。Undo/Redoは本文と選択・選択の向きを同時に戻してから
OnTextChange/OnSelectionChangeを通知する。履歴操作でプレビューの下線は復活させない。
新しいホスト編集とTSFのSetTextはRedo履歴を破棄する。読取専用・ロック中は履歴を消費しない。

`input-test` では次の検証が成功した。

- WM_CHARの入力・Backspace・Undoと自動再描画。
- WM_KEYDOWNからTSFを経由したEsc取消とEnter原文確認。Enterは1回だけ処理され、固定変換しない。
- F6プレビュー→F7固定変換→1回のUndoで原文を復元。
- 絵文字の入力・削除・Undo、不正な下位サロゲートの無視、読取専用時の非編集。
- Home/Endのカーソル移動、サロゲート境界とサイズ上限の拒否。
- WM_CHARの繰返し回数とUndoの単位。
- 左右キーとDeleteの実メッセージ経路、読取専用時のDelete抑止、空削除でUndo履歴を増やさないこと。
- Shift選択の向き・始点越え・Home/End、選択部分だけの変換・取消・DeleteとUndo。
- 入力・Delete・固定変換のRedo、選択の復元、新規編集での古いRedo破棄、読取専用・ロック時の履歴保持、Redoによるプレビュー取消。
- WM_CLOSEで許可を失効し、ウィンドウ破棄・TSF/COM解除・DLLのアンロード可能状態まで成功。

テストメッセージはこのプロセスが所有するウィンドウだけへPostMessageで送る。
SendInputやグローバルなキーボード状態の変更は使用しない。実キーボードの打鍵を検証済みとは扱わない。
Shift選択は同じ選択処理へ明示的に修飾状態を渡して検証する。実打鍵時のShift状態の取得は手動確認が必要。
`demo` のGetMessageループはウィンドウが閉じられるまで動作する。
自動検証には `input-test`、手動試験には `demo` を使用する。操作はDEVELOPMENT.mdを参照。

## APIの根拠

- [AdviseKeyEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-advisekeyeventsink)
- [UnadviseKeyEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-unadvisekeyeventsink)
- [ITfClientId::GetClientId](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfclientid-getclientid)
- [CoRegisterClassObject](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-coregisterclassobject)
- [RegisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile)
- [ActivateProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-activateprofile)
- [UnregisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-unregisterprofile)
- [RequestEditSession](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontext-requesteditsession)
- [ITextStoreACP::RequestLock](https://learn.microsoft.com/en-us/windows/win32/api/textstor/nf-textstor-itextstoreacp-requestlock)
- [PreserveKey](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-preservekey)
- [SimulatePreservedKey](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-simulatepreservedkey)
- [Providing Display Attributes](https://learn.microsoft.com/en-us/windows/win32/tsf/providing-display-attributes)
- [Using Display Attributes](https://learn.microsoft.com/en-us/windows/win32/tsf/using-display-attributes)
- [ITfProperty::SetValue](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfproperty-setvalue)
- [ITfReadOnlyProperty::EnumRanges](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfreadonlyproperty-enumranges)
- [ITfTextEditSink::OnEndEdit](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itftexteditsink-onendedit)
- [ITfKeystrokeMgr::TestKeyDown](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-testkeydown)
- [ITfThreadMgrEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfthreadmgreventsink)
- [ITfThreadMgr::AssociateFocus](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfthreadmgr-associatefocus)
- [ITextStoreACP::GetWnd](https://learn.microsoft.com/en-us/windows/win32/api/textstor/nf-textstor-itextstoreacp-getwnd)
- [WM_APP](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-app)
- [ITfKeystrokeMgr::KeyDown](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-keydown)
- [WM_CHAR](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-char)
