# ADR 0066: 登録操作の先行記録と中断後の復旧

2026-10-28 / Issue #4 / draft PR #10。状態: 取引エンジンと専用 HKCU fixture を実装・検証。製品 COM/TSF backend は未接続。
実APIを隔離レジストリへ接続した後続試験は [ADR 0067](0067-isolated-native-registration-backend.md) に記録する。

## 状況

[ADR 0045](0045-machine-registration-boundary.md) の保護配置は確認できた。
ただし「API が成功した項目をメモリー上の配列に入れ、失敗時に逆順解除」だけでは、
書込み後の API エラーやプロセス終了で所有情報を失う。削除後、完了記録前に終了することもある。
パッシブ DLL の機械登録へ進む前に、この取引と復旧の境界を実装する。

## 決定

`sumibi-setup` のライブラリに `install_transaction` と `install_journal` を追加する。
このADRで最初に接続した Windows backend は専用 HKCU 模擬fixtureである。後続の実API隔離backendもHKLM/恒久 TSF writerを公開しない。
製品用 API の振舞い・署名方針・保護 journal 配置が未検証のため、fixture の成功を機械登録の成功とは扱わない。

固定順序は COM server → 言語 profile → keyboard category → display attribute category。
`RegisterProfile` は service と profile を登録するので、別の広い service 登録は作らない。
実 backend の契約は profile を既定無効とし、ユーザー有効化・既定 IME 変更・証明書信頼追加を行わない。
`UnregisterProfile` の `TF_URP_ALLPROFILES`、再帰レジストリ削除、広い service 解除は使わない。

### 状態と所有証拠

| 状態 | 動作 |
| --- | --- |
| Installing | 作成 intent を同期保存してから各 API を呼ぶ。復旧は作成意図がある prefix だけを逆順解除 |
| Installed | 全項目が完全な所有状態であることを確認して commit。通常の recover は検証して保持 |
| Removing | 逆順解除。各削除後に残件 cursor を同期保存。cursor 更新前に終了しても既にない項目は再削除しない |
| Removed | 全対象の不在を確認して commit。同じ recover/uninstall は追加変更なし |

記録するのは対象 backend/政策版の識別子、取引 ID、DLL path、SHA-256、署名 thumbprint、形式版、段階と cursor。
Backend は復旧を含む各操作で識別子の一致を検査し、fixture の記録を実登録へ再利用しない。
本文・資格情報・秘密鍵は含めない。記録された path はメタデータであり、復旧時に DLL をロードしない。
記録から任意のレジストリ path・コマンド・TSF flags は受け付けない。

Backend は各項目を不在、完全な所有状態、所有は証明できるが未完成、衝突に分類する。
DLL path/GUID の一致だけを所有証拠にしない。未完成は解除だけを許し、登録完了の判定には使わない。
未知の値・子キー・他取引の所有情報・意図のない将来段階を見つけた場合、全体の変更を止める。
途中作成分が残っても、所有が曖昧な項目を自動削除しない。エラーには元の失敗と復旧失敗を両方残す。
毎段階の全体確認に加え、削除直前にも backend が対象の所有を再確認する。

実 backend は排他的作成、部分書込みの識別、登録と解除の永続化を保証する責任を持つ。
TSF API は複数キーを更新し得るため、fixture のレイアウトを TSF レジストリへコピーしてはいけない。
実 profile/category とその付随状態について API・列挙・所有 receipt の組合せで証拠を確定する。

### Journal

Windows の `FileJournal` は `create_new` と共有なしの handle を使う。
同じ file を他プロセスから開けない状態を全取引中保持し、終了後は OS が handle を解放する。
本番では同じ CLSID に対して固定の保護 journal/lock を使う。任意の別 journal path で
排他を回避できる CLI を作らない。親の ACL/所有者・再解析点・置換経路の検証は呼出側の責任とする。
file 自体の再解析点は拒否する。

各 frame は固定 magic、payload 長、CRC-32、厳密な JSON snapshot。
40 KiB/frame、16 frame、最大 655,616 bytes とし、未知 field、形式版、取引 ID/DLL 識別の変更、
段階飛ばし、逆戻り、checksum 不一致、完全な不正末尾は変更せず拒否する。
CRC は破損検出であり認証ではない。管理者専用 ACL と信頼できる journal path を省略できない。

append は `write_all` → `sync_all` の完了後だけメモリー上の確定点を進める。
書込み結果が不明なエラーでは session を無効にし、追加の登録・解除・記録を止める。
新しく開き直して完全な履歴を検証するまで復旧しない。最後の Installed 記録が永続化していた場合も、
API の返値を推測せず履歴から保持すべきかを判断する。

最後の不完全 frame は、以前の完全な履歴を検証してから切り戻す。
intent が同期保存される前には対象 API を呼ばないため、不完全 intent の項目を推測して削除する必要はない。
削除後の cursor が不完全なら、以前の intent と実状態の不在から復旧できる。
電源断やストレージが同期保証を破るケース、保護 journal 自体の紛失/改変は今回の実証範囲外とする。

### Windows fixture の境界

`registration-transaction-test` は子プロセスを使い、
`HKCU\Software\Sumibi\RegistrationTransactionFixture\<新しい取引 ID>` だけを操作する。
COM/CTF の名前空間、HKLM、証明書ストア、ユーザー IME 設定、DLL ロードには触れない。
fixture の profile/category は名前付きの模擬項目であり、実 TSF API は呼ばない。

専用 root の取引 marker、既知の子名、値の個数・型・内容、未知の子キーを照合する。
排他的に作った root と journal intent を根拠に、その配下の空/部分 key を解除できる。
この所有証拠は fixture 専用であり、共有される製品 COM/TSF の所有証拠へ流用しない。
通常ユーザーの HKCU fixture は同一ユーザーによる悪意のある並行書込みに対する防御を証明しない。
製品では保護された登録領域と journal、管理者による他 installer との競合方針が必要。

各ケースの journal は `target/debug/registration-transaction-fixture/<ID>/journal.bin` に残す。
成功後はその取引の既知の registry 子だけを解除し、空かつ所有が一致する root を削除する。
異常時は root と journal を診断のため保存する。実データを含む入力欄の操作は不要。

## 検証

単体では作成/削除 API の副作用前後、全記録段階の保存前/後エラー、未知の項目、
未完成項目、意図のない項目、別の取引への再利用拒否を検査する。
Journal は標準 CRC vector、payload の各 byte 破損、末尾 frame の全切断位置、
不正履歴、未知 field、上限、Windows の排他 open と再読込みを検査する。

Windows 子プロセス検査は、通常登録・復旧時保持・解除、4段階それぞれの作成直前/空キー後/
部分書込み後/完成後終了、API 失敗の自動巻戻し、削除前/後終了に加え、
各項目への未知 value/child・別 owner・有効化 flag 変更での全解除停止を確認する。
これはプロセス終了後の永続状態の再読込みを確認するもので、実 TSF API の取引保証ではない。

既存の開発用 `registration.rs` も、class/server の正確な値数・子数、値型と UTF-16、
path を含む v2 所有 marker を検査するよう強化した。未知の状態は通常解除と新規失敗時の
best-effort rollback の両方で保持する。実現在ユーザー COM の9ケースで拒否/不変と正常解除を確認した。
この旧開発経路は本 ADR の取引 journal へは接続しておらず、機械登録の耐中断保証とは区別する。

参考: Microsoft [RegisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile)、
[UnregisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-unregisterprofile)、
[UnregisterCategory](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcategorymgr-unregistercategory)、
[RegDeleteKeyExW](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regdeletekeyexw)。
