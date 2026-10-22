# 設計・開発資料

全体設計の整備: [Issue #3](https://github.com/kiyoka/Sumibi-win/issues/3)。

## 読む順序

1. [全体設計](TECHNICAL_OVERVIEW.md)：現行構成、将来の製品構成、変換・候補・送信・配布まで。
2. [所有者と寿命](INPUT_CONTROLLER_ARCHITECTURE.md)：TSFスレッド、COM、失効、非同期後始末。
3. [実装照合表](IMPLEMENTATION_AUDIT.md)：コード・未実装・検証結果・受入条件。
4. [開発と操作手順](DEVELOPMENT.md)、[TSFスパイク記録](TSF_SPIKE.md)：再現方法と限定条件。

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

## 更新の規則

コードを説明する記述には基準コミットと参照ファイルを付ける。将来案と実装済みを分ける。
実機結果にはOS・アプリ・版・CPU・権限・自動／手動を記録する。
Mac参照は固定コミットにし、未マージPRや作業版をmainの実績に含めない。
図の箱・矢印は責務と所有を本文で説明し、通信経路の図と寿命の図を混ぜない。
文書だけの変更では機能試験を再実行したと記録しない。未決事項は実装前にADRと受入条件へ落とす。
