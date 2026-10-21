# Windows版 設計と実装の照合表

2026-10-21 / Windows基準 `91552b0` / [全体設計](TECHNICAL_OVERVIEW.md)。
本表は設計の合意や一般アプリでの対応保証を表さない。専用ホスト、未実装の製品機能、未決判断を区別する。

## 1. 設計・コード・受入条件

| 領域 | 現在のコード・状態 | 次の受入条件 |
| --- | --- | --- |
| OS非依存Core | core/lib.rsのManualConversion。単一要求の適用判定のみ | Tracking・待機・候補・手動／自動の状態と効果を追加 |
| COMとDLL | tip/com.rs、build.rs。factory・独立lease・所有thread・解除再試行 | 一般アプリにロードされる常設登録と更新時の寿命 |
| TSF編集 | tip/edit.rsとmanual_edit.rs。同期read/write、全文照合、Core gate、IPC結果の一度だけ適用。所有ACP context限定 | 製品キーとの統合、一般アプリの対象分類と編集 |
| 保護入力 | EditPolicy.allowedによる開発用Grant。読取専用拒否 | 実スコープ・機密欄・不明時の拒否を実アプリで確認 |
| 追跡composition | 未実装。専用ホストの本文はホスト自身が編集 | 通常入力・移動・他IMEとの競合・Enter・取消・原文維持 |
| 表示属性 | display.rs / preview.rs。点線、原文保持、属性だけ遅延解除 | 通常アプリでの下線、アクセシビリティ、既存属性との共存 |
| 変更通知 | notifications.rs。本文・選択・フォーカスで失効 | 製品追跡・IPC・候補との統合 |
| キー配送 | keys.rs / input_host.rs。専用windowで実TSF経路 | US/JIS配列・変換キー・Terminal・実打鍵 |
| Undo/Redo | TextStoreに本文・選択・向きの履歴。固定変換含む | 一般アプリの標準履歴で変換が1回のUndoにまとまる |
| 描画 | live_render / render / window_host。通知→WM_APP→Frame | 製品候補UIのUIElement、DPI、高コントラスト、読み上げ |
| 手動待機と救済 | 未実装。初期設計に容量・保持期間の案 | 消費前容量確保・順序・超過・対象変更・期限と明示破棄の整合 |
| Protocol / Broker | 有限フレーム・JSON・client状態、固定process相互検査・8 endpoint・overlapped pipe。単発Host・容量4 worker・各1結果。実DLLキー・Core・実TSFから診断用EXE bridgeを通す経路も検証 | 製品Broker発見・常駐dispatch、一般アプリ内の配置と入力統合。AppContainerは初期非対応 |
| HTTP | 未実装。固定文字列で検証 | 模擬HTTPで認証・同意・転送拒否・上限・取消・エラー分類 |
| 設定・資格情報 | 未実装。LocalAppData / Credential Manager案 | 形式版・原子的保存・秘密分離・認可取消・破損回復 |
| 追加候補・辞書 | 未実装。Mac移植範囲に目的を記録 | 件数決定・現在候補・出所・索引ハッシュ・LICENSE |
| Decisions | 未実装。Mac契約を参照した後続案 | API再確認、独立同意、固定窓、永続回数予約、手動優先 |
| 導入・更新・配布 | プロセス内登録のみ。spike DLLは配布不可 | 署名、登録範囲、Host起動、更新・削除・旧版復旧 |

## 2. Windowsで実行済みの検証

| 検証 | 基準コミットまでの結果 | 限界 |
| --- | --- | --- |
| cargo test --workspace | Core 7件＋COM 1件が成功 | 製品用InputSession全体のテストではない |
| Protocol単体 | フレーム層6件。分割・不正・EOF・上限 | JSON解釈とpeer認証を含まない。単体合計14件 |
| load_smoke | 実DLLのexports、factory、模擬manager、独立属性Info参照とDLL寿命 | 実TSF登録や入力先アプリではない |
| self-test | 各登録段階の意図的失敗・逆順解除・正常系 | プロセス内登録。インストーラーの試験ではない |
| edit-test | 編集23＋属性3＋Preview 8ケース成功 | 一部の編集基盤はEXEにRustライブラリとしてリンク |
| key-test | 実DLLから固定変換、取消・通知・別context拒否 | 画面なし通常キー配送は診断用コールバックで補う |
| window-test | 実TSF通常キー・focus通知、描画5状態と失敗後回復、破棄・再接続 | 一時的に専用ウィンドウをアクティブにする |
| input-test | 文字・絵文字・移動・選択・Delete・変換・Undo/Redo・終了解除 | 所有HWNDへのPostMessage。Shift選択は処理へ明示状態を渡す |
| delayed-test | 実DLLで正常部分置換・1回Undoと失効条件を11ケース検証 | 有限channelの合成応答。製品Core/IPC/HTTP統合ではない |
| ipc-test | ログオンSID限定DACL・remote拒否フラグ・先取り防止・OS peer PID・フレーム | 通常ユーザーの同一プロセスprobe。別プロセス認証やAppContainerは未検証 |
| fmt / Clippy | workspaceとTIPのspikeなし構成を確認。直近変更のworkspace再確認済み | 一般アプリでの成立を証明しない |

本書作成は文書作業であり、過去の実行記録を本日再実行したと読み替えない。
実機手順と個別の条件は[DEVELOPMENT](DEVELOPMENT.md)、[TSF_SPIKE](TSF_SPIKE.md)を参照。

Issue #4の追加分（2026-10-21）は[IPC_PROTOCOL](IPC_PROTOCOL.md)に記録する。
Protocol 22件のうち16件が追加のJSON・session・Core模擬接続であり、基準コミット当時の14件と混同しない。
製品TSF配送や別プロセス認証を新たに成功とする更新ではない。

さらにsumibi-ipcを追加した段階では、単体合計34件と、専用親子processの相互検査・停止等14確認が成功。
固定ハンドルの相手だけへ接続する検証で、製品Brokerの起動・発見・TSF編集の完成ではない。
詳細と別ユーザー・実PID再利用等の未実行範囲は[ADR 0003](adr/0003-pinned-desktop-peers.md)を参照。

次に単発Host・有限worker・Core→所有TSFの実配送を追加。workspaceは36件、broker-testは
専用ACP storeで19ケースと解除・DLLアンロードが成功した。元へ戻した変更による失効も検証した。
一般アプリ、製品キー、HTTPへの接続の成功ではない。[ADR 0004](adr/0004-one-shot-broker-and-owner-edit.md)を参照。

実DLLのキー・Core・TSF編集を通す段階ではbroker-key-testの17ケース、非同期デモと同じ
入力・終了のbroker-input-test 6ケースが成功した。COMの既存試験に独立control leaseと
所有threadの拒否を追加し、workspace合計は36件のまま。
専用ACP storeと診断用EXE bridgeの範囲であり、一般アプリや常駐Brokerは未接続。
詳細は[ADR 0005](adr/0005-dll-key-broker-bridge.md)を参照。

## 3. アプリ互換表の記録形式

| 入力先 | 現在の状態 | 最初の確認範囲 |
| --- | --- | --- |
| 専用sumibi-setup | 自動試験成功、物理キーボードは未確認 | 実打鍵・Shift・IME切替 |
| Notepad | 未検証 | 通常入力・選択・Enter/Esc・1回Undo・読取専用 |
| VS Code | 未検証 | editorと検索欄、複数文書、キー競合 |
| Chromium系 | 未検証 | input / textarea / contenteditable、長文、パスワード |
| Windows Terminal | 未検証 | Ctrl+J、行実行、折返し、TUI、ホスト側Undo |
| AppContainer / 昇格アプリ | 未検証・初期対応に含めない | IPC権限・UI・セッション分離の成立を先に調査 |

各結果にはコミット、OSビルド、アプリ版、CPU、権限、入力欄種別、キー配列、手動／自動を付ける。
ある欄の成功を同じアプリ全体へ一般化しない。Macの既知不具合はケースとして参照するが、Windowsの不具合と断定しない。

## 4. Mac資料との対応と未決事項

Mac参照はmain `553c69021eda7a8a5944d8748357f3790cc2f45c`。資料内のmain/PR/作業版の区別を維持する。

| Macの資料・論点 | Windows側の参照先・差 |
| --- | --- |
| TECHNICAL_OVERVIEW：全体図・送信境界・手動／自動 | [全体設計](TECHNICAL_OVERVIEW.md)。現在の検証と将来の製品構成を別図にする |
| INPUT_CONTROLLER_ARCHITECTURE：Runtime共有と寿命 | [所有資料](INPUT_CONTROLLER_ARCHITECTURE.md)。TSF thread/context単位。0.3秒判定を移植しない |
| SPEC：製品要件 | Windowsの確定仕様と同一ではない。未実装・未決を全体設計と本表で表示 |
| WINDOWS_HANDOFF：共通契約・Windows再設計 | [MAC_PARITY](MAC_PARITY.md)。固定コミットから独立実装 |
| 追加候補12件と追加10件要件の差 | Windowsも未決。辞書追加枠10件と混同しない |
| 待機中の制御キー・容量 | 遅延再送なし案。超過時の安全な切替と救済期限を決定するまで有効化しない |
| String.count・Unicode比較 | 書記素分割依存を未選定。照合はUTF-16完全一致で、正準等価を自動許可しない |
| AppKit / Keychain / 単一プロセス / 公証 | TSF UIElement / Credential Manager案 / 別Broker / Windows署名へ再設計 |

更新時は製品仕様の変更、コードの変更、実機の観測を別々に記録する。
要件を未実装コードに合わせて黙って縮めたり、設計案を実装済みへ変更したりしない。
