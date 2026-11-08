//! Local dictionary completions (Nakaguro, Mozc symbols, Sudachi homophones).
//! Integrates with LLM additional candidates, preserving source provenance and deduplication.

pub use sumibi_core::candidate::{CandidateItem, CandidateSource};

/// Generate Nakaguro (・) delimiter variants from input or current text.
pub fn complement_nakaguro(source: &str, current_result: Option<&str>) -> Vec<String> {
    let mut results = Vec::new();
    if matches!(
        source.to_ascii_lowercase().as_str(),
        "ten" | "nakaguro" | "."
    ) {
        results.push("・".to_owned());
    }

    // Check delimiters in source: '/', '_', '・', or ASCII space
    let has_delim = source.contains('/') || source.contains('_') || source.contains(' ');
    if has_delim {
        let parts: Vec<&str> = source
            .split(['/', '_', ' '])
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() >= 2 {
            results.push(parts.join("・"));
        }
    }

    // Check delimiters in current result
    if let Some(current) = current_result {
        let has_curr_delim =
            current.contains('/') || current.contains('_') || current.contains(' ');
        if has_curr_delim {
            let parts: Vec<&str> = current
                .split(['/', '_', ' '])
                .filter(|s| !s.is_empty())
                .collect();
            if parts.len() >= 2 {
                let joined = parts.join("・");
                if !results.contains(&joined) {
                    results.push(joined);
                }
            }
        }
    }

    results
}

use std::{collections::BTreeMap, sync::LazyLock};

type Dictionary = BTreeMap<String, Vec<&'static str>>;

fn parse_dictionary(tsv: &'static str, comment: &str, min_candidates: usize) -> Dictionary {
    let mut entries = Dictionary::new();
    for line in tsv.lines().filter(|line| !line.starts_with(comment)) {
        let mut fields = line.split('\t');
        let Some(reading) = fields.next().filter(|reading| !reading.is_empty()) else {
            continue;
        };
        let candidates: Vec<_> = fields.filter(|value| !value.is_empty()).collect();
        if candidates.len() >= min_candidates {
            entries
                .entry(reading.to_ascii_lowercase())
                .or_default()
                .extend(candidates);
        }
    }
    entries
}

static MOZC_SYMBOLS: LazyLock<Dictionary> = LazyLock::new(|| {
    parse_dictionary(
        include_str!("../dictionaries/App/Resources/MozcSymbols.tsv"),
        "# ",
        1,
    )
});
static SUDACHI_HOMOPHONES: LazyLock<Dictionary> = LazyLock::new(|| {
    parse_dictionary(
        include_str!("../dictionaries/App/Resources/SudachiCandidates.tsv"),
        "#",
        2,
    )
});

/// Match Mozc readings using ASCII case folding only.
pub fn complement_mozc_symbols(source: &str) -> &[&'static str] {
    MOZC_SYMBOLS
        .get(&source.to_ascii_lowercase())
        .map_or(&[], Vec::as_slice)
}

/// Use the first whole hiragana reading returned by the model.
pub fn complement_sudachi_homophones(candidates: &[String]) -> &'static [&'static str] {
    let reading = candidates.iter().find(|value| {
        !value.is_empty() && value.chars().all(|ch| matches!(ch, 'ぁ'..='ゖ' | 'ー'))
    });
    reading
        .and_then(|value| SUDACHI_HOMOPHONES.get(value))
        .map_or(&[], Vec::as_slice)
}
/// Assemble a complete, deduplicated candidate list in strictly ordered priority:
/// 1. CurrentResult (index 0, if present)
/// 2. LLM candidates
/// 3. Nakaguro variants
/// 4. Mozc symbols
/// 5. Sudachi homophones
pub fn build_complete_candidates(
    source: &str,
    current_result: Option<&str>,
    llm_candidates: &[String],
    max_additional: usize,
) -> Vec<CandidateItem> {
    let max_total = if current_result.is_some() {
        max_additional + 1
    } else {
        max_additional
    };
    let mut items: Vec<CandidateItem> = Vec::with_capacity(max_total);

    // 1. Current result
    if let Some(current) = current_result {
        let trimmed = current.trim();
        if !trimmed.is_empty() && sumibi_protocol::message::valid_text(trimmed) {
            items.push(CandidateItem::new(trimmed, CandidateSource::CurrentResult));
        }
    }

    let original = source;
    let mut add_if_unique = |text: &str, source: CandidateSource, annotation: Option<&str>| {
        let trimmed = text.trim();
        if items.len() >= max_total {
            return true;
        }
        if trimmed.is_empty()
            || !sumibi_protocol::message::valid_text(trimmed)
            || (source != CandidateSource::Llm && trimmed == original)
            || items.iter().any(|item| item.text == trimmed)
        {
            return false;
        }
        let item = match annotation {
            Some(ann) => CandidateItem::with_annotation(trimmed, source, ann),
            None => CandidateItem::new(trimmed, source),
        };
        items.push(item);
        items.len() >= max_total
    };

    // 2. LLM candidates
    for cand in llm_candidates {
        if add_if_unique(cand, CandidateSource::Llm, None) {
            return items;
        }
    }

    // 3. Nakaguro variants
    let nakaguro_variants = complement_nakaguro(source, current_result);
    for cand in nakaguro_variants {
        if add_if_unique(&cand, CandidateSource::Nakaguro, Some("中黒")) {
            return items;
        }
    }

    // 4. Mozc symbols
    let mozc_symbols = complement_mozc_symbols(source);
    for cand in mozc_symbols {
        if add_if_unique(cand, CandidateSource::MozcSymbol, Some("記号")) {
            return items;
        }
    }

    // 5. Sudachi homophones
    let sudachi_homophones = complement_sudachi_homophones(llm_candidates);
    for cand in sudachi_homophones {
        if add_if_unique(cand, CandidateSource::SudachiHomophone, Some("辞書")) {
            return items;
        }
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nakaguro_variants() {
        let res = complement_nakaguro("leader/writer", None);
        assert_eq!(res, vec!["leader・writer"]);

        for trigger in ["TEN", "nakaguro", "."] {
            assert_eq!(complement_nakaguro(trigger, None), vec!["・"]);
        }
        assert!(complement_nakaguro("ten.", None).is_empty());

        let res2 = complement_nakaguro("foo bar", Some("フー バー"));
        assert_eq!(res2, vec!["foo・bar", "フー・バー"]);
    }

    #[test]
    fn test_mozc_symbols() {
        let arrows = complement_mozc_symbols("->");
        assert!(arrows.contains(&"→"));

        let ne = complement_mozc_symbols("!=");
        assert!(ne.contains(&"≠"));

        let hoshi = complement_mozc_symbols("hoshi");
        assert!(hoshi.contains(&"★"));
    }

    #[test]
    fn test_sudachi_homophones() {
        let homophones = complement_sudachi_homophones(&["こうしん".to_owned()]);
        assert!(homophones.contains(&"更新"));
        assert!(homophones.contains(&"行進"));
        assert!(complement_sudachi_homophones(&["koushin".to_owned()]).is_empty());
        assert!(complement_sudachi_homophones(&["文章です。".to_owned()]).is_empty());
        assert!(build_complete_candidates("ten", None, &[], 0).is_empty());
    }

    #[test]
    fn test_build_complete_candidates_ordering_and_deduplication() {
        let llm = vec![
            "更新".to_string(),
            "交信".to_string(),
            "こうしん".to_string(),
        ];
        let items = build_complete_candidates("koushin", Some("更新"), &llm, 10);

        // Index 0: CurrentResult ("更新")
        assert_eq!(items[0].text, "更新");
        assert_eq!(items[0].source, CandidateSource::CurrentResult);

        // Index 1: LLM ("交信") - "更新" in LLM was deduplicated against CurrentResult
        assert_eq!(items[1].text, "交信");
        assert_eq!(items[1].source, CandidateSource::Llm);

        // Sudachi homophones: "行進", "好転" added (excluding already present "更新", "交信")
        let texts: Vec<&str> = items.iter().map(|it| it.text.as_str()).collect();
        assert!(texts.contains(&"行進"));
        assert_eq!(texts.iter().filter(|&&value| value == "更新").count(), 1);
        assert!(items.len() <= 11);
    }
}
