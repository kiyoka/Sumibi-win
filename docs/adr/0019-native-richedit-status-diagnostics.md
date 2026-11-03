# ADR 0019: 標準RichEditでの拒否検証と本文を持たない状態診断

2026-10-22 / Issue #4 / PR #10。基準は34c2394。
標準コントロールの互換性調査と診断機能を実装。RichEditでの変換は未対応。

## 問題と判断

前段階でManualEditのITfRangeACP必須要件を除いたが、専用TextStoreだけでは
Windows標準コントロールの保護情報が現在の許可条件に合うかを確認できなかった。
そこで実際のMsftedit.dll/RICHEDIT50Wを専用の子windowとして作り、native TSF contextを検査する。
本文は固定の合成データだけで、既存の他アプリや利用者文書へ接続しない。

観測した通常欄はdwDynamicFlags=0、dwStaticFlags=0x2（TS_SS_REGIONS）であった。
TM_PLAINTEXTと既定のrich textの両方でTS_SS_NOHIDDENTEXTがなく、現在の保護gateは拒否する。
これは「実際に隠し文字が存在する」という判定ではなく、隠し文字がないとの肯定的保証がない状態である。
plain textの設定だけでTSFのstatusも変わるとは仮定しない。

現状を対応済みにするためのstatus偽装・compartment初期化・保護条件緩和は行わない。
拒否条件を固定の診断コードとして取り出し、実際のnative contextで本文取得前の拒否を確認する。

## 診断API

tip/edit.rsからinput_status_denial(TS_STATUS)とInputStatusDenialを公開する。
純粋なmetadataの分類であり、本文・scope object・COM参照・cookieを受け取らない。

| コード | 条件 |
| --- | --- |
| ReadOnly / Loading | 読取専用またはロード中 |
| ReservedDynamicFlags / UnknownDynamicFlags | 予約状態または未確認のdynamic bit |
| HiddenTextPossible | TS_SS_NOHIDDENTEXTの肯定的保証がない |
| MultipleSelections / MultipleRegions / TransitoryContext | 現在未対応のstatic bit |
| UnknownStaticFlags | 未確認のstatic bit |

複数の不成立条件があれば上表の順に最初の理由を返す。
Noneはstatus部分だけの成立であり、scope・compartment・focus・selection・compositionの許可ではない。
実際のprotection.classifyも同じ関数を使用し、診断と許可条件の分岐を別実装にしない。
許可されるstatusの集合、scopeのallowlist、失効履歴、本文取得・編集時の検査は維持する。
型には固定コードだけを格納し、本文ログを追加しない。wire/JSON版1にも変更しない。

## Native fixtureと寿命

native-edit-testはSystem32限定でMsftedit.dllを読み、所有STAに一時的なRichEdit子windowを作る。
空の状態でEM_SETTEXTMODEを設定し、SES_USECTFを有効にする。
SetInputScopeは同じthreadで所有するwindowだけに使い、破棄前にIS_DEFAULTで解除する。
固定済みwindows crateにこの関数のbindingがないため、既存TSF moduleの正式exportを文書化されたABIで解決する。

独自TextStore、CreateContext、AssociateFocus、診断compartmentや共有composition sinkは使わない。
manager.GetFocus/GetTopからRichEdit自身のcontextを取得する。
ManualEditの失敗とstatus診断を別に記録し、本文取得counter不変・native Undoなし・元本文不変を検査する。
本文比較のためのGetWindowTextWは固定fixtureの確認だけで、診断・変換adapterの処理には含めない。

COM参照を解放してからscope解除・DestroyWindow・loader参照解放を行う。
破棄失敗時には生きたwindow procedureのDLLを強制アンロードしない。
既存probeと同じprocess内登録・解除を使い、既定IMEや常設登録を変更しない。

## 実測と検証

Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1 / 自動検証。

| 構成 | 実際のstatus / 観測結果 |
| --- | --- |
| plain-text / rich-text | dynamic=0、static=0x2。HiddenTextPossible、captureはE_ACCESSDENIED |
| private / default-scope | 同じstatusで拒否。native scope分類へ到達したとの主張はしない |
| password（固定合成本文） | GetStatusはTF_E_EMPTYCONTEXT。ManualEditの準備はE_NOINTERFACEで拒否 |
| readonly | dynamic=1、static=0x2。診断はReadOnly、captureはE_ABORTで拒否 |
| readonly-restored | readonly時に拒否し、解除後もHiddenTextPossibleで新規captureを拒否 |

7ケースすべてでadapterのGetText counter増加0、native Undoなし、fixture本文不変を確認した。
counterはsetupにリンクしたTIPライブラリのもので、ロードDLLとOS内部読取のcounterではない。
readonlyのcapture HRESULTをstatus分類と同じ理由コードに読み替えない。
想定と異なるstatusやHRESULTを返す環境では検査を停止し、互換性の再評価を必要とする。

TIPに1テストを追加し、固定の各拒否理由と、statusが成立してもpassword/private/defaultを許可しないことを確認。
workspaceは66件（Core 26・Protocol 22・IPC 12・TIP 6）。
Test-Spike.ps1へnative-edit-testを追加。ログはtarget/issue-4-native-edit.logと
target/issue-4-native-edit-regression.log。下記message pump修正後、Test-Spike.ps1を省略なしで実行して成功した。
66件、fmt・各構成Clippy・build・load_smoke、既存native/pipe/peerと解除・DLLアンロードを確認した。
最終試験後の診断process残留もなかった。

## 入力デモのTSFメッセージ処理

初回全回帰は既存input-notify-testのfocusで通知event待機が失敗し、単独でも再現した。
SetFocus(None)が成功してもGetFocusに文書が残ることを観測したため、実contextを持つ別文書へ
明示的に切り替え、前後のIUnknown identityを検査する試験へ修正した。
別文書は入力を記録する前に用意し、OnInitDocumentMgrによるwakeをfocus通知の代用にしない。

それでも通常のuser32 PeekMessageだけでは通知待機が完了せず、同じ処理をITfMessagePumpへ通すと
OnSetFocusとwakeが届き、20ケースが成功した。これは今回の環境で観測した切り分け結果である。
input_hostの有限pumpとblocking demo loopをITfMessagePump.PeekMessageW/GetMessageWへ変更した。
MsgWaitForMultipleObjectsEx後も同じ有限pumpを通り、IPCや救済の待機構造は維持する。
通知callbackに本文取得・編集・待機を追加せず、従来の所有loopでcleanupする。
512件の処理上限・2秒の試験待機・本文/Undo/失効のassertionも維持する。

Windowは生成時に所有managerのmessage pumpを保持し、windowとともに所有STAで解放する。
focus以外の通知エラーにもケース名とevent待機段階を付け、今後の診断を可能にした。
初回ログはtarget/issue-4-native-edit-regression-focus-failure.log、元の単独再現は
target/issue-4-native-edit-notify-check.log、修正後20件はtarget/issue-4-native-edit-notify-fixed.log。
他の試験のcleanup用SetFocus(None)まで置換した変更ではない。

後続の[ADR 0020](0020-owner-cancellation-and-shared-tsf-pump.md)ではfocus待機の再発を確認した。
本ADRの成功は記載した実行の観測で、pump変更による全focus履歴の通知保証ではない。
後続は所有ホストの明示境界を追加し、自然通知のみの試験と区別している。

## 残る作業

この結果は自作メモリーstoreから標準コントロールへ検証を広げた段階で、変換・標準Undoの成功ではない。
一般アプリで安全に扱える範囲の根拠、隠し文字・領域境界・欠損metadataの扱いを先に決める必要がある。
Notepad/Word/VS Code等の実アプリ、実打鍵、非ACP入力先、所有compositionのsink寿命、
一般アプリの非同期配送と配置/認証は引き続き未検証・未完了。
専用ACP fixtureでの既存の成功検査をnative RichEdit対応へ一般化しない。

## 2026-10-25: 標準EDITの追加観測

同じ専用親window内にWindows標準の`EDIT`子windowを作り、固定合成本文を
設定してfocusした。既定scopeと`SetInputScope(IS_TEXT)`の両方で検査した。
今回のWindows 11環境では、どちらもHWNDのfocusは成立したが、
`ITfThreadMgr::GetFocus`はTSF文書を返さなかった。Rust bindingでは
成功HRESULTとnull出力が`Err(S_OK)`として現れた。
そのためstatus、入力scope、変換可否へは進めなかった。

検査は両構成でadapter本文読取counterの不変、native Undo履歴なし、固定本文不変、
TSF/COM解除とDLL unloadを確認する。別OSや入力サービスでcontextが現れた場合は
statusが現在の保護条件で拒否されることを要求し、肯定的statusへ変われば
自動検査を停止して互換性を再評価する。標準EDIT、RichEdit、一般アプリの
いずれも変換成功の主張には含めない。

## 公式APIの根拠

- [TS_STATUS](https://learn.microsoft.com/en-us/windows/win32/api/textstor/ns-textstor-ts_status)：static/dynamic flagの意味。
- [EM_SETTEXTMODE](https://learn.microsoft.com/en-us/windows/win32/controls/em-settextmode)：空コントロールのplain/rich設定。
- [EM_SETEDITSTYLE](https://learn.microsoft.com/en-us/windows/win32/controls/em-seteditstyle)、[EM_GETEDITSTYLE](https://learn.microsoft.com/en-us/windows/win32/controls/em-geteditstyle)：CTF等のstyle。
- [SetInputScope](https://learn.microsoft.com/en-us/windows/win32/api/inputscope/nf-inputscope-setinputscope)：同じthreadのwindowへの設定と破棄前の解除。
- [GetFocus](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfthreadmgr-getfocus)：現在のdocument managerの取得。
- [SetFocus](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfthreadmgr-setfocus)：非null文書への明示的な切替。
- [ITfMessagePump](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfmessagepump)：TSFがメッセージ処理の前後で必要な処理を実行するwrapper。
