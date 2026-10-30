# ADR 0067: 実COM/TSF登録APIを隔離レジストリで取引検証する

2026-10-29 / Issue #4 / draft PR #10。状態: Windows 11 x64 の通常ユーザーによる自動検査を実装・実行。
恒久HKLM登録の製品コマンドは未公開。

## 状況

[ADR 0066](0066-registration-journal-and-recovery.md) の先行記録、所有状態の分類、逆順復旧は
模擬HKCU項目で中断後まで検証した。しかし実際のTSF APIは一度の呼出しで複数の
レジストリ項目を更新する。解除API後も空の親キーが残り得る。模擬項目の成功だけでは
実APIの作成・観測・解除とjournal cursorの接続を検証できない。

## 決定

製品CLSIDとは異なる試験ごとのGUIDを使い、専用の低整合性プロセスからWindowsの
`ITfInputProcessorProfileMgr`と`ITfCategoryMgr`を実際に呼ぶ。
COMキーは試験用backendが排他的に作成する。各操作にはADR 0066の取引エンジンと
[ADR 0068](0068-fixed-registration-journal-store.md)の固定storeを使い、
プロセスを意図的に終了した後、新しいプロセスから再読込みする。

レジストリの変更先は試験ごとに新設する
`HKCU\Software\Sumibi\NativeRegistrationFixture\<32桁ID>` の `Machine`、`User`、
`Classes` 子キーに限定する。子キーとrootに低整合性ラベルを付ける。
ワーカーEXEも低整合性ラベルの専用ディレクトリへコピーし、通常のトークンから
起動した場合にOSが低整合性で実行する。実API呼出し前に、低整合性・NoWriteUp、
AppContainerでないこと、thread impersonationの不在、実HKCU/HKLMの`Software`を
書込用に開けないことを確認する。通常整合性の直接呼出しは拒否する。

同じプロセス内の`RegOverridePredefKey`でHKLM/HKCU/HKCRを試験用keyへ向ける。
HKCRは試験用Machine内の`Software\Classes`へ合わせ、COMの見え方を揃える。
OSの`msctf.dll`はこの後にSystem32からロードし、エクスポートされた
`DllGetClassObject`でfactoryを取得する。事前に同DLLがロードされていたら拒否する。
これは試験プロセス内の既存レジストリ参照が切替え前のviewを保持するのを避けるためである。
プロセスを操作ごとに終了し、DLLのprocess global状態を次の検査へ持ち越さない。

試験用profileは日本語`0x0411`、無効、設定UIでは非表示として登録する。
`TF_RP_LOCALPROCESS`、既定IMEの変更、有効化操作は使わない。
解除は正確なclass/profile/category/itemを指定し、`TF_URP_ALLPROFILES`は使わない。
`S_OK`以外のHRESULTは成功扱いにしない。登録状態は別プロセスでも
`GetLanguageProfileDescription`と`IsEnabledLanguageProfile`から検査する。
この環境では無効な試験用profileへの`GetProfile`が別プロセスで`E_FAIL`となったため、
それを登録不在の証拠として使わない。原因は特定していない。

### 所有・削除の境界

試験用COM classの正確な所有marker、DLL path、threading modelと、TSF profileの
説明・無効状態、2カテゴリの双方向bindingを閉じたschemaで確認する。
Windows Server 2022では非表示profileに`HiddenInSettingUI=1`と
`SubItemInSettingUI=0`のDWORD値が追加され、Windows 11 build 26300では追加されなかった。
この2値が両方ない形、または型と値まで一致する両方ある形だけを完成状態とする。
片側だけ残る場合は未完成として復旧対象にし、別の値なら所有衝突として停止する。
試験用Machine全体がこのschemaの部分集合であること、User/Classesのcanaryが空であることを
毎回確認する。未知の値・子・型、別の所有情報、意図のない段階があれば全操作を停止する。
観測は深さ・node数・値数・値長・合計byte数を制限し、読取り前後のキー統計で変化も拒否する。

Windows 11 build 26300 の試験では、`UnregisterProfile`と`UnregisterCategory`の後にも
サービス、言語、カテゴリの空コンテナが残った。既知の空コンテナだけを下から
`RegDeleteKeyExW`で非再帰削除する。COMやTSF本体の部分作成と同じく、空コンテナも
中断後の「OwnedIncomplete」として再観測する。未知の内容が加われば削除せず停止する。
この観測レイアウトを製品HKLMにそのまま転用しない。OS版と登録先ごとの所有証拠が必要。

journalは試験用ワーカーのディレクトリ内に、試験IDごとの
`journal-<ID>/registration.lock`と`journal-<ID>/current.bin`として置く。
各ディレクトリには低整合性ラベルを付け、workerが書込み・再開できるようにする。
固定ロックはTSFオブジェクトを生成する前に取得し、登録操作の終了まで保持する。
この試験用配置は通常ユーザーが変更できるため、製品の保護state配置を証明しない。
ハッシュ・thumbprint・DLL pathは試験用の固定値で、製品DLLの署名や配置を証明しない。
試験用DLL pathのファイルは存在せず、DLLをロード・有効化しない。
成功後は、完全に空で所有markerが一致する試験用rootだけを解除する。
失敗時はjournalとrootを診断用に残す。

## 検証

`tools/Test-NativeRegistrationBackend.ps1`は、通常・作成前後・失敗時巻戻し・
COM空/部分作成・解除API前後・空コンテナ整理中/後の中断を別プロセスで確認する。
未知の値/子、所有marker変更、有効化、別profile、User/Classesの異物は
復旧/解除の双方が全状態を変えずに拒否する。さらにprofileのEnable値と
各カテゴリの片側bindingを欠く、構成した途中状態からの復旧を確認する。
これらの構成例はWindows API内部でその瞬間に中断した再現ではない。

Windows 11 Home 10.0.26300 x64、Rust 1.99.1、通常ユーザー、
PowerShell 7とWindows PowerShell 5.1の双方で47ケースが成功した。
固定storeへの移行後も両PowerShellで同じ47ケースを再実行し、成功した。
終了後、試験用rootは消え、実HKCU/HKLMの試験GUIDのCOM/TIPキーは存在しない。
試験中に製品CLSID、既定IME、証明書信頼、恒久HKLM登録は変更しない。

## 次の条件

製品の機械登録には、保護された固定journal/排他、署名と依存物の検証、
実登録領域の所有・衝突・解除証拠、管理者実行時の回復、旧版参照を残す更新と
取消・再起動後の受入試験が必要である。現在の試験用backendは低整合性の
プロセス内レジストリ切替えを必須とし、製品登録の入口を持たない。

参考: Microsoftの[RegOverridePredefKey](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regoverridepredefkey)、
[Mandatory Integrity Control](https://learn.microsoft.com/en-us/windows/win32/secauthz/mandatory-integrity-control)、
[TSF登録](https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration)、
[RegisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile)、
[UnregisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-unregisterprofile)。
