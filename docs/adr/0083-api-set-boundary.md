# ADR 0083: API setのOS依存解決調査とフェイルセーフな未解決境界

## 目的と背景

ADR 0082において、Hostの静的依存17モジュールのうち非API setの10モジュール（137シンボル）は、
System32から排他読取で開いた実DLL lease（`SystemModuleLease` / `SystemModuleCatalog`）により
最終RVAまで追跡・解決された。
残る7モジュール（28シンボル）は以下のAPI setである：
1. `api-ms-win-core-synch-l1-2-0.dll`（3シンボル: `WaitOnAddress`, `WakeByAddressAll`, `WakeByAddressSingle`）
2. `api-ms-win-crt-*`（25シンボル: heap, locale, math, runtime, stdio, string）

実ファイル検査の観測では、前者は`kernelbase.dll`（およびフォワーダ経由の`ntdll.dll`）、
後者は`ucrtbase.dll`のエクスポートに一致するシンボルが存在することを確認した。
しかし、API setを物理ファイルと見なしたり、contract名から物理DLLを推測・固定バインドしてはならない。
本書では、一次資料の調査結果に基づき、API setの解決境界と安全な拒否方針を定義する。

## 一次資料の調査結果と技術的制約

1. **API Set Loader Operation（Microsoft公式仕様）**:
   API setは物理DLLファイル名ではなく、機能グループを抽象化する仮想コントラクト（契命名空間）である。
   Windowsライブラリローダーは、OSイメージ内に保持されるAPIセットスキーマを用いて、
   実行時にホストバイナリ（物理DLL）への名前空間リダイレクトを行う。

2. **PEB内の内部スキーマ構造（API_SET_NAMESPACE_ARRAY）**:
   Windows 10/11では、PEB（Process Environment Block）内のポインタ経由で
   スキーマv6（`API_SET_NAMESPACE_ARRAY`, `API_SET_NAMESPACE_ENTRY`, `API_SET_VALUE_ARRAY`）が
   メモリ上にマッピングされている。
   しかし、この構造体はWindows 8.1以降の公開SDKヘッダー（`apiset.h`等）から削除された非公開仕様であり、
   Win32/NTの公式な名前解決APIは提供されていない。

## 設計方針と境界決定

1. **推測・ハードコードの明示的禁止**:
   contract名（例: `api-ms-win-crt-*`）から特定の物理ファイル（例: `ucrtbase.dll`）を推測して
   catalogへエイリアス登録する処理は設けない。Windowsのエディション、デバイス種別、
   将来のOSアップデートによる分割・置換を安全に追跡できないためである。

2. **未公開PEB構造体の直接走査の禁止**:
   自プロセスのPEB内部オフセットを走査してスキーマテーブルを解釈する手法は、
   OSビルド依存の脆弱性や整合性境界の破壊を招くため採用しない。

3. **フェイルセーフな拒否（`ApiSetUnresolved`）の維持**:
   `ExportCatalog`、`SystemModuleLease`、`SystemModuleCatalog`において、
   `api-`および`ext-`で始まるモジュールに対する登録拒否と`ApiSetUnresolved`エラーの返却を厳格に維持する。

4. **製品起動・検証境界における役割分担**:
   Sumibi-winの静的PE検査ゲートでは「非API setの全依存関係が保護配置された実DLL lease内で完結していること」を証明する。
   API setの物理DLL解決は、OSローダー自身の責任境界として扱い、
   第2ゲートである「限定された子環境と画像読込ポリシー（`launch_environment.rs` / `launch_probe.rs`）を適用した停止起動診断」
   において、OSローダーが安全に解決・マッピングする挙動を検証する。

## 検証と残る条件

HostのAPI setインポート28シンボルの完全なインベントリを単体検査で検証した。
物理DLL（`kernelbase.dll`, `ucrtbase.dll`）への推測バインドは行わず、
catalogにおけるAPI set登録拒否と未解決エラーの決定的なテストを維持する。

製品Hostの安全な発見・起動経路、実際にmapされた画像セクションの同一性証明、
および製品BrokerとTIP間の認証された接続の具体化はIssue #4の残課題として継続する。
