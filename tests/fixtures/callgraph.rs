// Simple typed/trait-ish and callgraph-ish fixture for unit tests.
pub trait MyTrait {
    fn my_method(&self);
}

pub struct MyStruct;
impl MyTrait for MyStruct {
    fn my_method(&self) {}
}

pub fn a() { b(); }

pub fn b() { c(); } // anchor within or near this line in tests

pub fn c() {}
