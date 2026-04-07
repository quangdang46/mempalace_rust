pub fn detect_people(text: &str) -> Vec<PersonEntity> {
    let _ = text;
    vec![]
}

pub fn detect_projects(text: &str) -> Vec<ProjectEntity> {
    let _ = text;
    vec![]
}

pub fn detect_from_content(text: &str) -> DetectionResult {
    let _ = text;
    DetectionResult {
        people: vec![],
        projects: vec![],
    }
}

#[derive(Debug)]
pub struct PersonEntity {
    pub name: String,
    pub confidence: f32,
    pub context: String,
}

#[derive(Debug)]
pub struct ProjectEntity {
    pub name: String,
    pub confidence: f32,
    pub context: String,
}

#[derive(Debug)]
pub struct DetectionResult {
    pub people: Vec<PersonEntity>,
    pub projects: Vec<ProjectEntity>,
}
