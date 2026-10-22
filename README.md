# Sumibi for Windows

ローマ字／英語をLLMで日本語へ変換する、モードレスなWindows入力メソッド。
実装言語はRust、入力方式はTSFを採用します。
現在はCore・TSF DLLと、入力・下線表示を試す専用デモを実装しています。常設IMEやインストーラーはまだありません。
開発用ツール `sumibi-setup` で、プロセス内に限定した実TSFの登録・有効化・解除を検証できます。
`edit-test` では専用入力欄を使い、固定文字列の置換と誤適用防止を実TSF上で検証できます。
`key-test` では検証用DLLの予約キーから固定変換し、専用ホストでのUndoを確認できます。
下線の表示属性プロバイダーを追加し、`edit-test` で対象範囲への属性付与と解除を検証できます。
検証用のプレビューはセッション間で下線属性を保持し、取消・確定・許可取消で解除します。
原文・選択変更の通知による自動取消と、Esc取消／Enterで原文のまま通過する処理も検証しています。
`window-test` では一時ウィンドウを作り、TSFの通常キー配送と実ウィンドウ間のフォーカス通知を確認できます。
検証ホストでは本文と下線を描画し、取消・固定変換・Undo後の表示も確認できます。
本文や表示属性の変更通知に合わせて表示を自動更新します。
`input-test` で文字入力・カーソル移動・範囲選択・削除・Enter・Undo/Redoを自動検証し、`demo` では同じ入力経路を手動で試せます。
デモはF6で下線プレビュー、F7で固定結果へ変換、Escで取消します。LLM通信はまだ行いません。
Issue #4では版付きJSONとclient接続状態を追加し、取消・切断・再接続・期限切れをCoreと模擬通信で検証しています。
`ipc-peer-test`では起動した別プロセスとの相互照合・非同期pipe・取消を検証できます。
`broker-test`では別Host EXE→版付きIPC→Core→実TSF編集を専用入力欄で検証します。通常ビルドは変換を許可せず、固定結果は開発用feature限定です。
`broker-key-test`は実DLLのキーから、`broker-input-test`は非同期デモの入力と終了まで検証します。
`broker-demo`では同じ非同期の固定変換を手動で試せます（[起動と操作](docs/DEVELOPMENT.md#実dllのキーから非同期変換を試す)）。
常駐Broker、一般アプリの入力基盤、HTTP通信はまだありません。
入力保護の検査を追加し、通常テキストと確認できないscope・状態ではDLLの本文取得と要求送信を拒否します。
`protection-test`で実DLL/TSFの39ケースを検証できます（[境界と制限](docs/adr/0006-input-protection-metadata.md)）。
同じ入力欄に他サービスのcompositionがある場合も拒否します。`composition-test`で16ケースを検証でき、空compositionの履歴取消には専用ホストの通知を使います（[通知の境界](docs/adr/0007-foreign-composition-boundary.md)）。
F6プレビューは選択範囲をSumibi自身のcompositionとして保持し、確定・取消時に終了します。`owned-composition-test`で18ケースを検証できます（[所有と制限](docs/adr/0008-owned-selection-composition.md)）。
`tracking-demo`では実際に入力した範囲だけをF7で変換できます。移動・外部編集等で追跡が無効なら変換を見送り、全文を推測しません。`tracking-input-test`で25ケースを検証できます（[操作手順](docs/DEVELOPMENT.md#通常入力の範囲だけを変換する)、[条件と制限](docs/adr/0009-cooperating-host-input-run.md)）。
`composing-demo`では同じ入力を実DLLのcompositionと下線で追跡し、文字入力で広げ、Backspaceで縮めます。`composing-input-test`で29ケースを検証できます（[操作手順](docs/DEVELOPMENT.md#入力compositionの伸縮を試す)、[条件と制限](docs/adr/0010-cooperating-host-composition-growth.md)）。本文はホストで反映する診断経路で、一般アプリへの統合は未実装です。
外部編集や保護状態の通知では、次の打鍵を待たず所有ループで古い追跡を解除します。`input-notify-test`で通知20ケースを検証できます（[条件と制限](docs/adr/0011-owner-loop-input-invalidation.md)）。
`transaction-demo`のBackspaceは記録末尾だけを同じ同期writeで削除し、repeatも一括更新します。全削除でcompositionを終了し、失敗時には再削除しません。`backspace-input-test`で27ケースを検証できます（[条件と制限](docs/adr/0013-synchronous-tracked-backspace.md)）。
Notepad等の実アプリでの表示と実打鍵は未確認です。
`recovery-demo`では固定の入力失敗を再現し、F8で未確認入力を確認できます。1件・最大1024 UTF-16単位を最大10分だけ保持し、明示コピー／破棄を提供します。自動再入力はしません。`recovery-input-test`で25ケースを検証できます（[起動と操作](docs/DEVELOPMENT.md#未確認入力の救済を試す)、[保護と保持の契約](docs/adr/0014-bounded-input-recovery.md)）。
`transaction-demo`では文字入力・caret・compositionを同じ同期writeで更新します。書込後の失敗では自動再入力せず、確認を表示します。`transaction-input-test`で22ケースを検証できます（[起動と操作](docs/DEVELOPMENT.md#同期入力取引を試す)、[範囲と失敗時の契約](docs/adr/0012-synchronous-input-transaction.md)）。

- [実装着手 Issue #1](https://github.com/kiyoka/Sumibi-win/issues/1)
- [言語・方式の選定案](docs/adr/0001-language-and-ime.md)
- [Windows版全体設計](docs/TECHNICAL_OVERVIEW.md)
- [入力経路の責務と寿命](docs/INPUT_CONTROLLER_ARCHITECTURE.md)
- [設計と実装の照合表](docs/IMPLEMENTATION_AUDIT.md)
- [設計資料の入口](docs/README.md)
- [Windows版初期設計](docs/DESIGN.md)
- [Mac版からの移植範囲](docs/MAC_PARITY.md)
- [開発環境と現在の実装範囲](docs/DEVELOPMENT.md)
- [TSFスパイクの検証状況](docs/TSF_SPIKE.md)
- [IPCフレーム層](docs/IPC_PROTOCOL.md)
- [IPC境界の調査と最小検証](docs/adr/0002-ipc-boundary-and-desktop-probe.md)
- [別プロセスの固定ハンドル認証と非同期IPC](docs/adr/0003-pinned-desktop-peers.md)
- [単発Brokerと所有TSFスレッドへの結果配送](docs/adr/0004-one-shot-broker-and-owner-edit.md)
- [実DLLのキーと診断用非同期bridge](docs/adr/0005-dll-key-broker-bridge.md)
- [入力保護メタデータの検査](docs/adr/0006-input-protection-metadata.md)
- [他サービスのcompositionへの非干渉](docs/adr/0007-foreign-composition-boundary.md)
- [所有する選択compositionの寿命](docs/adr/0008-owned-selection-composition.md)
- [所有ホストの入力範囲追跡](docs/adr/0009-cooperating-host-input-run.md)
- [所有ホストの入力compositionの伸縮](docs/adr/0010-cooperating-host-composition-growth.md)
- [所有ループによる入力追跡の失効処理](docs/adr/0011-owner-loop-input-invalidation.md)
- [同期入力取引と二重入力の防止](docs/adr/0012-synchronous-input-transaction.md)
- [記録済み入力末尾の同期Backspace](docs/adr/0013-synchronous-tracked-backspace.md)
- [未確認入力の有限救済と明示コピー](docs/adr/0014-bounded-input-recovery.md)
- [Issue #1の受入記録](docs/ISSUE_1_ACCEPTANCE.md)
- [後続の実装順序](docs/NEXT_STEPS.md)

設計基準はSumibi-mac main `553c69021eda7a8a5944d8748357f3790cc2f45c`。
Mac版とコードを自動同期せず、このリポジトリで独立して実装・保守する案です。
採用判断と製品としての確定事項、実機で確認済みの事実は各資料で区別します。
