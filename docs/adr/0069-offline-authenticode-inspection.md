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
