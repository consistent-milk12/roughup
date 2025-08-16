//! Week-2 A2: explicit char vs word behavior on subtle edits
use roughup::core::budgeter::{Budgeter, DedupeConfig, DedupeEngine, Item, NgramMode, Priority};

// Build quick items
fn it(
    id: &str,
    s: &str,
) -> Item
{
    Item {
        id: id.to_string(),
        content: s.to_string(),
        priority: Priority::medium(),
        hard: false,
        min_tokens: 0,
    }
}

#[test]
fn test_char_vs_word_similarity()
{
    // Budgeter for tie-break counts
    let budgeter = Budgeter::new("cl100k_base").unwrap();

    // Two strings differing mainly in spacing/punctuation/tokenization
    let a = "fn handle_error(code: i32) { log::error!(\"Error: {}\", code); }";
    let b = "fn handle_error(code:i32){log::error!(\"Error: {}\",code);}";

    let left = it("A", a);
    let right = it("B", b);

    // Word n-grams engine (defaults). Prefilter off for tiny inputs.
    let cfg_w = DedupeConfig {
        ngram_mode: NgramMode::Word,
        hash_window: 0,       // disable prefilter for small strings
        char_fallback: false, // ensure PURE word behavior
        ..DedupeConfig::default()
    };
    let eng_w = DedupeEngine::with_config(cfg_w);

    // Char n-grams engine with a slightly lower threshold.
    let cfg_c = DedupeConfig {
        ngram_mode: NgramMode::Char,
        jaccard_threshold: 0.5,
        hash_window: 0, // disable prefilter for small strings
        ..DedupeConfig::default()
    };
    let eng_c = DedupeEngine::with_config(cfg_c);

    // Word n-grams: NOT duplicates, kept should still be ["A"]
    let mut kept_w = vec![left.clone()];
    let dup_w = eng_w.is_duplicate_with_better_selection(&right, &mut kept_w, &budgeter);
    assert!(!dup_w, "word n-grams should not flag as duplicate here");
    assert_eq!(kept_w.len(), 1);
    assert_eq!(kept_w[0].id, "A");

    // Char n-grams: ARE duplicates; with fixed tie-break, "A" should remain
    let mut kept_c = vec![left.clone()];
    let dup_c = eng_c.is_duplicate_with_better_selection(&right, &mut kept_c, &budgeter);
    assert!(dup_c, "char n-grams should flag as duplicate");
    assert_eq!(kept_c.len(), 1);
    assert_eq!(kept_c[0].id, "A");
}
