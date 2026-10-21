# Issue #1後の実装順序

2026-10-22更新。最小試作の根拠は[受入記録](ISSUE_1_ACCEPTANCE.md)。
各段階は直前の安全条件を満たしてから進める。未実装を本Issueの既存機能として扱わない。

## 1. Core・認証付きIPC・製品用入力基盤

登録済み: [Issue #4](https://github.com/kiyoka/Sumibi-win/issues/4)。

最初の段階として、版1の単一結果JSON、1接続1保留のclient状態、世代・取消・期限・切断・
再接続とCore模擬接続を実装した（[契約と検証範囲](IPC_PROTOCOL.md)）。
続いてsumibi-ipcに、固定process handleによるOS相互検査、合計8 endpoint、非同期pipeと
期限・取消を追加し、専用親子processで検証した（[ADR 0003](adr/0003-pinned-desktop-peers.md)）。
さらに単発Host EXE・容量4のworker・所有TSFへの結果配送を接続し、専用ACP storeで19ケースを検証した
（[ADR 0004](adr/0004-one-shot-broker-and-owner-edit.md)）。
実DLLのキー・Core・TSF編集から診断用EXE bridgeへ接続し、17ケースと非同期デモの6ケースも検証した
（[ADR 0005](adr/0005-dll-key-broker-bridge.md)）。
製品Brokerの発見・常駐dispatch、一般アプリの入力基盤は未完了で、受入条件全体を満たした意味ではない。
続いてscope/status/compartmentの保護検査を実装し、本文取得・source送信前・結果適用時に検査する。
状態を変更して戻した場合も拒否し、実DLL/TSFで39ケースを確認した
（[ADR 0006](adr/0006-input-protection-metadata.md)）。一般アプリでの判定は引き続き未検証。
次に他サービスのcompositionを同じcontextで検出して拒否し、通知による取消を追加した。
実DLL/TSFで16ケースと専用デモの空composition回帰を確認した
（[ADR 0007](adr/0007-foreign-composition-boundary.md)）。空compositionの履歴取消には所有ホストの協力が必要。
自分の追跡compositionと一般アプリへの統合は引き続き未実装。
続いてF6プレビューの選択範囲を自己所有のcompositionとして保持する基盤を追加し、
開始拒否・確定・取消・強制終了・別compositionへの非適用を18ケースで確認した
（[ADR 0008](adr/0008-owned-selection-composition.md)）。これは既存選択の寿命管理であり、
通常打鍵の追跡・範囲拡張や製品InputSessionの完了ではない。

- 版付きJSONメッセージ、有限接続・未完了集合、ID・世代・取消・切断・重複・再接続を定義。
- fake peerと模擬トランスポートから開始し、別プロセスのユーザー／logon SID／PID／起動世代を照合。
- named pipeの非同期I/Oと停止伝播、偽server・PID再利用・容量超過・終了待機を試験。
- composition、通常入力、入力スコープの機密性判定、読取専用、未知条件での拒否を実装。
- 待機容量を消費前に確保し、超過切替と救済期限を決定。遅延ホストキーは再送しない。
- Notepad等で標準Undo・実打鍵・別欄拒否を確認。AppContainerは別ADRで成立するまで非対応。

受入条件：実APIなしで、正常要求と全失効条件をCore→IPC→所有スレッド編集まで一貫して検証すること。

## 2. 手動変換の縦断実装

登録済み: [Issue #5](https://github.com/kiyoka/Sumibi-win/issues/5)。前提はIssue #4。

- Host EXE、送信先別同意、設定形式版・原子的保存、Credential Managerのキー保存と削除。
- 書記素分割の依存とUnicode版、1000書記素とバイト／UTF-16上限の検査。
- 模擬HTTP、転送拒否・再送なし・60秒・応答上限・固定エラー。実通信は明示操作の別検証。
- 初回1候補と選択変換、取消・設定変更・同意取消・応答後の対象再検査。

受入条件：未同意・キー欠損・異なる認可版で送信せず、失敗時に原文を失わないこと。

## 3. 追加候補・ユーザー辞書・ローカル補完

登録済み: [Issue #6](https://github.com/kiyoka/Sumibi-win/issues/6)。前提はIssue #5。

- 候補件数のMac SPECとの差を決定し、現在結果を先頭へ。取得だけでは書かない。
- TSF UIElementのホスト表示指示、非アクティブUI、DPI・複数画面・読み上げ。
- 登録辞書の検証と送信量、ローカル索引の出所・固定入力・ハッシュ・生成手順・LICENSE。
- 中黒→Mozc→Sudachiの補完、重複・順序・本文と注釈の分離、API失敗時の非代替。

受入条件：候補選択時だけ対象を再照合して置換し、1回Undoで切替前へ戻ること。

## 4. Decisions自動変換

登録済み: [Issue #7](https://github.com/kiyoka/Sumibi-win/issues/7)。手動経路成立後。

- 公式API・モデル・料金・回答仕様を実装時に再確認し、独立同意・既定オフ。
- 200ms固定窓、32状態、未送信256、512書記素案、状態IDによる対応付けと厳格な応答検査。
- 送信前の回数永続予約、初期10000回、失敗・取消も計数。回数を料金と混同しない。
- 入力は即時、手動待機行列を使わない。Broker全体の直列化と手動優先、最新世代だけ適用。

受入条件：未許可入力を収集せず、古い判定・変換・認可を破棄し、回数上限を同時要求で超えないこと。

## 5. 公開準備

登録済み: [Issue #8](https://github.com/kiyoka/Sumibi-win/issues/8)。機能と互換範囲の確定後。

登録範囲、署名、配置、Host起動、更新中DLL、旧版復旧、削除と設定保持をADRで決定する。
x86/ARM64、昇格、AppContainer、Terminal等を個別の互換表で扱う。
既定IMEを強制変更せず、導入・更新・削除を実機で確認してから配布する。
