//! Week-2 A3: bucket-local caps and refusal behavior

use roughup::core::budgeter::{Budgeter, Item, Priority};
use roughup::core::budgeter::{
    fit_with_buckets, BucketCaps, TaggedItem, SpanTag,
};

// Helper to tag an item into a bucket
fn tag_item(it: Item, tag: SpanTag) -> TaggedItem
{
    // Convert into TaggedItem and attach tag
    let mut t: TaggedItem = it.into();
    t.tags.insert(tag);
    t
}

// Build a short code span by repeating a token
fn body(tokens: usize) -> String
{
    // Generate space-separated identifiers to hit token counts
    (0..tokens)
        .map(|i| format!("v{}", i))
        .collect::<Vec<_>>()
        .join(" ")
}

// Test: each bucket stays within its cap; trims do not spill across
#[test]
fn test_bucket_caps_enforced_locally()
{
    // Create budgeter
    let budgeter = Budgeter::new("cl100k_base").unwrap();

    // Build items for three buckets
    let mut items: Vec<TaggedItem> = Vec::new();

    // Code bucket: 3 items of ~30 tokens each
    for i in 0..3
    {
        let it = Item {
            id: format!("code-{}", i),
            content: body(30),
            priority: Priority::medium(),
            hard: false,
            min_tokens: 0,
        };
        items.push(tag_item(it, SpanTag::Code));
    }

    // Interface bucket: 2 items of ~40 tokens each
    for i in 0..2
    {
        let it = Item {
            id: format!("iface-{}", i),
            content: body(40),
            priority: Priority::high(),
            hard: false,
            min_tokens: 0,
        };
        items.push(tag_item(it, SpanTag::Interface));
    }

    // Test bucket: 2 items of ~25 tokens each
    for i in 0..2
    {
        let it = Item {
            id: format!("test-{}", i),
            content: body(25),
            priority: Priority::low(),
            hard: false,
            min_tokens: 0,
        };
        items.push(tag_item(it, SpanTag::Test));
    }

    // Caps force trimming: leave ~60/60/40 tokens respectively
    let caps = BucketCaps { code: 60, interfaces: 60, tests: 40 };

    // No novelty floor for this test
    let res = fit_with_buckets(&budgeter, items, caps, None).unwrap();

    // Sum up per bucket totals by id prefix
    let mut code = 0usize;
    let mut iface = 0usize;
    let mut test = 0usize;

    for fi in &res.fitted.items
    {
        if fi.id.starts_with("code-") { code += fi.tokens; }
        if fi.id.starts_with("iface-") { iface += fi.tokens; }
        if fi.id.starts_with("test-") { test += fi.tokens; }
    }

    // Enforce bucket-local caps; allow tiny drift <= 2 tokens
    assert!(code <= 62, "code cap overflow: {}", code);
    assert!(iface <= 62, "iface cap overflow: {}", iface);
    assert!(test <= 42, "test cap overflow: {}", test);
}