# Mozc記号辞書の出典と再生成

- 元データ：[google/mozc](https://github.com/google/mozc)、固定コミット `0136c35782536cb43d710575c311cd4185982738`。
- 利用ファイル：`src/data/symbol/symbol.tsv`、`src/data/preedit/romanji-hiragana.tsv`。
- 適用条件：BSD 3-Clause。`LICENSE` は同じコミットの原文全体を無変更で同梱する。後半の一般語彙辞書向けの別条件は今回の利用ファイルの条件ではない。
- 出典・加工内容・著作権表示は `ATTRIBUTION.txt` にも記録する。生成索引をSumibiのMITコードへ再ライセンスしない。
- 生成処理はiOS版PR #134、参照コミット `5c545d378c8efd453b495489f448a2bef6a830fa` から移植したMITコード。

## 固定SHA-256

| ファイル | SHA-256 |
| --- | --- |
| symbol.tsv | `bc68b032a195f203dccba6f95667424199ba9b185fee2be64eeaa959253c097a` |
| romanji-hiragana.tsv | `fcb900e311d32ced05499c86e236d1084fb0c0937398deb6311fc6c7287a7821` |
| LICENSE | `44cdd923b91ea9199293abecc2762c70c87dbf1e581c027a94c416368d1a648c` |
| App/Resources/MozcSymbols.tsv | `43b5c2eac87e8e94fbfb1477084871b062cda9f81c724e6cee9461040a7c5449` |

## 再生成手順

上記コミットの2つの元ファイルを取得し、リポジトリのトップで実行する。
生成処理は入力ハッシュが一致しなければ停止する。処理自体は通信しない。

```sh
python3 Tools/generate_mozc_symbols.py \
  --symbols /path/to/symbol.tsv \
  --romaji /path/to/romanji-hiragana.tsv \
  --output /tmp/MozcSymbols.tsv
cmp App/Resources/MozcSymbols.tsv /tmp/MozcSymbols.tsv
python3 Tests/MozcSymbolGeneratorTests.py
```

読みとUnicode記号を抽出し、かなとローマ字の別名を追加する。拗音は最長のかな単位、
促音・撥音・長音・数字混じりを扱う。別名の組合せは各読み最大128件で、すべての
分割入力は網羅しない。キーをソートし、記号は元の行順を保って重複を除く。
互換正規化（NFKC）は行わず、①などを保持する。字形画像・フォントは取り込まない。
索引は18,581読み、406,740 bytes。iOS参照版とバイト一致する。

通常版・開発版の `Contents/Resources/Mozc/` にLICENSEとATTRIBUTION.txtを同梱する。
Sumibi設定の「記号データ（Mozc）のライセンス…」から全文を閲覧・コピーできる。
