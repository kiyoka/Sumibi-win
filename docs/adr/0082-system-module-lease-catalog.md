# ADR 0082: 保持された実DLL leaseによる静的依存エクスポート検査

## 目的と入口

ADR 0081の`ExportCatalog`は公開表metadataのみを借用し、物理ファイル・配置保護・file handle保持と結合していなかった。
本書では、固定された保護配置（System32）から排他読取で開いた実DLLの保持（`SystemModuleLease`）と、
その寿命に結び付いた公開表カタログ（`SystemModuleCatalog`）を定義する。

入口は`SystemModuleLease::open_system_dll`で正規化module名を受け取り、`GetSystemDirectoryW`由来のパスから
`FILE_SHARE_READ`のみ・reparse point拒否でファイルを開き、`DllExports`をパースして保持する。
`SystemModuleCatalog::insert`でleaseを登録し、`resolve`でHostの静的インポートシンボルを最終RVAまで追跡する。
catalogが生存する間、登録されたすべての実DLLのfile handleが開いたまま維持され、書込・削除を遮断する。

これは静的PE構造とディスク上実DLLの読み取り整合性の診断であり、LoadLibrary、動的解決、
実ローダーのバインド先、またはプロセスの起動権限を証明するものではない。

## 保護配置とファイル保持の契約

1. **モジュール名とパス解決**:
   module名はASCII英数字・hyphen・underscoreのbasenameに限定し、小文字`.dll`へ正規化する（拡張子込み128byte以内）。
   パス区切り、drive、親移動、余分なdot、空要素は拒否する。
   API set（`api-ms-win-*`, `ext-ms-win-*`）は同名ファイルとしての登録を拒否し、`ApiSetUnresolved`を維持する。
   ファイル探索（PATH, Current Directory, Safe DLL search mode等）は一切行わず、
   `GetSystemDirectoryW`が返すパス直下の`<normalized_name>`のみを対象とする。

2. **ファイルオープンと保護**:
   `OpenOptions`に`FILE_SHARE_READ`のみ、`FILE_FLAG_OPEN_REPARSE_POINT`を指定して開く。
   書込共有（`FILE_SHARE_WRITE`）や削除共有（`FILE_SHARE_DELETE`）は含めない。
   開いたハンドルに対し、通常のプレーンファイルであること、および`FILE_ATTRIBUTE_REPARSE_POINT`属性がないことを検査する。
   開いた後、同一ハンドルから`inspect_file`により`DllExports`を読み取る。

3. **Leaseの寿命とカタログ結合**:
   `SystemModuleLease`はファイルハンドルと`DllExports`、正規化名を所有する。
   `SystemModuleCatalog`は登録された全leaseの所有権を保持し、最大32 moduleまで受け付ける。
   重複するmodule名や上限超過は拒否する。
   カタログの`resolve`は、保持された全leaseの`DllExports`を用いてADR 0081と同等の有限forwarder追跡を行う。

## 検証と残る条件

合成ファイルによる単体検査で、正規化名、重複拒否、reparse point拒否、共有モード、API set拒否、未登録module欠落を検証する。

実ファイル検査では、Hostの全静的依存（17モジュール）のうち、非API setの10モジュール
（`advapi32.dll`, `bcrypt.dll`, `combase.dll`, `gdi32.dll`, `kernel32.dll`, `ntdll.dll`, `oleaut32.dll`, `shell32.dll`, `user32.dll`, `vcruntime140.dll`）
および転送先となる`kernelbase.dll`の計11モジュールを`SystemModuleLease`として開いてカタログへ登録する。
Hostの非API setインポート137シンボルすべてが、この保持されたカタログ内で最終RVAへ到達し、
未解決・欠落・循環・深さ超過がないことを確認する。

API set（`api-ms-win-*`）のOSスキーマ解決、動的依存（`LoadLibrary`）、製品署名者方針（Authenticode）、
実行中プロセスのメモリマップ同一性、および製品Hostの起動許可は残る。
実DLLの整合性検査成功を製品起動や一般アプリ受入完了とは扱わない。
