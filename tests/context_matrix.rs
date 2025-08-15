//! Cross-platform determinism tests for Week 2 gates
//! Validates identical results across Linux/macOS/Windows with mixed CRLF/LF

use std::collections::HashSet;

use anyhow::Result;
use roughup::core::budgeter::{
    BucketCaps, Budgeter, DedupeConfig, DedupeEngine, Item, Priority, SpanTag, TaggedItem,
    fit_with_buckets,
};

#[test]
fn test_line_ending_normalization() -> Result<()>
{
    // Test content with mixed line endings
    let content_lf = "fn test() {\n    println!(\"Hello\");\n}";
    let content_crlf = "fn test() {\r\n    println!(\"Hello\");\r\n}";
    let content_mixed = "fn test() {\r\n    println!(\"Hello\");\n}";

    let items = vec![
        Item {
            id: "lf_version".to_string(),
            content: content_lf.to_string(),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 0,
        },
        Item {
            id: "crlf_version".to_string(),
            content: content_crlf.to_string(),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 0,
        },
        Item {
            id: "mixed_version".to_string(),
            content: content_mixed.to_string(),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 0,
        },
    ];

    let config = DedupeConfig { jaccard_threshold: 0.8, ..Default::default() };

    let engine = DedupeEngine::with_config(config);
    let result = engine.dedupe_items(items);

    // Should normalize line endings and detect as duplicates
    assert!(
        result.len() < 3,
        "Mixed line endings should be normalized and deduplicated"
    );

    Ok(())
}

#[test]
fn test_deterministic_hash_seeding() -> Result<()>
{
    // Test that hash functions use deterministic seeds
    let content = "fn deterministic_test() { /* content */ }";

    let item = Item {
        id: "hash_test".to_string(),
        content: content.to_string(),
        priority: Priority::medium(),
        hard: false,
        min_tokens: 0,
    };

    let config = DedupeConfig { jaccard_threshold: 0.5, ..Default::default() };

    // Run multiple engines and verify consistent hashing
    let mut hashes = Vec::new();
    for _ in 0..5
    {
        let engine = DedupeEngine::with_config(config.clone());
        let hash = engine.compute_rolling_hash(&item.content);
        hashes.push(hash);
    }

    // All hashes should be identical (deterministic seeding)
    assert!(
        hashes
            .iter()
            .all(|&h| h == hashes[0]),
        "Hash function should be deterministic across instances"
    );

    Ok(())
}

#[test]
fn test_cross_platform_sorting() -> Result<()>
{
    // Test that sorting is consistent across platforms
    let mut items = Vec::new();

    // Create items with various priority combinations
    for i in 0..10
    {
        items.push(Item {
            id: format!("item_{:02}", i),
            content: format!("Content for item {}", i),
            priority: Priority::custom(
                ((i * 37) % 255) as u8, // Pseudo-random but deterministic
                (i as f32 * 0.1) % 1.0,
                (i as f32 * 0.07) % 1.0,
            ),
            hard: i % 3 == 0,
            min_tokens: (i as usize) * 5,
        });
    }

    let budgeter = Budgeter::new("gpt-4o")?;

    // Run fitting multiple times
    let mut results = Vec::new();
    for _ in 0..3
    {
        let fit_result = budgeter.fit(items.clone(), 200)?;
        results.push(fit_result);
    }

    // Verify consistent ordering
    for result in &results[1..]
    {
        assert_eq!(
            result
                .items
                .len(),
            results[0]
                .items
                .len()
        );
        for (a, b) in result
            .items
            .iter()
            .zip(
                results[0]
                    .items
                    .iter(),
            )
        {
            assert_eq!(a.id, b.id, "Item ordering should be deterministic");
        }
    }

    Ok(())
}

#[test]
fn test_bucket_determinism_cross_platform() -> Result<()>
{
    // Test bucket caps with deterministic results
    let budgeter = Budgeter::new("gpt-4o")?;

    let mut items = Vec::new();

    // Create tagged items with various content
    for i in 0..6
    {
        let mut item = TaggedItem {
            id: format!("cross_platform_{}", i),
            content: format!("Cross platform content item {}", i),
            priority: Priority::custom((i * 41) as u8, 0.5, 0.3),
            hard: false,
            min_tokens: 10,
            tags: HashSet::new(),
        };

        match i % 3
        {
            0 =>
            {
                item.tags
                    .insert(SpanTag::Code);
            }
            1 =>
            {
                item.tags
                    .insert(SpanTag::Interface);
            }
            2 =>
            {
                item.tags
                    .insert(SpanTag::Test);
            }
            _ => unreachable!(),
        }

        items.push(item);
    }

    let caps = BucketCaps { code: 50, interfaces: 30, tests: 20 };

    // Run multiple times to verify consistency
    let mut all_results = Vec::new();
    for _ in 0..3
    {
        let result = fit_with_buckets(&budgeter, items.clone(), caps.clone(), None)?;
        all_results.push(result);
    }

    // Verify deterministic results
    for result in &all_results[1..]
    {
        assert_eq!(
            result
                .fitted
                .items
                .len(),
            all_results[0]
                .fitted
                .items
                .len()
        );
        assert_eq!(
            result
                .fitted
                .total_tokens,
            all_results[0]
                .fitted
                .total_tokens
        );

        // Verify same items in same order
        for (a, b) in result
            .fitted
            .items
            .iter()
            .zip(
                all_results[0]
                    .fitted
                    .items
                    .iter(),
            )
        {
            assert_eq!(a.id, b.id);
            assert_eq!(a.tokens, b.tokens);
        }

        // Verify same refusals
        assert_eq!(
            result
                .refusals
                .len(),
            all_results[0]
                .refusals
                .len()
        );
    }

    Ok(())
}

#[test]
fn test_unicode_handling_consistency() -> Result<()>
{
    // Test Unicode normalization and handling
    let unicode_content = "fn тест() { println!(\"🦀 Rust 测试\"); }";

    let items = vec![Item {
        id: "unicode_test".to_string(),
        content: unicode_content.to_string(),
        priority: Priority::high(),
        hard: false,
        min_tokens: 0,
    }];

    let budgeter = Budgeter::new("gpt-4o")?;

    // Run multiple times to ensure consistent token counting
    let mut token_counts = Vec::new();
    for _ in 0..3
    {
        let result = budgeter.fit(items.clone(), 1000)?;
        if !result
            .items
            .is_empty()
        {
            token_counts.push(result.items[0].tokens);
        }
    }

    // Token counts should be consistent
    assert!(
        token_counts
            .iter()
            .all(|&count| count == token_counts[0]),
        "Unicode token counting should be deterministic"
    );

    Ok(())
}

#[test]
fn test_floating_point_precision() -> Result<()>
{
    // Test floating-point operations for cross-platform consistency
    let items = vec![Item {
        id: "float_test".to_string(),
        content: "floating point test content".to_string(),
        priority: Priority::custom(128, 0.123_456_79, 0.987_654_3),
        hard: false,
        min_tokens: 0,
    }];

    let config = DedupeConfig {
        jaccard_threshold: 0.7777777777777777, // High precision threshold
        ..Default::default()
    };

    // Verify consistent floating-point behavior
    let mut results = Vec::new();

    for _ in 0..3
    {
        let engine = DedupeEngine::with_config(config.clone());
        let result = engine.dedupe_items(items.clone());
        results.push(result);
    }

    // Results should be identical despite floating-point operations
    for result in &results[1..]
    {
        assert_eq!(result.len(), results[0].len());

        if !result.is_empty()
        {
            assert_eq!(result[0].id, results[0][0].id);
        }
    }

    Ok(())
}

#[test]
fn test_content_digest_stability() -> Result<()>
{
    // Test that content digests are stable across runs
    let test_content = r#"
pub struct TestStruct {
    field1: String,
    field2: i32,
}

impl TestStruct {
    pub fn new(field1: String, field2: i32) -> Self {
        Self { field1, field2 }
    }
}
"#;

    let item = Item {
        id: "digest_test".to_string(),
        content: test_content.to_string(),
        priority: Priority::medium(),
        hard: false,
        min_tokens: 0,
    };

    let engine = DedupeEngine::new();

    // Calculate digest multiple times
    let mut digests = Vec::new();
    for _ in 0..5
    {
        let digest = engine.compute_rolling_hash(&item.content);
        digests.push(digest);
    }

    // All digests should be identical
    assert!(
        digests
            .iter()
            .all(|&d| d == digests[0]),
        "Content digests must be stable across multiple calculations"
    );

    println!("Content digest (stable): {}", digests[0]);

    Ok(())
}
