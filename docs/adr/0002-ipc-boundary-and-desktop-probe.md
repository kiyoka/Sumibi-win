# ADR 0002: IPC境界とデスクトップ最小検証

2026-10-21 / 状態: 初期採用案・最小検証済み、製品Broker未実装 / 関連: Issue #1。

## 判断

通常デスクトップのTIP/Broker間はローカルnamed pipeを第一候補にする。
本文を送る前にpeerを確認し、pipeの存在と名前だけを信頼しない。
対象ユーザーの同じログオンセッションだけへ明示DACLを与え、リモートクライアントを拒否する。
Brokerの最初のpipe instanceはFIRST_PIPE_INSTANCEを要求し、既存サーバーへの上書きや便乗をしない。
設定やAPIキーをDLLへ配布せず、COM参照・編集cookieをIPCへ流さない。

ワイヤーは[IPC_PROTOCOL](../IPC_PROTOCOL.md)の4バイトlittle endian長＋最大256KiBのUTF-8本文。
過大長は本文確保前に拒否し、不正・途中切断後は終端化する。再接続で新しいDecoderと認証を必要とする。
上位のJSON・版・要求ID・設定/認可版・件数・タイムアウトは後続実装で検査する。

## 認証の契約と残る判断

製品ではOSから取得したclient/server PIDを、起動時に把握した相手プロセスへ結び付ける。
さらにプロセストークンのユーザー・ログオンセッション・integrity・AppContainer状態を検査する。
SID限定ACLだけで「同一ユーザーの全プロセスが正しいTIP/Broker」とはみなさない。
PID再利用、偽サーバー、Broker再起動、多重起動、署名・配置パスの確認と起動nonceの渡し方を後続で固定する。
署名やパスだけを完全な相手証明ともみなさない。同一ユーザー侵害への完全耐性は保証しない。

接続数・同時未完了数・送受信バッファを有限にする。フレーム上限だけでは接続数によるメモリー増加を防げない。
pipeの非同期I/O、切断・取消・終了時の待機解除を実装し、TSFスレッド上で待たない。
本試作のPIPE_NOWAITを製品の非同期I/O方式として採用したわけではない。

## 実行した検証

`cargo run -p sumibi-setup --locked --offline -- ipc-test`

[pipe_probe.rs](../../crates/sumibi-setup/src/pipe_probe.rs)は同一プロセス内の一時pipeを作る。
TokenLogonSidで取得したSIDだけを許可するprotected DACLを使い、Everyoneを追加しない。
PIPE_REJECT_REMOTE_CLIENTS、FIRST_PIPE_INSTANCEを指定し、2個目のserver作成失敗を確認する。
client/serverのPIDをOSから取得して想定PIDと照合し、想定外PID・0の検査も確認する。
固定の合成フレームを小分けに読み、Decoderで復元後にdisconnect・全handle解放する。
SID・pipe名・本文をログ出力しない。標準のユーザー設定・既定IME・資格情報を変更しない。

2026-10-21、制限付きサンドボックスではアクセス拒否。通常ユーザーの実行では全検査成功。
これは製品クライアント／Brokerの相互認証、別ユーザー拒否、リモート接続実試行、
別プロセスのトークン照合が完了した証拠ではない。指定したリモート拒否フラグと実試行も区別する。

## AppContainer・昇格・別セッション

MicrosoftのIME要件では、DLLは入力先アプリのAppContainer制約を受ける。
通常デスクトップで通るpipeをAppContainerからそのまま利用できるとは判断しない。
同一AppContainer内pipeのLOCAL名前制約と、AppContainerから通常Brokerへの仲介は異なる問題である。
現在の採用範囲はWindows 11 x64・通常デスクトップ・同一ユーザーの通常権限に限定する案。
AppContainer、昇格アプリ、別ログオンセッションは初期非対応とし、対応のためにACLを全員へ拡大しない。
製品登録の前にホスト分類を行い、未対応環境は本文の収集・送信を開始しない。
別方式の仲介が必要なら、最小データ・認可取消・終了の伝播を含め別ADRで選定する。

## 根拠（2026-10-21確認）

- [Named pipeのアクセス制御](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)：既定ACLに依存しない理由。
- [GetTokenInformation](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-gettokeninformation)：トークン情報取得。
- [Client PID](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeclientprocessid)、[Server PID](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeserverprocessid)：OS経由のpeer確認。
- [IME要件](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements)：入力先の制約とTSF。

APIの説明は設計根拠であり、対応アプリ全体での実機成功を意味しない。
