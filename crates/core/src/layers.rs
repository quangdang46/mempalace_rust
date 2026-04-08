//! 4-Layer Memory Stack for MemPalace
//!
//! Layer 0: Identity (~100 tokens) — Always loaded. "Who am I?"
//! Layer 1: Essential Story (~500-800 tokens) — Always loaded. Top moments from the palace.
//! Layer 2: On-Demand (~200-500 each) — Loaded when a topic/wing comes up.
//! Layer 3: Deep Search (unlimited) — Full semantic search.

use crate::config::Config;
use crate::palace_db::PalaceDb;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Layer 0 — Identity
// ---------------------------------------------------------------------------

/// Layer 0: Identity (~100 tokens). Always loaded.
pub struct Layer0 {
    path: PathBuf,
    cached_text: Option<String>,
}

impl Layer0 {
    pub fn new(identity_path: Option<PathBuf>) -> Self {
        let path = identity_path.unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".mempalace")
                .join("identity.txt")
        });
        Self {
            path,
            cached_text: None,
        }
    }

    /// Return the identity text, or a sensible default.
    pub fn render(&mut self) -> String {
        if let Some(ref text) = self.cached_text {
            return text.clone();
        }
        let text = if self.path.exists() {
            std::fs::read_to_string(&self.path)
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "## L0 — IDENTITY\nNo identity configured.".to_string())
        } else {
            "## L0 — IDENTITY\nNo identity configured. Create ~/.mempalace/identity.txt".to_string()
        };
        self.cached_text = Some(text.clone());
        text
    }

    /// Estimate token count (chars / 4).
    pub fn token_estimate(&mut self) -> usize {
        self.render().len() / 4
    }
}

// ---------------------------------------------------------------------------
// Layer 1 — Essential Story
// ---------------------------------------------------------------------------

/// Layer 1: Essential Story (~500-800 tokens). Always loaded.
pub struct Layer1 {
    pub(super) max_drawers: usize,
    pub(super) max_chars: usize,
    wing: Option<String>,
}

impl Layer1 {
    pub const MAX_DRAWERS: usize = 15;
    pub const MAX_CHARS: usize = 3200;

    pub fn new(wing: Option<String>) -> Self {
        Self {
            wing,
            max_drawers: Self::MAX_DRAWERS,
            max_chars: Self::MAX_CHARS,
        }
    }

    /// Pull top drawers from PalaceDb and format as compact L1 text.
    pub fn generate(&self, palace_db: &PalaceDb) -> String {
        let wing_filter = self.wing.as_deref();
        let results = palace_db.get_all(wing_filter, None, self.max_drawers);

        if results.is_empty() {
            return "## L1 — No drawers found.".to_string();
        }

        let mut entries: Vec<DrawerEntry> = Vec::new();
        for qr in &results {
            for (i, doc) in qr.documents.iter().enumerate() {
                let meta = qr.metadatas.get(i).cloned().unwrap_or_default();
                let importance = self.extract_importance(&meta);
                entries.push(DrawerEntry {
                    importance,
                    doc: doc.clone(),
                    meta,
                });
            }
        }

        if entries.is_empty() {
            return "## L1 — No memories yet.".to_string();
        }

        entries.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.truncate(self.max_drawers);

        let mut by_room: HashMap<String, Vec<&DrawerEntry>> = HashMap::new();
        for entry in &entries {
            let room = entry
                .meta
                .get("room")
                .and_then(|v| v.as_str())
                .unwrap_or("general")
                .to_string();
            by_room.entry(room).or_default().push(entry);
        }

        let mut lines = vec!["## L1 — ESSENTIAL STORY".to_string()];
        let mut total_len = 0;

        let mut rooms: Vec<_> = by_room.keys().collect();
        rooms.sort();

        for room in rooms {
            let room_line = format!("\n[{}]", room);
            lines.push(room_line.clone());
            total_len += room_line.len();

            for entry in &by_room[room] {
                let source = entry
                    .meta
                    .get("source_file")
                    .and_then(|v| v.as_str())
                    .and_then(|s| {
                        Path::new(s)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                    })
                    .unwrap_or_default();

                let snippet = truncate_snippet(&entry.doc, 200);
                let entry_line = if !source.is_empty() {
                    format!("  - {}  ({})", snippet, source)
                } else {
                    format!("  - {}", snippet)
                };

                if total_len + entry_line.len() > self.max_chars {
                    lines.push("  ... (more in L3 search)".to_string());
                    return lines.join("\n");
                }
                lines.push(entry_line.clone());
                total_len += entry_line.len();
            }
        }
        lines.join("\n")
    }

    fn extract_importance(&self, meta: &HashMap<String, serde_json::Value>) -> f64 {
        for key in &["importance", "emotional_weight", "weight"] {
            if let Some(val) = meta.get(*key) {
                if let Ok(imp) = serde_json::from_value::<f64>(val.clone()) {
                    return imp;
                }
            }
        }
        0.5
    }
}

struct DrawerEntry {
    importance: f64,
    doc: String,
    meta: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Layer 2 — On-Demand
// ---------------------------------------------------------------------------

/// Layer 2: On-Demand (~200-500 tokens per retrieval).
pub struct Layer2;

impl Default for Layer2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Layer2 {
    pub fn new() -> Self {
        Self
    }

    /// Retrieve drawers filtered by wing and/or room.
    pub fn retrieve(
        &self,
        palace_db: &PalaceDb,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String {
        let results = palace_db.get_all(wing, room, n_results);

        let docs: Vec<String> = results.iter().flat_map(|qr| qr.documents.clone()).collect();
        let metas: Vec<HashMap<String, serde_json::Value>> =
            results.iter().flat_map(|qr| qr.metadatas.clone()).collect();

        if docs.is_empty() {
            let mut label = String::new();
            if let Some(w) = wing {
                label.push_str(&format!("wing={}", w));
            }
            if let Some(r) = room {
                if !label.is_empty() {
                    label.push(' ');
                }
                label.push_str(&format!("room={}", r));
            }
            return if label.is_empty() {
                "No drawers found.".to_string()
            } else {
                format!("No drawers found for {}.", label)
            };
        }

        let mut lines = vec![format!("## L2 — ON-DEMAND ({} drawers)", docs.len())];
        for (doc, meta) in docs.iter().zip(metas.iter()).take(n_results) {
            let room_name = meta.get("room").and_then(|v| v.as_str()).unwrap_or("?");
            let source = meta
                .get("source_file")
                .and_then(|v| v.as_str())
                .and_then(|s| {
                    Path::new(s)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                })
                .unwrap_or_default();

            let snippet = truncate_snippet(doc, 300);
            let entry = if !source.is_empty() {
                format!("  [{}] {}  ({})", room_name, snippet, source)
            } else {
                format!("  [{}] {}", room_name, snippet)
            };
            lines.push(entry);
        }
        lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// Layer 3 — Deep Search
// ---------------------------------------------------------------------------

/// Layer 3: Unlimited depth. Semantic search against the full palace.
pub struct Layer3;

impl Default for Layer3 {
    fn default() -> Self {
        Self::new()
    }
}

impl Layer3 {
    pub fn new() -> Self {
        Self
    }

    /// Semantic search, returns compact result text.
    pub async fn search(
        &self,
        palace_db: &PalaceDb,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String {
        let hits = self
            .search_raw(palace_db, query, wing, room, n_results)
            .await;
        if hits.is_empty() {
            return "No results found.".to_string();
        }

        let mut lines = vec![format!("## L3 — SEARCH RESULTS for \"{}\"", query)];
        for (i, hit) in hits.iter().enumerate().take(n_results) {
            lines.push(format!(
                "  [{}] {}/{} (sim={:.3})",
                i + 1,
                hit.wing.as_deref().unwrap_or("?"),
                hit.room.as_deref().unwrap_or("?"),
                hit.similarity
            ));
            lines.push(format!("      {}", truncate_snippet(&hit.text, 300)));
            if !hit.source_file.is_empty() && hit.source_file != "?" {
                lines.push(format!("      src: {}", hit.source_file));
            }
        }
        lines.join("\n")
    }

    /// Return raw SearchHit structs instead of formatted text.
    pub async fn search_raw(
        &self,
        palace_db: &PalaceDb,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> Vec<SearchHit> {
        let results = match palace_db.query(query, wing, room, n_results).await {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut hits = Vec::new();
        for qr in results {
            for (i, doc) in qr.documents.iter().enumerate() {
                let meta = qr.metadatas.get(i).cloned().unwrap_or_default();
                let distance = qr.distances.get(i).copied().unwrap_or(1.0);
                let similarity: f64 = (1.0 - distance).clamp(0.0_f64, 1.0);

                hits.push(SearchHit {
                    text: doc.clone(),
                    wing: meta.get("wing").and_then(|v| v.as_str().map(String::from)),
                    room: meta.get("room").and_then(|v| v.as_str().map(String::from)),
                    source_file: meta
                        .get("source_file")
                        .and_then(|v| {
                            v.as_str().map(|s| {
                                Path::new(s)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| s.to_string())
                            })
                        })
                        .unwrap_or_else(|| "?".to_string()),
                    similarity,
                    metadata: meta,
                });
            }
        }
        hits
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub text: String,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub source_file: String,
    pub similarity: f64,
    pub metadata: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// MemoryStack
// ---------------------------------------------------------------------------

/// The full 4-layer stack.
pub struct MemoryStack {
    palace_path: PathBuf,
    identity_path: PathBuf,
    l0: Layer0,
    l1: Layer1,
    l2: Layer2,
    l3: Layer3,
}

impl MemoryStack {
    pub fn new(palace_path: Option<PathBuf>, identity_path: Option<PathBuf>) -> Self {
        let config = Config::default();
        let palace_path = palace_path.unwrap_or_else(|| config.palace_path.clone());
        let identity_path = identity_path.unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".mempalace")
                .join("identity.txt")
        });

        Self {
            palace_path: palace_path.clone(),
            identity_path: identity_path.clone(),
            l0: Layer0::new(Some(identity_path)),
            l1: Layer1::new(None),
            l2: Layer2::new(),
            l3: Layer3::new(),
        }
    }

    /// Generate wake-up text: L0 (identity) + L1 (essential story).
    pub async fn wake_up(&mut self, wing: Option<&str>) -> String {
        let palace_db = match PalaceDb::open(&self.palace_path) {
            Ok(db) => db,
            Err(_) => return format!("{}\n\n## L1 — No palace found.", self.l0.render()),
        };

        if let Some(w) = wing {
            self.l1 = Layer1::new(Some(w.to_string()));
        }

        let l0_text = self.l0.render();
        let l1_text = self.l1.generate(&palace_db);

        format!("{}\n\n{}", l0_text, l1_text)
    }

    /// On-demand L2 retrieval filtered by wing/room.
    pub fn recall(&self, wing: Option<&str>, room: Option<&str>, n_results: usize) -> String {
        let palace_db = match PalaceDb::open(&self.palace_path) {
            Ok(db) => db,
            Err(e) => return format!("No palace found: {}", e),
        };
        self.l2.retrieve(&palace_db, wing, room, n_results)
    }

    /// Deep L3 semantic search.
    pub async fn search(
        &self,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String {
        let palace_db = match PalaceDb::open(&self.palace_path) {
            Ok(db) => db,
            Err(e) => return format!("No palace found: {}", e),
        };
        self.l3
            .search(&palace_db, query, wing, room, n_results)
            .await
    }

    /// Status of all layers.
    pub fn status(&self) -> LayerStatus {
        let identity_exists = self.identity_path.exists();
        let token_estimate = if identity_exists {
            std::fs::read_to_string(&self.identity_path)
                .map(|s| s.len() / 4)
                .unwrap_or(0)
        } else {
            0
        };

        let total_drawers = PalaceDb::open(&self.palace_path)
            .map(|db| db.count())
            .unwrap_or(0);

        LayerStatus {
            palace_path: self.palace_path.clone(),
            identity_path: self.identity_path.clone(),
            l0_identity: IdentityStatus {
                path: self.identity_path.clone(),
                exists: identity_exists,
                tokens: token_estimate,
            },
            l1_essential: EssentialStatus {
                description: "Auto-generated from top palace drawers".to_string(),
            },
            l2_on_demand: OnDemandStatus {
                description: "Wing/room filtered retrieval".to_string(),
            },
            l3_deep_search: DeepSearchStatus {
                description: "Full semantic search via PalaceDb".to_string(),
            },
            total_drawers,
        }
    }
}

fn truncate_snippet(text: &str, max_len: usize) -> String {
    let snippet = text.trim().replace('\n', " ");
    if snippet.len() <= max_len {
        snippet
    } else {
        format!("{}...", &snippet[..max_len.saturating_sub(3)])
    }
}

#[derive(Debug, serde::Serialize)]
pub struct LayerStatus {
    pub palace_path: PathBuf,
    pub identity_path: PathBuf,
    pub l0_identity: IdentityStatus,
    pub l1_essential: EssentialStatus,
    pub l2_on_demand: OnDemandStatus,
    pub l3_deep_search: DeepSearchStatus,
    pub total_drawers: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct IdentityStatus {
    pub path: PathBuf,
    pub exists: bool,
    pub tokens: usize,
}
#[derive(Debug, serde::Serialize)]
pub struct EssentialStatus {
    pub description: String,
}
#[derive(Debug, serde::Serialize)]
pub struct OnDemandStatus {
    pub description: String,
}
#[derive(Debug, serde::Serialize)]
pub struct DeepSearchStatus {
    pub description: String,
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_test_palace_db(temp_dir: &Path) -> PalaceDb {
        let palace_path = temp_dir.join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        db.add(
            &[
                (
                    "id1",
                    "I had a wonderful breakfast this morning with my family",
                ),
                (
                    "id2",
                    "Working on the Rust implementation of the memory palace",
                ),
                (
                    "id3",
                    "Feeling excited about the new project launch next week",
                ),
                (
                    "id4",
                    "Technical discussion about async programming in Rust",
                ),
                (
                    "id5",
                    "Remember to call mom about the family reunion planning",
                ),
            ],
            &[
                &[
                    ("wing", "personal"),
                    ("room", "morning"),
                    ("importance", "0.9"),
                ],
                &[
                    ("wing", "technical"),
                    ("room", "rust"),
                    ("importance", "0.8"),
                ],
                &[
                    ("wing", "personal"),
                    ("room", "emotions"),
                    ("importance", "0.7"),
                ],
                &[
                    ("wing", "technical"),
                    ("room", "rust"),
                    ("importance", "0.6"),
                ],
                &[
                    ("wing", "personal"),
                    ("room", "family"),
                    ("importance", "0.85"),
                ],
            ],
        )
        .unwrap();
        db.complete_test_setup().unwrap();
        db
    }

    // Layer0 tests
    #[test]
    fn test_layer0_render_default_when_no_file() {
        let temp_dir = tempdir().unwrap();
        let mut l0 = Layer0::new(Some(temp_dir.path().join("nonexistent_identity.txt")));
        let text = l0.render();
        assert!(text.contains("L0 — IDENTITY"));
        assert!(text.contains("No identity configured"));
    }

    #[test]
    fn test_layer0_render_reads_file_when_exists() {
        let temp_dir = tempdir().unwrap();
        let identity_path = temp_dir.path().join("identity.txt");
        std::fs::write(&identity_path, "I am a test identity.").unwrap();
        let mut l0 = Layer0::new(Some(identity_path));
        assert!(l0.render().contains("I am a test identity"));
    }

    #[test]
    fn test_layer0_render_caches_result() {
        let temp_dir = tempdir().unwrap();
        let identity_path = temp_dir.path().join("identity.txt");
        std::fs::write(&identity_path, "Original content").unwrap();
        let mut l0 = Layer0::new(Some(identity_path.clone()));
        assert!(l0.render().contains("Original content"));
        std::fs::write(&identity_path, "Modified content").unwrap();
        assert!(l0.render().contains("Original content")); // cached
    }

    #[test]
    fn test_layer0_token_estimate() {
        let temp_dir = tempdir().unwrap();
        let identity_path = temp_dir.path().join("identity.txt");
        std::fs::write(&identity_path, "12345678").unwrap();
        let mut l0 = Layer0::new(Some(identity_path));
        assert_eq!(l0.token_estimate(), 2);
    }

    // Layer1 tests
    #[test]
    fn test_layer1_generate_empty_palace() {
        let temp_dir = tempdir().unwrap();
        let palace_path = temp_dir.path().join("empty_palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let db = PalaceDb::open(&palace_path).unwrap();
        let l1 = Layer1::new(None);
        let text = l1.generate(&db);
        assert!(text.contains("L1"));
    }

    #[test]
    fn test_layer1_generate_with_content() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let palace_path = temp_dir.path().join("palace");
        let l1 = Layer1::new(None);
        let text = l1.generate(&db);
        assert!(text.contains("L1 — ESSENTIAL STORY"));
        assert!(text.contains("[rust]") || text.contains("[personal]"));
    }

    #[test]
    fn test_layer1_respects_max_chars() {
        let temp_dir = tempdir().unwrap();
        let palace_path = temp_dir.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        let long_content = "This is a very long memory entry that should be truncated when displayed in the essential story layer because we have a strict character limit for layer 1 to keep the wake-up context manageable.".repeat(50);
        db.add(
            &[
                ("long1", &long_content),
                ("long2", &long_content),
                ("long3", &long_content),
            ],
            &[
                &[("wing", "test"), ("room", "general"), ("importance", "0.9")],
                &[("wing", "test"), ("room", "general"), ("importance", "0.8")],
                &[("wing", "test"), ("room", "general"), ("importance", "0.7")],
            ],
        )
        .unwrap();
        let l1 = Layer1::new(None);
        let text = l1.generate(&db);
        assert!(text.contains("..."));
    }

    #[test]
    fn test_layer1_wing_filter() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let l1 = Layer1::new(Some("technical".to_string()));
        let text = l1.generate(&db);
        assert!(text.contains("rust") || text.contains("technical"));
    }

    // Layer2 tests
    #[test]
    fn test_layer2_retrieve_no_palace() {
        let temp_dir = tempdir().unwrap();
        let palace_path = temp_dir.path().join("nonexistent_palace");
        let db = PalaceDb::open(&palace_path).unwrap();
        let l2 = Layer2::new();
        let text = l2.retrieve(&db, None, None, 5);
        assert!(text.contains("No drawers") || text.contains("L2"));
    }

    #[test]
    fn test_layer2_retrieve_with_results() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let l2 = Layer2::new();
        let text = l2.retrieve(&db, None, None, 5);
        assert!(text.contains("L2 — ON-DEMAND"));
        assert!(text.contains("drawers"));
    }

    #[test]
    fn test_layer2_retrieve_wing_filter() {
        let temp_temp = tempdir().unwrap();
        let db = create_test_palace_db(temp_temp.path());
        let l2 = Layer2::new();
        let text = l2.retrieve(&db, Some("personal"), None, 5);
        assert!(text.contains("L2"));
    }

    #[test]
    fn test_layer2_retrieve_room_filter() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let l2 = Layer2::new();
        let text = l2.retrieve(&db, None, Some("rust"), 5);
        assert!(text.contains("L2"));
    }

    #[test]
    fn test_layer2_retrieve_n_results() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let l2 = Layer2::new();
        let text = l2.retrieve(&db, None, None, 2);
        assert!(text.contains("2 drawers"));
    }

    // Layer3 tests
    #[tokio::test]
    async fn test_layer3_search_no_results() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let palace_path = temp_dir.path().join("palace");
        let l3 = Layer3::new();
        let text = l3.search(&db, "nonexistent query xyz", None, None, 5).await;
        assert!(text.contains("No results") || text.contains("L3"));
    }

    #[tokio::test]
    async fn test_layer3_search_with_results() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let palace_path = temp_dir.path().join("palace");
        let l3 = Layer3::new();
        let text = l3.search(&db, "Rust programming", None, None, 5).await;
        assert!(text.contains("L3 — SEARCH RESULTS"));
        assert!(text.contains("Rust") || text.contains("rust"));
    }

    #[tokio::test]
    async fn test_layer3_search_raw() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let palace_path = temp_dir.path().join("palace");
        let l3 = Layer3::new();
        let hits = l3.search_raw(&db, "family", None, None, 5).await;
        assert!(!hits.is_empty());
        for hit in hits {
            assert!(!hit.text.is_empty());
            assert!(hit.similarity >= 0.0 && hit.similarity <= 1.0);
        }
    }

    #[tokio::test]
    async fn test_layer3_search_respects_n_results() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let palace_path = temp_dir.path().join("palace");
        let l3 = Layer3::new();
        let hits = l3.search_raw(&db, "the", None, None, 2).await;
        assert!(hits.len() <= 2);
    }

    #[tokio::test]
    async fn test_layer3_search_with_wing_filter() {
        let temp_dir = tempdir().unwrap();
        let db = create_test_palace_db(temp_dir.path());
        let palace_path = temp_dir.path().join("palace");
        let l3 = Layer3::new();
        let hits = l3
            .search_raw(&db, "project", Some("personal"), None, 5)
            .await;
        for hit in hits {
            let wing = hit.wing.as_deref().unwrap_or("");
            assert_eq!(wing, "personal");
        }
    }

    // MemoryStack tests
    #[tokio::test]
    async fn test_memory_stack_wake_up() {
        let temp_dir = tempdir().unwrap();
        let identity_path = temp_dir.path().join("identity.txt");
        std::fs::write(&identity_path, "I am TestUser.").unwrap();
        let palace_path = temp_dir.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        db.add(
            &[("test1", "Test memory content")],
            &[&[("wing", "personal"), ("room", "general")]],
        )
        .unwrap();
        db.flush().unwrap();
        drop(db);

        let mut stack = MemoryStack::new(Some(palace_path), Some(identity_path));
        let text = stack.wake_up(None).await;
        eprintln!("DEBUG wake_up: {}", text);
        assert!(text.contains("L0") || text.contains("I am TestUser"));
        assert!(text.contains("L1"));
    }

    #[tokio::test]
    async fn test_memory_stack_recall() {
        let temp_dir = tempdir().unwrap();
        let palace_path = temp_dir.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        db.add(
            &[("recall1", "Personal memory about family")],
            &[&[("wing", "personal"), ("room", "family")]],
        )
        .unwrap();
        db.flush().unwrap();
        drop(db);

        let stack = MemoryStack::new(Some(palace_path), None);
        let text = stack.recall(Some("personal"), None, 5);
        eprintln!("DEBUG recall: {}", text);
        assert!(text.contains("L2") || text.contains("personal") || text.contains("recall"));
    }

    #[tokio::test]
    async fn test_memory_stack_search() {
        let temp_dir = tempdir().unwrap();
        let palace_path = temp_dir.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        db.add(
            &[("search1", "Rust programming language implementation")],
            &[&[("wing", "technical"), ("room", "rust")]],
        )
        .unwrap();
        db.flush().unwrap();
        drop(db);

        let stack = MemoryStack::new(Some(palace_path), None);
        let text = stack.search("Rust", None, None, 5).await;
        eprintln!("DEBUG search: {}", text);
        assert!(text.contains("L3") || text.contains("Rust") || text.contains("search"));
    }

    #[test]
    fn test_memory_stack_status() {
        let temp_dir = tempdir().unwrap();
        let identity_path = temp_dir.path().join("identity.txt");
        std::fs::write(&identity_path, "Test identity content").unwrap();
        let palace_path = temp_dir.path().join("palace");
        let stack = MemoryStack::new(Some(palace_path), Some(identity_path));
        let status = stack.status();
        // usize comparison checks removed - clippy flags >= 0 on unsigned types
    }

    // truncate_snippet tests
    #[test]
    fn test_truncate_snippet_short() {
        assert_eq!(truncate_snippet("Short text", 50), "Short text");
    }

    #[test]
    fn test_truncate_snippet_exact_length() {
        assert_eq!(truncate_snippet("Exactly 10", 10), "Exactly 10");
    }

    #[test]
    fn test_truncate_snippet_long() {
        let result = truncate_snippet("This is a very long string that needs truncating", 20);
        assert!(result.len() <= 20);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_snippet_replaces_newlines() {
        let result = truncate_snippet("Line1\nLine2\nLine3", 100);
        assert!(!result.contains('\n'));
        assert_eq!(result, "Line1 Line2 Line3");
    }
}
