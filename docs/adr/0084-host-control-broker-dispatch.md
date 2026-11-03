# ADR 0084: Host所有調停ループへのBroker受付と変換ディスパッチの統合

## 目的と背景

ADR 0073で明示的な同意ウィンドウと寿命調停を行う`HostControl`が導入され、
ADR 0065/0074で共有Broker待受（`BrokerBoot`）とログオン排他ロック（`HostInstance`）が定義された。
しかし、これまでの`HostControl`内部のワーカーループでは、`BrokerBoot`の開始と同意更新の承認のみが行われ、
外部peerの受付（`begin_accept` / `begin_private_accept`）や変換要求のポーリング（`advance_native_chat`）は
未接続のままだった。

本書では、`HostControl`の所有ワーカーループにpeer受付チャネルと`BrokerBoot`の変換ディスパッチを統合し、
常駐Hostプロセスが安全に固定peerを受け付け、同意状態に基づいて変換を実行・返信する製品経路を確立する。

## 設計と契約

1. **ピア受付要求チャネル（`peer_requests`）**:
   - UIの同意コマンド用`commands`（容量1）とは独立したピア登録チャネル（容量8）を設ける。
   - `HostControl`は以下の非ブロッキング入口を公開する：
     - `try_accept_peer(&self, peer: PinnedProcess) -> Result<AcceptId, ControlError>`
     - `try_accept_private_peer(&self, peer: PinnedProcess) -> Result<(AcceptId, Nonce), ControlError>`
   - ワーカーループは各周回で`peer_requests.try_recv()`により要求を取り出し、
     `BrokerBoot::begin_tracked_accept`または`begin_private_accept`を呼び出して結果を返答する。
   - 容量超過、Host停止中、またはキャパシティ枯渇時は即座にエラーを返し、呼び出し元スレッドをブロックしない。

2. **所有ワーカーループ内でのディスパッチ統合**:
   - ワーカーループは、同意コマンド（Grant / Revoke）および同意更新の承認（`acknowledge_consent_update`）の処理に加え、
     `boot.advance_native_chat`（または設定されたトランスポート）を有限期限（5〜10ms）で呼び出す。
   - クライアント接続、Hello/Readyハンドシェイク、Convert要求の受付、ワーカーへの投入、および結果返信が
     Hostの所有スレッドで直列かつ安全に進行する。
   - 送信未同意時や同意取消時は、`PolicyAuthority`の既存のガードにより変換要求が安全に拒否（`PolicyDenied`）され、
     インフライトの送信はキャンセル・回収される。

3. **終了とリソース退役の保証**:
   - `request_close`時は、新規ピア受付を直ちに拒否し、`boot.retire_for_restart`によって
     すべての保留中受付・アクティブ接続・実行中ワーカーの正常終了を有限時間内に確認する。

## 検証と残る条件

`host_control`の単体・結合テストにおいて：
1. 起動した`HostControl`に対して固定peerを登録し、生成されたプライベートパイプ経由でクライアントが接続できること。
2. 送信同意が付与されている状態で、変換要求が正常にディスパッチされ結果が返ること。
3. 同意取消後、同一接続からの変換要求が直ちに拒否されること。
4. HostControlの終了時に、全受付とパイプが安全に後始末されること。

製品TIPが自律的に製品Hostを発見し固定ハンドルを取得する仕組み（製品発見プロトコル）、
および一般アプリのTSFへのキー配送とUndo保証はIssue #4の残課題として継続する。
