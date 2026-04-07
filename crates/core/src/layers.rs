use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLayer {
    pub layer: LayerLevel,
    pub content: String,
    pub tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LayerLevel {
    L0,
    L1,
    L2,
    L3,
}

pub fn get_layer(layer: LayerLevel) -> MemoryLayer {
    MemoryLayer {
        layer,
        content: String::new(),
        tokens: 0,
    }
}

pub fn build_wakeup_context(
    palace_path: &std::path::Path,
    wing: Option<&str>,
) -> anyhow::Result<String> {
    let _ = (palace_path, wing);
    Ok(String::new())
}
