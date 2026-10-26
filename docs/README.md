# 設計・開発資料

全体設計の整備: [Issue #3](https://github.com/kiyoka/Sumibi-win/issues/3)。

## 読む順序

1. [全体設計](TECHNICAL_OVERVIEW.md)：現行構成、将来の製品構成、変換・候補・送信・配布まで。
2. [所有者と寿命](INPUT_CONTROLLER_ARCHITECTURE.md)：TSFスレッド、COM、失効、非同期後始末。
3. [実装照合表](IMPLEMENTATION_AUDIT.md)：コード・未実装・検証結果・受入条件。
4. [開発と操作手順](DEVELOPMENT.md)、[TSFスパイク記録](TSF_SPIKE.md)：再現方法と限定条件。
5. [Issue #4 の受入進捗](ISSUE_4_PROGRESS.md)：現時点の根拠と、製品接続・一般アプリで残る条件。

## 採用判断と参照元

- [初期設計](DESIGN.md)：Issue #1時点の構成・数値・未決事項。現行全体説明は全体設計を参照。
- [ADR 0001](adr/0001-language-and-ime.md)：RustとTSFを採用した判断。
- [IPCのワイヤーと状態](IPC_PROTOCOL.md)、[ADR 0002](adr/0002-ipc-boundary-and-desktop-probe.md)、[ADR 0003](adr/0003-pinned-desktop-peers.md)：フレームから起動processの認証・非同期I/Oまで。
- [ADR 0004](adr/0004-one-shot-broker-and-owner-edit.md)：単発Host EXE、有限worker、Coreから所有TSFでの適用と失効。
- [ADR 0005](adr/0005-dll-key-broker-bridge.md)：実DLLキー・Core・TSFからEXE所有workerを通る非同期デモ。
- [ADR 0006](adr/0006-input-protection-metadata.md)：scope/status/compartmentの検査とTSF内部読取との境界。
- [ADR 0007](adr/0007-foreign-composition-boundary.md)：他サービスのcomposition拒否、通知と所有ホストの協力が必要な境界。
- [ADR 0008](adr/0008-owned-selection-composition.md)：選択compositionの所有・終了・強制終了とEXE通知サービスの寿命。
- [ADR 0009](adr/0009-cooperating-host-input-run.md)：所有ホストの実入力だけを追跡するCore、F7の一度だけの範囲選択・送信前再照合。
- [ADR 0010](adr/0010-cooperating-host-composition-growth.md)：実DLLによる入力compositionの伸縮とメタデータ専用の所有例外。
- [ADR 0011](adr/0011-owner-loop-input-invalidation.md)：メタデータ通知で所有ループを起こす仕組みと、本文を読まない失効cleanup。
- [ADR 0012](adr/0012-synchronous-input-transaction.md)：同期writeでの文字・caret・composition更新と、書込後の再入力禁止。
- [ADR 0013](adr/0013-synchronous-tracked-backspace.md)：記録末尾のBackspaceと全削除、repeat・UTF-16・失敗時の再削除禁止。
- [ADR 0014](adr/0014-bounded-input-recovery.md)：未確認入力の有限救済、保護失効、期限、明示コピー・破棄。
- [ADR 0015](adr/0015-broker-admission-through-delivery.md)：完了・受取済み結果も含めたBrokerの容量予約と解放。
- [ADR 0016](adr/0016-authenticated-reusable-broker-channel.md)：認証済み子Brokerの連続要求、worker専用の有限接続と取消。
- [ADR 0017](adr/0017-reusable-worker-and-owner-delivery.md)：EXE再利用worker、キー消費前のPermitと所有STA配送・取消・終了。
- [ADR 0018](adr/0018-retained-manual-range-and-source-validation.md)：generic保持rangeの選択変換と、本文を再取得しない送信前検査・観測した失敗の永久失効。
- [Mac版移植範囲](MAC_PARITY.md)：共通契約とOS固有の再設計。
- [ADR 0019](adr/0019-native-richedit-status-diagnostics.md)：標準RichEditのnative TSFでの拒否7構成と、本文を持たないstatus診断。
- [ADR 0020](adr/0020-owner-cancellation-and-shared-tsf-pump.md)：所有ホストの明示取消、原文bufferの解放と共通TSF pump・通知検証の境界。
- [ADR 0021](adr/0021-shared-input-recovery-budget.md)：EXEの共有救済4枠、消費前予約・レコード破棄までの保持と複数所有者のfallback。
- [ADR 0022](adr/0022-native-owner-window-boundaries.md)：所有windowの無効化・非アクティブ化・モード取消による追跡/変換/救済の失効と標準処理の維持。
- [ADR 0023](adr/0023-grapheme-backspace-within-recorded-input.md)：Unicode版を固定した記録内の書記素Backspace、通常ホストfallbackと範囲の限界。
- [ADR 0024](adr/0024-grapheme-navigation-in-owned-host.md)：専用ホストの書記素移動/Shift選択/Delete、本文を持たない境界表と保留結果の拒否。
- [ADR 0025](adr/0025-synchronous-selected-delete.md)：明示選択Deleteを実DLLの同期writeへ接続し、二重削除と削除本文の取得/救済コピーを防ぐ。
- [ADR 0026](adr/0026-initialize-explicit-selection-with-context.md)：初回文字入力なしで明示選択をF7へ接続し、選択方向と原文準備の一度だけの寿命を維持する。
- [ADR 0027](adr/0027-tracked-caret-and-forward-delete.md)：記録範囲内の左右移動と前方Delete、記録全体の変換継続、試行後の再操作禁止と共有救済枠。
- [ADR 0028](adr/0028-tracked-internal-backspace.md)：内部caretからのBackspace、前後の記録保持、共有削除計画と全repeatの境界検査。
- [ADR 0029](adr/0029-tracked-internal-insertion.md)：記録内caretへの文字挿入、前後の記録保持、合計容量と供給断片だけの救済。
- [ADR 0030](adr/0030-tracked-shift-selection.md)：記録内のShift選択、選択方向の照合、選択解除後の入力継続と明示選択の変換。
- [ADR 0031](adr/0031-recorded-selection-replacement.md)：記録内の選択置換/Delete/Backspaceで前後の入力を保持し、失効後に既存本文を採用しない条件。
- [ADR 0032](adr/0032-recorded-document-edge-navigation.md)：専用単一行ホストの文書端が記録内にある場合のHome/End・Shift選択と、位置だけを使う照合・二重移動の防止。
- [ADR 0033](adr/0033-bounded-core-waiting-input.md)：手動変換待機入力のCore上限・FIFO・試行ticket・Enter停止。専用adapterへの後続接続はADR 0034。
- [ADR 0034](adr/0034-shared-recovery-waiting-adapter.md)：専用adapterでの共有枠の取得前予約・実DLL/workerの順序・未適用救済と自己状態の照合。専用デモへの後続接続はADR 0035。
- [ADR 0035](adr/0035-waiting-demo-and-record-recovery-ui.md)：明示的なwaiting-demoの受付・複数救済の巡回/1件確認・window境界と公開直前の取消。製品キーは未接続。
- [ADR 0036](adr/0036-unified-demo-recovery-and-copy-expiry.md)：待機/通常入力の救済UI統合、本文除去後の操作情報の確認/破棄、コピー公開直前の期限照合。
- [ADR 0037](adr/0037-cancel-failed-waiting-start.md)：F7後の待機初期化失敗で捕捉した要求だけを取消し、重複通知と古い初期化の上書きを拒否する。
- [ADR 0038](adr/0038-owner-wake-for-ordinary-recovery.md)：待機デモで通常救済だけが残る間も表示と保護状態を定期確認する。
- [ADR 0039](adr/0039-waiting-demo-recovery-loss-warning.md)：救済がなくなっても未確認の入力結果が残る場合は文書確認を促す。
- [ADR 0040](adr/0040-waiting-deadline-and-unadmitted-fallback.md)：待機期限を表示し、未受入れ入力で選択中の原文を置換しない。
- [ADR 0041](adr/0041-configurable-shared-recovery-budget.md)：共有救済の件数と本文総量を一度に予約し、既定の診断容量を維持する。
- [ADR 0042](adr/0042-waiting-capacity-fallback-feedback.md)：待機容量超過による通常入力への切替を診断デモに表示する。
- [ADR 0043](adr/0043-interactive-native-key-activation.md)：診断windowを手動で前面化し、実TSFキー検証を続ける専用コマンド。
- [ADR 0045](adr/0045-machine-registration-boundary.md)：恒久登録前の配置・署名・更新条件。
- [ADR 0057](adr/0057-broker-side-session-order.md)〜[ADR 0059](adr/0059-bounded-broker-policy-fanout.md)：Broker側の要求順序、結果と政策更新の公開順序、有限の複数接続通知。
- [ADR 0060](adr/0060-host-to-broker-policy-relay.md)：Hostの政策版をBrokerの受付と接続通知へ渡す境界。
- [ADR 0061](adr/0061-two-client-broker-pipe-diagnostic.md)：同じ親processから開く2本の認証済みpipeでの通知・退役・再接続診断。

## 更新の規則

コードを説明する記述には基準コミットと参照ファイルを付ける。将来案と実装済みを分ける。
実機結果にはOS・アプリ・版・CPU・権限・自動／手動を記録する。
Mac参照は固定コミットにし、未マージPRや作業版をmainの実績に含めない。
図の箱・矢印は責務と所有を本文で説明し、通信経路の図と寿命の図を混ぜない。
文書だけの変更では機能試験を再実行したと記録しない。未決事項は実装前にADRと受入条件へ落とす。
