use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_MAX_BYTES: u64 = 500 * 1024 * 1024; // 500MB

pub fn images_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".mempalace").join("images")
}

pub fn max_bytes() -> u64 {
    std::env::var("MEMPALACE_IMAGE_STORE_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_BYTES)
}

pub fn is_managed_image_path<P: AsRef<Path>>(path: P) -> bool {
    let resolved = path.as_ref().canonicalize().unwrap_or_else(|_| path.as_ref().to_path_buf());
    let images = images_dir();
    let normalized = images.canonicalize().unwrap_or(images);
    resolved.starts_with(&normalized)
}

fn content_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn detect_extension(base64_data: &str) -> &str {
    if base64_data.starts_with("data:image/") {
        if let Some(comma_idx) = base64_data.find(',') {
            let meta = &base64_data[..comma_idx];
            if meta.contains("jpeg") || meta.contains("jpg") {
                return "jpg";
            } else if meta.contains("webp") {
                return "webp";
            } else if meta.contains("gif") {
                return "gif";
            }
        }
    } else if base64_data.starts_with("/9j/") {
        return "jpg";
    }
    "png"
}

fn clean_base64(data: &str) -> (&str, &str) {
    let ext = detect_extension(data);
    if data.starts_with("data:image/") {
        if let Some(comma_idx) = data.find(',') {
            return (&data[comma_idx + 1..], ext);
        }
    }
    (data, ext)
}

pub fn save_image_to_disk(base64_data: &str) -> Result<(PathBuf, u64)> {
    if base64_data.is_empty() {
        return Ok((PathBuf::new(), 0));
    }

    let images = images_dir();
    fs::create_dir_all(&images)?;

    let (clean, ext) = clean_base64(base64_data);
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, clean)
        .map_err(|e| anyhow::anyhow!("Failed to decode base64: {}", e))?;

    let hash = content_hash(&decoded);
    let file_path = images.join(format!("{}.{}", hash, ext));

    if file_path.exists() {
        return Ok((file_path, 0));
    }

    fs::write(&file_path, &decoded)?;
    let size = fs::metadata(&file_path)?.len();

    Ok((file_path, size))
}

pub fn delete_image<P: AsRef<Path>>(path: P) -> Result<u64> {
    let path = path.as_ref();
    if !is_managed_image_path(path) {
        return Ok(0);
    }
    if path.exists() {
        let size = fs::metadata(path)?.len();
        fs::remove_file(path)?;
        return Ok(size);
    }
    Ok(0)
}

pub fn touch_image<P: AsRef<Path>>(path: P) -> Result<()> {
    let path = path.as_ref();
    if !is_managed_image_path(path) || !path.exists() {
        return Ok(());
    }
    let now = filetime::FileTime::now();
    filetime::set_file_mtime(path, now)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_test_dir() -> PathBuf {
        let test_dir = images_dir().join("test_temp");
        fs::create_dir_all(&test_dir).unwrap();
        test_dir
    }

    #[test]
    fn test_detect_extension_png() {
        let data = "data:image/png;base64,abc";
        assert_eq!(detect_extension(data), "png");
    }

    #[test]
    fn test_detect_extension_jpeg() {
        let data = "data:image/jpeg;base64,abc";
        assert_eq!(detect_extension(data), "jpg");
    }

    #[test]
    fn test_detect_extension_webp() {
        let data = "data:image/webp;base64,abc";
        assert_eq!(detect_extension(data), "webp");
    }

    #[test]
    fn test_detect_extension_jpeg_magic() {
        let data = "/9j/abc";
        assert_eq!(detect_extension(data), "jpg");
    }

    #[test]
    fn test_detect_extension_default() {
        let data = "abc123";
        assert_eq!(detect_extension(data), "png");
    }

    #[test]
    fn test_clean_base64_strips_data_uri() {
        let data = "data:image/png;base64,aGVsbG8=";
        let (clean, ext) = clean_base64(data);
        assert_eq!(clean, "aGVsbG8=");
        assert_eq!(ext, "png");
    }

    #[test]
    fn test_content_hash_deterministic() {
        let h1 = content_hash(b"test");
        let h2 = content_hash(b"test");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_content_hash_different() {
        let h1 = content_hash(b"test1");
        let h2 = content_hash(b"test2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_save_image_to_disk_empty() {
        let result = save_image_to_disk("").unwrap();
        assert_eq!(result.1, 0);
    }

    #[test]
    fn test_max_bytes_default() {
        assert_eq!(max_bytes(), 500 * 1024 * 1024);
    }
}
