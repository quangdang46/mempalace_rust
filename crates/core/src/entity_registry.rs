use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityCode {
    pub code: String,
    pub canonical_name: String,
    pub variants: Vec<String>,
}

pub fn register_entity(
    name: &str,
    code: Option<&str>,
    registry: &mut HashMap<String, EntityCode>,
) -> String {
    let _ = (name, code, registry);
    String::new()
}

pub fn lookup_code(name: &str, registry: &HashMap<String, EntityCode>) -> Option<String> {
    let _ = (name, registry);
    None
}

pub fn get_or_create_code(name: &str, registry: &mut HashMap<String, EntityCode>) -> String {
    let _ = (name, registry);
    String::new()
}
