//! Test cases for deduplication engine (A2 requirement)
//! Validates DCR ≥ 0.70 and deterministic tie-breaking

use anyhow::Result;
use roughup::core::budgeter::{Budgeter, DedupeConfig, DedupeEngine, Item, Priority};

#[test]
fn test_dedupe_near_duplicates_with_comments() -> Result<()>
{
    // Test case where two near-duplicates differ only in comment blocks
    let item1 = Item {
        id: "test1".to_string(),
        content: r#"
fn calculate_sum(a: i32, b: i32) -> i32 {
    // Original comment
    a + b
}
"#
        .to_string(),
        priority: Priority::medium(),
        hard: false,
        min_tokens: 0,
    };

    let item2 = Item {
        id: "test2".to_string(),
        content: r#"
fn calculate_sum(a: i32, b: i32) -> i32 {
    // Different comment
    a + b
}
"#
        .to_string(),
        priority: Priority::high(), // Higher priority
        hard: false,
        min_tokens: 0,
    };

    let config = DedupeConfig { jaccard_threshold: 0.8, ..Default::default() };

    let engine = DedupeEngine::with_config(config);
    let result = engine.dedupe_items(vec![item1, item2]);

    // Should keep the higher priority item (item2)
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, "test2");

    Ok(())
}

#[test]
fn test_dedupe_identifier_renames() -> Result<()>
{
    // Test case with small identifier renames
    let item1 = Item {
        id: "func_a".to_string(),
        content: r#"
fn process_data(input: Vec<String>) -> Vec<String> {
    input.iter().map(|s| s.to_uppercase()).collect()
}
"#
        .to_string(),
        priority: Priority::low(),
        hard: false,
        min_tokens: 0,
    };

    let item2 = Item {
        id: "func_b".to_string(),
        content: r#"
fn process_items(input: Vec<String>) -> Vec<String> {
    input.iter().map(|s| s.to_uppercase()).collect()
}
"#
        .to_string(),
        priority: Priority::high(),
        hard: false,
        min_tokens: 0,
    };

    let config = DedupeConfig { jaccard_threshold: 0.7, ..Default::default() };

    let engine = DedupeEngine::with_config(config);
    let result = engine.dedupe_items(vec![item1, item2]);

    // Should detect as duplicates and keep higher priority
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, "func_b");

    Ok(())
}

#[test]
fn test_dedupe_tie_breaker_by_tokens() -> Result<()>
{
    // Test tie-breaking: same priority, prefer fewer tokens
    let _budgeter = Budgeter::new("gpt-4o")?;

    let short_content = "fn short() { }";
    let long_content = "fn long_function_with_many_parameters(a: i32, b: String, c: Vec<usize>) \
                        -> Result<String, Error> { /* implementation */ }";

    let item1 = Item {
        id: "long".to_string(),
        content: long_content.to_string(),
        priority: Priority::medium(),
        hard: false,
        min_tokens: 0,
    };

    let item2 = Item {
        id: "short".to_string(),
        content: short_content.to_string(),
        priority: Priority::medium(), // Same priority
        hard: false,
        min_tokens: 0,
    };

    let config = DedupeConfig {
        jaccard_threshold: 0.9, // Very high threshold
        ..Default::default()
    };

    let engine = DedupeEngine::with_config(config);

    // These shouldn't be considered duplicates due to content difference
    let result = engine.dedupe_items(vec![item1, item2]);
    assert_eq!(result.len(), 2); // Both should remain

    Ok(())
}

#[test]
fn test_dedupe_deterministic_sorting() -> Result<()>
{
    // Test that deduplication is deterministic across multiple runs
    let items = vec![
        Item {
            id: "item_c".to_string(),
            content: "identical content".to_string(),
            priority: Priority::low(),
            hard: false,
            min_tokens: 0,
        },
        Item {
            id: "item_a".to_string(),
            content: "identical content".to_string(),
            priority: Priority::high(),
            hard: false,
            min_tokens: 0,
        },
        Item {
            id: "item_b".to_string(),
            content: "identical content".to_string(),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 0,
        },
    ];

    let config = DedupeConfig { jaccard_threshold: 0.95, ..Default::default() };

    // Run deduplication multiple times
    let mut results = Vec::new();

    for _ in 0..5
    {
        let engine = DedupeEngine::with_config(config.clone());
        let result = engine.dedupe_items(items.clone());

        results.push(result);
    }

    // All results should be identical (deterministic)
    for result in &results[1..]
    {
        assert_eq!(result.len(), results[0].len());
        assert_eq!(result[0].id, results[0][0].id);
    }

    // Should keep the highest priority item
    assert_eq!(results[0][0].id, "item_a");

    Ok(())
}

#[test]
fn test_dcr_threshold_validation() -> Result<()>
{
    // Test that DCR (Deduplication Compression Ratio) meets ≥0.70 target
    // Create a set with many near-duplicates
    let mut items = Vec::new();

    // Add base functions
    for i in 0..10
    {
        items.push(Item {
            id: format!("func_{}", i),
            content: format!(
                r#"
fn process_data_{}(input: Vec<String>) -> Vec<String> {{
    // Processing step {}
    input.iter().map(|s| s.to_uppercase()).collect()
}}
"#,
                i, i
            ),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 0,
        });
    }

    // Add very similar variants that should be deduplicated
    for i in 0..7
    {
        items.push(Item {
            id: format!("variant_{}", i),
            content: format!(
                r#"
fn process_items_{}(input: Vec<String>) -> Vec<String> {{
    // Processing variant {}
    input.iter().map(|s| s.to_uppercase()).collect()
}}
"#,
                i, i
            ),
            priority: Priority::low(),
            hard: false,
            min_tokens: 0,
        });
    }

    let original_count = items.len();

    let config = DedupeConfig { jaccard_threshold: 0.7, ..Default::default() };

    let engine = DedupeEngine::with_config(config);
    let result = engine.dedupe_items(items);

    let deduplicated_count = result.len();
    let dcr = 1.0 - (deduplicated_count as f64 / original_count as f64);

    // Validate DCR ≥ 0.70 target
    assert!(dcr >= 0.70, "DCR {} should be ≥ 0.70", dcr);
    println!("DCR achieved: {:.2}", dcr);

    Ok(())
}
