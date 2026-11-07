# Sumibi-winの配布・導入・更新・削除

[Issue #17](https://github.com/kiyoka/Sumibi-win/issues/17) のZIP配布を説明します。
実装判断は [ADR 0105](adr/0105-windows-installer-and-distribution-package.md)、
利用者向け手順は [installer/README.txt](../installer/README.txt) を参照してください。

## 配布ZIPの生成と検証

Rust依存を取得済みのWindows x64開発環境で実行します。
ビルドは `--locked --offline` を使います。

```powershell
.\dev.ps1 package
.\tools\Test-DistributionPackage.ps1
```

既定ではCargo.tomlのworkspace版を使用します。
`dist/Sumibi-v<version>-windows-x64.zip` と `dist/SHA256SUMS.txt` を生成します。
同名ZIPがある場合は上書きせず停止します。別の出力先を指定できます。

```powershell
.\tools\New-DistributionPackage.ps1 -OutputDir .\target\package-trial
.\tools\Test-DistributionPackage.ps1 -DistDir .\target\package-trial
```

本番TIPとHost/Setupは別のtargetディレクトリでビルドします。
ステージング時とZIP展開後に、ハッシュ・必須11ファイル・本番DLLのエクスポートを検査します。
エクスポート検査はDLLをロードします。署名や全依存DLLの信頼を保証する検査ではありません。
パッケージ作成だけではIME登録や設定変更を行いません。

## インストールと初期設定

ZIPをすべて展開し、`Install.cmd` を実行します。
管理者権限がなければUAC昇格を要求し、完了まで待ちます。
Program Files内のSumibiに配置し、COM/TSF・ログオン時起動・ショートカットを登録します。
既定IMEは変更しません。

完了画面から、サインイン中の通常ユーザーとして Sumibi を開けます。
すでに昇格した状態でインストーラーを実行した場合は、完了後にスタートメニューから「Sumibi」を開きます。
初回設定で接続先とモデルを保存し、APIキーを保存して「送信の確認へ進む」を押します。
送信管理で接続先を確認して送信を許可し、対象アプリで Win + Space から Sumibi を選びます。
破損した既存設定は上書きしません。接続先とモデルが同じ場合、Host は保存済みの手動送信許可を引き継ぎます。送信管理から取り消すと次回起動時も無効になります。
通知領域のアイコンが見つからない場合も、スタートメニューの「Sumibi」から送信管理を開けます。

## 更新と復旧

新しいZIPの `Install.cmd` を実行します。
毎回 `versions/<version>-<guid>/` にHost・Setup・DLLを配置します。
同じ版の再導入でも使用中のDLLを上書きしません。
インストール先のHostだけを停止し、新しい配置へ登録と起動先を切り替えます。
利用していたアプリとHostは更新後に開き直してください。

途中で失敗した場合は非ゼロで終了し、旧版・新配置・`installation.json` を残します。
問題を解消して再インストールするか、インストール先の `Uninstall.cmd` で登録を解除します。
レジストリ更新全体の自動ロールバックは実装していません。

## アンインストール

スタートメニューの「Uninstall Sumibi」またはインストール先の `Uninstall.cmd` を実行します。
登録解除を確認した後、管理対象の起動設定・ショートカット・ファイルを削除します。
無関係なファイルと従来の手動配置は自動削除しません。
使用中のDLLが残った場合はエラーで知らせ、Setupと復旧記録を保持します。
アプリを終了して再実行してください。再起動時削除は予約しません。
設定とAPIキーは保持します。

## 開発時の回帰テスト

```powershell
.\tools\Test-Installer.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\Test-Installer.ps1
cargo test -p sumibi-host --locked --offline
```

インストーラーの回帰テストは一時ファイルとAPIの置換を使い、マシン登録を変更しません。
UAC、実際のHKLM導入・削除、ログオン起動は専用の実機受入項目です。
Antigravityなどの変換互換性は [Issue #16](https://github.com/kiyoka/Sumibi-win/issues/16) で扱います。
