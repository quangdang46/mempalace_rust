pub fn detect_room(file_path: &std::path::Path, content: &str) -> Option<String> {
    let _ = (file_path, content);
    None
}

pub fn get_room_patterns() -> &'static [(&'static str, &'static str)] {
    &[]
}
