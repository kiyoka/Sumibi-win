# ADR 0006: 本文取得・送信前・適用時の入力保護検査

2026-10-21 / Issue #4 / PR #10。[ADR 0005](0005-dll-key-broker-bridge.md)の専用ACP経路へ追加。
一般アプリの機密欄を完全に識別できるという保証や、Issue #4全体の完了を意味しない。

## 許可条件

明示Grantだけで本文を取得せず、入力先のscope・status・compartmentを確認する。
欠損、未対応、型不一致、API失敗を許可へ変換しない。

| 観測 | 条件 |
| --- | --- |
| scope | 非空、最大16件。全要素がIS_TEXT・IS_CHAT・IS_CHAT_WITHOUT_EMOJIのいずれか |
| 特殊scope | password、numeric password、各種PIN、private、login、default、grammar、未レビュー値を拒否。通常値との混在も拒否 |
| dynamic status | READONLY・LOADING・RESERVED・不明bitを拒否 |
| static status | NOHIDDENTEXTが必要。複数領域・複数選択・一時context・UWP control等の未対応構造も拒否 |
| compartments | KEYBOARD_DISABLED・EMPTYCONTEXTとも明示的なVT_I4の0だけ。未設定・非ゼロ・別型は拒否 |
| app property | GUID_PROP_INPUTSCOPEをGetAppPropertyで取得。未対応S_FALSE、空・部分・混在値、IUnknown/ITfInputScopeへ変換できない値を拒否 |

InputScopeは認識用のヒントであり、外部送信への同意を代替しない。
独自の機密欄や、機密欄を通常テキストと偽って報告するホストを全て識別する保証はない。
一般アプリで判定不能なら非対応として扱う。IS_DEFAULTや省略から通常テキストと推測しないため、
現段階では多くの一般アプリを許可しない可能性がある。

## コードと寿命

`tip/protection.rs`はstatusとscope集合の有限なStampを作る。scopeを正規順序にし、
許可される種類の間の変更やstatus変更でも旧Stampと区別する。
`edit.rs`はDLLがITfRange.GetTextを呼ぶ前、分割読取の前後、書込セッションで原文を再照合する前に検査する。
従来の専用プレビューも同じ編集基盤を通り、Grantは検査を迂回しない。

ManualEdit.sourceは送信用Stringを返す直前、別の同期readセッションでメタデータを再取得する。
通知が欠けても現在private等であれば拒否し、DLLはここで本文を再取得しない。
guardは本文／属性／context／focusに加えて2つのcompartmentの変更通知を保持する。
一度変更して0へ戻しても失効する。Controllerもscope属性とcompartment通知から取消を伝播する。
弱参照とDLL leaseを保持し、所有threadを確認してからRcへ触れる。

GetInputScopesのcountを検査してから配列を参照し、CoTaskMemFreeで解放する。
エラー時の部分初期化配列もRAIIで解放する。phrase・grammar・XML等の取得APIは呼ばない。
TIPがTSFの状態を書き換えて未知の入力先を許可状態へ変換することはしない。
setup/scope_fixture.rsの0初期化は、setup自身が所有する合成contextだけのホスト動作である。
scopeもITextStoreACPの属性要求・返却を通してホストが報告する。

## Windows内部の読取と検証境界

MSCTFが属性の問い合わせ等を処理する過程でITextStoreACP.GetTextを呼び、ホスト本文を内部参照することを観測した。
空範囲の問い合わせでも回避できるとは扱わず、「OSを含めて一切本文を読まない」という保証をしない。
spike限定export SumibiSpikeBodyReadCountで実DLLがITfRange.GetTextを呼んだ回数を直接測り、
ホスト側のコピー回数と区別する。このexportは本文・対象・resetを公開せず、非spike DLLには含めない。
初めから保護／不明な27ケースで、DLLの本文取得呼出し0・Broker job開始0を確認した。
MSCTF内部コピーの観測数は実行ごとに変わり、合否の固定値にしない。合成本文だけを使う。

本文照会中にホストが保護指定を変えるケースも取得後に再検査し、要求を送信しない。
OS等が既に取得した値の消去を保証する機能ではない。
TSFは本文をキャッシュするため、編集確認ごとに実ホストGetTextが呼ばれるとも仮定しない。
読取不能への変更はホストの正式なstatus通知で表す。

## 検証記録

2026-10-21 / Windows 11 x64 build 26300 / 通常ユーザーmedium / Rust 1.99.1。

- workspace 39件。従来36件にscope全組合せの拒否、status/容量、Stampの差異を扱う3件を追加。
- protection-test 39ケース：通常scope4、初期保護／不明27、照会中の指定変更1、
  応答待ち中のscope/compartment変更・復元5、通知なしのsource直前検査1、TSF側での不正型拒否1。
  compartmentの別型はWindowsのSetValue自体が拒否し、DLLでその値を受け取れたとは扱わない。
- broker-key-test 17、broker-input-test 7、broker-test 19、close-test 4ケースが成功。
  入力loopには待機中WM_KILLFOCUSでの取消も追加した。
- 画面なしSetFocusがdesktop通知を必ず配信すると仮定しない。有効な別欄を先に用意し、
  DLL経路は同じDLLのfocus callback、直接アダプター経路は所有ホストの明示取消で検証する。
  物理的な前面window移動・実打鍵の成功へ読み替えない。
- 既存read-deniedケースはキャッシュを考慮し、LOADINGのホスト通知も行う。
  未通知のGetText返却制限だけで既存キャッシュが失効すると仮定しない。
- fmt・各構成Clippy・ビルド・load_smoke・self/edit/key検査、解除・DLLアンロードも成功。
  ログはtarget/issue-4-protection-*.log。

前段階cd1dc2fのbroker-demoについて、利用者から「おおむねうごいています」と手動試用の報告を得た。
概括的な報告であり、本ADRの保護入力や個別キー操作の手動合格記録ではない。
一般アプリ、製品composition、Broker発見・常駐dispatch、HTTP・同意UI・設定・資格情報は残る。

根拠: [ITfInputScope](https://learn.microsoft.com/en-us/windows/win32/api/inputscope/nn-inputscope-itfinputscope)、
[InputScope](https://learn.microsoft.com/en-us/windows/win32/api/inputscope/ne-inputscope-inputscope)、
[GetInputScopes](https://learn.microsoft.com/en-us/windows/win32/api/inputscope/nf-inputscope-itfinputscope-getinputscopes)、
[TS_STATUS](https://learn.microsoft.com/en-us/windows/win32/api/textstor/ns-textstor-ts_status)、
[Predefined Compartments](https://learn.microsoft.com/en-us/windows/win32/tsf/predefined-compartments)、
[GetValue](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfreadonlyproperty-getvalue)。
