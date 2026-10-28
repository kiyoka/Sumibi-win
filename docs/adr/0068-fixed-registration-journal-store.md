# ADR 0068: 登録journalの固定ロックと解除後の世代交代

2026-10-29 / Issue #4 / draft PR #10。状態: journal storeの寿命と開発用保護配置の検証手順を追加。
実機の保護stateディレクトリを作成・検証。製品の機械登録は未接続。

## 状況

[ADR 0066](0066-registration-journal-and-recovery.md) の `FileJournal` は単一取引を
排他的に保持する。しかし製品登録を固定名のjournalにするだけでは、`Removed`で
解除を終えた後に次の取引を開始できない。journalを上書きすると、途中終了時の
復旧根拠を失う。取引ごとに任意のjournal名を指定させると、同じCLSIDの排他を迂回できる。

## 決定

`RegistrationStoreLock`は1つのディレクトリ内で`registration.lock`を共有なしで開き、
`LockedJournal`の寿命中も保持する。`LockedJournal`はロックへの借用を持つため、
Rustの型上、journalだけを残してロックを先に破棄できない。
現在の取引は固定名`current.bin`に記録する。`begin_new`は次だけを許す。

| 現在の状態 | 次の登録開始 |
| --- | --- |
| `current.bin`なし | 排他的に新規作成 |
| 空の`current.bin` | 開き直して使用。最初の同期済みintent前には登録APIを呼ばない |
| `Installing` / `Installed` / `Removing` | 拒否。復旧または明示解除を先に行う |
| `Removed` | 完全な履歴を確認し、`removed-<取引ID>.bin`へ退避後、新しい`current.bin`を排他的に作成 |
| 破損・未知項目・既存の同名退避先 | 内容を保持して拒否 |

退避後、新規作成前に終了した場合は`current.bin`がなく、次回の開始で作成できる。
新規作成後、最初の記録前に終了した場合は空journalを再利用できる。
退避対象は`Removed`だけなので、ディレクトリ名変更の永続化が不明でも
未完了の登録取引を退避済みと誤認しない。別processの同時操作は固定ロックで拒否する。
退避ファイルは64件までとし、上限時は新規登録を止める。手動整理は別の管理者手順とする。

storeは`registration.lock`、`current.bin`、32桁小文字hexの
`removed-<取引ID>.bin`だけを受け付ける。全項目を通常ファイルと確認し、
再解析点や未知名を拒否する。退避済みjournalは読取専用で完全な履歴を検査し、
末尾が`Removed`でファイル名の取引IDと一致する場合だけ受け付ける。
不完全な末尾や誤った名前を見つけても修復・上書きせず、新しい取引を止める。
各journalの形式・遷移・サイズ上限は`FileJournal`が検査する。
storeはレジストリを変更せず、登録APIの所有判定は[ADR 0067](0067-isolated-native-registration-backend.md)の
試験用backendから製品へ直接流用しない。

## 開発用の保護配置

将来の管理者試験では、利用者が選んだ`D:\Program Files\Sumibi`直下の
`registration-state`をjournal store候補とする。`tools/Prepare-RegistrationStoreDev.ps1`は
通常実行でドライブroot、`D:\Program Files`、install rootの所有者・ACL・再解析点を
読み取り、stateが未作成ならその旨を報告する。`-Provision`は管理者権限でだけ
stateを新規作成し、Administrators/SYSTEMにFullControl、UsersにReadAndExecuteの
明示ACLとAdministrators所有者を設定する。既存stateは変更せず検証する。

store内のファイルは新規作成直後には保護された親からACEを継承し得る。
読み取り検証は、Administrators/SYSTEM/Usersの正確な権限、
AdministratorsまたはSYSTEMの所有者、通常ファイル、固定名とサイズ上限を確認する。
継承直後を拒否して復旧不能にしない。検証済みの親を前提とするため、
storeのRust API自体は任意pathのACLを認証したとは主張しない。
製品CLIを作る際には固定pathをコードで選び、管理者権限で全親の検証を
ロック取得前に行う。任意の`--journal`引数を公開しない。

## 検証と残件

Windows x64の単体試験で、同時ロック拒否、途中取引の退避拒否、解除後の世代交代、
退避と新journal作成の間の中断、空journal再開、未知・破損ファイルの保持を確認した。
退避済み履歴についても、不完全な末尾・取引ID不一致・`Removed`以外の末尾を
変更せず拒否することを確認した。
固定storeと取引エンジンの結合試験では、登録後にstoreを開き直して`Installed`を保持し、
明示解除から`Removed`へ進め、再度開き直して完全な履歴を退避した後に
次の空journalを開始した。backendは単体試験用モデルであり、HKLM/TSFは変更しない。
ACL policyはメモリー内117ケースをPowerShell 7とWindows PowerShell 5.1で通した。
このPCの保護install rootを読取確認した後、利用者のUAC承認を得て
`registration-state`を管理者権限で作成した。通常ユーザーからPowerShell 7と
Windows PowerShell 5.1の両方で、Administrators所有、継承を止めた正確な3 ACE、
再解析点なし、内容0件を再検証した。通常ユーザーによる新規ファイル作成はOSに拒否され、
再検査でも内容0件だった。保護state上での実journalファイル作成と
管理者権限でのクラッシュ復旧は未検証である。
`Test-RegistrationStoreFileAclDev.ps1`は、管理者として開いたPowerShellから
空の保護stateに`current.bin`を作り、実ファイルの継承ACLを確認する手順として用意した。
通常ユーザーからは権限不足で作成前に拒否され、stateが空のままなことを確認した。
管理者権限での実行は未実施である。

次に必要なのは、保護state内に作るjournalファイルの実ACL・中断後の再開確認、固定pathを使う
製品backendの保護配置・署名・依存物検査、実HKLM/TSFの所有証拠と衝突判定、
登録・解除の管理者試験、版更新と旧版DLL参照の扱いである。
