# SudachiDict Core-derived candidate index

`App/Resources/SudachiCandidates.tsv` is derived from SudachiDict Core
version **20260723** (the `small_lex.csv` and `core_lex.csv` sources). It is not
covered by Sumibi's MIT license. SudachiDict is distributed under Apache License
2.0; retain `LICENSE-2.0.txt` and `LEGAL` with the application bundle.

Upstream: https://github.com/WorksApplications/SudachiDict/tree/v20260723

Sources (official SudachiDict distribution):

- https://d2ej7fkh96fzlu.cloudfront.net/sudachidict-raw/20260723/small_lex.zip
  SHA-256: `b578ac9545899783d5d7e30d5d78d5d9dcf40b36965d4af0663ec2eb041c1093`
- https://d2ej7fkh96fzlu.cloudfront.net/sudachidict-raw/20260723/core_lex.zip
  SHA-256: `a2b39e1572adab08a649b1390b134517adc55f1d733c59358b113298788bf31c`

Regenerate with:

```sh
python3 Scripts/generate_sudachi_candidates.py \
  --small /path/to/small_lex.zip \
  --core /path/to/core_lex.zip \
  --output App/Resources/SudachiCandidates.tsv
```

The generator keeps common nouns with a kanji-containing surface and a
katakana reading, groups distinct surfaces under their hiragana reading,
removes readings with fewer than two surfaces, and retains up to 32 surfaces
per reading by Sudachi word cost. This is a candidate index, not a definition
or a semantic-equivalence dictionary. It does not include entries from the
GPL-licensed SKK-JISYO.L or the Emacs Sumibi generated dictionary.

When an additional-candidates response contains a whole-word hiragana
candidate, Sumibi appends up to 10 distinct dictionary surfaces after the LLM
candidate list. The dictionary is never sent to the LLM.
