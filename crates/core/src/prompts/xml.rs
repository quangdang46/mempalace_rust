/// XML parsing utilities for extracting structured data from LLM responses.
/// 1:1 port from mempalace `src/prompts/xml.ts`.
use regex::Regex;

/// Valid XML tag pattern: starts with letter or underscore, followed by alphanumeric, underscore, or hyphen.
fn is_valid_tag(tag: &str) -> bool {
    let pattern = Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_-]*$").unwrap();
    pattern.is_match(tag)
}

/// Extract the text content of the first occurrence of a tag.
/// Returns an empty string if the tag is not found or the tag name is invalid.
///
/// # Example
/// ```ignore
/// let xml = "<response><title>Hello</title></response>";
/// assert_eq!(get_xml_tag(xml, "title"), "Hello");
/// ```
pub fn get_xml_tag(xml: &str, tag: &str) -> String {
    if !is_valid_tag(tag) {
        return String::new();
    }

    let pattern = format!(r"<{tag}>([\s\S]*?)</{tag}>");
    let re = match Regex::new(&pattern) {
        Ok(re) => re,
        Err(_) => return String::new(),
    };

    match re.captures(xml) {
        Some(caps) => caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default(),
        None => String::new(),
    }
}

/// Extract text content of all child elements with the given tag name
/// that are direct children of the specified parent tag.
/// Returns an empty Vec if the parent is not found or tag names are invalid.
///
/// # Example
/// ```ignore
/// let xml = "<facts><fact>one</fact><fact>two</fact></facts>";
/// let facts = get_xml_children(xml, "facts", "fact");
/// assert_eq!(facts, vec!["one", "two"]);
/// ```
pub fn get_xml_children(xml: &str, parent: &str, child: &str) -> Vec<String> {
    if !is_valid_tag(parent) || !is_valid_tag(child) {
        return Vec::new();
    }

    let parent_pattern = format!(r"<{parent}>([\s\S]*?)</{parent}>");
    let parent_re = match Regex::new(&parent_pattern) {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };

    let Some(parent_caps) = parent_re.captures(xml) else {
        return Vec::new();
    };

    let Some(parent_content) = parent_caps.get(1) else {
        return Vec::new();
    };

    let child_pattern = format!(r"<{child}>([\s\S]*?)</{child}>");
    let child_re = match Regex::new(&child_pattern) {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };

    child_re
        .captures_iter(parent_content.as_str())
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().trim().to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_xml_tag_simple() {
        let xml = "<response><title>Hello World</title></response>";
        assert_eq!(get_xml_tag(xml, "title"), "Hello World");
    }

    #[test]
    fn test_get_xml_tag_nested() {
        let xml = "<outer><inner>content</inner></outer>";
        assert_eq!(get_xml_tag(xml, "inner"), "content");
    }

    #[test]
    fn test_get_xml_tag_not_found() {
        let xml = "<response><title>Hello</title></response>";
        assert_eq!(get_xml_tag(xml, "missing"), "");
    }

    #[test]
    fn test_get_xml_tag_invalid_tag_name() {
        let xml = "<response><title>Hello</title></response>";
        assert_eq!(get_xml_tag(xml, "123invalid"), "");
        assert_eq!(get_xml_tag(xml, ""), "");
        assert_eq!(get_xml_tag(xml, "has<angle"), "");
    }

    #[test]
    fn test_get_xml_tag_multiline() {
        let xml = "<narrative>line one\nline two</narrative>";
        assert_eq!(get_xml_tag(xml, "narrative"), "line one\nline two");
    }

    #[test]
    fn test_get_xml_children_simple() {
        let xml = "<facts><fact>one</fact><fact>two</fact><fact>three</fact></facts>";
        let children = get_xml_children(xml, "facts", "fact");
        assert_eq!(children, vec!["one", "two", "three"]);
    }

    #[test]
    fn test_get_xml_children_empty() {
        let xml = "<facts></facts>";
        let children = get_xml_children(xml, "facts", "fact");
        assert!(children.is_empty());
    }

    #[test]
    fn test_get_xml_children_parent_not_found() {
        let xml = "<other><fact>one</fact></other>";
        let children = get_xml_children(xml, "facts", "fact");
        assert!(children.is_empty());
    }

    #[test]
    fn test_get_xml_children_invalid_tag_names() {
        let xml = "<facts><fact>one</fact></facts>";
        assert!(get_xml_children(xml, "123facts", "fact").is_empty());
        assert!(get_xml_children(xml, "facts", "123fact").is_empty());
    }

    #[test]
    fn test_get_xml_children_with_whitespace() {
        let xml = "<facts>\n  <fact> one </fact>\n  <fact>two</fact>\n</facts>";
        let children = get_xml_children(xml, "facts", "fact");
        assert_eq!(children, vec!["one", "two"]);
    }

    #[test]
    fn test_valid_tag_pattern() {
        assert!(is_valid_tag("title"));
        assert!(is_valid_tag("my-tag"));
        assert!(is_valid_tag("_underscore"));
        assert!(is_valid_tag("tag123"));
        assert!(!is_valid_tag("123tag"));
        assert!(!is_valid_tag(""));
        assert!(!is_valid_tag("has space"));
        assert!(!is_valid_tag("has<angle"));
    }
}
