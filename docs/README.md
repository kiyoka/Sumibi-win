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
- [Mac版移植範囲](MAC_PARITY.md)：共通契約とOS固有の再設計。

## 更新の規則

コードを説明する記述には基準コミットと参照ファイルを付ける。将来案と実装済みを分ける。
実機結果にはOS・アプリ・版・CPU・権限・自動／手動を記録する。
Mac参照は固定コミットにし、未マージPRや作業版をmainの実績に含めない。
図の箱・矢印は責務と所有を本文で説明し、通信経路の図と寿命の図を混ぜない。
文書だけの変更では機能試験を再実行したと記録しない。未決事項は実装前にADRと受入条件へ落とす。
