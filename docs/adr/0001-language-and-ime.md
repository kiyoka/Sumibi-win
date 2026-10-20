# ADR 0001: C++20とTSFを用いる初期構成

日付: 2026-10-20 / 状態: 初期採用案（Windows実機スパイクで再評価） / 関連: [#1](https://github.com/kiyoka/Sumibi-win/issues/1)

## 判断

C++20、Windows SDK、TSF（Text Services Framework）のネイティブCOM DLLを第一候補とする。
入力状態機械とユーザー権限の通信・設定EXEも当初はC++20に揃える。
CMake＋MSVCでビルドし、Windows CIで検証する案。SDKとコンパイラーの具体的バージョンは
最初に実ビルドが成立した組合せを固定する。依存ライブラリは必要性・ライセンスを確認して追加する。
この文書の段階では製品コード、依存導入、CI設定、Windowsでのビルド検証はない。

## 比較

| 候補 | 本プロジェクトでの利点 | 負担と判断 |
| --- | --- | --- |
| C++20 | Windows SDKのCOM/TSFに直接対応でき、Microsoft SampleIMEの構成を参照しやすい。DLLとEXEを同じ道具で構築できる | メモリ・COM寿命管理の責任が大きい。RAII、スマートポインター、静的解析、境界テストを必須にして採用案とする |
| Rust | 状態機械や通信のメモリ安全性、列挙型による状態表現に魅力がある | COM境界、TSFサンプルとの対応、FFIとWindows登録を含めて習得・検証する範囲が増える。実装不能という判断ではないが、初期試作では見送る |
| C#/.NET | 設定UIと非同期通信を記述しやすい | ホスト内TSF DLLまで同一構成にするにはCOM公開・ランタイム配置を別途検証する必要がある。C++ DLL＋C# EXEも可能な候補だが、当初は言語・配布構成を増やさない |
| Swift | Macの状態機械を読み替えやすい | InputMethodKit/AppKitはWindowsへ移植できず、TSF境界は別実装となる。Swift採用だけで移植量が減るとは判断しない |

選定理由はWindowsとの接続を最短で確かめること。処理速度の実測比較は行っていない。
C++のリスクが実装上大きくなれば、CoreまたはEXEだけRustへ分けるADRを追加する。
UIが複雑化した場合のC#採用も再評価対象で、現段階の決定で将来の選択を閉じない。

## TSFとプロセス

MicrosoftはカスタムIMEの実装にTSFを案内している。ホストにロードされるDLLはホストの制約を受ける。
このため入力・範囲編集だけをDLLに残し、通信・秘密キー・設定管理をユーザーセッションのEXEに置く案とする。
ただし、別プロセス化すればAppContainerから利用可能になるとは限らない。
IPCが許される範囲を実機で検証し、制約を迂回する実装は行わない。

- DLL: TSFイベント、composition、範囲照合、候補UI、状態機械。APIキーを持たず、通信を待ってUIを停止しない。
- EXE: 通信・資格情報保存・設定UI・トレイ表示・回数制限。TSFの範囲やCOMオブジェクトを受け取らない。
- EXEが停止・不通でも、DLLは原文と入力操作を保全し、変換だけを固定エラーにする。
- キーボードフックやクリップボード貼付けを入力方式の基盤にしない。

## 対応範囲の提案

最初の試作はWindows 11 x64、通常権限のデスクトップアプリ、物理キーボード。
製品の最低OSビルドは未確定。Windows 10、ARM64、AppContainer、昇格アプリ、タッチ、RDPは
別検証とし、この段階で対応を約束しない。x64 OS上のx86アプリには対応するDLLの検証が必要で、
x64 DLLだけで検証済みにはしない。x86ホスト対応は公開判断までの検証項目に置く。

## 採用案を確かめる条件

1. クリーンなWindows環境でビルド・登録・解除できる。
2. 下線入力、Enterの一度だけの到達、Esc、選択変換、原文保持が成立する。
3. 模擬遅延応答が別入力欄へ適用されず、ホストの1回のUndoで変換前へ戻る。
4. EXE切断、32/64ビット、IPCのアクセス拒否とサイズ違反を確認する。
5. AppContainer等の失敗は対応不可・保留として明示し、広い権限で回避しない。

## 公式資料（2026-10-20参照）

- [IME requirements](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements)
- [Microsoft SampleIME](https://github.com/microsoft/Windows-classic-samples/tree/main/Samples/IME/cpp/SampleIME)
- [TSF](https://learn.microsoft.com/en-us/windows/win32/tsf/text-services-framework)

Microsoft資料はAPI上の根拠、言語比較・分割構成は本プロジェクトの設計判断である。
SampleIMEのコードを取り込む場合は、その時点のライセンスと必要な表示を確認する。
