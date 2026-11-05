# ADR 0105: Windows用インストーラーと配布パッケージ

- 日付: 2026-11-05
- 関連: [Issue #17](https://github.com/kiyoka/Sumibi-win/issues/17)、[PR #17](https://github.com/kiyoka/Sumibi-win/pull/17)
- 状態: 実装・検証中

## 配布と起動

Windows PowerShell 5.1以上で動くZIP形式を採用する。
`Install.cmd` / `Uninstall.cmd` は対応するPowerShellスクリプトを起動する。
スクリプト側で管理者権限を判定し、必要ならUAC昇格を待ち、子の終了コードを返す。
cmdからPowerShellへ、さらにcmdへ戻す多重の文字列展開は行わない。

ZIPには次の11ファイルを含める。

- `Install.cmd`, `Install.ps1`
- `Uninstall.cmd`, `Uninstall.ps1`
- `Installer.Common.ps1`, `README.txt`, `LICENSE`, `package.json`
- `bin/sumibi-host.exe`, `bin/sumibi-setup.exe`, `bin/sumibi_tip.dll`

`package.json` は形式版、製品名、版番号、ファイルごとのSHA-256を記録する。
ZIPのSHA-256は外部の `SHA256SUMS.txt` に記録する。
チェックサムは破損検出であり、発行者認証やコード署名を代替しない。

## 配置と更新

配置先を64-bit WindowsのProgram Files内の `Sumibi` に限定する。
版番号とGUIDから毎回新しい `versions/<version>-<guid>/` を作り、
Host・Setup・TIPをまとめて配置する。同じ版の再インストールでも上書きしない。
祖先と管理対象パスのjunction/symbolic linkを拒否する。

新規ディレクトリのACLはSIDで設定する。
AdministratorsとSYSTEMにFullControl、UsersとALL APPLICATION PACKAGESにReadAndExecuteを付与する。
複数の導入・削除は同じ名前付きmutexで排他する。

配置後のハッシュと本番DLLのエクスポート検査が成功したら、復旧用の
`installation.json` とアンインストーラーを保存する。
その後、インストール先配下にあるHostだけを停止し、SetupでCOM/TSFを登録する。
終了コードに加えてHKLM/HKCUのCOM参照先とTSFプロファイルの存在も確認する。
スタートアップとショートカットの更新後に完了状態を記録する。

レジストリ変更全体が原子的になるわけではない。途中で失敗したらエラーとして終了し、
旧版・新しい配置・復旧記録を残す。再インストールまたはアンインストールで復旧する。
既存アプリは古いDLLを保持し得るため、更新後に利用者が再起動する。
アプリの強制終了、旧版の自動削除、送信への自動同意は行わない。

## 利用開始

スタートメニューとHKLMのRun値は、新しい版のHostを指す。
未設定時にも起動できるよう、常駐起動は `sumibi-host.exe --minimized` を使う。
初回は空のホスト名・モデルと `/v1` のパスを示す設定画面を開く。
最初の保存はworkerから新規作成のみを行い、既存の正常・破損ファイルを置き換えない。
設定保存後にキー操作を有効にし、設定・APIキー・送信への同意を分けて扱う。
昇格したインストーラーから個人設定画面を起動しない。

## 削除

配置記録とSetupがなければファイル削除へ進まない。
登録解除の終了コードと、COM参照・TSFプロファイルが消えたことを確認する。
別配置先を指すCOM登録や起動設定は削除しない。
配置記録にある版の既知ファイルだけを削除し、無関係なファイルは保持する。
ディレクトリの再帰的な全削除や、レジストリ直接削除へのフォールバックは行わない。

DLLがロックされていれば非ゼロで終了し、Setupと復旧記録を残す。
アプリ終了後に再実行できる。再起動時削除は予約しない。
LocalAppDataの設定と資格情報ストアのAPIキーは保持する。
従来の手動配置ファイルは所有を証明できないため、自動削除しない。

## 検証

`tools/Test-Installer.ps1` は一時フォルダーで初回導入、同じ版の更新、
ファイルロック、登録失敗、削除失敗、復旧、設定外ファイルの保持を検査する。
実ファイルと実ロックを使い、レジストリ・プロセス終了・ACL書込・ショートカット変更は置換する。
PowerShell 5.1と7をCIで実行する。

パッケージ生成ではHost/Setupと本番TIPのビルド先を分け、検査時の再ビルドで
spike DLLに置き換わらないようにする。検証はZIPを展開したファイルにも実施する。
全体のHKLM導入・UAC・ログオン起動の実機試験は、上記の非破壊テストとは別の受入項目である。
