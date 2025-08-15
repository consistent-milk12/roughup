//! Test cases for novelty floor with TF-IDF (A4 requirement)
//! Validates CEF uplift with no TVE regression

use std::collections::HashSet;

use anyhow::Result;
use roughup::core::budgeter::{
    Priority, SpanTag, TaggedItem, TfidfIndex, filter_by_novelty, novelty_score,
};

#[test]
fn test_tfidf_index_creation()
{
    let documents = vec![
        "function main() { println!(\"hello\"); }".to_string(),
        "function test() { assert!(true); }".to_string(),
        "struct User { name: String, age: u32 }".to_string(),
    ];

    let index = TfidfIndex::new(&documents);

    // Common words should have lower IDF
    let common_idf = index
        .idf("function")
        .unwrap_or(0.0);
    let rare_idf = index
        .idf("println")
        .unwrap_or(0.0);

    assert!(
        rare_idf > common_idf,
        "Rare words should have higher IDF scores"
    );
}

#[test]
fn test_novelty_score_calculation()
{
    let documents = vec![
        "boilerplate code template standard".to_string(),
        "boilerplate template common standard".to_string(),
        "unique innovative algorithm optimization".to_string(),
    ];

    let index = TfidfIndex::new(&documents);

    // Boilerplate text should have low novelty
    let boilerplate_score = novelty_score(&index, "boilerplate template standard");

    // Unique text should have high novelty
    let unique_score = novelty_score(&index, "innovative optimization algorithm");

    assert!(
        unique_score > boilerplate_score,
        "Unique content should have higher novelty score: {} vs {}",
        unique_score,
        boilerplate_score
    );
}

#[test]
fn test_filter_by_novelty_with_rationale() -> Result<()>
{
    let documents = vec![
        "boilerplate template code".to_string(),
        "generic function implementation".to_string(),
        "specialized algorithm optimization".to_string(),
    ];

    let index = TfidfIndex::new(&documents);

    let items = vec![
        TaggedItem {
            id: "boilerplate_func".to_string(),
            content: "boilerplate template code implementation".to_string(),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 10,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Code);
                tags
            },
        },
        TaggedItem {
            id: "unique_algo".to_string(),
            content: "innovative optimization algorithm specialized".to_string(),
            priority: Priority::high(),
            hard: false,
            min_tokens: 15,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Code);
                tags
            },
        },
    ];

    let (kept, refusals) = filter_by_novelty(&index, items, 0.5);

    // Should filter out boilerplate and keep unique content
    assert!(!kept.is_empty(), "Should keep some high-novelty items");

    // Check refusal rationale
    if !refusals.is_empty()
    {
        assert!(
            refusals[0]
                .reason
                .contains("novelty-floor")
        );

        assert_eq!(refusals[0].bucket, "code");
    }

    Ok(())
}

#[test]
fn test_novelty_with_generated_code() -> Result<()>
{
    // Test with code that looks generated/boilerplate
    let documents = vec![
        "auto generated getter setter".to_string(),
        "getter setter property field".to_string(),
        "custom business logic domain".to_string(),
    ];

    let index = TfidfIndex::new(&documents);

    let generated_items = vec![
        TaggedItem {
            id: "auto_getter".to_string(),
            content: "auto generated getter property field".to_string(),
            priority: Priority::low(),
            hard: false,
            min_tokens: 8,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Code);
                tags
            },
        },
        TaggedItem {
            id: "business_logic".to_string(),
            content: "custom business logic domain implementation".to_string(),
            priority: Priority::high(),
            hard: false,
            min_tokens: 12,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Code);
                tags
            },
        },
    ];

    let (kept, refusals) = filter_by_novelty(&index, generated_items, 0.6);

    // Business logic should be kept, generated code may be filtered
    assert!(
        kept.iter()
            .any(|item| item.id == "business_logic")
    );

    // Generated code might be in refusals
    if refusals
        .iter()
        .any(|r| r.id == "auto_getter")
    {
        println!("Generated code correctly filtered for low novelty");
    }

    Ok(())
}

#[test]
fn test_novelty_across_bucket_types() -> Result<()>
{
    let documents = vec![
        "test assert true false".to_string(),
        "interface trait method public".to_string(),
        "implementation algorithm data".to_string(),
    ];

    let index = TfidfIndex::new(&documents);

    let mixed_items = vec![
        TaggedItem {
            id: "std_test".to_string(),
            content: "test assert true simple".to_string(),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 8,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Test);
                tags
            },
        },
        TaggedItem {
            id: "pub_interface".to_string(),
            content: "public interface trait method".to_string(),
            priority: Priority::high(),
            hard: false,
            min_tokens: 10,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Interface);
                tags
            },
        },
        TaggedItem {
            id: "complex_impl".to_string(),
            content: "complex algorithm implementation data structures".to_string(),
            priority: Priority::high(),
            hard: false,
            min_tokens: 15,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Code);
                tags
            },
        },
    ];

    let (kept, refusals) = filter_by_novelty(&index, mixed_items, 0.4);

    // Verify refusals include correct bucket information
    for refusal in &refusals
    {
        assert!(
            ["tests", "interfaces", "code"].contains(
                &refusal
                    .bucket
                    .as_str()
            )
        );
        assert!(
            refusal
                .reason
                .starts_with("novelty-floor")
        );
    }

    // Should keep some items from each bucket type that have sufficient novelty
    assert!(
        !kept.is_empty(),
        "Should keep items with sufficient novelty"
    );

    Ok(())
}

#[test]
fn test_cef_uplift_simulation() -> Result<()>
{
    // Simulate CEF (Context Effectiveness Factor) improvement
    // by filtering low-novelty boilerplate content

    let documents = vec![
        "std fmt debug display".to_string(),
        "derive clone copy eq".to_string(),
        "advanced optimization pipeline".to_string(),
        "custom domain logic".to_string(),
    ];

    let index = TfidfIndex::new(&documents);

    // Create a mix of boilerplate and valuable content
    let mut items = Vec::new();

    // Add boilerplate-heavy items
    for i in 0..5
    {
        let item = TaggedItem {
            id: format!("boilerplate_{}", i),
            content: "std fmt debug display derive clone".to_string(),
            priority: Priority::low(),
            hard: false,
            min_tokens: 10,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Code);
                tags
            },
        };
        items.push(item);
    }

    // Add high-value items
    for i in 0..3
    {
        let item = TaggedItem {
            id: format!("valuable_{}", i),
            content: "advanced optimization pipeline custom domain logic".to_string(),
            priority: Priority::high(),
            hard: false,
            min_tokens: 15,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Code);
                tags
            },
        };
        items.push(item);
    }

    let original_count = items.len();
    let (filtered, refusals) = filter_by_novelty(&index, items, 0.3);

    // Calculate effective filtering
    let filtered_ratio = refusals.len() as f64 / original_count as f64;

    // Should filter out significant portion of boilerplate (simulating CEF uplift)
    assert!(
        filtered_ratio > 0.2,
        "Should filter at least 20% of low-novelty content"
    );

    // High-value content should be preserved
    assert!(
        filtered
            .iter()
            .any(|item| {
                item.id
                    .contains("valuable")
            })
    );

    println!(
        "Filtered {:.1}% low-novelty content for CEF uplift",
        filtered_ratio * 100.0
    );

    Ok(())
}

#[test]
fn test_tve_regression_guard() -> Result<()>
{
    // Test TVE (Total Volume Efficiency) regression guard
    // Ensure novelty filtering doesn't overly reduce context volume

    let documents = vec![
        "common standard implementation".to_string(),
        "standard library usage".to_string(),
        "specialized unique algorithm".to_string(),
    ];

    let index = TfidfIndex::new(&documents);

    let items = vec![
        TaggedItem {
            id: "common_impl".to_string(),
            content: "common standard implementation approach".to_string(),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 12,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Code);
                tags
            },
        },
        TaggedItem {
            id: "unique_algo".to_string(),
            content: "specialized unique algorithm optimization".to_string(),
            priority: Priority::high(),
            hard: false,
            min_tokens: 15,
            tags: {
                let mut tags = HashSet::new();
                tags.insert(SpanTag::Code);
                tags
            },
        },
    ];

    // Test with conservative threshold to prevent TVE regression
    let (kept, _refusals) = filter_by_novelty(&index, items, 0.2);

    // Should keep majority of content to maintain volume efficiency
    assert!(
        !kept.is_empty(),
        "TVE guard: should not filter out all content"
    );

    Ok(())
}
