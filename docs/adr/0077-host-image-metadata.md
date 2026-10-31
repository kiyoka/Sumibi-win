# ADR 0077: 保持中HostのPEメタデータ検査

## 目的と入口

安全なHost起動に向け、保持した実行ファイルの形式と静的import名を、コードを
ロードせずに取得する。`host_image::inspect_bytes`は完全なbufferを、
`VerifiedFile::host_image`と`DevelopmentPlacement::host_image`は保持中の同じfile
handleを読む。後者はHost artifactだけを受け付ける。handleの読取位置を先頭へ
戻してから検査し、読取のための可変参照を要求してcursorの競合を防ぐ。

`SignedDevelopmentPlacement::probe_suspended_host`は子作成前にこの検査を行う。
一般の署名済みツールを使う`VerifiedFile::probe_suspended`の診断入口は従来どおりで、
Host専用profileを適用しない。署名検査と配置検査はそれぞれ別の条件として保持する。

## 受け付ける形式と上限

Windows x64のPE32+を対象に、実行可能属性がありDLL属性がないこと、GUIまたは
Console subsystemであることを確認する。entry pointは実行属性のあるsectionの
実ファイル内へ収まる必要がある。sectionの仮想範囲・raw範囲の重なり、整数overflow、
header/section tableやraw dataの範囲外を拒否する。

ファイル64MiB、PE header offset 1MiB、section 96件、optional header 4096byte、
image size 1GiB、data directory 16件という限定profileを使う。
import descriptorは終端を除き128件、DLL名はASCII 128byteまでとする。
終端descriptorと名前のNULが必要で、パス区切りやcolon、親移動を含む名前を拒否する。
名前は小文字へ正規化し、重複をまとめてソートする。descriptorの上限は重複分も数える。
delay import、CLR、bound import directoryは、いずれかのRVA/sizeが非ゼロなら未対応として拒否する。

PE構造とimport descriptorの意味はMicrosoftの
[PE Format](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)に従う。
上記の追加上限と形式制限は、本実装が扱うHostの範囲を狭めるための判断である。

## 検証

`tools/Test-HostImage.ps1`はparser 13件と、生成した`sumibi-host.exe`を読む1件を実行する。
DLLの公開表検査8件とKernel32照合1件も[ADR 0080](0080-dll-export-metadata.md)で追加した。
合成PEでConsole/GUI、名前の正規化、DLL・別architecture・delay/CLRの拒否、全prefixの
切詰め、overflow、重複section、実体のないRVA、パス名、終端欠落と件数超過を検査する。
2048件の決定的な変異入力でもpanicしないことを確認する。保持handleは読取位置を
変えた後も繰り返し同じ結果になることを検査する。

実ビルドには`kernel32.dll`と`KERNEL32.dll`の別descriptorがあり、これらを1件へ
まとめた静的import名17件を取得した。VCRUNTIMEとAPI setの名前も含まれる。
これは今回のdebug buildの観測であり、製品allowlistではない。

## 残る条件

このparserはWindows loader全体の妥当性検証ではなく、署名・起動許可を与えない。
import thunkの名前・序数・容量・終端検査は[ADR 0079](0079-static-import-symbols.md)で追加した。
exportを解決せず、API set、forwarder、動的LoadLibrary、TLS callback、
manifest、依存DLLの検索順と実ファイルの信頼も判定しない。
製品launcherには依存物・環境・更新の方針、保護配置の成功fixture、製品署名者方針、
実際にmapされた画像の同一性、信頼できるHost発見が引き続き必要である。
停止診断を再開するAPIや、一般アプリから本文を渡す入口は追加しない。
