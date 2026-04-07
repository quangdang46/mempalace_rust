use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PalaceGraph {
    wings: Vec<Wing>,
    room_index: HashMap<String, Vec<RoomRef>>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum HallType {
    Facts,
    Events,
    Discoveries,
    Preferences,
    Advice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalResult {
    pub wing: String,
    pub room: String,
    pub hall: String,
    pub distance: usize,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    pub room: String,
    pub wing_a: String,
    pub wing_b: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStats {
    pub total_wings: usize,
    pub total_rooms: usize,
    pub total_halls: usize,
    pub total_tunnels: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct RoomRef {
    pub wing_name: String,
    pub room: Room,
    pub hall: HallType,
}

impl Default for PalaceGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl PalaceGraph {
    pub fn new() -> Self {
        Self {
            wings: Vec::new(),
            room_index: HashMap::new(),
        }
    }

    pub fn add_wing(&mut self, wing: Wing) {
        for room in &wing.rooms {
            let ref_entry = RoomRef {
                wing_name: wing.name.clone(),
                room: room.clone(),
                hall: room.hall.clone(),
            };
            self.room_index
                .entry(room.name.clone())
                .or_default()
                .push(ref_entry);
        }
        self.wings.push(wing);
    }

    pub fn traverse(&self, start_room: &str, max_depth: usize) -> Vec<TraversalResult> {
        let mut results = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, String, usize)> = VecDeque::new();

        if let Some(start_refs) = self.room_index.get(start_room) {
            for room_ref in start_refs {
                queue.push_back((room_ref.wing_name.clone(), room_ref.room.name.clone(), 0));
            }
        }

        while let Some((wing_name, room_name, dist)) = queue.pop_front() {
            if dist > max_depth {
                continue;
            }

            let key = format!("{}:{}", wing_name, room_name);
            if visited.contains(&key) {
                continue;
            }
            visited.insert(key);

            if let Some(refs) = self.room_index.get(&room_name) {
                for room_ref in refs {
                    let hall_str = hall_to_string(&room_ref.hall);
                    results.push(TraversalResult {
                        wing: room_ref.wing_name.clone(),
                        room: room_ref.room.name.clone(),
                        hall: hall_str,
                        distance: dist,
                        content: room_ref.room.closet_id.clone().unwrap_or_default(),
                    });

                    if dist < max_depth {
                        queue.push_back((
                            room_ref.wing_name.clone(),
                            room_ref.room.name.clone(),
                            dist + 1,
                        ));
                    }
                }
            }
        }

        results
    }

    pub fn find_tunnels(&self, wing_a: &str, wing_b: &str) -> Vec<Tunnel> {
        let mut tunnels = Vec::new();

        for room_refs in self.room_index.values() {
            if room_refs.len() >= 2 {
                let wings_in_room: HashSet<_> =
                    room_refs.iter().map(|r| r.wing_name.as_str()).collect();

                if wings_in_room.contains(wing_a) && wings_in_room.contains(wing_b) {
                    if let Some(first_ref) = room_refs.first() {
                        tunnels.push(Tunnel {
                            room: first_ref.room.name.clone(),
                            wing_a: wing_a.to_string(),
                            wing_b: wing_b.to_string(),
                        });
                    }
                }
            }
        }

        tunnels
    }

    pub fn stats(&self) -> GraphStats {
        let total_rooms: usize = self.wings.iter().map(|w| w.rooms.len()).sum();
        let total_halls: usize = self
            .wings
            .iter()
            .flat_map(|w| w.rooms.iter().map(|r| &r.hall))
            .collect::<HashSet<_>>()
            .len();

        let mut tunnel_count = 0;
        for room_refs in self.room_index.values() {
            if room_refs.len() >= 2 {
                tunnel_count += 1;
            }
        }

        GraphStats {
            total_wings: self.wings.len(),
            total_rooms,
            total_halls,
            total_tunnels: tunnel_count,
        }
    }
}

fn hall_to_string(hall: &HallType) -> String {
    match hall {
        HallType::Facts => "facts".to_string(),
        HallType::Events => "events".to_string(),
        HallType::Discoveries => "discoveries".to_string(),
        HallType::Preferences => "preferences".to_string(),
        HallType::Advice => "advice".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_graph() {
        let graph = PalaceGraph::new();
        assert_eq!(graph.wings.len(), 0);
        assert!(graph.room_index.is_empty());
    }

    #[test]
    fn test_traverse_empty() {
        let graph = PalaceGraph::new();
        let results = graph.traverse("nonexistent", 3);
        assert!(results.is_empty());
    }

    #[test]
    fn test_stats_empty() {
        let graph = PalaceGraph::new();
        let stats = graph.stats();
        assert_eq!(stats.total_wings, 0);
        assert_eq!(stats.total_rooms, 0);
    }

    #[test]
    fn test_add_wing_and_traverse() {
        let mut graph = PalaceGraph::new();
        let wing = Wing {
            name: "test_wing".to_string(),
            wing_type: WingType::Project,
            rooms: vec![
                Room {
                    name: "room1".to_string(),
                    hall: HallType::Facts,
                    closet_id: Some("closet1".to_string()),
                },
                Room {
                    name: "room2".to_string(),
                    hall: HallType::Events,
                    closet_id: Some("closet2".to_string()),
                },
            ],
        };
        graph.add_wing(wing);

        let results = graph.traverse("room1", 3);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_find_tunnels_none() {
        let graph = PalaceGraph::new();
        let tunnels = graph.find_tunnels("wing_a", "wing_b");
        assert!(tunnels.is_empty());
    }
}
