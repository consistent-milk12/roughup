//! Test fixture for methods in impl blocks and trait implementations.
//!
//! This file tests anchor detection on methods within impl blocks
//! and default methods in trait definitions.

pub struct Container<T> {
    pub data: T,
}

pub trait Processable {
    fn process(&self) -> String;
    
    /// Default trait method - should be detected as Method kind
    fn default_method(&self) -> &'static str {
        "default implementation"
    }
    
    /// Another default method with complex signature
    fn complex_default<U>(&self, param: U) -> Result<U, Box<dyn std::error::Error>>
    where
        U: Clone,
    {
        Ok(param.clone())
    }
}

impl<T> Container<T> {
    /// Constructor method
    pub fn new(data: T) -> Self {
        Self { data }
    }
    
    /// Instance method - should be detected as Method kind
    pub fn get_data(&self) -> &T {
        &self.data
    }
    
    /// Mutable method
    pub fn set_data(&mut self, new_data: T) {
        self.data = new_data;
    }
}

impl<T> Processable for Container<T> 
where
    T: std::fmt::Display,
{
    /// Trait implementation method
    fn process(&self) -> String {
        format!("Processing: {}", self.data)
    }
}

/// Free function for comparison
pub fn free_function() -> i32 {
    100
}

impl Container<String> {
    /// Specialized method for String containers
    pub fn append(&mut self, suffix: &str) {
        self.data.push_str(suffix);
    }
}