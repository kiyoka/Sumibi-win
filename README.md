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
Brokerの予約は未受取・受取済み結果も含め合計4件に制限し、解放後に枠を戻します（[容量と寿命](docs/adr/0015-broker-admission-through-delivery.md)）。
`broker-key-test`は実DLLのキーから、`broker-input-test`は非同期デモの入力と終了まで検証します。
`broker-demo`では同じ非同期の固定変換を手動で試せます（[起動と操作](docs/DEVELOPMENT.md#実dllのキーから非同期変換を試す)）。
起動したBrokerを連続要求で再利用するworker専用APIも追加しています。`broker-channel-test`で同じ子process・取消・期限・容量・終了を検証できます（[起動手順](docs/DEVELOPMENT.md#broker接続の再利用を検証する)、[契約](docs/adr/0016-authenticated-reusable-broker-channel.md)）。既存デモもEXE workerを通して同じBrokerを再利用し、取消・失敗後は次の明示操作で新しい接続を使います。`broker-worker-test`でworkerの容量・配送・終了を検証できます（[所有と寿命](docs/adr/0017-reusable-worker-and-owner-delivery.md)）。
常駐Broker、一般アプリの入力基盤、HTTP通信はまだありません。
手動変換待ちの文字・Backspace・Enter・Convertを有限に保持するCoreを追加しました。FIFO順序、消費前の容量確保、結果不明時の停止を検証し、Enterは自動配送しません（[Coreの設計](docs/adr/0033-bounded-core-waiting-input.md)）。
`waiting-input-test`で専用adapterから実DLL・非同期workerへ接続し、26ケースを検証できます。受入れ前に既存の共有救済枠を予約し、未適用の入力と結果不明を区別します（[共有枠と順序](docs/adr/0034-shared-recovery-waiting-adapter.md)）。
`waiting-demo`でF7変換中の入力を試せます。最大4件・8秒で待機を止め、Enterは救済へ移します。F8で待機・通常入力の救済を順に確認し、表示中の1件をF9で破棄・F10でコピーできます。救済は最大10分です。`waiting-demo-test`で配送・確認・開始失敗・期限とEsc後の通常入力の57ケースを検証できます（[起動と操作](docs/DEVELOPMENT.md#待機入力デモを試す)、[期限と通常入力](docs/adr/0040-waiting-deadline-and-unadmitted-fallback.md)）。製品キー・一般アプリへの接続は未実装です。
`native-edit-test`で標準RichEditの実TSF contextと拒否理由を検証できます。現在のRichEditは保護条件を満たさず、変換は未対応です（[手順と実測](docs/adr/0019-native-richedit-status-diagnostics.md)）。
入力保護の検査を追加し、通常テキストと確認できないscope・状態ではDLLの本文取得と要求送信を拒否します。
`protection-test`で実DLL/TSFの39ケースを検証できます（[境界と制限](docs/adr/0006-input-protection-metadata.md)）。
同じ入力欄に他サービスのcompositionがある場合も拒否します。`composition-test`で16ケースを検証でき、空compositionの履歴取消には専用ホストの通知を使います（[通知の境界](docs/adr/0007-foreign-composition-boundary.md)）。
F6プレビューは選択範囲をSumibi自身のcompositionとして保持し、確定・取消時に終了します。`owned-composition-test`で18ケースを検証できます（[所有と制限](docs/adr/0008-owned-selection-composition.md)）。
`tracking-demo`では実際に入力した範囲だけをF7で変換できます。移動・外部編集等で追跡が無効なら変換を見送り、全文を推測しません。`tracking-input-test`で25ケースを検証できます（[操作手順](docs/DEVELOPMENT.md#通常入力の範囲だけを変換する)、[条件と制限](docs/adr/0009-cooperating-host-input-run.md)）。
`composing-demo`では同じ入力を実DLLのcompositionと下線で追跡し、文字入力で広げ、Backspaceで縮めます。初回の文字入力前でも明示選択のF7変換を開始でき、逆向き選択も維持します。`composing-input-test`で46ケースを検証できます（[操作手順](docs/DEVELOPMENT.md#入力compositionの伸縮を試す)、[追跡の条件](docs/adr/0010-cooperating-host-composition-growth.md)、[初回選択](docs/adr/0026-initialize-explicit-selection-with-context.md)）。本文はホストで反映する診断経路で、一般アプリへの統合は未実装です。
外部編集や保護状態の通知では、次の打鍵を待たず所有ループで古い追跡を解除します。所有ホストのフォーカス喪失・非アクティブ化・無効化・モード取消も転送し、`input-notify-test`で30ケースを検証できます（[通知の条件](docs/adr/0011-owner-loop-input-invalidation.md)、[明示境界と制限](docs/adr/0020-owner-cancellation-and-shared-tsf-pump.md)、[ウィンドウ通知](docs/adr/0022-native-owner-window-boundaries.md)）。
`transaction-demo`のBackspaceは記録内の拡張書記素を同じ同期writeで削除し、左右移動後も削除前後に残った入力を同じcompositionで保持します。結合文字・複合絵文字とrepeatもまとめて扱い、全削除でcompositionを終了し、失敗時には再削除しません。`backspace-input-test`で55ケースを検証できます（[入力取引の条件](docs/adr/0013-synchronous-tracked-backspace.md)、[書記素の版と範囲](docs/adr/0023-grapheme-backspace-within-recorded-input.md)、[内部caretの削除](docs/adr/0028-tracked-internal-backspace.md)）。
専用editorの左右移動・Shift選択・ホストDeleteも書記素単位で操作できます。ホストのBackspace連打は選択範囲を広げず、一度の編集とUndoにまとめます。`grapheme-input-test`で22ケースを検証できます（[操作と検証範囲](docs/adr/0024-grapheme-navigation-in-owned-host.md)）。`transaction-demo`は記録範囲内の通常左右移動で追跡を保ち、その位置からの前方Deleteも実DLLで扱います。一般アプリのキー配送は引き続き未実装です。
Notepad等の実アプリでの表示と実打鍵は未確認です。
`recovery-demo`では固定の入力失敗を再現し、F8で未確認入力を確認できます。editorごとに1件・最大1024 UTF-16単位を最大10分だけ保持し、診断EXE全体では予約中も含め合計4件に制限します。明示コピー／破棄を提供し、自動再入力はしません。`recovery-input-test`で47ケースを検証できます（[起動と操作](docs/DEVELOPMENT.md#未確認入力の救済を試す)、[保護と保持の契約](docs/adr/0014-bounded-input-recovery.md)、[共有容量](docs/adr/0021-shared-input-recovery-budget.md)）。
`transaction-demo`では文字入力・記録内の移動/前方Delete・明示選択Deleteを実DLLへ接続します。途中への文字挿入も前後に残る入力を同じcompositionで保持します。Shift＋左右での選択も記録内なら追跡を維持し、通常の左右キーで選択を解除して入力を続けられます。記録内の選択置換・Delete・Backspace後も残った入力を保持します。専用の単一行入力欄のHome/End・Shift＋Home/Endも、実際の文書端が記録内にあれば追跡を継続します。F7は非空の明示選択があればその部分だけを変換します。試行後の失敗では自動再入力・再移動・再削除せず、確認を表示します。`transaction-input-test`で174ケースを検証できます（[起動と操作](docs/DEVELOPMENT.md#同期入力取引を試す)、[入力取引](docs/adr/0012-synchronous-input-transaction.md)、[選択Delete](docs/adr/0025-synchronous-selected-delete.md)、[記録内の移動とDelete](docs/adr/0027-tracked-caret-and-forward-delete.md)、[途中への挿入](docs/adr/0029-tracked-internal-insertion.md)、[Shift選択](docs/adr/0030-tracked-shift-selection.md)、[選択編集後の追跡](docs/adr/0031-recorded-selection-replacement.md)、[文書端の追跡](docs/adr/0032-recorded-document-edge-navigation.md)）。

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
- [Brokerの受取・破棄まで保持する容量予約](docs/adr/0015-broker-admission-through-delivery.md)
- [認証済み子Brokerを再利用する有限接続](docs/adr/0016-authenticated-reusable-broker-channel.md)
- [再利用Broker workerと所有STAへの結果配送](docs/adr/0017-reusable-worker-and-owner-delivery.md)
- [選択変換の保持rangeと送信前の失効検査](docs/adr/0018-retained-manual-range-and-source-validation.md)
- [Issue #1の受入記録](docs/ISSUE_1_ACCEPTANCE.md)
- [標準RichEditでの拒否検証と状態診断](docs/adr/0019-native-richedit-status-diagnostics.md)
- [所有ホストの明示取消と共通TSFメッセージ処理](docs/adr/0020-owner-cancellation-and-shared-tsf-pump.md)
- [所有者間で共有する救済枠](docs/adr/0021-shared-input-recovery-budget.md)
- [所有ウィンドウの無効化・非アクティブ化による失効](docs/adr/0022-native-owner-window-boundaries.md)
- [記録範囲内の拡張書記素Backspace](docs/adr/0023-grapheme-backspace-within-recorded-input.md)
- [所有ホストの書記素移動・選択・前方削除](docs/adr/0024-grapheme-navigation-in-owned-host.md)
- [明示選択Deleteの同期取引](docs/adr/0025-synchronous-selected-delete.md)
- [contextを指定した初回選択変換](docs/adr/0026-initialize-explicit-selection-with-context.md)
- [記録範囲内のcaret移動と前方Delete](docs/adr/0027-tracked-caret-and-forward-delete.md)
- [記録範囲内のcaretからの同期Backspace](docs/adr/0028-tracked-internal-backspace.md)
- [記録範囲内のcaretへの同期挿入](docs/adr/0029-tracked-internal-insertion.md)
- [記録範囲内のShift選択と選択解除](docs/adr/0030-tracked-shift-selection.md)
- [記録内の選択置換・削除後の追跡継続](docs/adr/0031-recorded-selection-replacement.md)
- [記録内にある文書端への移動・選択](docs/adr/0032-recorded-document-edge-navigation.md)
- [手動変換待機入力の有限Coreモデル](docs/adr/0033-bounded-core-waiting-input.md)
- [共有救済枠を先に確保する専用待機adapter](docs/adr/0034-shared-recovery-waiting-adapter.md)
- [待機入力デモと複数救済レコードの確認](docs/adr/0035-waiting-demo-and-record-recovery-ui.md)
- [待機・通常入力の救済UI統合とコピー期限](docs/adr/0036-unified-demo-recovery-and-copy-expiry.md)
- [待機入力の開始失敗と要求IDの取消](docs/adr/0037-cancel-failed-waiting-start.md)
- [待機デモの通常救済を定期確認する](docs/adr/0038-owner-wake-for-ordinary-recovery.md)
- [救済終了後の入力確認案内](docs/adr/0039-waiting-demo-recovery-loss-warning.md)
- [待機期限と未受入れ入力の通常経路](docs/adr/0040-waiting-deadline-and-unadmitted-fallback.md)
- [後続の実装順序](docs/NEXT_STEPS.md)

設計基準はSumibi-mac main `553c69021eda7a8a5944d8748357f3790cc2f45c`。
Mac版とコードを自動同期せず、このリポジトリで独立して実装・保守する案です。
採用判断と製品としての確定事項、実機で確認済みの事実は各資料で区別します。
