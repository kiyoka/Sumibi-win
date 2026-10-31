# ADR 0076: 署名検証済みファイルの停止起動診断

## 診断の範囲

`VerifiedFile::probe_suspended`は署名検証で保持したファイルを受け取り、
要求パスの現在のfile IDが一致することを調べてから停止状態の子を作る。
`SignedDevelopmentPlacement::probe_suspended_host`では、Host artifactに限定して
検査済みの階層・署名ファイルをまとめて保持する。
Host向け入口では[ADR 0077](0077-host-image-metadata.md)の限定PE profileも子作成前に検査する。
署名の失敗を許す別入口や、自己署名証明書を信頼する変更は追加しない。

CreateProcessの戻り値から得たprocess handleをPinnedProcessで固定し、生存とtokenを
照合する。そこから観測した画像パスの現在のfile IDを、保持ファイルと比較する。
これは名前の観測であり、実際にmapされた画像sectionのfile IDを取得した証明ではない。
このAPIは製品Hostの認証や実行許可に使わず、同意・本文・キーを渡す経路にもつながない。

## プロセス作成と保持

lpApplicationNameへ完全なローカルEXEパスを明示し、command lineはそのパスだけを
引用符で囲んだ書換可能なUTF-16 bufferとする。追加引数・shell・昇格fallbackはない。
親ディレクトリを明示し、handle継承を無効にする。相対・UNC・親移動・EXE以外・
NUL・引用符を含む入力や長さ超過は作成前に拒否する。
[CreateProcessW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw)。

CREATE_SUSPENDEDとCREATE_NO_WINDOWを指定し、ResumeThreadを呼ばない。
process/thread handle、PIDやPipe認証用のPinnedProcessを呼出元へ公開しない。
環境や依存DLLの安全性を確認してアプリを実行する製品launcherとは区別する。
[process creation flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags)。

名前のない、継承不可のjobを作り、KILL_ON_JOB_CLOSEを設定する。
PROC_THREAD_ATTRIBUTE_JOB_LISTを使い、子の作成と同時に所属させる。
後からAssignProcessToJobObjectするまでの隙間は作らず、jobを使用できない場合は
未所属の子を作るfallbackを行わない。Windows 10以降の対応機能を必要とする。
属性listのstorageとjob配列はDeleteProcThreadAttributeListまで保持する。
[UpdateProcThreadAttribute](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)、
[SetInformationJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-setinformationjobobject)、
[KILL_ON_JOB_CLOSE](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information)。

## 終了と容量

本診断の子保持枠は最大4件で、作成より先に確保する。
closeは自分が作った子のhandleだけをTerminateProcessし、最大5秒の期限内で終了を待つ。
終了済みなら再度の終了要求を必要としない。Dropは100msの後始末を試す。
TerminateProcessの復帰だけを、終了完了として扱わない。
[TerminateProcess](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess)。

終了確認に失敗した場合はprocess/thread/固定handleとjob、ファイル・配置、枠を
診断元processの終了まで保持する。終了したと偽って枠を再利用しないための、有限の
保守的保持である。診断元processが終了するとjob handleも閉じ、子の終了を要求する。
明示closeの失敗は成功表示にしない。停止中の子を発見名や裸のPIDから終了する処理はない。

## 検証と残作業

署名済みvswhereの一時コピーを、Unicode・空白を含むパスで作成した。
Windowsの署名検証後に保持し、実子の観測パス照合、保持中の書込・削除拒否、
終了後の書込再許可、終了後のprocess照会拒否、Dropによる終了を検査した。
別ファイルIDのコピー拒否、同時4件と超過拒否、終了後の枠回復も確認した。
job handleだけを閉じても子が終了し、close完了まではファイルが保持されることを確認した。

`tools/Test-SuspendedImageProbe.ps1`は4検査を明示実行し、fixtureや通常desktop tokenが
ない場合のスキップを成功として扱わない。テストが取得するvswhereの署名者指紋は
fixtureの期待値であり、製品の署名者allowlistを構成する方法ではない。
保護配置Hostでの成功検査、製品署名方針、画像section・依存物・更新モデルの検証、
安全なHost発見と一般アプリ接続は引き続き残る。
