# ADR 0080: DLL公開表の読取と静的参照の照合

## 目的と入口

Hostが参照する関数名・序数を、実行せずにDLLの公開表と照合できるようにする。
`dll_exports::inspect_bytes`と`inspect_file`が`DllExports`を返し、`lookup`へ既存の
`ImportSymbol`を渡す。結果は未公開、画像内の`Rva`、未解決の`Forwarder`を区別する。
RVAは呼出可能なpointerではなく、forwarderも別DLLの検査を終えた意味ではない。

`VerifiedFile::dll_exports`は署名検査で保持した同じfile handleを読む入口とする。
通常の`inspect_file`はfileを保持・認証しない。返すmetadataもhandleを所有しないため、
呼出元は必要な共有制限と保持期間、信頼する配置・署名方針を別途満たす必要がある。
DLL内部のmodule名も単なるmetadataで、ファイルの同一性を証明しない。

## 限定したDLL profile

x64 PE32+、実行可能属性とDLL属性を要求する。ファイル64MiB、section 96件、
image 1GiBなどのheader・section検査はHostと共通化し、Host側のDLL拒否・entry point・
subsystem・importの条件は維持する。DLLの公開表読取ではentry pointやsubsystemを
実行可能性の証明として検査しない。DLL全体をWindows loaderと同等に検証するものではない。

export directory、address/name/ordinalの各表、ordinal baseの加算、名前の範囲と終端を
検査する。公開名はASCIIの大文字・小文字を区別し、名前表の整列と一意性を要求する。
複数の名前が同じslotを指すaliasは受け付ける。ゼロRVAのslotは未公開として扱う。
序数ではbaseを差し引いてslotを探し、名前表がないDLLでも照合できる。

export directoryのRVA範囲に入るtargetはforwarder文字列として取得し、そのNULも
同じ範囲内にあることを要求する。文字列はDLLとsymbolを区切るdotを必要とし、
`DLL.#ordinal`も未解決の文字列として保持する。転送先の自動読込や探索は行わない。
範囲外のtargetは画像内sectionのRVAに限定する。dataやzero-fill領域の公開もあり得るため、
実行属性は要求せず、関数としての呼出可能性を保証しない。
表とRVA・forwarderの意味はMicrosoftの
[PE Format](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)に従う。

slotと名前はそれぞれ65536件まで、module名128byte、公開名・forwarder各512byteまで。
全文字列はNULを含め4MiBへ制限し、同じforwarderの反復も重複除去前に数える。
名前は1つのfile-backed section内に終端まで収まる必要がある。

## 検証

`tools/Test-HostImage.ps1`はHost parser 13件、DLL parser 8件に加え、
[ADR 0081](0081-bounded-export-forwarders.md)の転送検査7件と実ファイル検査3件を実行する。
合成DLLではalias、序数の全u16範囲、名前の大小文字、公開slotの欠番、
data、名前なしDLL、転送名・転送序数と転送範囲の境界を確認した。
全prefixの切詰め、表の範囲外・overflow・件数超過、名前の不正順序・重複、
反復forwarderによる文字列容量超過、2048件の変異入力も検査する。

実ファイル試験はGetSystemDirectoryWで得たdirectoryのKernel32を、書込・削除を
共有しないread-only handleで開く。最後の再解析点を拒否し、EOFからの読取と再読取の
一致を確認する。今回のHostの89件の参照は85件の画像内RVAと4件の未解決forwarderに
対応し、未公開の参照はなかった。個数はこのOSとdebug buildの観測で、固定allowlistではない。
LoadLibraryやGetProcAddress、DLL初期化は使わない。

## 残る条件

このKernel32試験は配置の信頼や署名を認証せず、実際のloaderが同じfileへbindすることも
証明しない。明示した公開表間のforwarder追跡は[ADR 0081](0081-bounded-export-forwarders.md)で追加した。
全依存DLLの信頼された取得・保持と署名方針、API setの解決、
動的依存、更新とmapされた画像の同一性は残る。製品launcherやHost発見の許可には使わず、
Issue #4の製品受入完了にも数えない。
