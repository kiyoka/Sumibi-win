# ADR 0090: 製品 Host エントリーのデフォルト GUI 起動と設定自動誘導

## 状況

これまでの `sumibi-host.exe` は、明示的なサブコマンド（`run`, `host-window`, `settings-window`, `settings-init`, `startup-check`）の指定を前提としていた。
引数なしで実行された場合、ブートストラップ引数のパースに落ちてエラー終了していた。

しかし製品としての利用シナリオにおいて：
1. 一般ユーザーがエクスプローラーから `sumibi-host.exe` をダブルクリックする。
2. Windows の「スタートアップ」フォルダやタスクスケジューラに `sumibi-host.exe` を登録する。
3. デスクトップやスタートメニューのショートカットから起動する。
これらの起動形態では、引数なし（`args.is_empty()`）で実行されるのが標準的である。

また、設定ファイル（`%LOCALAPPDATA%\Sumibi\settings.json`）が未作成の状態では、いきなり常駐待受を行うことはできず、まず設定 UI で接続先や API キーを登録する必要がある。

## 決定

1. **引数なし起動時のデフォルト GUI ルーティング**:
   - `crates/sumibi-host/src/main.rs` において、`args.is_empty()` の場合の挙動を定義：
     - `host_paths::settings_path()`（`%LOCALAPPDATA%\Sumibi\settings.json`）の存在を確認する。
     - **設定ファイルが存在する場合**: `consent_window::run(path)` を起動。
       - 送信管理画面（Consent Window）を表示し、バックグラウンドで `HostService` のログオン限定待受ループを同時稼働する。
     - **設定ファイルが存在しない場合**: `settings_window::run(path)` を自動起動。
       - 設定画面（Settings Window）を表示し、ホスト名・モデル名・API キーの初期入力を促す。
   - これにより、ユーザーは `sumibi-host.exe` を単独起動するだけで、初回は設定画面、次回以降は送信管理画面がシームレスに立ち上がる。

2. **コマンドラインヘルプの標準装備**:
   - `--help`, `-h`, `help` 引数を受け付け、サポートされている全サブコマンド（`run`, `host-window`, `settings-window`, `settings-init`, `startup-check`）の簡潔な使用方法と説明を標準出力する。

3. **送信管理画面の案内文の更新**:
   - `consent_window.rs` の説明文を更新し、バックグラウンドで一般アプリ（`TipHostSession`）からの接続待受が稼働中であることを明示。

## 影響・境界

- **後方互換性**:
  - 既存のすべての CLI サブコマンド（`run`, `host-window`, `settings-window`, `settings-init`, `startup-check`）の挙動・引数検証・終了コードは 100% 維持される。
- **安全性**:
  - 設定ファイル未作成時に誤って送信待受が開始されることはなく、必ず設定入力画面へ安全に誘導される。
  - 同一ログオン内での多重起動抑止（`HostService` / `HostControl` の単一インスタンス制限）はそのまま機能し、多重起動時は既存画面の確認を促すエラーが正しく返される。
