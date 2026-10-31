# ADR 0079: 静的importの関数名・序数と参照表の検査

## 目的と入口

依存DLLのexportと将来照合できるよう、Hostの静的importをDLL名だけでなく関数名・
序数まで取得する。`HostImage::import_libraries`は`ImportLibrary`の一覧を返し、
各項目の`symbols`で`ImportSymbol::Name`または`Ordinal`を区別する。
従来の`imports`によるDLL名一覧も維持する。

この処理は保持中のHostを読む既存のPE検査に含まれ、保護配置Hostの停止診断では
子作成前に実行される。DLLや関数をロード・実行する処理は追加しない。

## 形式の限定と上限

PE32+の64bit import lookup tableを走査し、名前による参照では2byteのhintと
NUL終端のASCII関数名、序数による参照では16bitのordinalを取得する。
名前の大文字・小文字は保持する。hintは実体があることを確認するが、export照合の
代わりには使わない。予約bitが立った参照、範囲外やzero-fill部分の参照を拒否する。
DLL名と関数名はそれぞれ1つのfile-backed section内に終端まで収まる必要がある。
名前のRVAは文字ごとでなく1回だけ解決し、長い名前の反復でもsection探索を繰り返さない。

各参照と終端でimport address tableの対応値がlookup tableと一致することを要求する。
bound import directoryまたはdescriptorのtimestampが非ゼロなら未対応として拒否する。
lookup tableのRVAがゼロの形式も、本Host profileでは受け付けない。
WindowsでロードできるPEすべてを検証するparserではなく、未boundのHostを対象とする。
各表と名前の形式はMicrosoftの
[PE Format](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)に従う。

上限はdescriptorごとに4096参照、全descriptorで8192参照、関数名はASCIIの
可視文字512byteとする。0参照の表もNUL終端を必要とする。
DLL名の大文字・小文字違いは従来どおり統合し、そのDLLの同じ関数名・序数もまとめる。
関数名とordinalは別の参照として扱う。容量は重複をまとめる前に消費するので、
同じ表や同じordinalを繰り返しても全体上限を迂回できない。

## 検証

`tools/Test-HostImage.ps1`でparser 13件と実Hostビルド1件を検査した。
名前と序数の混在、大文字・小文字が異なる関数名の保持、DLL descriptorの統合、
予約bit、hint/nameの範囲外、lookup/IAT不一致、終端欠落、bound descriptorの拒否を確認した。
512byteの名前と4096参照の境界を受け付け、超過と重複による全体容量超過を拒否する。
既存の全prefix切詰め、変異入力、保持handleの再読取も実行した。

今回のdebug Hostでは17種類のDLLと、正規化・重複除去後の165件の参照を取得した。
実ビルドの`kernel32.dll`に`GetCurrentProcessId`の名前参照が含まれることも確認した。
これは固定allowlistではなく、今回生成した実行ファイルの観測である。

## 残る条件

DLL実体の取得・署名・配置、exportとの照合、API setやforwarderの解決、動的依存は
未実装である。静的参照が妥当でも、依存物の信頼や実際のロード先を証明しない。
製品の署名・更新方針、保護配置Hostの成功fixture、mapされた画像の同一性、
安全なHost発見と一般アプリ接続も引き続き必要である。
