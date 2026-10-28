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
