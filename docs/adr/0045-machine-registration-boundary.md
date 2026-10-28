# ADR 0045: マシン登録前にDLL配置と更新境界を確定する

## 状況

現ユーザーのCOM登録と`TF_RP_LOCALPROCESS`による実TSF検査は成功した。
一般アプリで使う恒久登録にはCOMに加えTSF言語プロファイルとカテゴリが必要になる。
一方、現在のDLLはユーザーが書き換えられるワークスペースの`target/debug`にある。
この場所をHKLMのCOM登録先にすると、別プロセスが後から変更されたDLLをロードし得る。

## 決定

現段階ではHKLM/恒久TSFを書き換えるコマンドを公開しない。
`plan-system-dev`だけを用意し、64-bit DLLのCOM export、診断用spike exportの不在、
既存CLSID/TSF登録との衝突を調べる。これはDLLをロードするがレジストリは読取専用とする。
「事前確認成功」は設置可能という判定ではない。

後続の取引と復旧は [ADR 0066](0066-registration-journal-and-recovery.md) に実装する。
現時点の backend は専用 HKCU fixture のみで、この決定の HKLM/恒久 TSF 境界を変更しない。

登録実装へ進む条件は、管理者だけが変更できる安定配置先、DLLと依存物のACL検証、
署名方針、別の版へ更新する際の参照寿命と失敗時復旧、同一CLSIDの所有確認、
正確なTSF登録解除、プロファイルのユーザー有効化と既定IMEを変更しない操作の分離である。
AppContainerと32-bitプロセス対応も別途判断する。

## 検証範囲

2026-10-24、診断用spike DLLを拒否し、単独ビルドしたパッシブDLLの事前確認が成功した。
パッシブDLLの従来のプロセス内`self-test`も成功した。事前確認後、HKLMの対象COM/TSFキーが
存在しないことを確認した。マシン登録・解除は実行していない。

参考: Microsoftの[Text Service Registration](https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration)、
[RegisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile)、
[COM Classes and Servers](https://learn.microsoft.com/en-us/windows/win32/com/classes-and-servers)。

## 2026-10-28 の登録準備確認

パッシブDLLを`cargo build -p sumibi-tip --no-default-features --locked --offline`で
明示的に再ビルドし、`plan-system-dev`がCOM export、spike export不在、既存の
CLSID/TSF登録衝突なしを確認した。現在ユーザー限定の`Test-RegisteredTsf.ps1`では
レジストリからのCOM起動、プロセス内TSF profileの有効化と解除が成功した。
終了後の`plan-system-dev`でも衝突なしを再確認した。

現ワークスペースの`target/debug/sumibi_tip.dll`は`Get-AuthenticodeSignature`で
`NotSigned`、`icacls`では作業ユーザーに`(F)`がある。これをHKLMのCOMサーバー
として登録する条件は満たしていない。今回の確認はHKLMを書き換えていない。
次に、管理者だけが変更できる配置先と依存DLLを固定し、署名・ACL確認、版別の
配置と参照寿命、登録失敗時の復旧と完全な解除を実装してから機械登録を試す。

最初の機械登録試験は、次の順序で隔離した検証環境に限定する。

1. 版別の保護ディレクトリへパッシブDLLと依存物を配置し、全親ディレクトリを含む
   書込権限と署名を検証する。作業ツリーのDLLを登録先にしない。
2. 管理者権限で既存CLSID・TSF profile・categoryの所有を再確認してから、
   COM class、TSF service、言語profile、必要なcategoryを登録する。
   `RegisterProfile`の`bEnabledByDefault`はfalseとし、診断用の
   `TF_RP_LOCALPROCESS`を製品profileに付けない。ユーザー有効化は別操作とし、
   既定IMEを変更しない。
3. 各段階の失敗では、この試行が作成したキー・profile・categoryだけを逆順に
   解除する。既存の登録を上書きしない。解除後に対象が残っていないことを確認する。
4. 旧版DLLを使用中のprocessを強制終了・上書きせず、別版への更新と失敗時の
   旧版復旧を試験してから通常PCへの導入を判断する。

## 検証用自己署名コピー

`tools/Prepare-SystemRegistrationDev.ps1`は、パッシブDLLと既存登録の読取専用
検査後、現在ユーザーの`My`ストアにエクスポート不可の開発用コード署名証明書を
作成する。元のDLLを変更せず、`target/system-dev/`の別コピーをSHA-256で照合して
署名する。署名後もCOM export、spike export不在、登録衝突を再検査し、公開証明書と
ハッシュ・署名状態のmanifestを出す。秘密鍵はエクスポートしない。

この自己署名ルートは信頼ストアに追加せず、`Get-AuthenticodeSignature`は
`UnknownError (UntrustedRoot)`を報告する。署名済みコピーもユーザーが書き換えられる
ワークスペース内にあるため、まだHKLM登録先にはしない。製品用の信頼された署名、
保護配置からの更新、参照中DLLの復旧、登録解除の検証は引き続き必要。

検証用の保護配置先は `D:\Program Files\Sumibi` とする。
`tools/Stage-SystemRegistrationDev.ps1` は管理者権限でのみ実行し、
`manifest.json` に記録した署名済み DLL のハッシュと自己署名状態を検査する。
親フォルダの一般ユーザー書込権限を拒否し、Administrators 所有の版別フォルダと
DLL に明示 ACL を設定する。同じ版を上書きしない。配置後も自己署名ルートは
未信頼のままで、HKLM/TSF 登録には使用しない。

2026-10-28、`D:\Program Files\Sumibi\dev-d74b8348334397d6` への配置を実行した。
親、版別ディレクトリ、DLL、receipt は Administrators 所有で継承なし、
Administrators/SYSTEM は FullControl、Users は ReadAndExecute だった。
署名済み DLL の SHA-256 は元の manifest と一致し、`plan-system-dev` も成功した。
署名は `UnknownError (UntrustedRoot)` のままで、ルート信頼と機械登録は行っていない。

後続の読取専用 `Verify-SystemRegistrationDev.ps1` は、配置後にドライブ root・親・install root・版・DLL・
receipt の ACL と所有者を再検査し、版内を DLL と receipt の2ファイルに限定する。
receipt のハッシュ・版名・開発用署名と未信頼状態を照合してから、配置先 DLL を
`plan-system-dev` で検査する。配置コマンドも最後に同じ検証を呼び出す。

同日の配置 DLL を Visual Studio の `dumpbin /DEPENDENTS` で確認したところ、
COM/Windows API と API set のほかに `VCRUNTIME140.dll` を参照していた。
検証 PC では `C:\Windows\System32\VCRUNTIME140.dll` が存在し、署名状態は
`Valid` だった。この観測は将来のビルドの依存物を保証しないため、製品導入時は
ビルドごとに依存 DLL と実際のロード先を再確認する。

保護配置した同じ DLL を `Test-ProtectedRegisteredTsf.ps1` で現在ユーザーに一時登録し、
別プロセス COM 起動、プロセス内 TSF profile の有効化、解除後の再検証が成功した。
これは保護配置先からの実ロードと現在ユーザー登録の後始末を確認する段階であり、
HKLM/恒久 profile の登録や一般アプリでの有効化は確認していない。

同日の追加点検で、親 ACL を広い group の SID だけで検査すると個別 user/独自 group への
危険な許可を見落とすことが分かった。配置と読取検証の共通 policy で全非特権 principal の
適用される Allow ACE と所有者を検査する。SYSTEM/Administrators/TrustedInstaller のみを
特権扱いし、保護された次の子へ伝播しない InheritOnly ACE は現 ancestor の権限から除く。
`Program Files` の継承無効と、root/版/DLL/receipt の正確な3 ACE を要求する。
メモリー内の105 ACL ケースと既存保護 DLL の読取検証が成功し、実 ACL の変更は行っていない。
