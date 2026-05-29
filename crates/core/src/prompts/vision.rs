/// Vision description prompt template.
/// For Phase 7: Vision embedding and image management.

/// System prompt for describing images/screenshots.
pub const VISION_DESCRIPTION_SYSTEM: &str = r#"You are a vision-to-text description engine. Given an image (screenshot, diagram, or UI), describe its contents in detail.

Focus on:
- What is visible in the image
- Text content if any
- UI elements and their arrangement
- Code or technical diagrams if present
- Colors, layout, and structure

Be concise but thorough. Output a single paragraph description."#;

/// User prompt sent along with the image to trigger description.
pub const VISION_DESCRIPTION_PROMPT: &str = "Describe this image in detail.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vision_description_system_content() {
        assert!(VISION_DESCRIPTION_SYSTEM.contains("vision-to-text description engine"));
        assert!(VISION_DESCRIPTION_SYSTEM.contains("screenshot"));
        assert!(VISION_DESCRIPTION_SYSTEM.contains("UI elements"));
    }
}
