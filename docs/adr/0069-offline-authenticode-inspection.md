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

このPCで、署名済み`vswhere.exe`が成功し、未署名のテスト実行ファイル、
相対パスとUNCが拒否された。署名済みツールの検査はファイルがある環境でだけ実行する。
自己署名の開発DLLを製品用に信頼する変更、証明書ストアへの追加や機械登録は行わない。

根拠: [WinVerifyTrust](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/nf-wintrust-winverifytrust)、
[WINTRUST_FILE_INFO](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/ns-wintrust-wintrust_file_info)、
[WINTRUST_DATA](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/ns-wintrust-wintrust_data)。
