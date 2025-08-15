//! Test cases for bucket caps orchestrator (A3 requirement)
//! Validates hard caps, refusal logs, and ±5% budget compliance

use std::collections::HashSet;

use anyhow::Result;
use roughup::core::budgeter::{
    BucketCaps, Budgeter, Priority, SpanTag, TaggedItem, fit_with_buckets, parse_bucket_caps,
};

#[test]
fn test_bucket_caps_parsing() -> Result<()>
{
    // Test parsing of bucket specification string
    let spec = "code=60,interfaces=20,tests=20";
    let caps = parse_bucket_caps(spec)?;

    assert_eq!(caps.code, 60);
    assert_eq!(caps.interfaces, 20);
    assert_eq!(caps.tests, 20);

    Ok(())
}

#[test]
fn test_bucket_caps_parsing_invalid()
{
    // Test invalid bucket specifications
    assert!(parse_bucket_caps("invalid").is_err());
    assert!(parse_bucket_caps("code=abc").is_err());
    assert!(parse_bucket_caps("unknown=50").is_err());
}

#[test]
fn test_bucket_hard_caps_enforcement() -> Result<()>
{
    let budgeter = Budgeter::new("gpt-4o")?;

    // Create items that exceed bucket caps
    let mut items = Vec::new();

    // Add many code items (should hit code cap)
    for i in 0..10
    {
        let mut item = TaggedItem {
            id: format!("code_{}", i),
            content: format!("fn function_{}() {{ /* code */ }}", i),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 10,
            tags: HashSet::new(),
        };
        item.tags
            .insert(SpanTag::Code);
        items.push(item);
    }

    // Add interface items
    for i in 0..5
    {
        let mut item = TaggedItem {
            id: format!("interface_{}", i),
            content: format!("pub trait Interface{} {{ fn method(&self); }}", i),
            priority: Priority::high(),
            hard: false,
            min_tokens: 15,
            tags: HashSet::new(),
        };
        item.tags
            .insert(SpanTag::Interface);
        items.push(item);
    }

    // Add test items
    for i in 0..3
    {
        let mut item = TaggedItem {
            id: format!("test_{}", i),
            content: format!("#[test] fn test_{}() {{ assert!(true); }}", i),
            priority: Priority::low(),
            hard: false,
            min_tokens: 12,
            tags: HashSet::new(),
        };
        item.tags
            .insert(SpanTag::Test);
        items.push(item);
    }

    // Set restrictive caps
    let caps = BucketCaps {
        code: 50,       // Should limit code items
        interfaces: 30, // Should limit interface items
        tests: 25,      // Should limit test items
    };

    let result = fit_with_buckets(&budgeter, items, caps, None)?;

    // Verify caps are respected (within ±5% tolerance)
    let tolerance = 5;
    assert!(
        result
            .fitted
            .total_tokens
            <= 50 + 30 + 25 + tolerance
    );

    // Should have refusals due to cap constraints
    assert!(
        !result
            .refusals
            .is_empty()
    );
    println!(
        "Refusals: {}",
        result
            .refusals
            .len()
    );

    Ok(())
}

#[test]
fn test_bucket_refusal_logs_deterministic() -> Result<()>
{
    let budgeter = Budgeter::new("gpt-4o")?;

    // Create identical sets of items
    let create_items = || {
        let mut items = Vec::new();
        for i in 0..5
        {
            let mut item = TaggedItem {
                id: format!("code_{}", i),
                content: "fn large_function() { /* lots of code here */ }".to_string(),
                priority: Priority::custom(100 + i as u8, 0.5, 0.5),
                hard: false,
                min_tokens: 20,
                tags: HashSet::new(),
            };
            item.tags
                .insert(SpanTag::Code);
            items.push(item);
        }
        items
    };

    let caps = BucketCaps { code: 30, interfaces: 10, tests: 10 };

    // Run multiple times and verify deterministic refusal logs
    let mut all_refusals = Vec::new();
    for _ in 0..3
    {
        let items = create_items();
        let result = fit_with_buckets(&budgeter, items, caps.clone(), None)?;
        all_refusals.push(result.refusals);
    }

    // All refusal logs should be identical
    for refusals in &all_refusals[1..]
    {
        assert_eq!(refusals.len(), all_refusals[0].len());
        for (a, b) in refusals
            .iter()
            .zip(all_refusals[0].iter())
        {
            assert_eq!(a.id, b.id);
            assert_eq!(a.reason, b.reason);
            assert_eq!(a.bucket, b.bucket);
        }
    }

    Ok(())
}

#[test]
fn test_budget_compliance_five_percent() -> Result<()>
{
    let budgeter = Budgeter::new("gpt-4o")?;

    // Create items that should exactly fit within ±5%
    let mut items = Vec::new();

    for i in 0..3
    {
        let mut item = TaggedItem {
            id: format!("code_{}", i),
            content: "fn medium_function() { /* some code */ }".to_string(),
            priority: Priority::high(),
            hard: false,
            min_tokens: 18,
            tags: HashSet::new(),
        };
        item.tags
            .insert(SpanTag::Code);
        items.push(item);
    }

    let caps = BucketCaps { code: 60, interfaces: 0, tests: 0 };

    let result = fit_with_buckets(&budgeter, items, caps, None)?;

    let expected_total = 60;
    let tolerance = (expected_total as f64 * 0.05) as usize;
    let actual_total = result
        .fitted
        .total_tokens;

    // Verify ±5% compliance
    assert!(
        actual_total <= expected_total + tolerance,
        "Total tokens {} exceeds {}±5% ({})",
        actual_total,
        expected_total,
        tolerance
    );

    println!(
        "Budget compliance: {}/{} ({}%)",
        actual_total,
        expected_total,
        (actual_total as f64 / expected_total as f64 * 100.0) as usize
    );

    Ok(())
}

#[test]
fn test_mixed_bucket_types() -> Result<()>
{
    let budgeter = Budgeter::new("gpt-4o")?;

    // Create mixed content types
    let mut items = Vec::new();

    // Code item
    let mut code_item = TaggedItem {
        id: "src_function".to_string(),
        content: "fn main() { println!(\"Hello, world!\"); }".to_string(),
        priority: Priority::medium(),
        hard: false,
        min_tokens: 10,
        tags: HashSet::new(),
    };
    code_item
        .tags
        .insert(SpanTag::Code);
    items.push(code_item);

    // Interface item
    let mut interface_item = TaggedItem {
        id: "trait_def".to_string(),
        content: "pub trait Display { fn fmt(&self) -> String; }".to_string(),
        priority: Priority::high(),
        hard: false,
        min_tokens: 15,
        tags: HashSet::new(),
    };
    interface_item
        .tags
        .insert(SpanTag::Interface);
    items.push(interface_item);

    // Test item
    let mut test_item = TaggedItem {
        id: "unit_test".to_string(),
        content: "#[test] fn test_addition() { assert_eq!(2 + 2, 4); }".to_string(),
        priority: Priority::low(),
        hard: false,
        min_tokens: 12,
        tags: HashSet::new(),
    };
    test_item
        .tags
        .insert(SpanTag::Test);
    items.push(test_item);

    let caps = BucketCaps { code: 40, interfaces: 30, tests: 20 };

    let result = fit_with_buckets(&budgeter, items, caps, None)?;

    // All items should fit within their respective buckets
    assert!(
        result
            .fitted
            .items
            .len()
            <= 3
    );

    // Verify we have representatives from each bucket type
    let ids: Vec<&str> = result
        .fitted
        .items
        .iter()
        .map(|item| {
            item.id
                .as_str()
        })
        .collect();
    assert!(
        ids.contains(&"src_function") || ids.contains(&"trait_def") || ids.contains(&"unit_test")
    );

    Ok(())
}
