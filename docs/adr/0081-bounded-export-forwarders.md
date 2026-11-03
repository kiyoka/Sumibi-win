# ADR 0081: 明示したDLL公開表だけで転送参照を追跡する

## 目的と入口

未解決forwarderを、呼出元が与えたDLL公開表の中で追跡する。
`ExportCatalog::insert`へmodule名と`DllExports`の借用を登録し、`resolve`へ元の
moduleと`ImportSymbol`を渡す。結果は最終RVAと、元の参照から最終module/symbolまでの
`ExportStep`列である。RVAは画像内metadataで、呼出可能なpointerや認証receiptではない。

catalogはmetadataだけを借用し、file handleを所有しない。呼出元がmodule名と意図した
ファイルを結び付け、署名・配置とfileの保持条件を満たす必要がある。
DLL自身が名乗るexport directoryのmodule名を登録名に採用する処理は設けない。
ファイル検索、LoadLibrary、GetProcAddress、DLL初期化や製品起動への接続も行わない。

## 名前と転送形式

通常のmodule名はASCII英数字・hyphen・underscoreのbasenameを対象にする。
catalog登録と最初の参照では`.dll`を省略可能とし、小文字の`basename.dll`へ正規化する。
拡張子込み128byteまでとし、パス区切り、drive、親移動、空要素、余分なdotを拒否する。
同じ正規化名の再登録は上書きせず拒否する。関数名は大文字・小文字を区別する。

転送文字列は512byte以下のASCII可視文字で、`MODULE.Symbol`または`MODULE.#ordinal`
に限定する。序数は1～5桁の十進表記をu16へ変換し、符号やoverflowを拒否する。
ゼロと先頭ゼロは受け付ける。追加dotを含む転送は、`.dll`の明記やdot入り公開名を含め、
解釈を推測せず未対応として拒否する。これは限定profileであり、すべてのWindowsで
ロード可能な転送形式を扱う意味ではない。
[転送名・序数のPE形式](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)。

`api-`または`ext-`で始まる名前はAPI setとして`ApiSetUnresolved`を返す。
catalogへの物理DLLとしての登録も拒否する。group名のtildeを含むcontract aliasも
同様に未解決とする。API setはファイル名ではなくOSがschemaで解決するcontractであり、
同名ファイルや推測したhost DLLへ置き換える処理は設けない。
[Windows API sets](https://learn.microsoft.com/en-us/windows/win32/apiindex/windows-apisets)。

## 有限の追跡と失敗

catalogは128 module、追跡は16 forwarderまでとする。直接参照ならstepは1件、
16段の転送なら最大17件となる。17段目の転送は拒否する。
循環の検出keyには正規化module名と公開表slotを使い、同じ公開先を名前・序数や
aliasで切り替える循環も検出する。
module欠落、symbol欠落、API set、循環、深さ超過と不正形式は別の失敗として返す。
失敗時に別directoryを探したり、存在する別のsymbolへ切り替えるfallbackはない。

## 検証と残る条件

合成DLLによる7件で、名前と序数の多段追跡、大小文字、alias循環、module間循環、
不正なパス・転送形式と序数、API setの偽装登録拒否、欠落、長さ・128 moduleの境界、
16段の成功と17段の拒否を確認した。

実ファイル検査ではGetSystemDirectoryW由来の固定したKernel32、KernelBase、Ntdllを
read-onlyかつ書込・削除共有なしで開き、3つのhandleを保持したまま公開表を読む。
転送文字列から新しいパスは作らない。今回のHostのKernel32参照89件はすべて最終RVAへ
到達し、そのうち4件が1段の転送だった。これは今回のOSとdebug buildの観測である。

`tools/Test-HostImage.ps1`で既存のparser・公開表照合とともに再現できる。
全体回帰では既存の準備失敗テストが一度失敗し、同じtest mutexを使う4件へpoisonが
波及した。準備eventの通知だけでは、非ブロッキングsnapshotのstate lockが直ちに
取得できる保証はない。通知済みかつlock保持中なら`None`になることを決定的な単体検査へ
加え、成功側は元の期限内で`None`だけを再観測するようにした。失敗側は通知後の
入力拒否を検査し、workerの終了を確認してから保存された正確なerrorを照合する。
製品のworker実装や非ブロッキングの契約は変えていない。

API set、全依存DLLの署名・保護配置と信頼された取得、動的依存、実loaderのbind先や
mapされた画像の同一性、製品launcherとHost発見は残る。metadataの追跡成功を
製品の起動・一般アプリ接続の許可には使わず、Issue #4の未完了扱いを維持する。
