# ADR 0078: 停止起動診断の環境と画像読込制限

## 目的と範囲

停止起動診断で親の環境をそのまま渡すことを止め、DLLの読込条件を明示する。
`VerifiedFile::probe_suspended`と保護配置Host向けの入口が共通で使う作成処理に適用する。
これは停止状態の子を作って終了させる診断であり、製品Hostを実行するlauncherの完成ではない。

## 明示的な環境

親の`PATH`、`SystemRoot`、`WINDIR`を読まず、Windows APIが返したdirectoryから
次の4変数だけを組み立てる。

| 変数 | 値 |
| --- | --- |
| PATH | GetSystemDirectoryWの結果1件 |
| SystemDrive | システムWindows directoryのdrive |
| SystemRoot | GetSystemWindowsDirectoryWの結果 |
| WINDIR | SystemRootと同じ値 |

通常ユーザーごとのWindows directoryと区別するため、Windows rootには
[GetSystemWindowsDirectoryW](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getsystemwindowsdirectoryw)
を使う。ルート配置で返る`C:`のような値は末尾へbackslashを追加する。
DLL検索用の値は[GetSystemDirectoryW](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getsystemdirectoryw)
から取得する。取得失敗や上限超過時に親環境へ戻す処理は設けない。

ローカルdriveの絶対パスに限定し、制御文字、NUL、semicolon、余分なcolon、
区切りの空要素や末尾のdot/spaceを拒否する。UTF-16の名前順に固定した4変数を
各NULと追加の終端NULで連結し、全体を32767 UTF-16単位以下へ制限する。
CreateProcessWへ非NULLの環境を渡し、CREATE_UNICODE_ENVIRONMENTを指定する。
親の実キー・proxy設定・ユーザーPATH・`=C:`等のdrive別状態はコピーしない。
[CreateProcessWの環境形式](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw)。

この診断ではTEMP、TMP、USERPROFILEも渡さない。製品Hostの実行時に必要な設定・
保存先は、将来のlauncherで別途設計する。

## 子作成時の画像読込制限

PROC_THREAD_ATTRIBUTE_MITIGATION_POLICYで、以下のALWAYS_ONを指定する。

- IMAGE_LOAD_NO_REMOTE: リモート上の画像を拒否する。
- IMAGE_LOAD_NO_LOW_LABEL: Low mandatory labelの画像を拒否する。
- IMAGE_LOAD_PREFER_SYSTEM32: 標準の検索順でapplication directoryよりSystem32を優先する。

job listとmitigationを同じattribute listへ設定し、各値の参照とaligned storageを
DeleteProcThreadAttributeListまで保持する。非対応のbitを削って起動するfallbackはない。
[作成時のmitigation属性](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)。

子作成後は所有process handleへGetProcessMitigationPolicyを行い、3項目がすべて
有効であることを確認する。問合せ失敗や不足は診断失敗とし、既存の終了処理を通す。
終了確認に失敗すればfile・job・枠を保持する。画像パスを再検査するときも制限を読み戻す。
[画像読込policy](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-process_mitigation_image_load_policy)、
[GetProcessMitigationPolicy](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocessmitigationpolicy)。

## 検証と残作業

`tools/Test-SuspendedImageProbe.ps1`は既存4件と環境検査2件を実行する。
Unicodeを含む4変数の値と二重NUL、実OSからの環境生成、パス/環境注入の拒否、
個別・合計長上限を確認する。署名済みvswhereの子では、再開せずに作成時の画像読込policyを
OSから読み戻し、画像パス、書込拒否、終了・Drop、job終了と4件の容量制限も再検査した。
子の環境メモリーを読み出した試験や、DLLを実際にロードさせる試験は行っていない。

System32優先は読込先の限定ではなく、アプリdirectoryや動的依存の安全性を保証しない。
OSの画像読込制限は署名者・依存DLL実体・実際にmapされた画像の同一性も証明しない。
保護配置Hostの成功fixture、製品署名・依存物・更新方針、信頼できる発見、
実Hostの起動・一般アプリ接続は引き続き残る。
