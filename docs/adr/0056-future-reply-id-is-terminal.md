# ADR 0056: 未発行の要求番号を持つBroker応答は接続を閉じる

2026-10-25 / Issue #4 / PR #10。

`ClientSession`は対応しないresult/errorの要求番号をすべて無視していた。
取消済みや完了済みの古い番号には適切だが、まだ発行していない未来の番号まで
無視するとBrokerの順序違反を見逃し、保留中の要求を待ち続ける。

要求番号が現在の保留番号より小さい場合、または保留がなく最後に発行した番号以下の
場合は従来どおり`Ignored`とする。現在の保留番号より大きい場合、または保留がなく
最後に発行した番号より大きい場合は`UnexpectedMessage`で接続を閉じる。
同じ番号の版不一致は従来どおり`RevisionMismatch`で閉じる。

Protocolの単体検査で、保留中の未来result/error、要求発行前のresult、
取消後の古いresultと未来resultを確認した。session/generationの異なる応答を
無視する既存契約や、古い応答で新しい保留要求を消費しない契約は維持する。
ワイヤー版1とBroker送信形式は変更しない。

診断専用の`future-reply`子Brokerは、認証済みpipeで要求番号を一つ進めた
結果を返す。`broker-channel-test`は`InvalidMessage`、接続の退役、再送不可、
子process回収を確認した。既定Hostを別にビルドした
`broker-channel-denied-test`では診断モードの受付拒否を確認した。
