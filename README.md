# Sumibi for Windows

ローマ字／英語をLLMで日本語へ変換する、モードレスなWindows入力メソッド。
実装言語はRust、入力方式はTSFを採用します。
現在は設計段階で、動作するIMEやインストーラーはまだありません。

- [実装着手 Issue #1](https://github.com/kiyoka/Sumibi-win/issues/1)
- [言語・方式の選定案](docs/adr/0001-language-and-ime.md)
- [Windows版初期設計](docs/DESIGN.md)
- [Mac版からの移植範囲](docs/MAC_PARITY.md)

設計基準はSumibi-mac main `553c69021eda7a8a5944d8748357f3790cc2f45c`。
Mac版とコードを自動同期せず、このリポジトリで独立して実装・保守する案です。
採用判断と製品としての確定事項、実機で確認済みの事実は各資料で区別します。
