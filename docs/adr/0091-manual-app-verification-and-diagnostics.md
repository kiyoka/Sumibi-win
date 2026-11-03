# ADR 0091: 一般アプリ実機検証手順と非機密 TSF 保護診断の整備

2026-11-02 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) / [draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
状態: 承認済みの開発方針。一般アプリ実機検証の手順ガイド、非機密デバッグ診断、および統合開発ランナーを整備する。

## 状況

[ADR 0087](0087-tip-host-session-lifecycle.md)、[ADR 0088](0088-sta-async-drain-window.md)、[ADR 0089](0089-end-to-end-session-key-conversion.md) により、実 DLL（`sumibi_tip.dll`）と常駐 Host（`sumibi-host.exe`）間での自律セッション、相互 peer 認証、Fail-Closed ホットキー変換、非同期 STA テキスト置換（`DrainWindow`）、1-step Undo、失効検知・再接続待機がエンドツーエンドで成立した。
さらに [ADR 0090](0090-default-gui-host-entry.md) により、製品 Host のデフォルト GUI ルーティングが完了した。

残る主要な受入条件は「Notepad 等の実アプリで肯定的な保護条件と標準 Undo を確認する」ことである。
一方、[ADR 0019](0019-native-richedit-status-diagnostics.md) の実測で判明しているとおり、Windows 標準の RichEdit コントロール（`TS_SS_REGIONS`、`TS_SS_NOHIDDENTEXT` 欠如 → `HiddenTextPossible`）や EDIT コントロール（`TS_SS_TRANSITORY`、CUAS `0x40000000` → `TransitoryContext` / `UnknownDynamicFlags`）は、セキュリティ原則に基づく Fail-Closed により正しく取得・送信を拒否される。

一般アプリ（Notepad やエディタ等）の実プロセス内で `sumibi_tip.dll` が動作した際、外部からその保護判定状態や動作状況を安全に観測し、かつ安全に登録・検証・クリーンアップできる環境整備が不可欠である。

## 決定

### 1. 非機密 TSF 保護診断（`OutputDebugStringW`）

一般アプリのプロセス内で実行される `sumibi_tip.dll` の `TipHostSession` において、ホットキー（Ctrl+J）押下時の判定結果を `OutputDebugStringW` を通じてシステムデバッガ（DebugView、WinDbg、PowerShell 等）へ出力できるようにする。

- **機密性の厳格な保護**:
  - ドキュメントの本文テキスト、選択内容、入力文字、ファイルパス、ユーザー認証情報は**一切出力しない**。
  - 出力内容は固定の静的文字列（プリフィックス `[Sumibi-TIP]`）と、定型化された理由コード（`NotReady`, `NoContext`, `EmptySelection`, `ReadOnly`, `HiddenTextPossible`, `TransitoryContext`, `ScopeDenied` 等）に限定する。
- **Fail-Closed の維持**:
  - 診断出力は処理の成否に影響を与えず、未知の条件やエラー時は直ちに安全に処理を打ち切る。

### 2. 開発用 COM 登録・解除の統合ランナー化

現在 `sumibi-setup register-com-dev` / `unregister-com-dev` として実装されている現ユーザー（HKCU）限定の安全な COM 登録・解除を、統合ランナー `dev.ps1` のタスク（`register-dev` / `unregister-dev`）として統合する。

- **安全なマーカーとロールバック保証**:
  - [ADR 0045](0045-machine-registration-boundary.md) の決定を遵守し、HKLM や共有システム領域への変更は行わない。
  - HKCU の専用マーカー付きキーのみを操作し、不整合時は自動的に切り戻す。
  - テスト終了後は `dev.ps1 unregister-dev` で確実に元の状態へ復元する。

### 3. 実機手動検証手順書（`docs/MANUAL_APP_VERIFICATION.md`）の策定

開発者が実機デスクトップ環境で安全に動作確認を行えるよう、明確な手順書を整備する。

1. ビルド準備（`.\dev.ps1 build-tip`, `.\dev.ps1 build`）
2. Host 起動（`sumibi-host.exe` の起動、設定確認、送信同意の付与）
3. TIP 登録（`.\dev.ps1 register-dev`）
4. アプリケーション起動とホットキー挙動の観察（Notepad、RichEdit アプリ等）
5. デバッグ診断の確認（Fail-Closed 理由コードの観測）
6. 確実なクリーンアップ（`.\dev.ps1 unregister-dev`、Host 停止）

## 影響と検証

- 一般アプリ内での TIP 動作状況が機密情報を漏洩することなく可視化され、保護条件の適合状況の調査が容易になる。
- 統合ランナー `dev.ps1` を通じて、ワンコマンドで登録・解除が可能となり、開発作業の安全性が向上する。
- 既存のすべての単体テスト・E2E セッションテスト（`session-key-test`）の振る舞いは維持される。
