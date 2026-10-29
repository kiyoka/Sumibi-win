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
