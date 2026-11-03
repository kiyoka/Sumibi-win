# Windows版 設計書

現行コードに照合した全体説明は[全体設計](TECHNICAL_OVERVIEW.md)および[入力コントローラ設計](INPUT_CONTROLLER_ARCHITECTURE.md)を参照。
本書は 2026-10-20 の初期判断を保存しつつ、[Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4)（Core・認証付きIPC・TSF入力基盤 / [PR #10](https://github.com/kiyoka/Sumibi-win/pull/10)）および [Issue #5](https://github.com/kiyoka/Sumibi-win/issues/5)（手動変換の縦断統合 / [PR #11](https://github.com/kiyoka/Sumibi-win/pull/11)）の成果を反映して最新化された全体設計書である。

言語はRustを採用（メモリ安全性・長期保守を優先）。Windows 11 x64＋Visual Studio／Rustツールチェーンで検証完了。
言語の比較は[ADR 0001](adr/0001-language-and-ime.md)、参照範囲は[移植表](MAC_PARITY.md)。手動変換の縦断統合仕様は[ADR 0098](adr/0098-manual-conversion-end-to-end-integration.md)。

## 構成と責務

```text
入力先アプリ（それぞれのプロセス／TSF STAスレッド）
  sumibi_tip.dll (COM Inproc Server)
    TSF KeyEventSink (Ctrl+J, Ctrl+Z)
    → TipHostSession (ログオン排他ブートストラップパイプ自律発見・相互Peer認証)
    → TargetGuard / ProtectionProfile (Standard / Strict 二層保護判定)
    → TextStore / Context (原文取得・選択範囲検査)
    → DrainWindow (非同期STAメッセージ通知: WM_SUMIBI_DRAIN)
    → DrainSink (テキスト自動置換・1-step Undo履歴管理)
             │ 要求／取消／結果（名前付きパイプ IPC）。秘密キーとCOM参照は渡さない
ユーザーのログオンセッション
  sumibi-host.exe (常駐バックグラウンドサービス)
    HostService (ログオン限定 BootstrapServer: \\.\pipe\Sumibi.Bootstrap.v1.<SID>)
    → HostControl / PolicyAuthority (設定・認可版管理・動的政策同期)
    → WinHTTP Client (OpenAI API / chat completions HTTPS 通信)
    → Windows Credential Manager ("Sumibi-win/API-key/v1" 暗号化キー管理)
    → GUIマネージャー (ConsentWindow 送信同意管理 / SettingsWindow 設定管理 / System Tray)
```

Cargo workspaceを用い、以下のクレートに分割・実装済み：
- `sumibi-core`: OS非依存。Unicode 17.0.0書記素クラスタ境界判定、1,000書記素上限、要求/世代/対象照合。unsafe禁止。
- `sumibi-protocol`: バージョン管理された型付きIPCフレーム、JSON直列化、要求・応答・失効メッセージ。unsafe禁止。
- `sumibi-ipc`: Windows名前付きパイプ、相互プロセスPeer認証（ログオンSID・PID・トークン検証）、非対称ハンドオフ、Overlapped I/O。
- `sumibi-winhttp`: 安全なWinHTTPラッパー。TLS/HTTPS通信、localhost限定平文例外、リダイレクト拒否、60秒タイムアウト。
- `sumibi-credentials`: Windows 資格情報マネージャー（`CredReadW`/`CredWriteW`）の安全な暗号化キー読み書き。
- `sumibi-host`: 常駐Hostサービス、送信同意UI、設定UI、システムトレイ、外部API通信ブローカー。
- `sumibi-tip`: TSF TIP COM DLL、ホットキー（Ctrl+J/Ctrl+Z）、STA非同期ドレイン、二層保護プロファイル、1-step Undo。
- `sumibi-setup`: 開発・診断・TSFテストハーネス、HKCU COM登録、E2Eセッションテスト、実機Live API変換診断。

CoreとProtocolはunsafeを禁止し、TSF/COM・OS APIをアダプターへ隔離する。
CoreはOSオブジェクトを保持せず、意味上のText/Enter/Backspace/Convert/Cancelと効果で接続する。
TSF thread/contextごとに状態を持ち、別入力欄・文書の状態を混在させない。
Brokerが複数アプリから要求を受けるため、セッションIDと要求IDはプロセス横断で衝突しないものを用いる。

TSF呼出しとCOMオブジェクト操作は所有スレッド（STA）で行う。IPCの完了を隠しウィンドウ（`DrainWindow`）へのメッセージポスト（`WM_SUMIBI_DRAIN`）でそのスレッドへ通知し、新しい編集セッションで安全に読取・書込する。編集cookieや生の範囲参照を通信待ちの権限として使い回さない。
RequestEditSessionの延期・拒否も通常の失敗／待機として扱い、ロック待ちでホストを止めない。

## 非同期の適用条件

要求は sessionId / requestId / inputGeneration / settingsRevision / authorizationRevision と、
操作種別、原文、現在結果を持つ。DLL内の対象記録にはcontextの同一性、範囲、選択、期待原文を保持する。
BrokerにはTSF座標やアプリ名を渡す必要を作らない。辞書と設定はBrokerが一貫した版で読み取る。

応答時には以下をすべて確認してから、許可された編集セッション内で一度だけ置換する。

1. 要求が未完了で、同じsessionId・requestId・世代である。
2. 入力先・context・選択・対象範囲が維持されている。
3. 記録範囲の全文を読み取れ、期待原文と一致する。
4. 設定・送信同意・有効化・保護入力の条件が現在も成立する。

読取不能・部分一致・別欄・古い結果は適用しない。近傍検索による置換先推定をしない。
取消要求は通信節約のためで、取消成功を安全性の根拠にしない。
設定取消時はBrokerも旧認可の未送信要求を拒否し、DLLへ認可版の更新を通知する。
再接続時は旧要求を復活させず、状態を再確認して新しい操作を待つ。

## 手動変換

Tracking → ManualPending → Applied / Failed / Cancelled。
初回成功は候補窓を出さず1候補を確定。再変換は元原文と現在結果で追加取得し、選択時だけ置換する。
通信待ちの文字と制御イベントは順序を保持する。後続のConvertが次の要求を開始したら排出を止める。
Mac同様、遅延したホスト向けEnter等は自動再送しない案で、固定の未適用表示にする。
この案は「後でEnterを必ず実行する」というMac要件との差を残す。SendInputで補わない。

通常Enterでは原文を確定して元イベントをホストへ一度渡す。確定自体が延期された場合に二重入力や
原文消失が起きないかを試作で確認し、成立しなければキー配送設計を見直す。
Escは候補窓→要求／追跡→ホストへの通過の順に扱う。応答適用より取消操作を優先する。
選択変換は追跡と直前候補の優先条件を整理し、選択全文を読める場合だけ始める。

待機と救済には上限が必要。提案値は待機1,024イベントかつ文字本文16,384 UTF-16単位、
プロセスごとの救済本文も合計16,384単位、保持10分。容量はIPCの上限とは別。
上限に達したら要求を取り消し、既存待機文字は救済へ移し、収まらない文字は削除・切捨てず
同じ入力先へ安全に原文として確定するか、収容可能なことを確認してからキーを消費する。
この切替を安全に実装できることが着手前の設計ゲート。値と超過動作は確定仕様ではない。
コピー・破棄は明示操作のみ。救済を別欄へ自動入力しない。クラッシュ後の本文復元は保証しない。

## 自動変換（後続）

Observing → Scoring → Converting → Validating → Applied / Discarded。
打鍵は常時即座に反映し、手動待機行列を使わない。200ms固定窓・最大32状態・未送信256状態を
引き継ぐ案。判定・変換・確認は入力セッションごとに直列化し、全体送信量もBrokerで制限する。
Macの「同時1件」がWindowsの複数ホストでどう共有されるかは別設計とし、
初期案ではBroker全体でも自動処理1件に制限し、入力先切替で旧観測を破棄する。
既定オフ、独立同意、公式接続先限定、閾値0.5、判定10秒、512文字、累積判定10,000回。
回数は送信前の予約を直列・永続化し、複数DLLからの同時要求で上限を超えないようにする。
手動要求は自動処理を取り消して優先し、辞書・設定・同意の変更で古い結果を拒否する。

## IPC・保存・送信

通常デスクトップの最初の候補はローカルnamed pipe。明示DACLで対象ユーザー／ログオンセッションを
制限し、リモート接続拒否、接続相手のトークン／セッション確認、プロトコル版を設ける。
ユーザー制限だけで同じユーザーの全プロセスが信頼できるとはみなさず、想定相手の確認と
メッセージ検証を行う。同一ユーザーの侵害プロセスから秘密を完全保護する保証はしない。
AppContainerからの接続と許容される仲介方式は未解決。ACLを全員へ広げて対応しない。

メッセージは長さ付きUTF-8 JSON案、1メッセージ最大256KiB、件数・文字列長・種別も検証。
過大・不正・未知版は拒否し、本文をログに出さない。フレーム上限は入力待機の容量と独立。
タイムアウト、切断、応答の重複、取消と成功の競合を模擬する。バックプレッシャー時に本文を無制限蓄積しない。

APIキーはBrokerだけがWindows Credential Managerへ保存する案。設定JSONとは分離し、画面は短い伏せ字。
ユーザー単位の設定をLocalAppDataに保存し、一時ファイルからの置換・形式版・破損時の安全な初期値を設ける。
送信先変更で同意が別の送信先へ引き継がれない契約にする。詳細な同意キーと移行形式は実装前に定義する。
入力原文・候補・文脈・キー・生のHTTPエラーを通常ログへ書かない。固定分類だけを表示する。

通信はWinHTTPを第一候補に調査し、HTTPS、localhostのみのHTTP例外、タイムアウト60秒、
リダイレクト拒否、自動再送なし、Cookieと不要なキャッシュなしを実装契約にする案。
API認証情報を別ホストへ転送しない。プロンプトはMac固定コミットの契約を参照して移植する。
現段階ではHTTP／Decisionsのコードを導入していない。

## Unicode・安全な入力先

送信文字数は拡張書記素クラスタで原文＋現在結果＋辞書を合算して最大 1,000 書記素（`MAX_TOTAL_GRAPHEMES = 1,000`）に制限される。
Unicode 版は Mac 版と完全に一致する **Unicode 17.0.0**（`CONVERSION_UNICODE_VERSION: (17, 0, 0)`、[ADR 0098](adr/0098-manual-conversion-end-to-end-integration.md)）を採用。
さらに、UTF-16 1,024 単位上限、4,096 バイト制限、および制御文字の排除（安全な空白類を除く `\u{0000}..\u{001F}`, `\u{007F}..\u{009F}` の拒絶）を実装済み。不正 UTF-16 は送信・置換を拒絶する。
対象照合は UTF-16 コード単位の完全一致を適用し、大小文字や空白を変えず、正規化もしない。

### 二層保護プロファイルと絶対的防御境界（[ADR 0095](adr/0095-app-protection-profiles-and-acceptance-criteria.md) 〜 [ADR 0097](adr/0097-settings-window-protection-profile-ui.md)）

1. **Standard プロファイル（既定）**:
   - `TS_SS_REGIONS`、`IS_DEFAULT`、`IS_TEXT` 等を持つ Notepad（Windows 11 メモ帳）や RichEdit アプリ等の一般エディタでの変換を許可。
2. **Strict プロファイル**:
   - `TS_SS_NOHIDDENTEXT` を必須とし、高機密監査環境向けに `HiddenTextPossible`（0x80070005）で安全に遮断。
3. **絶対的防御境界（Inviolable Guards）**:
   - パスワード・PIN・Private（`IS_PASSWORD`, `ES_PASSWORD`, `EM_GETPASSWORDCHAR` 等）、読み取り専用（`TS_SD_READONLY`, `ES_READONLY`）、複数選択（`TS_SS_DISJOINTSEL`）、一時コンテキスト（`TS_SS_TRANSITORY`）、空選択、送信未同意時は、プロファイル設定に関わらず**無条件で即座に拒絶（Fail-Closed）**し、本文テキストの取得・送信は一切行わない。

## 実装順序と達成状況

| 段階 | 成果と確認 | 状態 |
| --- | --- | --- |
| 0: 基本設計 | 言語比較・移植表・Issue登録・アーキテクチャ境界策定（[ADR 0001](adr/0001-language-and-ime.md)） | **完了** (PR #2, #9) |
| 1: TSFスパイク | 登録／解除、点線下線、Enter／Esc、固定結果置換、Undo、別欄非適用（[ADR 0002](adr/0002-tsf-spike-scope.md)） | **完了** (PR #2) |
| 2: Core＋IPC | 型付きProtocol、ログオン限定ブートストラップ、相互Peer認証、STA非同期ドレイン（[ADR 0003](adr/0003-host-process-architecture.md)〜[0097](adr/0097-settings-window-protection-profile-ui.md)） | **完了** (PR #10 / Issue #4) |
| 3: 手動変換 | WinHTTP製品通信、資格情報管理、送信同意GUI、Unicode 17.0.0、1,000書記素、実機Live API変換・Undo実証（[ADR 0098](adr/0098-manual-conversion-end-to-end-integration.md)） | **完了** (PR #11 / Issue #5) |
| 4: 追加候補・辞書 | 追加候補取得・表示、ユーザー辞書（`<user_dictionary>`タグ動的展開実装済み）、ローカル辞書補完 | 次期段階 (Issue #6) |
| 5: 自動変換 | Decisions自動変換、固定窓・回数制限・独立同意・手動優先 | 後続段階 (Issue #7) |
| 6: 公開準備 | 互換表、署名、本番登録／更新／解除、公開パッケージング | 最終段階 (Issue #8) |

テストは偽キー・専用設定・模擬HTTP。CoreテストはMacでも実行可能な構成を目指すが、
Windows SDK依存部はWindows CIと実機で検証する。Windows 11 x64とVisual Studioは利用者から利用可能と確認済み。
必要なC++ワークロード／SDK、Rustツールチェーン、実機への操作方法は未確認。
インストーラーの管理者権限、登録範囲、Broker起動方式、署名手段、更新時DLL解放はスパイクで決める。
IMEの既定設定を利用者の代わりに強制変更しない。

## 根拠（2026-10-20参照）

- [TSF編集セッション](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontext-requesteditsession)
- [Named pipeのアクセス制御](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)
- [Credential Manager書込](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew)
- [IME要件・AppContainer制約](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements)

上記はAPIの根拠で、構成・数値・対応範囲の案が実機で成立した証拠ではない。
