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
