use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::Duration;

const GRAPH_CACHE_TTL: Duration = Duration::from_secs(60);
const TOPIC_ROOM_PREFIX: &str = "topic:";

fn topic_room(name: &str) -> String {
    format!("{}{}", TOPIC_ROOM_PREFIX, name)
}

fn _normalize_topic(name: &str) -> String {
    name.trim().to_lowercase()
}

fn _tunnel_file() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mempalace")
        .join("tunnels.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    let path = _tunnel_file();
    if !path.exists() {
        return Vec::new();
    }
    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn _save_tunnels(tunnels: &[ExplicitTunnel]) -> std::io::Result<()> {
    let path = _tunnel_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let tmp_path = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(tunnels)?;
    fs::write(&tmp_path, json)?;
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
            wing_topics.insert(wing.trim().to_string(), bucket);
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
            let shared_keys: HashSet<String> = keys_a.intersection(&keys_b).map(|s| (*s).clone()).collect();
            if shared_keys.len() < min_count {
                continue;
            }
            for key in &shared_keys {
                let topic_name = topics_a.get(key).cloned().unwrap_or_else(|| topics_b.get(key).cloned().unwrap_or_default());
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
        Some(w) => tunnels
            .into_iter()
            .filter(|t| t.source_wing == w || t.target_wing == w)
            .collect(),
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

static _GRAPH_CACHE: RwLock<GraphCache> = RwLock::new(GraphCache {
    nodes: None,
    edges: None,
    cached_at: 0,
    invalidate_counter: 0,
});

static _GRAPH_BUILD_VERSION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct GraphCache {
    nodes: Option<HashMap<String, GraphNode>>,
    edges: Option<Vec<GraphEdge>>,
    cached_at: u64,
    invalidate_counter: u64,
}

pub fn invalidate_cache() {
    let mut cache = _GRAPH_CACHE.write().unwrap();
    cache.nodes = None;
    cache.edges = None;
    cache.cached_at = 0;
    cache.invalidate_counter += 1;
    _GRAPH_BUILD_VERSION.fetch_add(1, Ordering::SeqCst);
}

pub fn cache_invalidation_count() -> u64 {
    _GRAPH_CACHE.read().unwrap().invalidate_counter
}

fn _cache_is_warm() -> bool {
    let cache = match _GRAPH_CACHE.read() {
        Ok(c) => c,
        Err(_) => return false,
    };
    if cache.nodes.is_none() {
        return false;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let elapsed = Duration::from_secs(now.saturating_sub(cache.cached_at));
    elapsed < GRAPH_CACHE_TTL
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PalaceGraph {
    nodes: HashMap<String, GraphNode>,
    edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wing {
    pub name: String,
    pub wing_type: WingType,
    pub rooms: Vec<Room>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WingType {
    Person,
    Project,
    Topic,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct Room {
    pub name: String,
    pub hall: HallType,
    pub closet_id: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum HallType {
    Facts,
    Events,
    Discoveries,
    Preferences,
    Advice,
    Raw(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
pub struct TraverseError {
    pub error: String,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tunnel {
    pub room: String,
    pub wings: Vec<String>,
    pub halls: Vec<String>,
    pub count: usize,
    pub recent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TopTunnel {
    pub room: String,
    pub wings: Vec<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphStats {
    pub total_rooms: usize,
    pub tunnel_rooms: usize,
    pub total_edges: usize,
    pub rooms_per_wing: HashMap<String, usize>,
    pub top_tunnels: Vec<TopTunnel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphNode {
    pub wings: Vec<String>,
    pub halls: Vec<String>,
    pub count: usize,
    pub dates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphEdge {
    pub room: String,
    pub wing_a: String,
    pub wing_b: String,
    pub hall: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
}
