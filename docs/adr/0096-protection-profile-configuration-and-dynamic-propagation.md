# ADR 0096: 保護プロファイル設定（settings.json）と Host/TIP 政策動的同期

2026-11-03 / [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4) / [draft PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)。
状態: 承認済み・実装完了。

## 状況

[ADR 0095](0095-app-protection-profiles-and-acceptance-criteria.md) により、一般アプリ（RichEdit / Notepad 等）で安全に変換を行うための二層保護プロファイル（`Strict` / `Standard`）と絶対的防御境界（Inviolable Guards）が確立された。

しかし、初期実装では TIP セッション（`crates/sumibi-tip/src/session.rs`）内部で `ProtectionProfile::Standard` が固定値として使用されており、以下の課題があった：
1. ユーザー設定（`settings.json`）や Host 政策管理からプロファイルを切り替える永続化・設定機構が存在しない。
2. セキュリティ要件の極めて高い環境（監査環境や高機密文書作成環境）において、ユーザーまたはシステム管理者が `Strict` プロファイル（`TS_SS_REGIONS` 拒絶・`TS_SS_NOHIDDENTEXT` 必須・明示的 `IS_TEXT`/`IS_CHAT` 必須）へ戻す手段がない。
3. Host 政策が更新された際に、ログオンセッション内で動作中の TIP セッションへ動的にプロファイル変更を伝播するプロトコル経路が確立されていない。

## 決定

ユーザー設定ファイル（`settings.json`）でのプロファイル永続化、公開 IPC プロトコルによる政策配信、および TIP セッションへの動的アトミック同期を統合的に実装する。

### 1. プロトコル層への型配置と後方互換性

`sumibi-protocol` クレートに `ProtectionProfile` 列挙型を配置し、IPC プロトコルメッセージ `Policy` 構造体へ追加する。

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtectionProfile {
    #[default]
    Standard,
    Strict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub settings: u64,
    pub authorization: u64,
    pub allowed: bool,
    #[serde(default)]
    pub protection_profile: ProtectionProfile,
}
```

- `#[serde(default)]` により、過去のバージョンまたはプロファイル情報を含まないメッセージを受信した際も、自動的に既定の `Standard` プロファイルへフォールバックする。
- 既存の `deny_unknown_fields` 原則を維持し、未知のプロファイル文字列や不正なフィールドは厳格に拒絶する。

### 2. 設定ファイル（settings.json）へのオプション統合

`crates/sumibi-host/src/settings.rs` の `Record` 構造体にフィールドを追加する：

```rust
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    scheme: StoredScheme,
    host: String,
    port: u16,
    chat_path: String,
    model: String,
    #[serde(default)]
    protection_profile: ProtectionProfile,
}
```

- **後方互換性**: 既存の `version: 1` 設定ファイルに `protection_profile` が存在しない場合、`#[serde(default)]` によりデフォルトで `ProtectionProfile::Standard` が設定される。
- **未知フィールドの拒絶**: `deny_unknown_fields` により、未知のプロパティやスペルミス（例: `"protection_profile": "ultra"`）は安全側に倒してパースエラー（`SettingsError::Invalid`）とする。
- **API 提供**: `Settings::new` はデフォルトで `Standard` を設定し、`with_protection_profile(profile)`、`set_protection_profile(profile)`、`protection_profile()` ゲッターを提供する。

### 3. Host 政策（PolicyAuthority）での管理と動的更新

`crates/sumibi-host/src/policy.rs` および `manual.rs`:
- `PolicySnapshot` に `pub protection_profile: ProtectionProfile` を追加。
- `PolicyAuthority::new(destination)` は既定で `Standard` を採用し、`new_with_profile(destination, profile)` で明示指定可能。
- `PolicyAuthority::set_protection_profile(&self, profile)` を提供。プロファイルが変更された場合、設定バージョン（`settings`）をインクリメントし、`self.changed.notify_all()` で接続中の全 Broker / IPC クライアントへブロードキャスト通知する。
- `Settings::save_and_apply` / `save_and_apply_if_unchanged` による設定ファイル更新時にも、アトミックに新しいプロファイルが反映される。

### 4. TIP セッション（TipHostSession）への動的伝播と適用

`crates/sumibi-tip/src/session.rs`:
- `SharedSessionState` に `protection_profile: AtomicU8`（0 = Standard, 1 = Strict）を追加。
- バックグラウンド `worker_loop` において、Host から `Worker::prepared_policy()` またはポリシー変更通知を受信した際、`state.set_protection_profile(policy.protection_profile)` を呼び出してアトミックに同期。
- `on_preserved_key`（Ctrl+J 押下時）において、動的に同期された `self.state.protection_profile()` を `EditPolicy` に設定して `ManualEdit::capture` を実行。

これにより、ユーザーが設定を変更すると、実行中の一般アプリを再起動することなく、次回のホットキー押下から新しい保護プロファイルが即座に反映される。

## 結果と影響

### メリット
1. **動的変更可能性**: Host 側の設定や運用要件に応じて、アプリ再起動なしに `Standard` と `Strict` を切り替え可能。
2. **完全な後方互換性**: 既存の `settings.json` および IPC メッセージとの互換性が保たれ、プロファイル省略時は安全な `Standard` にフォールバックする。
3. **絶対的防御境界の堅持**: プロファイル設定に関わらず、パスワード・PIN・Private・ReadOnly の絶対的拒絶（Inviolable Guards）は常に保証される。

### 検証
- 単体テスト `settings_protection_profile_default_and_explicit_options`: 省略時フォールバック、明示的 Standard / Strict、ラウンドトリップ、serde 拒絶、動的政策更新を検証。
- 単体テスト `shared_session_state_protection_profile_roundtrip`: TIP 共有状態における動的切り替えを検証。
- 全体検証 `dev.ps1 test-all`（534 テスト、`session-key-test`、`native-edit-test`、`Test-HostImage`、`Test-SuspendedImageProbe`）の完全合格を確認。
