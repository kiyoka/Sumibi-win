# ADR 0097: 設定画面（settings_window）への保護プロファイル選択 UI の追加

2026-11-03 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) / [draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
状態: 承認済み・実装完了。

## 状況

[ADR 0095](0095-app-protection-profiles-and-acceptance-criteria.md) および [ADR 0096](0096-protection-profile-configuration-and-dynamic-propagation.md) により、二層保護プロファイル（Standard / Strict）のアーキテクチャ、`settings.json` への永続化、および Host / TIP 政策の動的同期が実装された。

しかし、一般ユーザーが保護プロファイルを切り替えるための GUI インターフェースが存在せず、プロファイルの変更には `settings.json` を直接手動編集する必要があった。
安全で誤操作のない操作体験を提供するため、製品設定画面（`settings_window`）において視覚的かつ明瞭にプロファイルを選択・保存できる UI が必要とされた。

## 決定

製品設定画面（`crates/sumibi-host/src/settings_window.rs`）に、Windows 標準のオートラジオボタン（`BS_AUTORADIOBUTTON`）を用いた保護プロファイル選択 UI を追加する。

### 1. UI レイアウトとコントロール配置

- ウィンドウの高さを従来の 445px から 485px へ拡張。
- パス入力欄（y=135）と設定保存ボタン（y=215）の間の y=175 に、以下のコントロールを配置：
  - `STATIC` ラベル: 「保護設定」（x=18, y=177, w=90, h=24）
  - ラジオボタン 1: 「標準 (Standard: Notepad等一般アプリ対応)」（x=115, y=175, w=270, h=24, ID=21, style=`BS_AUTORADIOBUTTON | WS_GROUP | WS_TABSTOP`）
  - ラジオボタン 2: 「厳格 (Strict: 高機密監査環境)」（x=395, y=175, w=165, h=24, ID=22, style=`BS_AUTORADIOBUTTON`）
- 後続の API キー入力欄（y=260）、キー保存・削除ボタン（y=300）、説明ラベル（y=340）、ステータスバー（y=380）をそれぞれ等間隔に配置調整。

### 2. 初期状態の反映と保存処理

- **初期化**: `View::new` 時に渡された `settings.protection_profile()` を参照し、`BM_SETCHECK`（`0x00F1`）により該当するラジオボタンを `BST_CHECKED`（1）で初期選択する。
- **保存**: 「設定を保存」ボタン押下時、`profile_strict` のチェック状態を `BM_GETCHECK`（`0x00F0`）で照会し、チェックされていれば `ProtectionProfile::Strict`、そうでなければ `ProtectionProfile::Standard` を決定。`Settings::with_protection_profile` を通じて保存タスクへ渡す。
- **排他制御・状態保護**: 保存処理中（`pending` が存在する場合）は `EnableWindow` によりラジオボタンも無効化（Disabled）し、保存中の不正な状態変更や競合を防止する。

### 3. 多重テスト実行への配慮

- `View::new` における `RegisterClassW` 呼び出しにおいて、既にウィンドウクラスが登録されている場合（`GetLastError() == ERROR_CLASS_ALREADY_EXISTS`）を許容し、同一プロセステスト内での複数回呼び出しを安全化。

## 結果と影響

### メリット
1. **直感的な操作性**: ユーザーは設定画面のラジオボタンをクリックするだけで、日常利用（Standard: Notepad等対応）と高機密監査（Strict）を簡単に切り替え可能。
2. **安全性の維持**: 保存処理中の UI 無効化と排他制御により、保存整合性を完全に担保。
3. **テスト検証性**: 単体テストで非表示ウィンドウ（`visible: false`）を生成し、初期化および切り替え操作後の選択値照会を決定論的に自動検証可能。

### 検証
- 単体テスト `settings_window::tests::view_initializes_and_reads_protection_profile_radio_buttons`: Standard / Strict の初期化照会および動的切り替えの完全合格を確認。
- `dev.ps1 check`: フォーマット・Clippy 警告ゼロ。
- `dev.ps1 test-all`: 全 535 単体テスト、`session-key-test`、`native-edit-test`、HostImage 診断の全件完全合格を確認。
