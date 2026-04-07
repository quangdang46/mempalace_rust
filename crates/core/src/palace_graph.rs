#[derive(Debug, Clone)]
pub struct PalaceGraph {
    wings: Vec<Wing>,
}

#[derive(Debug, Clone)]
pub struct Wing {
    pub name: String,
    pub wing_type: WingType,
    pub rooms: Vec<Room>,
}

#[derive(Debug, Clone)]
pub enum WingType {
    Person,
    Project,
    Topic,
}

#[derive(Debug, Clone)]
pub struct Room {
    pub name: String,
    pub hall: HallType,
    pub closet_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum HallType {
    Facts,
    Events,
    Discoveries,
    Preferences,
    Advice,
}

impl PalaceGraph {
    pub fn new() -> Self {
        Self { wings: vec![] }
    }

    pub fn traverse(&self, start_room: &str, max_depth: usize) -> Vec<TraversalResult> {
        let _ = (start_room, max_depth);
        vec![]
    }

    pub fn find_tunnels(&self, wing_a: &str, wing_b: &str) -> Vec<Tunnel> {
        let _ = (wing_a, wing_b);
        vec![]
    }

    pub fn stats(&self) -> GraphStats {
        GraphStats {
            total_wings: self.wings.len(),
            total_rooms: self.wings.iter().map(|w| w.rooms.len()).sum(),
            total_halls: 0,
            total_tunnels: 0,
        }
    }
}

#[derive(Debug)]
pub struct TraversalResult {
    pub wing: String,
    pub room: String,
    pub hall: String,
    pub distance: usize,
    pub content: String,
}

#[derive(Debug)]
pub struct Tunnel {
    pub room: String,
    pub wing_a: String,
    pub wing_b: String,
}

#[derive(Debug)]
pub struct GraphStats {
    pub total_wings: usize,
    pub total_rooms: usize,
    pub total_halls: usize,
    pub total_tunnels: usize,
}
