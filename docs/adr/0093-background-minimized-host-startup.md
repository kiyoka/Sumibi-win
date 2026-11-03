# ADR 0093: 製品 Host のバックグラウンド常駐起動オプション（`--minimized` / `--tray`）

2026-11-03 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) / [draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
状態: 承認済みの製品 Host 機能拡張。スタートアップおよびバックグラウンド常駐のための起動オプションを実装する。

## 状況

[ADR 0090](0090-default-gui-host-entry.md) により、引数なし起動での GUI 自動ルーティングが完成し、
[ADR 0092](0092-system-tray-and-background-minimize.md) により、タスクバー通知領域（システムトレイ）アイコンおよび最小化常駐が実装された。

しかし現状では、起動時に必ず送信管理ウィンドウ（`consent_window`）が表示（`SW_SHOW`）される。
Windows のスタートアップフォルダ（`shell:startup`）やタスクスケジューラ、あるいはログイン時の自動起動スクリプトから Host を起動する場合、毎回デスクトップ前面にウィンドウが表示されるのはユーザーの作業を妨げる。
自然な常駐 IME Host として、起動時からウィンドウを表示させず、システムトレイ通知領域のみにアイコンを表示してバックグラウンド待受を開始するオプションが必要である。

## 決定

### 1. 起動オプション `--minimized` / `--tray` / `-m`

- `sumibi-host.exe` の起動引数に `--minimized`、`--tray`、または `-m` を追加する。
- 該当オプションが指定されている場合、送信管理画面（`consent_window`）は `SW_HIDE` で作成され、画面を表示することなくシステムトレイ通知領域にアイコンを登録し、直ちにバックグラウンド待受を開始する。
- ユーザーがトレイアイコンをクリック（または右クリックメニューの「送信管理画面を開く」）した時点で、通常どおりウィンドウが前面に表示（`SW_RESTORE` + `SetForegroundWindow`）される。
- 設定ファイルが未作成の場合（初期起動時）は、ユーザーによる初期設定が必須であるため、`settings_window` を通常表示してセットアップを誘導する。

### 2. コマンドライン仕様の拡張

- `sumibi-host [options...]`:
  - `sumibi-host --minimized`（または `--tray`）
  - `sumibi-host host-window --minimized [settings-path]`
- コマンドラインヘルプ（`--help`）にオプションの説明を追加：
  ```text
  Options:
    --minimized, --tray, -m   Start minimized to system tray without displaying the window
  ```

## 影響と検証

- スタートアップ登録時にデスクトップ前面を占有せず、静かに通知領域に常駐させることができる。
- トレイアイコンからの操作性（クリックでの復帰、右クリックメニューからの設定・終了）と完全に協調する。
- 既存の引数なし起動（通常表示）およびコンソール起動（`run`）の動作はそのまま維持される。
