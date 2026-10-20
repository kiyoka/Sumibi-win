# ADR 0001: RustとTSFを採用する

日付: 2026-10-20 / 状態: 採用（Windows実機での成立確認は未実施）
関連: [Issue #1](https://github.com/kiyoka/Sumibi-win/issues/1)

## 決定と利用者の条件

メモリ安全性・長期保守を優先するという利用者の回答に基づき、Rustを採用する。
確認できる実機はWindows 11 x64、Visual Studioあり。初期案のC++20優先をこの判断で置き換える。
TSF DLL、OS非依存Core、通信・設定EXEをRustで実装する。
Windows接続はMicrosoftの`windows`クレート（windows-rs）、ビルドはCargo、
初期ターゲットは`x86_64-pc-windows-msvc`とする。Visual StudioのC++ビルドツールとWindows SDKを利用する。
Visual Studioのインストールだけで必要ワークロードが揃っているとはみなさず、実機で確認する。

Rustのstableツールチェーンと依存版は初回ビルド時に固定し、rust-toolchain.tomlとCargo.lockを管理する。
Windows SDK版もその時点で記録する。未検証の版番号をこの文書で確定しない。
CMakeは使わず、Rustのcdylibと通常のEXEを成果物にする。C++の併用は初期構成に含めない。
製品コードとWindows上のビルド実績はまだない。採用判断と動作確認の成功を区別する。

## 比較と選定理由

| 候補 | 本プロジェクトでの評価 | 結論 |
| --- | --- | --- |
| Rust | 安全なCoreで所有権と状態遷移を表現でき、TSFもwindows-rs経由で実装可能。COM・FFI境界のunsafe監査は必要 | 利用者が重視する安全性・長期保守に合わせて採用 |
| C++20 | Windows SDKと公式SampleIMEへ直接対応し、接続試作を進めやすい。一方、Coreと通信にもメモリ・寿命管理の規約が必要 | 初期案から変更。Rustに比べた本件の保守負担を理由に見送る |
| C#/.NET | 設定UIや通信に適する。COM公開の仕組みもあるが、ホスト内DLLに用いるとランタイム・配置を別途検証する必要がある | DLLには採用しない。別EXEのUI候補としては将来再評価できる |
| C++ DLL＋Rust Core/EXE | TSF公式サンプルを追いやすく、主要処理をRustにできる | 言語間ABIと二系統のビルドを初期から持つ負担があるため見送る |
| Swift | Macの設計を参照しやすいが、InputMethodKit/AppKitはWindowsへ移せずTSFは別実装になる | 採用しない |

これは本プロジェクトの優先順位による判断で、性能測定や全候補のTSF実機比較による順位ではない。
Rustでも入力範囲・世代・同意の論理的な検査は必要で、メモリ安全性だけで誤置換は防げない。
RustからCOMを実装するためにC++ラッパーが必須という前提は置かない。

## API上の根拠と推論

- Microsoftの[windows-rs](https://github.com/microsoft/windows-rs)はWindows APIとCOMのRustバインディングを提供する。
- [windows-implement](https://github.com/microsoft/windows-rs/blob/master/docs/crates/windows-implement.md)はCOMインターフェースを実装する属性を提供する。
- [ITfTextInputProcessorEx](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/TextServices/struct.ITfTextInputProcessorEx.html)のTSFバインディングを確認した。
- [.NETのCOM公開](https://learn.microsoft.com/en-us/dotnet/core/native-interop/expose-components-to-com)も可能だが、標準COM hostingには配布上の条件がある。C#で不可能とは判断しない。
- [MicrosoftのIME要件](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements)に従いTSFを使う。

以上からRustでネイティブTSF DLLを試作する根拠はある。ただしDllGetClassObject、DllCanUnloadNow、
クラスファクトリー、TSF登録・解除、各sink、スレッドと参照の寿命まで自前で成立させる必要がある。
APIが存在することは、このIMEの実装が完成・検証済みという意味ではない。資料確認日: 2026-10-20。

## クレート構成と安全性

| クレート案 | 責務 | Windows依存 |
| --- | --- | --- |
| sumibi-core | 純粋な入力状態機械・要求世代・候補管理 | なし。`#![forbid(unsafe_code)]` |
| sumibi-protocol | IPCメッセージ、形式版、長さ・値の検証 | なし。`#![forbid(unsafe_code)]` |
| sumibi-tip | cdylib、TSF/COM、範囲照合、候補、IPC接続 | あり。unsafeを境界モジュールへ限定 |
| sumibi-host | EXE、通信、辞書、設定、同意・キー、トレイ | OS API部分のみ境界へ隔離 |

unsafeブロックにはポインターの妥当性、所有権、スレッド、呼出し期間の根拠を記す。
COM・Win32の生参照をCoreや通信タスクへ渡さない。TSF操作は所有スレッドへ戻す。
RustのpanicをCOMの外へ伝播させず、ABI入口で固定エラーへ変換する。
回復不能な状態で編集を継続せず、参照とホスト原文の保護を優先する。
DllMainでは通信・待機・UI初期化を行わない。循環参照とDLLアンロード条件を検査する。

DLLとEXEはIPCで接続し、Rustの内部型をABIやプロセス境界で直接共有しない。
EXEはAPIキーと通信を担当し、DLLにはAPIキーを配らない。
別プロセス化だけでAppContainer互換が得られるとは扱わず、通常デスクトップから検証する。

## UI・通信・依存の選定範囲

初回試作はWin32の最小UIと固定の模擬変換だけ。WinUI、WebView、C#のUIランタイムを追加しない。
設定UIの本実装フレームワーク、非同期ランタイム、HTTP／JSON／Unicode用クレートは別の選定項目。
WinHTTPはWindows通信アダプターの候補として維持するが、Rustでの使い勝手と取消を確認して決定する。
依存を増やす際は更新状況・ライセンス・MSRV・unsafe境界・配布サイズを記録する。

## 検証と判断の見直し条件

1. Windows 11 x64でCargoビルド、COM/TSF登録・解除、DLLのロード／アンロードが成立する。
2. 下線、Enterの一度だけの到達、Esc、選択変換、原文保持、1回のUndoを確認する。
3. 模擬遅延・別入力欄・ホスト終了・EXE切断で古い結果が適用されない。
4. x86ホストは別DLLが必要な検証範囲。ARM64、AppContainer、昇格、RDPは未確認として管理する。
5. unsafe監査と所有スレッドへの復帰をレビューでき、CoreをOSなしでテストできる。

COMバインディングの不足が見つかった場合は、まず局所的な宣言追加を評価する。
C++ shimが不可避なら、該当API・再現例・ABI契約を示した別ADRで変更する。
不具合の原因を特定せず言語全体を変更しない。Macの0.3秒判定やclient pokeは移植しない。
