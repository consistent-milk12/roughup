//! Test fixture for multi-line function signatures.
//!
//! This file tests anchor detection on functions with complex signatures
//! that span multiple lines, including async, generics, and where clauses.

use std::future::Future;

/// Simple function for baseline testing  
pub fn simple_function() -> i32 {
    42
}

/// Multi-line async function with generics and where clause
pub async fn complex_async_function<T, E>(
    param1: T,
    param2: Vec<String>,
) -> Result<T, E>
where
    T: Clone + Send + Sync,
    E: std::error::Error + Send + Sync + 'static,
{
    // Function body starts here - anchor should work on any line inside
    let cloned = param1.clone();
    Ok(cloned)
}

/// Function with extern ABI
pub extern "C" fn c_compatible_function(
    x: i32,
    y: i32,
) -> i32 {
    x + y
}

/// Unsafe function with complex signature
pub unsafe fn unsafe_function_with_generics<'a, T>(
    slice: &'a mut [T],
    index: usize,
) -> &'a mut T 
where
    T: Default,
{
    slice.get_mut(index).unwrap()
}