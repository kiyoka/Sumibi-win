# IPCフレーム・型付きメッセージ・接続状態

2026-10-21 / [Issue #1](https://github.com/kiyoka/Sumibi-win/issues/1)。
Issue #1でOS非依存のフレーム化を実装し、Issue #4の最初の段階で版付きJSONと
client側の要求状態管理を追加した。`sumibi-ipc`では、起動ハンドルで固定した別プロセス間の
相互照合とoverlapped pipeを追加した（[ADR 0003](adr/0003-pinned-desktop-peers.md)）。
単発HostからCore→所有TSFへは専用ACP storeで接続した（[ADR 0004](adr/0004-one-shot-broker-and-owner-edit.md)）。
実DLLキーからも診断用EXE bridgeで単発Hostへ接続した（[ADR 0005](adr/0005-dll-key-broker-bridge.md)）。
製品Brokerの発見・常駐dispatch、一般入力への製品配送はまだ実装していない。
フレーム・JSON・Readyの成功だけで、本文の収集や変換を認可しない。

## ワイヤー形式

| 要素 | 形式 |
| --- | --- |
| 長さ | 4バイト、符号なし32bit、little endian。本文のUTF-8バイト数 |
| 本文 | 非空の厳密なUTF-8。最大262144バイト（256KiB） |
| フレーム全体 | 長さ4バイト＋本文。最大262148バイト |

Decoder単体はJSONであることまで検査しない。上位の`message::decode`が構造と値を検査する。
開発中のprotocolであり、公開済み製品との互換性を保証する段階ではない。
入力欄の1024 UTF-16単位や送信1000書記素の案とは別の上限である。

## デコーダーの契約

`Decoder::decode`は受信チャンクの先頭から最大1フレームまでを消費し、消費バイト数と
完成時の本文借用を返す。未完成なら本文はNone。受信データは分割されても結合されてもよい。
本文の途中でUTF-8文字が分割されても、フレーム完成後に検査するため破損扱いにしない。
完成した本文を処理して借用を終えてから、未消費部分を次のdecodeへ渡す。
空チャンクはEOFを意味せず、EOFは明示的に`finish`で通知する。

デコーダー内に完成フレームの待ち行列を作らず、本文バッファは最大1フレームに限定する。
ヘッダーを検査してから容量を予約し、分割受信によるバッファの倍増を避ける。
呼出し側の受信バッファ・接続数・処理中メッセージ数の上限は別途必要である。
このcrateだけでBroker全体のメモリー量やbackpressureを保証しない。

ゼロ長・過大長・不正UTF-8で接続状態を終端化する。途中EOFも終端化し、固定のTruncatedを返す。
不正フレームの後に正常フレームがあっても再同期しない。正常EOF後も同じDecoderを再利用しない。
再接続は新しいDecoder・ハンドシェイク・接続世代を必要とする。ClientSessionがこれを管理する。

本文を含むDecoderとDecodedにDebugを実装せず、エラーは本文を含まない固定enumとする。
エラー・EOFでは内部バッファを解放する。通常処理時も不要になった本文は保持対象から外すが、
アロケーターやOSメモリーからの完全消去を保証するものではない。呼出し側も本文をログへ出さない。

## フレーム検証

`cargo test -p sumibi-protocol --locked --offline`で次を検証する。

- 日本語・絵文字・結合文字を含むフレームの全分割位置と1バイトずつの受信。
- 複数フレームの結合、消費数、空チャンク、次の小さなフレームへの再利用。
- ゼロ長・上限値・上限超過・u32最大値。過大長を本文確保前に拒否。
- 不正UTF-8、過長表現、サロゲート、途中のマルチバイト文字。
- 全途中切断位置、正常EOF、不正後の後続フレーム非受理。
- 大きな不均等チャンクでも1フレーム分の容量を超えないこと。

## 版1のJSON契約

`message.rs`はSerde 1.0.229 / serde_json 1.0.154へ版を固定する。Cargo.lockで推移依存も固定する。
独自JSONパーサーを作らず、Serdeの型付きdeserializeと追加の値検査を使用する。
解析エラーの説明文・不明フィールド名・サーバー本文を外へ返さず、固定MessageErrorへ変換する。
本文や識別子を含む構造にはDebugを実装しない。codecを迂回してSerdeのエラーをログへ出さない。

すべてのメッセージは、以下の外包と1個だけの種別キーを持つ。

```json
{
  "version": 1,
  "session": "00000000000000000000000000000001",
  "generation": 1,
  "message": {
    "convert": {
      "request": 1,
      "revision": { "input": 1, "settings": 1, "authorization": 1 },
      "timeout_ms": 1000,
      "text": "nihongo"
    }
  }
}
```

| 項目 | 検査・意味 |
| --- | --- |
| version | 整数1だけ。互換性推測・自動降格をしない |
| session / broker | 非ゼロ128bit、32桁の小文字16進文字列。製品の生成・安全な起動時受渡しは後続 |
| session | client寿命の識別子。別clientで再利用しない。PID・COMアドレス・TSF文書IDを送らない |
| generation | 非ゼロu64。再接続ごとに増加。sessionと合わせて接続を識別 |
| request | 非ゼロu64。client寿命中単調増加し、再接続・Broker再起動でもリセットしない |
| revision | input / settings / authorization。非ゼロu64の厳密な整数。浮動小数・文字列・桁あふれを拒否 |
| text | 原文と単一結果とも非空、最大1024 UTF-16単位。埋め込みオブジェクト境界を表す`U+FFFC`は拒否。client.beginはコピー前にも検査。1000書記素という製品案とは別 |
| timeout_ms | 整数1〜60000。現段階のclient期限上限。HTTPの実装や製品期限の実証ではない |

必須フィールド欠損、不明フィールド、重複キー（入れ子も含む）、複数種別、未知種別、
不正なJSON／サロゲート、後続の別JSON、整数の型違いを拒否する。外側の256KiB上限は解析前に確認する。
送信側も同じ値検査を行い、文字列のエスケープ後のフレームサイズを再検査する。
型付きメッセージは単一結果だけを扱い、任意件数の候補配列を受け付けない。

| 種別 | payload |
| --- | --- |
| hello | 想定するBroker起動識別子broker |
| ready | brokerとpolicy（settings / authorization / allowed） |
| convert | request、revision、timeout_ms、text |
| cancel | request。同じsession・generationの要求に限定 |
| result | request、revision、text |
| error | request、revision、固定code（cancelled / timeout / unavailable / rejected） |
| policy_changed | settings / authorization / allowed |

Hello/Readyの識別子は照合値であり、秘密や相手プロセスの証明ではない。
製品transportはHelloを含む一切の送信より前にOS peerを相互検証する必要がある。
Ready.allowedも、利用者の送信同意・機密欄判定を代替しない。製品の本文収集はまだ有効化しない。

## ClientSessionの寿命・上限

`session.rs`はI/OもCOM参照も持たない。Connecting→Ready→Closedを扱い、
明示reconnectだけが世代を増やしてConnectingへ戻す。閉じた接続のDecoderを再利用しない。
同じBroker起動識別子では設定・認可版の下限を再接続後も保持する。
allowedの変更は認可版の増加を必須とし、版の巻き戻りは接続を閉じる。
設定だけの変更は設定版を増やし、認可版を維持できる。Broker側のpolicy gateも
この規則で更新し、古い設定版を持つ要求は受付前に拒否する。
実子Brokerの診断経路では、Result直後の設定版変更を認証済み接続で通知し、
旧結果を配送せず接続を閉じる。通常版Hostはこの診断モードを拒否する。
Broker側の`BrokerSession`も認証済み接続ごとにHello/Ready、要求番号、
一件だけの保留、Cancel、期限、policy通知、通常のResult/Errorを検査する。
単発Jobと再利用Channelの診断Hostがこの状態から送信frameを生成する。
OS peer認証とpolicy変更元はこの型の外側にあり、詳細は
[ADR 0057](adr/0057-broker-side-session-order.md)に記録する。
再利用Channelの通常結果は政策の読取guardをpipe書込完了まで保持し、
Broker内の更新との公開順序を固定する。Hostの通知カーソルは設定版だけの
進行も変更として返す（[ADR 0058](adr/0058-policy-publication-order.md)）。
複数接続向けの`PolicyFanout`は8接続・各1通知を上限とし、遅い接続だけを
退役させる（[ADR 0059](adr/0059-bounded-broker-policy-fanout.md)）。
診断用の単一Channelはこの状態を1接続として通す。複数clientの実I/Oと
製品の政策変更元はまだ接続していない。
別の、OS検証済みBroker起動へ接続する場合だけpolicy版の新しいepochを許す。
世代や要求番号が上限へ達したらラップせず拒否する。

接続ごとに未完了手動要求は最大1件、受信Decoderは最大1フレーム。
beginは値検査・frame生成後にだけ保留状態と番号を更新する。第2要求はBusyで既存要求を維持する。
送受信キューはこの型内に設けず、receiveは1イベントまでと消費バイト数を返す。
transportの総接続数・送信容量・受信容量の上限、消費前の予約は後続の実装条件である。
このclient型だけでBroker全体の容量やbackpressureが完成したとは扱わない。

取消は直ちにローカル要求を失効させ、cancelの送信成功を待たない。取消済み要求は復元しない。
送信キューへcancelを置けないときはtransportを閉じる。期限切れはexpireと受信の両方で確認し、
期限ちょうども非適用とする。呼出し側は最新の単調時刻を渡す。
古いsession・接続世代・要求番号、完了済み重複はIgnoredとなり、現在の要求を消費しない。
発行していない未来の要求番号に対するresult/errorは順序違反として接続を閉じる。
実装と検査の理由は[ADR 0056](adr/0056-future-reply-id-is-terminal.md)に記録する。
対応する要求の版不一致、受信方向違い、不正フレーム／JSON／版、途中EOFは終端化する。
有効な対応応答は一度だけ取り出すが、これだけでTSF編集を許可しない。

## Coreとの接続と検証範囲

`tests/core_session.rs`では容量1のchannel上で所有バイト列だけをfake peerへ渡す。
Hello/Ready、分割された要求／応答を通し、受け手の模擬編集スコープで新しいTargetを取得して
ManualConversion.finishを呼ぶ。対象・範囲・選択・同長別本文・入力／設定／認可版・許可の変化を拒否する。
cancel、EOF、通信エラー、期限切れ、policy変更では所有側がCore.cancelも呼ぶ契約を検証する。
送信キュー満杯も再送せず、接続とCore要求を失効させる。

Protocolは23テスト（既存frame 6、session 9、JSON 4、Core模擬接続4）。
偽の固定ID・合成本文だけを使い、解析エラーに元の本文を含めない。
これは製品Core→実pipe→実TSF編集の縦断試験ではなく、既存DLLやdemoにもまだ接続していない。
次段階のsumibi-ipcは、BCrypt乱数と起動ハンドルの明示継承、SID/PID/作成時刻の相互検査、
プロセス合計8 endpoint、overlapped I/Oと停止伝播を実装し、2プロセスの合成Core接続で検証した。
次段階では単発Hostの起動、認証済み接続の取得、容量4のworkerと各1結果、所有TSFへの配送を
専用ACP storeで接続し19ケースを確認した。既定Hostは変換を許可せず、固定結果はspike限定。
実DLLキー・Core・TSFからも診断用bridgeへ接続し19ケースと非同期入力・終了8ケースを確認した。
完了済みjob・受取済み結果の保持中も予約を維持し、worker・job・結果の最後の解放後に枠を返す
（[ADR 0015](adr/0015-broker-admission-through-delivery.md)）。満杯時は本文取得前にキーを通過させる。
同じ認証済み子processとpipeの再利用もworker専用Channelとして追加した。wire版1は維持し、
最大64要求・1要求/結果・共通4枠・有限期限を設ける。取消・通信失敗で永久に接続を閉じる
（[ADR 0016](adr/0016-authenticated-reusable-broker-channel.md)）。直接アダプターの単発検証も維持する。
再利用ChannelへのEXE worker dispatchと所有STA配送を接続した。既存デモは同じ認証済み子を再利用し、
取消・失敗後は次の明示操作で新しい接続を作る。取得前Permitは結果配送の最後まで保持する
（[ADR 0017](adr/0017-reusable-worker-and-owner-delivery.md)）。
Client側のpipe受信は1 readに複数frameが含まれても未消費部分を1 read分だけ保持し、
次のreadより先に処理する。`Ready`直後の失効通知を本文送信前に破棄できることを
[ADR 0046](adr/0046-inflight-policy-revocation.md)の実子Broker検査で確認した。
結果の直後に結合された失効通知も、結果を所有側へ返す前に確認して破棄する。
scope/status/compartmentの保護検査を追加し、実DLLで39ケースを確認した
（[ADR 0006](adr/0006-input-protection-metadata.md)）。
同一contextのcompositionを拒否し、実DLL/TSFで16ケースを確認した
（[ADR 0007](adr/0007-foreign-composition-boundary.md)）。空compositionの履歴取消には所有ホストの通知を使う。
選択プレビューの自己所有compositionを18ケースで検証した
（[ADR 0008](adr/0008-owned-selection-composition.md)）。所有objectや診断sinkをIPCへ渡さず、
Brokerの本文取得では引き続きcompositionのない入力先だけを扱う。
所有ホストの実入力範囲もCoreで記録し、tracking-demoから実DLLへ一度だけ選択を渡す経路を追加した
（[ADR 0009](adr/0009-cooperating-host-input-run.md)）。Startの直前に記録原文・範囲・保護状態・通知世代を
照合し、失敗した予約は再利用しない。追跡のCOM参照やclosureをworker/IPCへ渡さず、wire形式も変更しない。
さらに所有ホストの実入力を実DLLのcompositionへ接続し、伸縮・取消・変換を29ケースで確認した
（[ADR 0010](adr/0010-cooperating-host-composition-growth.md)）。F7前に自分のcompositionを終了し、
Start前の一度だけのCheckSourceで記録原文・範囲・保護状態・通知世代を確認する。wire形式は変えない。
外部通知からEXEのeventで所有ループを起こす仕組みも追加した
（[ADR 0011](adr/0011-owner-loop-input-invalidation.md)）。callbackは本文やworkerを持たず、
所有ループがmetadataだけを後始末する。status変更後の復元もManualEditのGuardで拒否する。
所有ホストでは本文・caret・compositionを同じ同期writeで更新するISpikeInputWriterも追加した
（[ADR 0012](adr/0012-synchronous-input-transaction.md)）。書込後の失敗では再入力せず変換を停止する。
追跡対象・未確認入力・dispositionをwireやworkerへ渡さず、既存のF7 source照合とidle gateを維持する。
記録末尾のBackspaceも同じwrite取引へ接続した（[ADR 0013](adr/0013-synchronous-tracked-backspace.md)）。
Coreの計画と実DLLの削除だけで処理し、削除本文・COM range・cookieをBrokerへ渡さずwire形式も変えない。
未確認入力の有限救済も所有editor内だけで完結させる（[ADR 0014](adr/0014-bounded-input-recovery.md)）。
保護履歴・期限を検査し、救済本文・操作ticket・コピー先をwire/workerへ渡さない。本文をBrokerから復旧しない。
製品の常駐Broker、前方Delete等のキー配送・救済・一般アプリでの判定はIssue #4で引き続き実装する。
Protocol自体がOS認証する責務には変更しない。
所有ホストのManualEdit.cancelはCore保留と原文bufferだけを退役し、worker/jobの通信取消は別に行う。
callbackで子process回収を待たず、共通の所有loopも保持したITfMessagePumpを通す。
wire変更や自動再送は追加しない（[ADR 0020](adr/0020-owner-cancellation-and-shared-tsf-pump.md)）。
ManualEditの選択はgeneric保持rangeの長さstampへ変更したが、これは所有STAとCoreの内部契約である。
native range・stamp・cookieをwireへ追加せず、JSON版1を維持する
（[ADR 0018](adr/0018-retained-manual-range-and-source-validation.md)）。

型付き変換は純粋なRustの模擬通信だけで検証するため、前面ウィンドウや実API・資格情報を必要としない。
既存のデスクトップ試験で前面化できない場合でも、その失敗をProtocol成功で置き換えない。

2026-10-21、`tools/Test-Spike.ps1 -SkipDesktop`が成功。workspace合計30テスト、
fmt、両構成Clippy、ビルド、load_smoke、self-test、edit-test、key-testを確認した。
この実行ではwindow/input/close/delayed/pipeのデスクトップ試験を省略している。
合成データのみの検証ログは `target/issue-4-protocol-verification.log`。

新規依存のmetadata上のlicenseはSerde/serde_json/itoa/synがMIT OR Apache-2.0、
memchrがUnlicense OR MIT、zmijがMIT。Cargo.lockの固定版の宣言MSRVは最大1.71で、
本リポジトリの実ビルドは固定済みRust 1.99.1を使用した。古いRustでの動作は今回検証していない。

根拠：[Serdeの型付き変換](https://docs.rs/serde/1.0.229/serde/)、
[serde_json](https://docs.rs/serde_json/1.0.154/serde_json/)。
