use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::Duration;
use tracing::warn;

#[allow(unused)]
const GRAPH_CACHE_TTL: Duration = Duration::from_secs(60);
const TOPIC_ROOM_PREFIX: &str = "topic:";

fn topic_room(name: &str) -> String {
    format!("{}{}", TOPIC_ROOM_PREFIX, name)
}

fn _normalize_topic(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Normalize a wing name for consistent lookup (#1504).
///
/// `init` stores wing names with hyphens/spaces collapsed to underscores.
/// Callers that pass the raw directory name (`mempalace-public`) would
/// silently miss. This helper aligns the lookup key with stored metadata.
fn _normalize_wing(wing: &str) -> Option<String> {
    let w = wing.trim();
    if w.is_empty() {
        return None;
    }
    Some(crate::config::normalize_wing_name(w))
}

// Explicit tunnels are stored as a JSON file alongside the palace itself
// (`dirname(palace_path)/tunnels.json`) so they persist across palace
// rebuilds (not in the vector DB which can be recreated) and so they live
// next to the rest of the palace state instead of in `~/.mempalace`
// regardless of where the palace was configured (#1467).
fn _get_tunnel_file() -> PathBuf {
    crate::config::Config::load()
        .unwrap_or_default()
        .tunnel_file()
}

/// The pre-#1467 hardcoded path. Kept only for one-time orphan detection
/// in `_load_tunnels`; never written to.
fn _legacy_tunnel_file() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mempalace")
        .join("tunnels.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExplicitTunnel {
    pub id: String,
    pub source_wing: String,
    pub source_room: String,
    pub target_wing: String,
    pub target_room: String,
    pub label: String,
    pub kind: String,
    pub created_at: String,
    pub updated_at: String,
}

fn _load_tunnels() -> Vec<ExplicitTunnel> {
    // Backwards-compatibility: prior to #1467 the tunnel file was hardcoded
    // at `~/.mempalace/tunnels.json` regardless of the configured
    // `palace_path`. If the configured tunnel file is missing but a legacy
    // file exists at a different path, log a one-line warning naming both
    // paths so users can move the file manually. We do NOT auto-migrate —
    // auto-merging tunnel state across two locations is too magical for a
    // bugfix and risks clobbering newer data.
    let path = _get_tunnel_file();
    if path.exists() {
        return match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|_| {
                warn!(
                    "Mempalace tunnels file {:?} is corrupt or unreadable; starting empty.",
                    path
                );
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
    }
    let legacy = _legacy_tunnel_file();
    if legacy != path && legacy.exists() {
        warn!(
            "Legacy tunnels file at {:?} is being ignored; configured location is {:?}. \
             Move or copy the legacy file to the configured path to recover its tunnels.",
            legacy, path
        );
    }
    Vec::new()
}

fn _save_tunnels(tunnels: &[ExplicitTunnel]) -> std::io::Result<()> {
    let path = _get_tunnel_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let tmp_path = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(tunnels)?;
    fs::write(&tmp_path, json)?;
    #[cfg(unix)]
    if let Ok(meta) = tmp_path.metadata() {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&tmp_path, perms)?;
    }
    if let Ok(file) = std::fs::File::open(&tmp_path) {
        let _ = file.sync_all();
    }
    fs::rename(&tmp_path, &path)?;
    Ok(())
}

pub fn create_tunnel(
    source_wing: &str,
    source_room: &str,
    target_wing: &str,
    target_room: &str,
    label: &str,
    kind: &str,
) -> ExplicitTunnel {
    let mut tunnels = _load_tunnels();
    let endpoints = if (source_wing, source_room) < (target_wing, target_room) {
        vec![(source_wing, source_room), (target_wing, target_room)]
    } else {
        vec![(target_wing, target_room), (source_wing, source_room)]
    };
    let id_input = format!(
        "{}|{}|{}|{}",
        endpoints[0].0, endpoints[0].1, endpoints[1].0, endpoints[1].1
    );
    let id = hex::encode(Sha256::digest(id_input.as_bytes()));
    let now = Utc::now().to_rfc3339();
    let existing_idx = tunnels.iter().position(|t| t.id == id);
    if let Some(idx) = existing_idx {
        tunnels[idx].label = label.to_string();
        tunnels[idx].kind = kind.to_string();
        tunnels[idx].updated_at = now;
        let result = tunnels[idx].clone();
        let _ = _save_tunnels(&tunnels);
        return result;
    }
    let tunnel = ExplicitTunnel {
        id,
        source_wing: source_wing.to_string(),
        source_room: source_room.to_string(),
        target_wing: target_wing.to_string(),
        target_room: target_room.to_string(),
        label: label.to_string(),
        kind: kind.to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    tunnels.push(tunnel.clone());
    let _ = _save_tunnels(&tunnels);
    tunnel
}

pub fn compute_topic_tunnels(
    topics_by_wing: &HashMap<String, Vec<String>>,
    min_count: usize,
    label_prefix: &str,
) -> Vec<ExplicitTunnel> {
    if topics_by_wing.is_empty() {
        return Vec::new();
    }
    let min_count = min_count.max(1);

    let mut wing_topics: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (wing, names) in topics_by_wing {
        let mut bucket: HashMap<String, String> = HashMap::new();
        for n in names {
            let key = _normalize_topic(n);
            if !key.is_empty() {
                bucket.entry(key).or_insert_with(|| n.trim().to_string());
            }
        }
        if !bucket.is_empty() {
            // #1504: canonicalize wing keys so repeated mining runs with
            // mixed slug forms (hyphen vs underscore) cannot accumulate
            // parallel duplicate tunnels.
            let canon = crate::config::normalize_wing_name(wing.trim());
            wing_topics.entry(canon).or_default().extend(bucket);
        }
    }

    if wing_topics.is_empty() {
        return Vec::new();
    }

    let mut wings: Vec<&String> = wing_topics.keys().collect();
    wings.sort();
    let mut created: Vec<ExplicitTunnel> = Vec::new();

    for (i, wa) in wings.iter().enumerate() {
        let topics_a = &wing_topics[*wa];
        for wb in wings.iter().skip(i + 1) {
            let topics_b = &wing_topics[*wb];
            let keys_a: HashSet<&String> = topics_a.keys().collect();
            let keys_b: HashSet<&String> = topics_b.keys().collect();
            let shared_keys: HashSet<String> =
                keys_a.intersection(&keys_b).map(|s| (*s).clone()).collect();
            if shared_keys.len() < min_count {
                continue;
            }
            for key in &shared_keys {
                let topic_name = topics_a
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| topics_b.get(key).cloned().unwrap_or_default());
                let room = topic_room(&topic_name);
                let tunnel = create_tunnel(
                    wa,
                    &room,
                    wb,
                    &room,
                    &format!("{}: {}", label_prefix, topic_name),
                    "topic",
                );
                created.push(tunnel);
            }
        }
    }
    created
}

pub fn list_tunnels(wing: Option<&str>) -> Vec<ExplicitTunnel> {
    let tunnels = _load_tunnels();
    match wing {
        Some(w) => {
            // #1504: normalize both the query and stored wing names at read
            // time so legacy underscore tunnels and post-fix verbatim tunnels
            // both resolve via either form.
            let norm = _normalize_wing(w);
            tunnels
                .into_iter()
                .filter(|t| {
                    _normalize_wing(&t.source_wing) == norm
                        || _normalize_wing(&t.target_wing) == norm
                })
                .collect()
        }
        None => tunnels,
    }
}

pub fn delete_tunnel(tunnel_id: &str) -> bool {
    let mut tunnels = _load_tunnels();
    let len_before = tunnels.len();
    tunnels.retain(|t| t.id != tunnel_id);
    if tunnels.len() != len_before {
        let _ = _save_tunnels(&tunnels);
        true
    } else {
        false
    }
}

use std::sync::LazyLock;

#[allow(clippy::type_complexity)]
static _GRAPH_CACHE: LazyLock<
    RwLock<HashMap<PathBuf, GraphCache>>,
    fn() -> RwLock<HashMap<PathBuf, GraphCache>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

static _GRAPH_BUILD_VERSION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct GraphCache {
    nodes: Option<HashMap<String, GraphNode>>,
    edges: Option<Vec<GraphEdge>>,
    cached_at: u64,
    invalidate_counter: u64,
}

impl GraphCache {
    fn is_warm(&self) -> bool {
        if self.nodes.is_none() {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let elapsed = std::time::Duration::from_secs(now.saturating_sub(self.cached_at));
        elapsed < std::time::Duration::from_secs(60)
    }
}

pub fn invalidate_cache(palace_path: &std::path::Path) {
    let key = palace_path.to_path_buf();
    let mut cache = _GRAPH_CACHE
        .write()
        .expect("GRAPH_CACHE write lock poisoned");
    if let Some(entry) = cache.get_mut(&key) {
        entry.nodes = None;
        entry.edges = None;
        entry.cached_at = 0;
        entry.invalidate_counter += 1;
    }
    _GRAPH_BUILD_VERSION.fetch_add(1, Ordering::SeqCst);
}

pub fn cache_invalidation_count() -> u64 {
    _GRAPH_BUILD_VERSION.load(Ordering::SeqCst)
}

/// Returns a warm cached `PalaceGraph` built from `palace_path`, rebuilding
/// from the database if the TTL has expired.
///
/// Thread-safe. Uses a 60-second TTL so repeated graph-tool calls within
/// the same time window reuse the in-memory graph without re-querying the
/// vector DB. Any write to the palace (add_drawer, delete_drawer,
/// diary_write) calls `invalidate_cache(palace_path)` to bust the stale copy.
pub fn cached_graph(palace_path: &std::path::Path) -> PalaceGraph {
    let key = palace_path.to_path_buf();
    let cache = _GRAPH_CACHE.read().expect("GRAPH_CACHE read lock poisoned");
    if let Some(entry) = cache.get(&key) {
        if entry.is_warm() {
            return PalaceGraph {
                nodes: entry.nodes.as_ref().unwrap().clone(),
                edges: entry.edges.as_ref().unwrap().clone(),
            };
        }
    }
    drop(cache);

    let graph = build_graph_from_db_path(palace_path);
    {
        let mut cache = _GRAPH_CACHE
            .write()
            .expect("GRAPH_CACHE write lock poisoned");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let entry = cache.entry(key).or_insert_with(|| GraphCache {
            nodes: None,
            edges: None,
            cached_at: 0,
            invalidate_counter: 0,
        });
        entry.nodes = Some(graph.nodes.clone());
        entry.edges = Some(graph.edges.clone());
        entry.cached_at = now;
    }
    graph
}

/// Build a fresh `PalaceGraph` from the drawer database at `palace_path`.
///
/// This is the only function that touches `PalaceDb` in `palace_graph.rs`;
/// keeping it here (rather than duplicating the logic in `mcp_server.rs`)
/// ensures the DB→graph transformation stays consistent.
fn build_graph_from_db_path(palace_path: &std::path::Path) -> PalaceGraph {
    use crate::palace_db::PalaceDb;

    let mut by_wing: std::collections::HashMap<String, Vec<Room>> =
        std::collections::HashMap::new();

    let entries = PalaceDb::open(palace_path)
        .ok()
        .map(|db| db.get_all(None, None, usize::MAX))
        .unwrap_or_default();

    for entry in entries {
        if let Some(meta) = entry.metadatas.first() {
            let wing = meta
                .get("wing")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let room = meta
                .get("room")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let hall = match meta
                .get("hall")
                .and_then(|v| v.as_str())
                .unwrap_or("hall_facts")
            {
                "hall_events" | "events" => HallType::Events,
                "hall_discoveries" | "discoveries" => HallType::Discoveries,
                "hall_preferences" | "preferences" => HallType::Preferences,
                "hall_advice" | "advice" => HallType::Advice,
                "hall_facts" | "facts" => HallType::Facts,
                other => HallType::Raw(other.to_string()),
            };
            by_wing.entry(wing).or_default().push(Room {
                name: room,
                hall,
                closet_id: entry.ids.first().cloned(),
                date: meta
                    .get("date")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
            });
        }
    }

    let mut graph = PalaceGraph::new();
    for (wing_name, rooms) in by_wing {
        graph.add_wing(Wing {
            name: wing_name,
            wing_type: WingType::Topic,
            rooms,
        });
    }

    if let Ok(db) = crate::palace_db::PalaceDb::open(palace_path) {
        let syn_edges = db.compute_synonymy_edges(0.85);
        for (room_a, room_b, wing, sim) in syn_edges {
            graph.add_synonymy_edge(&room_a, &room_b, &wing, sim);
        }
    }

    graph
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PalaceGraph {
    nodes: HashMap<String, GraphNode>,
    edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Wing {
    pub name: String,
    pub wing_type: WingType,
    pub rooms: Vec<Room>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WingType {
    Person,
    Project,
    Topic,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
#[non_exhaustive]
pub struct Room {
    pub name: String,
    pub hall: HallType,
    pub closet_id: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
#[non_exhaustive]
pub enum HallType {
    Facts,
    Events,
    Discoveries,
    Preferences,
    Advice,
    Raw(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct TraversalResult {
    pub room: String,
    pub wings: Vec<String>,
    pub halls: Vec<String>,
    pub count: usize,
    pub hop: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_via: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct TraverseError {
    pub error: String,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct Tunnel {
    pub room: String,
    pub wings: Vec<String>,
    pub halls: Vec<String>,
    pub count: usize,
    pub recent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct TopTunnel {
    pub room: String,
    pub wings: Vec<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphStats {
    pub total_rooms: usize,
    pub tunnel_rooms: usize,
    pub total_edges: usize,
    pub rooms_per_wing: HashMap<String, usize>,
    pub top_tunnels: Vec<TopTunnel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphNode {
    pub wings: Vec<String>,
    pub halls: Vec<String>,
    pub count: usize,
    pub dates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphEdge {
    pub room: String,
    pub wing_a: String,
    pub wing_b: String,
    pub hall: String,
    pub count: usize,
    /// Kind of edge: "tunnel" for cross-wing room edges, "synonymy" for
    /// semantically similar room pairs (cosine similarity > 0.85 proxy via
    /// text overlap).mp-082.
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum TraverseOutcome {
    Results(Vec<TraversalResult>),
    Error(TraverseError),
}

impl Default for PalaceGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl PalaceGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
        }
    }

    pub fn add_wing(&mut self, wing: Wing) {
        for room in wing.rooms {
            if room.name.is_empty() || room.name == "general" || wing.name.is_empty() {
                continue;
            }

            let node = self
                .nodes
                .entry(room.name.clone())
                .or_insert_with(|| GraphNode {
                    wings: Vec::new(),
                    halls: Vec::new(),
                    count: 0,
                    dates: Vec::new(),
                });

            if !node.wings.iter().any(|existing| existing == &wing.name) {
                node.wings.push(wing.name.clone());
                node.wings.sort();
            }

            let hall = hall_to_string(&room.hall);
            if !hall.is_empty() && !node.halls.iter().any(|existing| existing == &hall) {
                node.halls.push(hall);
                node.halls.sort();
            }

            if let Some(date) = room.date.filter(|date| !date.is_empty()) {
                node.dates.push(date);
                node.dates.sort();
                if node.dates.len() > 5 {
                    let keep_from = node.dates.len() - 5;
                    node.dates = node.dates.split_off(keep_from);
                }
            }

            node.count += 1;
        }

        self.rebuild_edges();
    }

    pub fn add_synonymy_edge(&mut self, room_a: &str, room_b: &str, wing: &str, _similarity: f64) {
        if room_a.is_empty() || room_b.is_empty() || room_a == room_b {
            return;
        }
        for (node_name, node) in &mut self.nodes {
            let matches_a = node_name == room_a;
            let matches_b = node_name == room_b;
            if !matches_a && !matches_b {
                continue;
            }
            if !node.wings.iter().any(|w| w == wing) {
                node.wings.push(wing.to_string());
                node.wings.sort();
            }
            if matches_a && !node.halls.iter().any(|h| h == "synonymy") {
                node.halls.push("synonymy".to_string());
            }
            if matches_b && !node.halls.iter().any(|h| h == "synonymy") {
                node.halls.push("synonymy".to_string());
            }
        }

        let (r_a, r_b) = if room_a <= room_b {
            (room_a.to_string(), room_b.to_string())
        } else {
            (room_b.to_string(), room_a.to_string())
        };
        let already = self
            .edges
            .iter()
            .any(|e| e.kind == "synonymy" && e.room == r_a && e.wing_a == r_b);
        if !already {
            self.edges.push(GraphEdge {
                room: r_a,
                wing_a: r_b,
                wing_b: wing.to_string(),
                hall: "synonymy".to_string(),
                count: 1,
                kind: "synonymy".to_string(),
            });
        }
    }

    pub fn traverse(&self, start_room: &str, max_hops: usize) -> TraverseOutcome {
        let Some(start) = self.nodes.get(start_room) else {
            return TraverseOutcome::Error(TraverseError {
                error: format!("Room '{}' not found", start_room),
                suggestions: self.fuzzy_match(start_room, 5),
            });
        };

        let mut visited: HashSet<String> = HashSet::from([start_room.to_string()]);
        let mut frontier: VecDeque<(String, usize)> = VecDeque::from([(start_room.to_string(), 0)]);
        let mut results = vec![TraversalResult {
            room: start_room.to_string(),
            wings: start.wings.clone(),
            halls: start.halls.clone(),
            count: start.count,
            hop: 0,
            connected_via: None,
        }];

        while let Some((current_room, depth)) = frontier.pop_front() {
            if depth >= max_hops {
                continue;
            }

            let Some(current) = self.nodes.get(&current_room) else {
                continue;
            };
            let current_wings: HashSet<&String> = current.wings.iter().collect();

            for (room, data) in &self.nodes {
                if visited.contains(room) {
                    continue;
                }

                let shared_wings: BTreeSet<String> = data
                    .wings
                    .iter()
                    .filter(|wing| current_wings.contains(*wing))
                    .cloned()
                    .collect();

                if shared_wings.is_empty() {
                    continue;
                }

                visited.insert(room.clone());
                results.push(TraversalResult {
                    room: room.clone(),
                    wings: data.wings.clone(),
                    halls: data.halls.clone(),
                    count: data.count,
                    hop: depth + 1,
                    connected_via: Some(shared_wings.into_iter().collect()),
                });

                if depth + 1 < max_hops {
                    frontier.push_back((room.clone(), depth + 1));
                }
            }
        }

        results.sort_by(|a, b| a.hop.cmp(&b.hop).then_with(|| b.count.cmp(&a.count)));
        results.truncate(50);
        TraverseOutcome::Results(results)
    }

    pub fn find_tunnels(&self, wing_a: Option<&str>, wing_b: Option<&str>) -> Vec<Tunnel> {
        let mut tunnels: Vec<Tunnel> = self
            .nodes
            .iter()
            .filter_map(|(room, data)| {
                if data.wings.len() < 2 {
                    return None;
                }

                if let Some(wing_a) = wing_a {
                    if !data.wings.iter().any(|wing| wing == wing_a) {
                        return None;
                    }
                }

                if let Some(wing_b) = wing_b {
                    if !data.wings.iter().any(|wing| wing == wing_b) {
                        return None;
                    }
                }

                Some(Tunnel {
                    room: room.clone(),
                    wings: data.wings.clone(),
                    halls: data.halls.clone(),
                    count: data.count,
                    recent: data.dates.last().cloned().unwrap_or_default(),
                })
            })
            .collect();

        tunnels.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.room.cmp(&b.room)));
        tunnels.truncate(50);
        tunnels
    }

    pub fn stats(&self) -> GraphStats {
        let tunnel_rooms = self
            .nodes
            .values()
            .filter(|node| node.wings.len() >= 2)
            .count();

        let mut wing_counts: HashMap<String, usize> = HashMap::new();
        for node in self.nodes.values() {
            for wing in &node.wings {
                *wing_counts.entry(wing.clone()).or_insert(0) += 1;
            }
        }

        let mut top_tunnels: Vec<TopTunnel> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.wings.len() >= 2)
            .map(|(room, node)| TopTunnel {
                room: room.clone(),
                wings: node.wings.clone(),
                count: node.count,
            })
            .collect();
        top_tunnels.sort_by(|a, b| {
            b.wings
                .len()
                .cmp(&a.wings.len())
                .then_with(|| b.count.cmp(&a.count))
                .then_with(|| a.room.cmp(&b.room))
        });
        top_tunnels.truncate(10);

        GraphStats {
            total_rooms: self.nodes.len(),
            tunnel_rooms,
            total_edges: self.edges.len(),
            rooms_per_wing: sort_map_by_count_desc(wing_counts),
            top_tunnels,
        }
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    fn rebuild_edges(&mut self) {
        self.edges.clear();

        for (room, data) in &self.nodes {
            if data.wings.len() < 2 {
                continue;
            }

            for (index, wing_a) in data.wings.iter().enumerate() {
                for wing_b in data.wings.iter().skip(index + 1) {
                    for hall in &data.halls {
                        self.edges.push(GraphEdge {
                            room: room.clone(),
                            wing_a: wing_a.clone(),
                            wing_b: wing_b.clone(),
                            hall: hall.clone(),
                            count: data.count,
                            kind: "tunnel".to_string(),
                        });
                    }
                }
            }
        }
    }

    fn fuzzy_match(&self, query: &str, n: usize) -> Vec<String> {
        let query_lower = query.to_lowercase();
        let parts: Vec<&str> = query_lower.split('-').collect();
        let mut scored: Vec<(String, i32)> = Vec::new();

        for room in self.nodes.keys() {
            let room_lower = room.to_lowercase();
            if room_lower.contains(&query_lower) {
                scored.push((room.clone(), 2));
            } else if parts.iter().any(|part| room_lower.contains(part)) {
                scored.push((room.clone(), 1));
            }
        }

        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.into_iter().take(n).map(|(room, _)| room).collect()
    }

    /// Personalized PageRank over the palace graph.
    ///
    /// Seeds: rooms matching query tokens via text similarity.
    /// Teleport probability: 0.15 (standard for PPR).
    /// Max iterations: 30 with tolerance 1e-6.
    pub fn ppr_search(&self, query: &str, max_results: usize) -> Vec<(String, f64)> {
        if self.nodes.is_empty() {
            return Vec::new();
        }

        let rooms: Vec<String> = self.nodes.keys().cloned().collect();
        let n = rooms.len();

        let query_lower = query.to_lowercase();
        let query_tokens: Vec<&str> = query_lower.split_whitespace().collect();

        let mut seed_scores: Vec<f64> = vec![0.0; n];
        for (i, room) in rooms.iter().enumerate() {
            let room_lower = room.to_lowercase();
            let mut score = 0.0_f64;
            for token in &query_tokens {
                if room_lower.contains(token) {
                    score += 1.0;
                }
            }
            if score > 0.0 {
                seed_scores[i] = score;
            }
        }

        let total_seed: f64 = seed_scores.iter().sum();
        if total_seed > 0.0 {
            for s in seed_scores.iter_mut() {
                *s /= total_seed;
            }
        } else {
            for s in seed_scores.iter_mut() {
                *s = 1.0 / n as f64;
            }
        }

        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, room_a) in rooms.iter().enumerate() {
            for (j, room_b) in rooms.iter().enumerate() {
                if i == j {
                    continue;
                }
                let Some(node_a) = self.nodes.get(room_a) else {
                    continue;
                };
                let Some(node_b) = self.nodes.get(room_b) else {
                    continue;
                };
                let shared: Vec<&String> = node_a
                    .wings
                    .iter()
                    .filter(|w| node_b.wings.contains(w))
                    .collect();
                if !shared.is_empty() {
                    adj[i].push(j);
                }
            }
        }

        let teleport = 0.15_f64;
        let tolerance = 1e-6_f64;
        let mut prob = seed_scores.to_vec();
        let damped = 1.0 - teleport;

        for _ in 0..30 {
            let mut next = vec![0.0_f64; n];
            for i in 0..n {
                if adj[i].is_empty() {
                    continue;
                }
                let trans = 1.0 / adj[i].len() as f64;
                for &j in &adj[i] {
                    next[j] += damped * prob[i] * trans;
                }
            }
            for i in 0..n {
                next[i] += teleport * seed_scores[i];
            }

            let mut diff = 0.0_f64;
            for i in 0..n {
                diff += (next[i] - prob[i]).abs();
            }
            prob = next;
            if diff < tolerance {
                break;
            }
        }

        let mut results: Vec<(String, f64)> = rooms
            .iter()
            .enumerate()
            .map(|(i, room)| (room.clone(), prob[i]))
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(max_results);
        results
    }
}

fn hall_to_string(hall: &HallType) -> String {
    match hall {
        HallType::Facts => "hall_facts".to_string(),
        HallType::Events => "hall_events".to_string(),
        HallType::Discoveries => "hall_discoveries".to_string(),
        HallType::Preferences => "hall_preferences".to_string(),
        HallType::Advice => "hall_advice".to_string(),
        HallType::Raw(value) => value.clone(),
    }
}

fn sort_map_by_count_desc(input: HashMap<String, usize>) -> HashMap<String, usize> {
    let mut entries: Vec<(String, usize)> = input.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_graph() -> PalaceGraph {
        let mut graph = PalaceGraph::new();
        graph.add_wing(Wing {
            name: "wing_code".to_string(),
            wing_type: WingType::Project,
            rooms: vec![
                Room {
                    name: "auth-migration".to_string(),
                    hall: HallType::Facts,
                    closet_id: Some("drawer_1".to_string()),
                    date: Some("2026-01-01".to_string()),
                },
                Room {
                    name: "backend".to_string(),
                    hall: HallType::Events,
                    closet_id: Some("drawer_2".to_string()),
                    date: Some("2026-01-02".to_string()),
                },
            ],
        });
        graph.add_wing(Wing {
            name: "wing_myproject".to_string(),
            wing_type: WingType::Topic,
            rooms: vec![
                Room {
                    name: "auth-migration".to_string(),
                    hall: HallType::Advice,
                    closet_id: Some("drawer_3".to_string()),
                    date: Some("2026-01-03".to_string()),
                },
                Room {
                    name: "planning".to_string(),
                    hall: HallType::Discoveries,
                    closet_id: Some("drawer_4".to_string()),
                    date: Some("2026-01-04".to_string()),
                },
            ],
        });
        graph
    }

    #[test]
    fn test_new_graph() {
        let graph = PalaceGraph::new();
        assert_eq!(graph.stats().total_rooms, 0);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn test_traverse_missing_room_returns_error() {
        let graph = PalaceGraph::new();
        let results = graph.traverse("nonexistent", 3);
        assert!(matches!(results, TraverseOutcome::Error(_)));
    }

    #[test]
    fn test_stats_empty() {
        let graph = PalaceGraph::new();
        let stats = graph.stats();
        assert_eq!(stats.total_rooms, 0);
        assert_eq!(stats.tunnel_rooms, 0);
        assert_eq!(stats.total_edges, 0);
    }

    #[test]
    fn test_builds_tunnels_and_edges_per_hall() {
        let graph = sample_graph();
        assert_eq!(graph.stats().tunnel_rooms, 1);
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn test_traverse_matches_python_shape() {
        let graph = sample_graph();
        let results = match graph.traverse("auth-migration", 2) {
            TraverseOutcome::Results(results) => results,
            TraverseOutcome::Error(error) => {
                assert!(
                    error.error.is_empty(),
                    "unexpected traversal error: {}",
                    error.error
                );
                return;
            }
        };
        assert_eq!(results[0].room, "auth-migration");
        assert_eq!(results[0].hop, 0);
        assert!(results[0].connected_via.is_none());
        assert!(results.iter().any(|item| {
            item.room == "backend"
                && item.hop == 1
                && item.connected_via.as_ref() == Some(&vec!["wing_code".to_string()])
        }));
    }

    #[test]
    fn test_find_tunnels_returns_python_shape() {
        let graph = sample_graph();
        let tunnels = graph.find_tunnels(Some("wing_code"), Some("wing_myproject"));
        assert_eq!(tunnels.len(), 1);
        assert_eq!(tunnels[0].room, "auth-migration");
        assert_eq!(tunnels[0].recent, "2026-01-03");
        assert_eq!(
            tunnels[0].halls,
            vec!["hall_advice".to_string(), "hall_facts".to_string()]
        );
    }

    #[test]
    fn test_stats_top_tunnels_shape() {
        let graph = sample_graph();
        let stats = graph.stats();
        assert_eq!(stats.total_rooms, 3);
        assert_eq!(stats.tunnel_rooms, 1);
        assert_eq!(stats.top_tunnels.len(), 1);
        assert_eq!(stats.top_tunnels[0].room, "auth-migration");
    }

    #[test]
    fn test_general_room_is_excluded() {
        let mut graph = PalaceGraph::new();
        graph.add_wing(Wing {
            name: "wing_misc".to_string(),
            wing_type: WingType::Topic,
            rooms: vec![Room {
                name: "general".to_string(),
                hall: HallType::Facts,
                closet_id: None,
                date: None,
            }],
        });
        assert_eq!(graph.stats().total_rooms, 0);
    }

    // ── #1467 — tunnel file follows palace_path config ────────────────────
    //
    // Mirrors upstream Python's `TestTunnelFileFollowsConfig`. We can't
    // monkeypatch a module-level constant the way Python tests do, so we
    // exercise the `Config::load() → tunnel_file()` chain through the
    // public `MEMPALACE_PALACE_PATH` env var and verify that `_save_tunnels`
    // writes to / `_load_tunnels` reads from `dirname(palace_path)`.

    /// Set up an isolated XDG_CONFIG_HOME + on-disk `config.json` so that
    /// `Config::load()` returns a `palace_path` rooted in a temp dir.
    ///
    /// We don't rely solely on `MEMPALACE_PALACE_PATH` because the current
    /// `Config::load()` only honours the env var when a config file already
    /// exists (the env-var-first path lives behind the file-exists branch
    /// in `config.rs`). Writing a config file with the desired palace_path
    /// is the path-of-least-surprise: it exercises `tunnel_file()` through
    /// exactly the same `Config::load()` chain production code uses.
    fn _write_palace_config(xdg_dir: &std::path::Path, palace_path: &std::path::Path) {
        let cfg_dir = xdg_dir.join("mempalace");
        fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_path = cfg_dir.join("config.json");
        let json = serde_json::json!({
            "palace_path": palace_path,
            "collection_name": "test_collection",
        });
        fs::write(&cfg_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }

    #[test]
    fn test_tunnel_file_follows_palace_path_config() {
        let _guard = crate::test_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg = temp_dir.path().join("xdg");
        let palace_path = temp_dir.path().join("palace_root").join("custom_palace");
        fs::create_dir_all(palace_path.parent().unwrap()).unwrap();
        _write_palace_config(&xdg, &palace_path);
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
        std::env::remove_var("MEMPALACE_PALACE_PATH");
        std::env::remove_var("MEMPAL_PALACE_PATH");

        // _get_tunnel_file() must derive from the configured palace_path.
        let resolved = super::_get_tunnel_file();
        let expected = palace_path.parent().unwrap().join("tunnels.json");
        assert_eq!(resolved, expected);

        // Round-trip: a tunnel saved under the configured palace_path must
        // be readable from the same path (i.e. it really did land there,
        // not at ~/.mempalace/tunnels.json).
        let tunnel = ExplicitTunnel {
            id: "test_tunnel_id".to_string(),
            source_wing: "wing_a".to_string(),
            source_room: "room_a".to_string(),
            target_wing: "wing_b".to_string(),
            target_room: "room_b".to_string(),
            label: "test".to_string(),
            kind: "explicit".to_string(),
            created_at: "2026-05-25T00:00:00+00:00".to_string(),
            updated_at: "2026-05-25T00:00:00+00:00".to_string(),
        };
        super::_save_tunnels(std::slice::from_ref(&tunnel)).unwrap();
        assert!(resolved.exists(), "tunnels.json should be next to palace");

        let loaded = super::_load_tunnels();
        assert_eq!(loaded, vec![tunnel]);

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_load_tunnels_returns_empty_when_configured_file_missing() {
        let _guard = crate::test_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg = temp_dir.path().join("xdg");
        let palace_path = temp_dir.path().join("palace_root").join("empty_palace");
        fs::create_dir_all(palace_path.parent().unwrap()).unwrap();
        _write_palace_config(&xdg, &palace_path);
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
        std::env::remove_var("MEMPALACE_PALACE_PATH");
        std::env::remove_var("MEMPAL_PALACE_PATH");

        // No tunnels.json sibling exists; _load_tunnels must not panic and
        // must return an empty Vec rather than reading the legacy
        // `~/.mempalace/tunnels.json` (which might exist on the host).
        let resolved = super::_get_tunnel_file();
        assert!(!resolved.exists());
        let loaded = super::_load_tunnels();
        assert!(loaded.is_empty());

        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
