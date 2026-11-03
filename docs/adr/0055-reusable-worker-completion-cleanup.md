# ADR 0055: 再利用Workerの後片付けと結果取消を分離する

2026-10-25 / Issue #4 / PR #10。

再利用`Worker`は成功済み`Completion`を所有者へ渡した後でも、
`shutdown()`またはfacadeの破棄で共有`cancelled`を立てていた。
そのため後片付けだけで期限内の結果が`Cancelled`になり、
単発Jobの[ADR 0054](0054-one-shot-completion-cancellation.md)と異なる契約だった。

`shutdown()`と`Drop`は閉鎖とStop通知を行い、未完了I/Oを止める。
既に所有者が受け取った成功結果の取消印は変更しない。
対象IDを指定した`cancel(id)`だけが取消印を立てる。
`Completion::outcome()`と`exchange()`は引き続き明示取消と期限切れを
配送時に検査する。結果は容量予約を保持し、最後の解放まで枠を返さない。

実子Brokerと認証済みpipeを使う`broker-worker-test`で、成功結果の取得後に
`shutdown()`とfacade破棄を行っても期限内の結果が読めること、
`shutdown()`後の明示`cancel(id)`では結果が読めなくなることを確認した。
既存の進行中取消、期限切れ、64件での退役、親handle回収も通過した。

この変更は完了済み結果の寿命だけを定める。所有TSFでの対象・保護・版の
再検査を省略するものではなく、一般アプリでの配送を有効化しない。
