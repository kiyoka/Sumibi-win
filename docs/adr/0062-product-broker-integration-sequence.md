# ADR 0062: 製品Brokerの統合順序と完了境界

2026-10-26 / Issue #4 / PR #10。状態: 実装計画。以下の製品経路は未接続。

## 現在の境界

`sumibi-ipc`には固定process handleによる相互認証、有限な非同期pipe、
単発Job、再利用Channel、診断専用の2接続Brokerがある。
`sumibi-protocol`には接続ごとの`BrokerSession`と、最大8接続・各1通知の
`PolicyFanout`がある。`sumibi-host`には設定・資格情報・明示同意の
`PolicyAuthority`、要求の前後を検査する`manual::convert`、
Hostの版をBrokerの受付ゲートとFanoutへ渡す`BrokerPolicyRelay`がある。
実子Brokerの2接続検査は、同じ親processから開いた2本のpipeである。
異なるTIP processによる共有Hostの発見と接続、Hostの政策元から実pipeへの
配送、設定UIの完了通知はまだ製品で結ばれていない。

## 統合の責務

`sumibi-host`のEXEを製品Brokerの構成起点とする。Hostが一つの
`PolicyAuthority`と`BrokerPolicyRelay`、Broker受付ゲート、Fanoutを所有する。
`sumibi-ipc`は認証済みtransport、有限I/O、停止・期限・peer終了を扱い、
資格情報やTSF対象を所有しない。`sumibi-protocol`はOS参照を持たず、
接続と要求の順序を判定する。

Brokerは接続keyとpipeを対応付け、OS peerを検証してからHelloをFanoutへ渡す。
Readyの完全な書込後にだけ`ready_delivered`を呼ぶ。接続keyはBrokerの寿命中に
再利用しない。同じclient寿命IDの再接続は新しい世代と単調増加した要求番号を
要求する。接続数、待機要求、I/O、通知の各容量を受付前に確保する。
認証失敗、Ready書込失敗、不正frame、切断、通知書込失敗は対象接続だけを閉じる。
Brokerの政策元や状態機械そのものが失敗した場合は、本文受付を停止する。

要求受付、Fanoutの変更、結果の準備、政策の公開順序は一つの調停点で決める。
pipeの読書き中にFanoutを借用し続けない。各接続のI/O作業は有限な件数と期限で
処理し、完了を接続key・世代・政策版付きで調停点へ返す。遅い接続のI/Oが他の
接続の通知配送を無期限に止めない。通知frameは一度だけ取得し、書込が完全に
終わった版だけを`notice_delivered`へ渡す。失敗した接続は退役・closeし、
同じ通知を再送しない。結果の書込は`BrokerPolicyGate::authorize_for_publish`の
guardを完全な書込まで保持し、政策公開との順序を固定する。調停点のFanout
操作と結果側のguard取得でlock順序を逆にしない。

Hostの設定変更はRelayが最新版へまとめて公開する。公開後に退役keyのpipeを
閉じ、生存接続の通知書込または切断を記録する。設定UIに「変更完了」を返す
条件は、Hostの保存だけではなく、受付ゲートが該当版へ進み、変更時点の対象
接続がその版を受け取るか切断されたことである。途中で新しい版に置き換わった
場合も、古い版の通知が届いたと偽らず、最新版の配送・切断を完了条件にする。
新しい接続はその時点の最新版でReadyを受ける。

`BrokerPolicyRelay::publish_current`は公開直前の接続keyを有限な
`DeliveryReceipt`に保存する。Ready書込中の接続と、公開時に退役した接続も
対象に含める。通知frameの完全な書込をFanoutへ報告した後、またはOS pipeを
実際に閉じた後にだけreceiptを進める。後続の版の配送は先の版のreceiptも
満たせるが、単なるFanout退役やHost保存だけでは完了としない。設定UIから
receiptへの待機・通知経路は、製品HostのI/O調停点で接続する。

`manual::convert`の送信直前snapshotと実際の送信開始は現在は原子的でない。
実APIを有効にする前に、失効確定と未送信本文の送信開始を同じ順序で決める
境界、または失効時に送信を取消す境界を実装・検査する。
Brokerの結果だけではTSF編集を許可しない。所有STAに戻した後、context、
保持範囲、同長別本文、選択、保護条件、入力・設定・認可版を再検査して
一度だけ適用する。対象を失った要求は再送しない。

## 実装と検証の順序

1. Hostの調停点と接続key付きの有限I/O完了経路を作る。現行の2接続診断を
   使い、Ready失敗、通知失敗、遅い接続、取消、期限切れ、再接続が他接続を
   巻き込まないことを検査する。
2. 一つのHost内でPolicyAuthorityの変更をRelayから実pipeへ届ける。
   設定・認可版だけの変更、失効と再許可、結果公開との競合を含める。
   UI完了条件と切断後の再接続版を固定する。
3. 異なるTIP processから同じHostへ接続する発見・起動・相互認証を作る。
   パスやpipe名だけを信頼の根拠にせず、配置・署名・更新時の所有関係を
   [ADR 0045](0045-machine-registration-boundary.md)と合わせて検査する。
   AppContainerは別の成立確認まで拒否する。
4. 所有TSFの結果適用前再検査と製品キー配送を統合し、専用storeから
   一般アプリへ広げる。Notepad等で実打鍵、1回Undo、別欄・同長別本文・
   読取不能の非適用を確認する。

この順序では恒久的なTSF登録を先行させない。実APIと資格情報の永続保存も
Issue #4の診断成功から有効化しない。現時点の2接続検証の範囲は
[ADR 0061](0061-two-client-broker-pipe-diagnostic.md)に記録する。
