# ADR 0074: ログオン単位のHost単一起動

## 範囲と用途

`host-window`の起動時に`HostInstance::try_acquire`を呼び、同じログオン内では
Hostのownerを1つに制限する。設定ファイルのパスは枠の識別子に使わないため、
別設定を指定しても同じログオンでは並行起動しない。別ログオンのHostは別枠である。

名前は`Local`名前空間と、既存のtoken検査で得た現在processのlogon SIDから生成する。
外部からmutex名・SIDを受け取らず、子processにhandleを継承させない。
作成時のDACLは当該logon SIDへSYNCHRONIZEとMUTEX_MODIFY_STATEだけを与える。
作成・openに失敗した場合は起動を拒否する。
[CreateMutexExW](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createmutexexw)。

固定名は悪意ある先行作成で起動を妨げられる可能性がある。この枠は排他だけに使い、
使用中でも正しいHostの存在を断定せず、接続・送信・既存画面への転送を許可しない。
Hostの署名、保護配置と実行中processへの束縛は引き続き別途必要である。
[CreateMutexExWの単一起動に関する注意](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createmutexexw#remarks)。

## 所有と終了

初期所有を指定せず作成・openし、0msのwaitで所有を取得する。
他thread/processが所有中ならNoneを返し、待ち続けたり別名で迂回したりしない。
Windows mutexは同じthreadで再帰取得できるため、process内にも1件のatomic枠を設ける。
作成・wait失敗とbusy時にはその枠を返す。
[mutexの所有規則](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createmutexexw#remarks)。

RAII leaseを`!Send`・`!Sync`とし、取得threadから移動・共有できないようにする。
Dropは同じthreadでReleaseMutexを行い、その後handleとprocess枠を解放する。
解放に予期しない失敗があればprocess枠は返さず、同じprocess内での後続取得を許可しない。
[ReleaseMutex](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-releasemutex)。

HostControlはowner thread内でleaseを取得し、その後で初めて設定とキーを読む。
他Hostが枠を持っていればAlreadyRunningとして画面へ返し、loaderを呼ばない。
leaseはロード中、同意操作中、取消とBroker終了の待機中も保持する。
runの完了または起動失敗で解放してから最終状態を公開する。

WAIT_ABANDONEDなら新しいownerとして所有するが、旧Hostの状態を復元しない。
設定を改めて検証し、既存のPreparedHostと同じく未同意から始める。
共有データの再利用や旧pipe/peerの継続は行わない。
[abandoned mutex](https://learn.microsoft.com/en-us/windows/win32/sync/using-mutex-objects)。

## 検証と残作業

製品名とは異なるランダムな試験用名を使い、実子processで重複拒否と正常解放を確認した。
親が追加handleを保持して子をDropなしで終了させ、実際のWAIT_ABANDONEDを経由する
回収を確認した。同名eventの型衝突を拒否し、その失敗後に枠を再取得できることも検査した。
同じthreadの再帰取得拒否と、Sendを要求するコードがcompileできないことを確認した。

Host寿命の試験は独立した排他枠を注入し、ロード前の取得、2つ目がloaderを呼ばないこと、
保留操作と終了の完了までの保持、起動失敗時の解放、未同意での再起動を検査した。
通常の試験実行では製品名のmutexを取得せず、利用者の実Hostを停止・排除しない。
別途、試験用設定のhost-windowを実画面で2つ起動し、2つ目の起動枠使用中表示と
許可ボタンの無効化、2つ目を閉じても最初のHostが継続することを確認した。
最初のHostを×で正常終了した後、同じ設定で再起動して未許可表示に戻ることも確認した。
この画面試験では製品名の枠を一時的に保持し、正常終了で解放した。
キー操作、送信許可、外部送信や機械登録は行っていない。
自動常駐、安全なHost/TIP発見、一般アプリ接続と正常TLSによる変換成功は残る。
