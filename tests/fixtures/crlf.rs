//! Test fixture with CRLF line endings.
//!
//! This file tests anchor detection with Windows-style line endings
//! to ensure cross-platform compatibility.

pub fn first_function() -> i32 {
    10
}

pub fn second_function() -> String {
    "hello".to_string()
}

pub fn third_function(x: i32, y: i32) -> i32 {
    x + y
}