# ADR 0069: オフライン署名検査はHost認証と分ける

2026-10-30 / Issue #4 / draft PR #10。

`sumibi-ipc::authenticode::inspect_embedded`はWindowsの`WinVerifyTrust`でローカルの
通常ファイルに埋め込まれたAuthenticode署名とWindowsの信頼チェーンを検査する。
相対パスとUNCは受け付けず、最後の再解析点・通常ファイル以外を拒否する。
ファイルは読取専用かつ書込・削除を共有せず開いた状態で検査する。
UIとURL取得を無効化し、`VERIFY`後には失敗時も`CLOSE`を呼ぶ。これは同期APIであり、
TSFのcallbackや所有スレッドでは実行しない。

検査はWindowsのキャッシュ済み信頼情報を使い、オンライン失効確認を保証しない。
また、Windowsに信頼された任意の発行者を通し得るため、成功をSumibiの正規Hostの
証拠にしてはならない。親ディレクトリのACL・再解析点、発行者の識別、更新方針、
実行中processのイメージと検査したファイルの同一性はこのAPIの範囲外である。
製品の発見・起動にはこれらの追加検査と、明確な署名・失効方針が必要である。
検査対象を開いたままにしても、既存の実行中イメージの同一性は証明できない。

`inspect_signer`は署名検証の成功後、WinTrustの状態を閉じる前に最初の署名者証明書の
DERをSHA-256で識別し、証明書を保持せず32バイトだけを返す。
`inspect_expected_signer`は呼出し側が指定する指紋との不一致を拒否する。
この指紋の選定・保護・ローテーションは製品の導入方針に残る。証明書が一致しても、
保護配置や実行中イメージの同一性、オンライン失効確認を証明しない。
取得用のWinTrust helperはMicrosoftが将来変更・廃止の可能性を示しており、
製品組込み前に対応OSと複数署名の扱いを確認する。証明書状態の取得に失敗すれば拒否する。

`retain_expected_signer`は指紋が一致した場合に、署名検査時に開いた同じファイルの
handleを返す。保持中はWindowsの共有モードで他の書込・削除openを拒否し、読取を許す。
既に書込可能なhandleがある場合は検査用のopen自体が失敗する。署名済みツールの
一時コピーで、誤指紋、既存の書込handle、保持中の書込・削除拒否、保持中の起動と
解除後の書込を確認した。呼出し側は必要な期間handleを保持する。
このhandleは親ディレクトリの安全性、パス置換、実行中processのイメージとの同一性、
オンライン失効、製品の起動許可を単独では保証しない。製品起動への接続は未実装。

`VerifiedFile::matches_path`は保持中のhandleと別途開いた候補handleについて、
`FILE_ID_INFO`のvolume serialと128-bit file IDを照合する。再解析点・通常ファイル
以外を拒否し、IDを取得できない場合や未対応を示すゼロIDは一致と扱わない。
同じファイルのhard linkは一致し、内容と署名が同一でも別コピーは不一致とする。
両handleを開いている間の照合であり、IDを永続的な識別子として保存しない。
この判定だけでは、観測したprocessが同じファイルを実行している保証や、
親ディレクトリの保護・再解析点の安全性を得られない。

`VerifiedFile::require_development_file_acl`は保持中の同じファイルhandleから
Windowsの`GetSecurityInfo`で所有者とDACLを取得し、開発配置で使う厳格な構成
（Administrators所有、継承無効、Administrators/SYSTEMにFullControl、
UsersにReadAndExecuteのみ）と照合する。未知のACE・SID、重複、異なる権限、
NULL DACL、不正なACE長は拒否する。検査はスナップショットで、親ACLや
再解析点、署名の信頼方針、実行中processとの同一性を検証しない。
SDDLによる不正ACLの試験と、既存のDドライブ保護配置DLLの読取検査が成功した。
当該DLLの自己署名は未信頼であり、署名付きの製品Hostの検査成功を意味しない。

`open_development_directory`は最終ディレクトリを再解析点として開き、開発用の
厳格な所有者・DACLと、3 ACEそれぞれのObject/Container継承設定を確認する。
検査後は書込・削除を共有しないhandleを返す。不正な所有者・ACE・継承・権限は拒否する。
このhandleだけではそれ以前の親ディレクトリや中身の安全性を証明しない。
SDDL単体試験に加え、`D:\Program Files\Sumibi`、その開発版ディレクトリ、
`registration-state`のACLを読み取り専用で確認した。

`open_development_ancestor`はドライブrootとinstall parentを別の権限規則で検査し、
対象のdirectory handleを削除共有なしで保持する。所有者をAdministrators、SYSTEM、
TrustedInstallerに限定し、それ以外への危険な許可を拒否する。install parentでは
書込・削除も拒否し、継承を無効にする。rootと親で異なる規則の取り違え、相対パス、
親移動、未知のACE形式も拒否する。inherit-only ACEを無視できるのは、その子が
独立に継承無効と検証される場合に限る。
不正なSDDLの試験と、Dドライブのroot・`Program Files`・Sumibi・開発版・DLLを
同時に開いた読取試験が成功した。これは各部品の診断であり、一連の階層を製品の
起動経路として認証するAPIではない。署名の信頼と実行中イメージの同一性も残る。

`DevelopmentPlacement::open`は開発用の固定構造について、指定driveのroot、
`Program Files`、`Sumibi`、`dev-`と16桁小文字hexの版、固定名のTIPまたはHostを
順に検査し、各handleを一緒に保持する。`inspect_signer`は追加で指定証明書指紋を
検証し、署名検査側とACL検査側が同じfile IDを指す場合だけ両handleを保持する。
これは開発用の読取検査であり、実行許可、信頼された設定の選定、receiptや依存DLL、
実行中processの同一性、オンライン失効を検証しない。
Dドライブの開発版DLLで階層検査が成功し、未信頼の自己署名は署名検査で拒否された。
信頼された製品署名・Hostを使った結合成功はまだ確認していない。

`PinnedProcess::observed_image_path`は、既に固定した生存process handleからWindowsの
`QueryFullProcessImageNameW`でWin32形式のパスを取得する。取得前後にprocess生存と
tokenの一致を確認し、空・相対・不正な長さの結果を拒否する。これは同期照会なので
所有TSFスレッドのcallbackから呼ばない。パスはあくまで観測情報であり、そのパスに
現在あるファイルが実行中のイメージと同一である証明や、署名者・ACLの証明ではない。
特にパスを取り直して署名だけを確認しても、実行中イメージの認証とは扱わない。
バッファの検証を単体試験し、実processの試験も用意した。実process試験は
AppContainer等の非対応環境ではスキップするため、成功表示だけでは実行を意味しない。
通常のWindows環境で利用者が実行し、自processと別processのパス取得、
別process終了後の拒否がスキップされずに成功したことを出力で確認した。

このPCで、署名済み`vswhere.exe`が成功し、未署名のテスト実行ファイル、
相対パスとUNCが拒否された。署名済みツールの検査はファイルがある環境でだけ実行する。
同じ署名済みツールの証明書指紋はPowerShellの独立した証明書取得とSHA-256で照合し、
異なる指紋を拒否した。
自己署名の開発DLLを製品用に信頼する変更、証明書ストアへの追加や機械登録は行わない。

根拠: [WinVerifyTrust](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/nf-wintrust-winverifytrust)、
[WINTRUST_FILE_INFO](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/ns-wintrust-wintrust_file_info)、
[WINTRUST_DATA](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/ns-wintrust-wintrust_data)。
署名者取得とfingerprintの根拠: [WTHelperGetProvSignerFromChain](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/nf-wintrust-wthelpergetprovsignerfromchain)、
[CryptHashCertificate2](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/nf-wincrypt-crypthashcertificate2)。
実行イメージ名の照会: [QueryFullProcessImageNameW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-queryfullprocessimagenamew)。
ファイル共有: [CreateFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea)。
開いているファイルのID: [FILE_ID_INFO](https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_id_info)。
ACL取得と構造: [GetSecurityInfo](https://learn.microsoft.com/windows/win32/api/aclapi/nf-aclapi-getsecurityinfo)、
[ACCESS_ALLOWED_ACE](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-access_allowed_ace)。
