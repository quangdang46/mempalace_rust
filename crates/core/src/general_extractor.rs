use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum MemoryType {
    Decision,
    Preference,
    Milestone,
    Problem,
    Emotional,
}

pub fn classify(text: &str) -> Vec<Classification> {
    let _ = text;
    vec![]
}

#[derive(Debug)]
pub struct Classification {
    pub memory_type: MemoryType,
    pub confidence: f32,
    pub text: String,
}
