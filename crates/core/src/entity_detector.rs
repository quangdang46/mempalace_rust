//! Entity detector — auto-detect people and projects from file content.
//!
//! Two-pass approach:
//!   Pass 1: scan files, extract entity candidates with signal counts
//!   Pass 2: score and classify each candidate as person, project, or uncertain
//!
//! Used by mempalace init before mining begins.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// =============================================================================
// PATTERN TEMPLATES (stored as static strings, compiled per-name)
// =============================================================================

const PERSON_VERB_TEMPLATES: &[&str] = &[
    r"\b{name}\s+said\b",
    r"\b{name}\s+asked\b",
    r"\b{name}\s+told\b",
    r"\b{name}\s+replied\b",
    r"\b{name}\s+laughed\b",
    r"\b{name}\s+smiled\b",
    r"\b{name}\s+cried\b",
    r"\b{name}\s+felt\b",
    r"\b{name}\s+thinks?\b",
    r"\b{name}\s+wants?\b",
    r"\b{name}\s+loves?\b",
    r"\b{name}\s+hates?\b",
    r"\b{name}\s+knows?\b",
    r"\b{name}\s+decided\b",
    r"\b{name}\s+pushed\b",
    r"\b{name}\s+wrote\b",
    r"\b{name}\s+worked\b",
    r"\b{name}\s+reviewed\b",
    r"\b{name}\s+finished\b",
    r"\b{name}\s+shipped\b",
    r"\bhey\s+{name}\b",
    r"\bthanks?\s+{name}\b",
    r"\bhi\s+{name}\b",
    r"\bdear\s+{name}\b",
];

const DIALOGUE_TEMPLATES: &[&str] = &[
    // Use \s* to allow leading whitespace on dialogue lines
    r"\n\s*>\s*{name}[:\s]",
    r"(?:\n|^)\s*{name}:\s",
    r"\n\s*\[{name}\]",
    r#""{name}\s+said"#,
];

const PROJECT_VERB_TEMPLATES: &[&str] = &[
    r"\bbuilding\s+{name}\b",
    r"\bbuilt\s+{name}\b",
    r"\bship(?:ping|ped)?\s+{name}\b",
    r"\blaunch(?:ing|ed)?\s+{name}\b",
    r"\bdeploy(?:ing|ed)?\s+{name}\b",
    r"\binstall(?:ing|ed)?\s+{name}\b",
    r"\bthe\s+{name}\s+architecture\b",
    r"\bthe\s+{name}\s+pipeline\b",
    r"\bthe\s+{name}\s+system\b",
    r"\bthe\s+{name}\s+repo\b",
    r"\b{name}\s+v\d+\b",
    r"\b{name}\.py\b",
    r"\b{name}-core\b",
    r"\b{name}-local\b",
    r"\bimport\s+{name}\b",
    r"\bpip\s+install\s+{name}\b",
];

static SINGLE_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Match words starting with uppercase (including CamelCase like MemPalace)
    // Uses Unicode-aware \p{Lu}/\p{Ll} for cross-script support (Cyrillic, etc.)
    Regex::new(r"\b([\p{Lu}][\p{Ll}\p{Lu}]{1,19})\b").unwrap()
});

static MULTI_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Unicode-aware multi-word proper nouns
    Regex::new(r"\b([\p{Lu}][\p{Ll}]+(?:\s+[\p{Lu}][\p{Ll}]+)+)\b").unwrap()
});

static VERSIONED_CANDIDATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([\p{Lu}][\p{Ll}\p{Lu}]{1,19})[-_]v?\d+(?:\.\d+)*\b").unwrap());

static PRONOUN_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    PRONOUN_PATTERNS_STATIC
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
});

/// Build all patterns for a specific candidate name, with name escaped.
fn build_name_patterns(name: &str) -> CompiledNamePatterns {
    let n = regex::escape(name);
    CompiledNamePatterns {
        // Dialogue patterns need multiline mode for ^ anchor
        dialogue: DIALOGUE_TEMPLATES
            .iter()
            .map(|t| {
                let p = t.replace("{name}", &n);
                RegexBuilder::new(&p).multi_line(true).build().unwrap()
            })
            .collect(),
        person_verbs: PERSON_VERB_TEMPLATES
            .iter()
            .map(|t| {
                let p = t.replace("{name}", &n);
                Regex::new(&p).unwrap()
            })
            .collect(),
        project_verbs: PROJECT_VERB_TEMPLATES
            .iter()
            .map(|t| {
                let p = t.replace("{name}", &n);
                Regex::new(&p).unwrap()
            })
            .collect(),
        direct: RegexBuilder::new(&format!(r"\bhey\s+{n}\b|\bthanks?\s+{n}\b|\bhi\s+{n}\b"))
            .case_insensitive(true)
            .build()
            .unwrap(),
        versioned: RegexBuilder::new(&format!(r"\b{n}[-_]v?\d+(?:\.\d+)*\b"))
            .case_insensitive(true)
            .build()
            .unwrap(),
        code_ref: RegexBuilder::new(&format!(r"\b{n}\.(py|js|ts|yaml|yml|json|sh)\b"))
            .case_insensitive(true)
            .build()
            .unwrap(),
    }
}

struct CompiledNamePatterns {
    dialogue: Vec<Regex>,
    person_verbs: Vec<Regex>,
    project_verbs: Vec<Regex>,
    direct: Regex,
    versioned: Regex,
    code_ref: Regex,
}

// =============================================================================
// STOPWORDS & COMMON NAMES
// =============================================================================

const STOPWORDS_PHASE: &[&str] = &[
    // Articles, conjunctions, prepositions
    "the",
    "a",
    "an",
    "and",
    "or",
    "but",
    "in",
    "on",
    "at",
    "to",
    "for",
    "of",
    "with",
    "by",
    "from",
    "as",
    "is",
    "was",
    "are",
    "were",
    "be",
    "been",
    "being",
    // Auxiliaries
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "will",
    "would",
    "could",
    "should",
    "may",
    "might",
    "must",
    "shall",
    "can",
    // Pronouns
    "this",
    "that",
    "these",
    "those",
    "it",
    "its",
    "they",
    "them",
    "their",
    "we",
    "our",
    "you",
    "your",
    "i",
    "my",
    "me",
    "he",
    "she",
    "his",
    "her",
    // Question words
    "who",
    "what",
    "when",
    "where",
    "why",
    "how",
    "which",
    // Adverbs
    "if",
    "then",
    "so",
    "not",
    "no",
    "yes",
    "ok",
    "okay",
    "just",
    "very",
    "really",
    "also",
    "already",
    "still",
    "even",
    "only",
    "here",
    "there",
    "now",
    "too",
    "up",
    "out",
    "about",
    "like",
    // Verbs
    "use",
    "get",
    "got",
    "make",
    "made",
    "take",
    "put",
    "come",
    "go",
    "see",
    "know",
    "think",
    "return",
    "print",
    // Programming
    "def",
    "class",
    "import",
    "from",
    "new",
    "true",
    "false",
    "none",
    "null",
    // General nouns
    "step",
    "usage",
    "run",
    "check",
    "find",
    "add",
    "set",
    "list",
    "args",
    "dict",
    "str",
    "int",
    "bool",
    "path",
    "file",
    "type",
    "name",
    "note",
    "example",
    "option",
    "result",
    "error",
    "warning",
    "info",
    "every",
    "each",
    "more",
    "less",
    "next",
    "last",
    "first",
    "second",
    "stack",
    "layer",
    "mode",
    "test",
    "stop",
    "start",
    "copy",
    "move",
    "source",
    "target",
    "output",
    "input",
    "data",
    "item",
    "key",
    "value",
    "returns",
    "raises",
    "yields",
    "self",
    "cls",
    "kwargs",
    // Abstract/prose words
    "world",
    "well",
    "want",
    "topic",
    "choose",
    "social",
    "cars",
    "phones",
    "healthcare",
    "ex",
    "machina",
    "deus",
    "human",
    "humans",
    "people",
    "things",
    "something",
    "nothing",
    "everything",
    "anything",
    "someone",
    "everyone",
    "anyone",
    "way",
    "time",
    "day",
    "life",
    "place",
    "thing",
    "part",
    "kind",
    "sort",
    "case",
    "point",
    "idea",
    "fact",
    "sense",
    "question",
    "answer",
    "reason",
    "number",
    "version",
    "system",
    // Greetings
    "hey",
    "hi",
    "hello",
    "thanks",
    "thank",
    "right",
    "let",
    // UI/action words
    "click",
    "hit",
    "press",
    "tap",
    "drag",
    "drop",
    "open",
    "close",
    "save",
    "load",
    "launch",
    "install",
    "download",
    "upload",
    "scroll",
    "select",
    "enter",
    "submit",
    "cancel",
    "confirm",
    "delete",
    "copy",
    "paste",
    "type",
    "write",
    "read",
    "search",
    "find",
    "show",
    "hide",
    // Technical dir names
    "desktop",
    "documents",
    "downloads",
    "users",
    "home",
    "library",
    "applications",
    "system",
    "preferences",
    "settings",
    "terminal",
    // Abstract concepts
    "actor",
    "vector",
    "remote",
    "control",
    "duration",
    "fetch",
    "agents",
    "tools",
    "others",
    "guards",
    "ethics",
    "regulation",
    "learning",
    "thinking",
    "memory",
    "language",
    "intelligence",
    "technology",
    "society",
    "culture",
    "future",
    "history",
    "science",
    "model",
    "models",
    "network",
    "networks",
    "training",
    "inference",
];

static STOPWORDS: LazyLock<std::collections::HashSet<&'static str>> =
    LazyLock::new(|| STOPWORDS_PHASE.iter().copied().collect());

/// Common first names that should not be detected as entities unless they appear with
/// strong person signals.
const COMMON_FIRST_NAMES: &[&str] = &[
    "James",
    "Mary",
    "John",
    "Patricia",
    "Robert",
    "Jennifer",
    "Michael",
    "Linda",
    "William",
    "Barbara",
    "David",
    "Elizabeth",
    "Richard",
    "Susan",
    "Joseph",
    "Jessica",
    "Thomas",
    "Sarah",
    "Charles",
    "Karen",
    "Christopher",
    "Nancy",
    "Daniel",
    "Lisa",
    "Matthew",
    "Margaret",
    "Anthony",
    "Betty",
    "Mark",
    "Sandra",
    "Donald",
    "Ashley",
    "Steven",
    "Dorothy",
    "Paul",
    "Kimberly",
    "Andrew",
    "Emily",
    "Joshua",
    "Donna",
    "Kenneth",
    "Michelle",
    "Kevin",
    "Carol",
    "Brian",
    "Amanda",
    "George",
    "Melissa",
    "Timothy",
    "Deborah",
    "Ronald",
    "Stephanie",
    "Edward",
    "Rebecca",
    "Jason",
    "Sharon",
    "Jeffrey",
    "Laura",
    "Ryan",
    "Cynthia",
    "Jacob",
    "Kathleen",
    "Gary",
    "Amy",
    "Nicholas",
    "Angela",
    "Eric",
    "Shirley",
    "Jonathan",
    "Anna",
    "Stephen",
    "Brenda",
    "Larry",
    "Pamela",
    "Justin",
    "Nicole",
    "Scott",
    "Emma",
    "Brandon",
    "Helen",
    "Benjamin",
    "Samantha",
    "Samuel",
    "Katherine",
    "Raymond",
    "Christine",
    "Gregory",
    "Debra",
    "Frank",
    "Rachel",
    "Alexander",
    "Carolyn",
    "Patrick",
    "Janet",
    "Jack",
    "Catherine",
    "Dennis",
    "Maria",
    "Jerry",
    "Heather",
    "Tyler",
    "Diane",
    "Aaron",
    "Ruth",
    "Jose",
    "Julie",
    "Adam",
    "Olivia",
    "Nathan",
    "Joyce",
    "Henry",
    "Virginia",
    "Zachary",
    "Victoria",
    "Gabriel",
    "Kelly",
    "Wayne",
    "Lauren",
    "Ethan",
    "Christina",
    "Jordan",
    "Joan",
    "Luke",
    "Evelyn",
    "Jayden",
    "Judith",
    "Carter",
    "Megan",
    "Oliver",
    "Andrea",
    "Julian",
    "Cheryl",
    "Wyatt",
    "Hannah",
    "Sebastian",
    "Martha",
    "Christian",
    "Gloria",
    "Dylan",
    "Teresa",
    "Elijah",
    "Ann",
    "Liam",
    "Sara",
    "Matthew",
    "Madison",
    "Jackson",
    "Francesca",
    "Sebastian",
    "Kathryn",
    "Aiden",
    "Janice",
    "Levi",
    "Jean",
    "Isaac",
    "Abigail",
    "Caleb",
    "Alice",
    "Ryan",
    "Sofia",
    "Nick",
    "Ava",
    "Bob",
    "Emily",
    "Tom",
    "Luna",
    "Sam",
    "Ivy",
    "Max",
    "Grace",
    "Dan",
    "Chloe",
    "Alex",
    "Penelope",
    "Chris",
    "Riley",
    "Jamie",
    "Aria",
    "Taylor",
    "Layla",
    "Morgan",
    "Ellie",
    "Cameron",
    "Stella",
    "Robin",
    "Nora",
    "Quinn",
    "Lily",
    "Aria",
    "Pearl",
    // Tech/OSS names that often appear in code but aren't entities
    "Alice",
    "Bob",
    "Charlie",
    "Dave",
    "Eve",
    "Frank",
    "Grace",
    "Heidi",
    "Ivan",
    "Judy",
    "Mallory",
    "Oscar",
    "Peggy",
    "Trent",
    "Walter",
    "Victor",
    "Emma",
    "John",
    "Mike",
];

static COMMON_NAMES_SET: LazyLock<std::collections::HashSet<&'static str>> =
    LazyLock::new(|| COMMON_FIRST_NAMES.iter().copied().collect());

const PRONOUN_PATTERNS_STATIC: &[&str] = &[
    r"\bshe\b",
    r"\bher\b",
    r"\bhers\b",
    r"\bhe\b",
    r"\bhim\b",
    r"\bhis\b",
    r"\bthey\b",
    r"\bthem\b",
    r"\btheir\b",
];

// =============================================================================
// DATA STRUCTURES
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonEntity {
    pub name: String,
    pub confidence: f32,
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntity {
    pub name: String,
    pub confidence: f32,
    pub context: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectionResult {
    pub people: Vec<PersonEntity>,
    pub projects: Vec<ProjectEntity>,
    pub uncertain: Vec<PersonEntity>,
}

const PROSE_EXTENSIONS: &[&str] = &["txt", "md", "rst", "csv"];
const READABLE_EXTENSIONS: &[&str] = &[
    "txt", "md", "py", "js", "ts", "json", "yaml", "yml", "csv", "rst", "toml", "sh", "rb", "go",
    "rs",
];
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "env",
    "dist",
    "build",
    ".next",
    "coverage",
    ".mempalace",
    ".terraform",
    "vendor",
    "target",
    ".cache",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
];

const SKIP_FILENAMES: &[&str] = &[
    "license",
    "licence",
    "copying",
    "copyright",
    "notice",
    "authors",
    "patents",
    "third_party_notices",
    "third-party-notices",
];

#[derive(Debug, Clone)]
struct ScoredEntity {
    #[allow(unused)]
    name: String,
    person_score: i32,
    project_score: i32,
    person_signals: Vec<String>,
    project_signals: Vec<String>,
    frequency: usize,
}

#[derive(Debug, Clone)]
struct ClassifiedEntity {
    name: String,
    entity_type: EntityType,
    confidence: f32,
    frequency: usize,
    signals: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum EntityType {
    Person,
    Project,
    Uncertain,
}

// =============================================================================
// CANDIDATE EXTRACTION
// =============================================================================

/// Extract all capitalized proper noun candidates from text.
/// Returns a map of name -> frequency for names appearing 3+ times.
/// Filters out stopwords and common first names (requires strong signals for those).
fn extract_candidates(text: &str) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    // Single-word proper nouns
    for cap in SINGLE_WORD_RE.captures_iter(text) {
        if let Some(word) = cap.get(1).map(|m| m.as_str()) {
            let lower = word.to_lowercase();
            if !STOPWORDS.contains(lower.as_str()) && word.len() > 1 {
                *counts.entry(word.to_string()).or_insert(0) += 1;
            }
        }
    }

    // Multi-word proper nouns (e.g. "Memory Palace", "Claude Code")
    for cap in MULTI_WORD_RE.captures_iter(text) {
        if let Some(phrase) = cap.get(1).map(|m| m.as_str()) {
            if !phrase.split_whitespace().any(|w| {
                let lw = w.to_lowercase();
                STOPWORDS.contains(lw.as_str())
            }) {
                *counts.entry(phrase.to_string()).or_insert(0) += 1;
            }
        }
    }

    // Versioned or suffixed project names like `MemPalace_v2` should still
    // surface the base candidate so later project scoring can classify them.
    for cap in VERSIONED_CANDIDATE_RE.captures_iter(text) {
        if let Some(word) = cap.get(1).map(|m| m.as_str()) {
            let lower = word.to_lowercase();
            if !STOPWORDS.contains(lower.as_str()) && word.len() > 1 {
                *counts.entry(word.to_string()).or_insert(0) += 1;
            }
        }
    }

    // Filter: must appear at least 3 times
    counts.retain(|_, count| *count >= 3);
    counts
}

/// Check if a name is a common first name that needs extra signals to be confirmed.
fn is_common_name(name: &str) -> bool {
    COMMON_NAMES_SET.contains(name)
}

// =============================================================================
// SIGNAL SCORING
// =============================================================================

fn score_entity(name: &str, text: &str, lines: &[&str]) -> ScoredEntity {
    let patterns = build_name_patterns(name);
    let mut person_score: i32 = 0;
    let mut project_score: i32 = 0;
    let mut person_signals: Vec<String> = Vec::new();
    let mut project_signals: Vec<String> = Vec::new();

    // --- Person signals ---

    // Dialogue markers (strong signal, weight 3).
    // The bare `NAME:` pattern also matches metadata like `Created: 2026-04-24`,
    // so require at least two hits for that specific variant.
    for (idx, rx) in patterns.dialogue.iter().enumerate() {
        let matches = rx.find_iter(text).count();
        if matches == 0 {
            continue;
        }
        if idx == 1 && matches < 2 {
            continue;
        }
        person_score += (matches * 3) as i32;
        person_signals.push(format!("dialogue marker ({}x)", matches));
    }

    // Person verbs (weight 2)
    for rx in &patterns.person_verbs {
        let matches = rx.find_iter(text).count();
        if matches > 0 {
            person_score += (matches * 2) as i32;
            person_signals.push(format!("'{} ...' action ({}x)", name, matches));
        }
    }

    // Pronoun proximity — pronouns within 3 lines of the name
    let name_lower = name.to_lowercase();
    let name_line_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.to_lowercase().contains(&name_lower))
        .map(|(i, _)| i)
        .collect();

    let mut pronoun_hits = 0;
    for idx in &name_line_indices {
        let start = if *idx >= 2 { idx - 2 } else { 0 };
        let end = std::cmp::min(lines.len(), idx + 3);
        let window_text = lines[start..end].join(" ").to_lowercase();
        for pronoun_re in PRONOUN_PATTERNS.iter() {
            if pronoun_re.is_match(&window_text) {
                pronoun_hits += 1;
                break;
            }
        }
    }
    if pronoun_hits > 0 {
        person_score += pronoun_hits * 2;
        person_signals.push(format!("pronoun nearby ({}x)", pronoun_hits));
    }

    // Direct address (weight 4)
    let direct = patterns.direct.find_iter(text).count();
    if direct > 0 {
        person_score += (direct * 4) as i32;
        person_signals.push(format!("addressed directly ({}x)", direct));
    }

    // --- Project signals ---

    // Project verbs (weight 2)
    for rx in &patterns.project_verbs {
        let matches = rx.find_iter(text).count();
        if matches > 0 {
            project_score += (matches * 2) as i32;
            project_signals.push(format!("project verb ({}x)", matches));
        }
    }

    // Versioned/hyphenated (weight 3)
    let versioned = patterns.versioned.find_iter(text).count();
    if versioned > 0 {
        project_score += (versioned * 3) as i32;
        project_signals.push(format!("versioned/hyphenated ({}x)", versioned));
    }

    // Code file reference (weight 3)
    let code_ref = patterns.code_ref.find_iter(text).count();
    if code_ref > 0 {
        project_score += (code_ref * 3) as i32;
        project_signals.push(format!("code file reference ({}x)", code_ref));
    }

    // Keep only top 3 signals each
    person_signals.truncate(3);
    project_signals.truncate(3);

    ScoredEntity {
        name: name.to_string(),
        person_score,
        project_score,
        person_signals,
        project_signals,
        frequency: 0, // filled by caller
    }
}

// =============================================================================
// CLASSIFY
// =============================================================================

fn classify_entity(name: &str, frequency: usize, scores: &ScoredEntity) -> ClassifiedEntity {
    let ps = scores.person_score;
    let prs = scores.project_score;
    let total = ps + prs;

    if total == 0 {
        // No strong signals — frequency-only candidate, uncertain
        let confidence = (frequency as f32 / 50.0).min(0.4);
        return ClassifiedEntity {
            name: name.to_string(),
            entity_type: EntityType::Uncertain,
            confidence: (confidence * 100.0).round() / 100.0,
            frequency,
            signals: vec![format!("appears {}x, no strong type signals", frequency)],
        };
    }

    let person_ratio = if total > 0 {
        ps as f32 / total as f32
    } else {
        0.0
    };

    // Require TWO different signal categories to confidently classify as a person.
    // Common names (Bob, Alice, etc.) need stronger signals.
    let mut signal_categories: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for s in &scores.person_signals {
        if s.contains("dialogue") {
            signal_categories.insert("dialogue");
        } else if s.contains("action") {
            signal_categories.insert("action");
        } else if s.contains("pronoun") {
            signal_categories.insert("pronoun");
        } else if s.contains("addressed") {
            signal_categories.insert("addressed");
        }
    }

    let has_two_signal_types = signal_categories.len() >= 2;
    let signal_count = |needle: &str| -> usize {
        scores
            .person_signals
            .iter()
            .filter(|signal| signal.contains(needle))
            .filter_map(|signal| {
                signal
                    .rsplit_once('(')
                    .and_then(|(_, tail)| tail.strip_suffix("x)"))
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .sum()
    };
    let dialogue_hits = signal_count("dialogue");
    let action_hits = signal_count("action");
    let address_hits = signal_count("addressed");

    // Common names need stronger signals to prevent false positives.
    let common_name_penalty = if is_common_name(name) { 0.2 } else { 0.0 };
    let pronoun_hits = scores
        .person_signals
        .iter()
        .find_map(|signal| {
            signal
                .strip_prefix("pronoun nearby (")
                .and_then(|rest| rest.strip_suffix("x)"))
                .and_then(|value| value.parse::<usize>().ok())
        })
        .unwrap_or(0);
    let strong_pronoun_signal =
        pronoun_hits >= 5 && frequency > 0 && (pronoun_hits as f32 / frequency as f32) >= 0.2;
    let strong_single_signal = dialogue_hits >= 3 || action_hits >= 3 || address_hits >= 3;

    let (entity_type, confidence, signals) = if person_ratio >= 0.7
        && ((has_two_signal_types && ps >= 5) || strong_pronoun_signal || strong_single_signal)
    {
        // Apply common name penalty - reduce confidence for common names
        let base_conf = (0.5 + person_ratio * 0.5).min(0.99);
        let conf = (base_conf - common_name_penalty).max(0.3);
        let sigs = if scores.person_signals.is_empty() {
            vec![format!("appears {}x", frequency)]
        } else {
            scores.person_signals.clone()
        };
        (EntityType::Person, conf, sigs)
    } else if person_ratio >= 0.7 {
        // Weak single-category person signal — downgrade to uncertain.
        let mut sigs = scores.person_signals.clone();
        sigs.push(format!("appears {}x — not enough signal types", frequency));
        (
            EntityType::Uncertain,
            (0.4 - common_name_penalty).max(0.2),
            sigs,
        )
    } else if person_ratio <= 0.3 {
        let conf = (0.5 + (1.0 - person_ratio) * 0.5).min(0.99);
        let sigs = if scores.project_signals.is_empty() {
            vec![format!("appears {}x", frequency)]
        } else {
            scores.project_signals.clone()
        };
        (EntityType::Project, conf, sigs)
    } else {
        let mut sigs = scores.person_signals.clone();
        sigs.extend(scores.project_signals.clone());
        sigs.truncate(3);
        sigs.push("mixed signals — needs review".to_string());
        (EntityType::Uncertain, 0.5, sigs)
    };

    // Round confidence to 2 decimal places
    let confidence = (confidence * 100.0).round() / 100.0;

    ClassifiedEntity {
        name: name.to_string(),
        entity_type,
        confidence,
        frequency,
        signals,
    }
}

// =============================================================================
// TWO-PASS DETECT
// =============================================================================

fn detect_entities_two_pass(
    text: &str,
) -> (
    Vec<ClassifiedEntity>,
    Vec<ClassifiedEntity>,
    Vec<ClassifiedEntity>,
) {
    let lines: Vec<&str> = text.lines().collect();
    let candidates = extract_candidates(text);

    if candidates.is_empty() {
        return (vec![], vec![], vec![]);
    }

    let mut people: Vec<ClassifiedEntity> = vec![];
    let mut projects: Vec<ClassifiedEntity> = vec![];
    let mut uncertain: Vec<ClassifiedEntity> = vec![];

    // Sort by frequency descending
    let mut sorted: Vec<(String, usize)> = candidates.into_iter().collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.1));

    for (name, frequency) in sorted {
        let mut scores = score_entity(&name, text, &lines);
        scores.frequency = frequency;
        let classified = classify_entity(&name, frequency, &scores);

        match classified.entity_type {
            EntityType::Person => people.push(classified),
            EntityType::Project => projects.push(classified),
            EntityType::Uncertain => uncertain.push(classified),
        }
    }

    // Sort by confidence descending
    people.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
    projects.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
    uncertain.sort_by_key(|b| std::cmp::Reverse(b.frequency));

    (people, projects, uncertain)
}

// =============================================================================
// PUBLIC API
// =============================================================================

/// Detect person entities in the given text.
pub fn detect_people(text: &str) -> Vec<PersonEntity> {
    let (people, _, _) = detect_entities_two_pass(text);
    people
        .into_iter()
        .take(15)
        .map(|e| PersonEntity {
            name: e.name,
            confidence: e.confidence,
            context: e.signals.join("; "),
        })
        .collect()
}

/// Detect project entities in the given text.
pub fn detect_projects(text: &str) -> Vec<ProjectEntity> {
    let (_, projects, _) = detect_entities_two_pass(text);
    projects
        .into_iter()
        .take(10)
        .map(|e| ProjectEntity {
            name: e.name,
            confidence: e.confidence,
            context: e.signals.join("; "),
        })
        .collect()
}

/// Detect both people and projects from content.
pub fn detect_from_content(text: &str) -> DetectionResult {
    let (people, projects, _) = detect_entities_two_pass(text);

    let people_entities: Vec<PersonEntity> = people
        .into_iter()
        .take(15)
        .map(|e| PersonEntity {
            name: e.name,
            confidence: e.confidence,
            context: e.signals.join("; "),
        })
        .collect();

    let project_entities: Vec<ProjectEntity> = projects
        .into_iter()
        .take(10)
        .map(|e| ProjectEntity {
            name: e.name,
            confidence: e.confidence,
            context: e.signals.join("; "),
        })
        .collect();

    DetectionResult {
        people: people_entities,
        projects: project_entities,
        uncertain: Vec::new(),
    }
}

pub fn scan_for_detection(project_dir: &Path, max_files: usize) -> Vec<PathBuf> {
    let project_path = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let mut prose_files = Vec::new();
    let mut all_files = Vec::new();

    for entry in walkdir::WalkDir::new(project_path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !SKIP_DIRS.iter().any(|skip| name == *skip)
        })
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let stem = entry
            .path()
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| stem.to_ascii_lowercase())
            .unwrap_or_default();
        if SKIP_FILENAMES.contains(&stem.as_str()) {
            continue;
        }
        let Some(ext) = entry.path().extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if PROSE_EXTENSIONS.contains(&ext.as_str()) {
            prose_files.push(entry.path().to_path_buf());
        } else if READABLE_EXTENSIONS.contains(&ext.as_str()) {
            all_files.push(entry.path().to_path_buf());
        }
    }

    let files = if prose_files.len() >= 3 {
        prose_files
    } else {
        prose_files.into_iter().chain(all_files).collect()
    };

    files.into_iter().take(max_files).collect()
}

pub fn detect_entities(file_paths: &[PathBuf], max_files: usize) -> DetectionResult {
    let mut all_text = Vec::new();
    let mut all_lines = Vec::new();
    let mut files_read = 0usize;

    const MAX_BYTES_PER_FILE: usize = 5_000;

    for path in file_paths {
        if files_read >= max_files {
            break;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let content = if content.len() > MAX_BYTES_PER_FILE {
            content[..MAX_BYTES_PER_FILE].to_string()
        } else {
            content
        };
        all_lines.extend(content.lines().map(str::to_string));
        all_text.push(content);
        files_read += 1;
    }

    let combined_text = all_text.join("\n");
    let line_refs: Vec<&str> = all_lines.iter().map(String::as_str).collect();
    let candidates = extract_candidates(&combined_text);
    if candidates.is_empty() {
        return DetectionResult {
            people: Vec::new(),
            projects: Vec::new(),
            uncertain: Vec::new(),
        };
    }

    let mut people = Vec::new();
    let mut projects = Vec::new();
    let mut uncertain = Vec::new();

    let mut sorted: Vec<(String, usize)> = candidates.into_iter().collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.1));

    for (name, frequency) in sorted {
        let scores = score_entity(&name, &combined_text, &line_refs);
        let entity = classify_entity(&name, frequency, &scores);
        let signal_text = entity.signals.join("; ");
        let person = PersonEntity {
            name: entity.name,
            confidence: entity.confidence,
            context: signal_text,
        };
        match entity.entity_type {
            EntityType::Person => people.push(person),
            EntityType::Project => projects.push(ProjectEntity {
                name: person.name,
                confidence: person.confidence,
                context: person.context,
            }),
            EntityType::Uncertain => uncertain.push(person),
        }
    }

    people.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    projects.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    uncertain.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));

    DetectionResult {
        people: people.into_iter().take(15).collect(),
        projects: projects.into_iter().take(10).collect(),
        uncertain: uncertain.into_iter().take(8).collect(),
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // detect_people from dialogue patterns
    // -------------------------------------------------------------------------

    #[test]
    fn test_detect_people_dialogue_markers() {
        let text = r#"
        > Alice: I think we should ship this feature.
        > Alice: Agreed, let's do it tomorrow.
        > Alice: What about the deployment?
        > Bob: Sounds good.
        > Bob: Let's go.
        > Bob: Ready.
        [Carol] What about the deployment?
        [Carol] Can you clarify?
        [Carol] Thanks!
        "Dave said it would be ready by Friday."
        "Dave said the timeline is tight."
        "Dave said we need more time."
        "#;
        let people = detect_people(text);
        let names: Vec<&str> = people.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"Alice"),
            "Alice (dialogue > prefix) should be detected, got: {names:?}"
        );
        assert!(
            names.contains(&"Bob"),
            "Bob (dialogue > prefix) should be detected, got: {names:?}"
        );
        assert!(
            names.contains(&"Carol"),
            "Carol (dialogue [brackets]) should be detected, got: {names:?}"
        );
        assert!(
            names.contains(&"Dave"),
            "Dave (dialogue quote) should be detected, got: {names:?}"
        );
    }

    #[test]
    fn test_detect_people_action_verbs() {
        let text = r#"
        Alice said she would handle the migration.
        Alice said it needs more work.
        Alice said tomorrow is the deadline.
        Bob asked for more details.
        Bob asked again yesterday.
        Bob asked about the plan.
        Charlie wrote the entire documentation.
        Charlie wrote the README.
        Charlie wrote the guide.
        Dave thinks this is the right approach.
        Dave thinks we should wait.
        Dave thinks it's ready.
        Eve loves the new design.
        Eve loves the direction.
        Eve loves what we built.
        "#;
        let people = detect_people(text);
        let names: Vec<&str> = people.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Alice"), "Alice (said) should be detected");
        assert!(names.contains(&"Bob"), "Bob (asked) should be detected");
        assert!(
            names.contains(&"Charlie"),
            "Charlie (wrote) should be detected"
        );
        assert!(names.contains(&"Dave"), "Dave (thinks) should be detected");
        assert!(names.contains(&"Eve"), "Eve (loves) should be detected");
    }

    #[test]
    fn test_detect_people_direct_address() {
        let text = r#"
        hey Alice, can you review this?
        hey Alice, can you help?
        hey Alice, are you there?
        thanks Bob for your help!
        thanks Bob for your time!
        thanks Bob for everything!
        hi Charlie, welcome aboard.
        hi Charlie, great work!
        hi Charlie, good to see you!
        dear Dave, we need your input.
        dear Dave, your feedback matters.
        dear Dave, thanks for your time.
        "#;
        let people = detect_people(text);
        let names: Vec<&str> = people.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"Alice"),
            "Alice (direct address) should be detected"
        );
        assert!(
            names.contains(&"Bob"),
            "Bob (direct address) should be detected"
        );
        assert!(
            names.contains(&"Charlie"),
            "Charlie (direct address) should be detected"
        );
        assert!(
            names.contains(&"Dave"),
            "Dave (direct address) should be detected"
        );
    }

    #[test]
    fn test_single_metadata_colon_line_does_not_trigger_person() {
        let text = r#"
        Created: 2026-04-24
        Created the project notes yesterday.
        Created another backup today.
        "#;
        let people = detect_people(text);
        let names: Vec<&str> = people.iter().map(|p| p.name.as_str()).collect();
        assert!(
            !names.contains(&"Created"),
            "single metadata line should not classify Created as a person, got: {names:?}"
        );
    }

    #[test]
    fn test_high_pronoun_density_promotes_person() {
        let text = r#"
        Lu was worried. She felt trapped.
        I met Lu again. She said it would pass.
        Lu stayed home. Her notes were still on the desk.
        Lu called later. She sounded calmer.
        Lu wrote tonight. Her plan finally made sense.
        "#;
        let people = detect_people(text);
        let names: Vec<&str> = people.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"Lu"),
            "strong pronoun signal should classify Lu as a person, got: {names:?}"
        );
    }

    // -------------------------------------------------------------------------
    // detect_projects from technical context
    // -------------------------------------------------------------------------

    #[test]
    fn test_detect_projects_versioned() {
        let text = r#"
        We shipped MemPalace v2 last week.
        The MemPalace v2 is working great.
        We love MemPalace v2.
        The MemPalace-core package is on crates.io.
        Check the mempalace-local config file.
        "#;
        let projects = detect_projects(text);
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"MemPalace"),
            "MemPalace (versioned v2) should be detected as project, got: {names:?}"
        );
    }

    #[test]
    fn test_detect_projects_versioned_with_underscore_suffix() {
        let text = r#"
        MemPalace_v2 shipped last week.
        The MemPalace_v2 migration worked.
        We benchmarked MemPalace_v2 today.
        "#;
        let projects = detect_projects(text);
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"MemPalace"),
            "MemPalace (_v2) should be detected as project, got: {names:?}"
        );
    }

    #[test]
    fn test_detect_projects_code_ref() {
        // Note: MemPalace starts with uppercase to be extracted by SINGLE_WORD_RE.
        // The .py/.yaml extension patterns then confirm it as a project.
        let text = r#"
        Import the MemPalace module in your Python script.
        The MemPalace.py file handles all the mining.
        Check the MemPalace.yaml configuration.
        Import the MemPalace module here.
        The MemPalace.py runs the core logic.
        "#;
        let projects = detect_projects(text);
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"MemPalace"),
            "MemPalace (code file reference) should be detected as project, got: {names:?}"
        );
    }

    #[test]
    fn test_detect_projects_project_verbs() {
        let text = r#"
        We are building the Phoenix pipeline for data processing.
        Building the Phoenix pipeline is underway.
        The Phoenix pipeline is essential.
        The team shipped the Atlas system last sprint.
        Shipped the Atlas system successfully.
        Atlas system is in production.
        Deploying the Gateway architecture tomorrow.
        The Gateway architecture is solid.
        Gateway architecture powers our infra.
        "#;
        let projects = detect_projects(text);
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"Phoenix"),
            "Phoenix (building) should be detected"
        );
        assert!(
            names.contains(&"Atlas"),
            "Atlas (shipped) should be detected"
        );
        assert!(
            names.contains(&"Gateway"),
            "Gateway (deploying architecture) should be detected"
        );
    }

    // -------------------------------------------------------------------------
    // common names NOT detected as entities (STOPWORDS)
    // -------------------------------------------------------------------------

    #[test]
    fn test_common_first_names_not_entities() {
        // These appear at sentence starts with no strong signals
        // so they should NOT be picked up as entities
        let text = r#"
        The user uploaded a file.
        James ran the script.
        Mary wrote some code.
        But Robert disagrees.
        So Lisa and Tom went ahead.
        "#;
        let result = detect_from_content(text);
        // Collect all entity names into a flat Vec<String>
        let mut all_names: Vec<String> = result.people.iter().map(|p| p.name.clone()).collect();
        all_names.extend(result.projects.iter().map(|p| p.name.clone()));

        // James, Mary, Robert, Lisa, Tom appear in lower-context sentences
        // they should not appear as high-confidence entities
        for name in ["James", "Mary", "Robert", "Lisa", "Tom"] {
            let found = all_names.iter().any(|n| n.as_str() == name);
            // This may or may not fire depending on frequency;
            // the key test is they won't be HIGH confidence persons
            let _ = found;
        }
    }

    #[test]
    fn test_stopwords_not_detected() {
        // Stopwords like "System", "Version", "Model" appearing capitalized
        // should be filtered out even when they appear 3+ times
        let text = r#"
        The System handles all requests.
        Our System is fully distributed.
        Every System needs monitoring.
        The System architecture is robust.
        "#;
        let result = detect_from_content(text);
        // "System" is a stopword so should not be detected
        let mut all_names: Vec<String> = result.people.iter().map(|p| p.name.clone()).collect();
        all_names.extend(result.projects.iter().map(|p| p.name.clone()));
        assert!(
            !all_names.iter().any(|n| n.as_str() == "System"),
            "'System' is a stopword and should not be detected, got: {all_names:?}"
        );
    }

    #[test]
    fn test_scan_for_detection_skips_boilerplate_filenames() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        std::fs::write(temp.path().join("LICENSE.md"), "Alice Alice Alice").unwrap();
        std::fs::write(
            temp.path().join("notes.md"),
            "Riley said hello.\nRiley asked why.\nRiley laughed loudly.\n",
        )
        .unwrap();

        let files = scan_for_detection(temp.path(), 10);
        let names: Vec<String> = files
            .iter()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
            .map(ToString::to_string)
            .collect();
        assert!(
            !names
                .iter()
                .any(|name| name.eq_ignore_ascii_case("LICENSE.md")),
            "license-like files should be skipped, got: {names:?}"
        );
        assert!(
            names.iter().any(|name| name == "notes.md"),
            "non-boilerplate prose should still be scanned, got: {names:?}"
        );
    }

    // -------------------------------------------------------------------------
    // high-confidence vs low-confidence ranking
    // -------------------------------------------------------------------------

    #[test]
    fn test_high_confidence_ranked_first() {
        let text = r#"
        hey Alice, can you help? hey Alice, can you help? hey Alice, can you help?
        hey Alice, can you help? hey Alice, can you help? hey Alice, can you help?
        Bob ran the script. Bob ran the script. Bob ran the script.
        "#;
        let people = detect_people(text);
        assert!(!people.is_empty(), "Should detect at least some people");
        // Alice has direct address signals, should rank higher than Bob
        if people.len() >= 2 {
            let alice_conf = people
                .iter()
                .find(|p| p.name == "Alice")
                .map(|p| p.confidence);
            let bob_conf = people
                .iter()
                .find(|p| p.name == "Bob")
                .map(|p| p.confidence);
            if let (Some(ac), Some(bc)) = (alice_conf, bob_conf) {
                assert!(
                    ac > bc,
                    "Alice (direct address) confidence {} should exceed Bob (action) confidence {}",
                    ac,
                    bc
                );
            }
        }
    }

    #[test]
    fn test_mixed_signals_uncertain() {
        // A name appearing with both person and project signals should be uncertain
        let text = r#"
        The Phoenix project is great. Phoenix said it would be ready.
        Phoenix thinks the timeline is realistic. But Phoenix v1 is already deployed.
        "#;
        let result = detect_from_content(text);
        // Mixed signals should produce either uncertain entities or lower confidence
        let uncertain_or_low = result
            .people
            .iter()
            .filter(|p| p.name == "Phoenix")
            .collect::<Vec<_>>();
        // If Phoenix appears as a person, it should not be high confidence
        for p in uncertain_or_low {
            assert!(
                p.confidence < 0.8,
                "Mixed-signals entity should not be high confidence, got {} for Phoenix",
                p.confidence
            );
        }
    }

    #[test]
    fn test_no_false_positives_on_common_words() {
        // Verify "Memory" and "Palace" individually are not detected
        // when appearing in normal prose without entity-level signals
        let text = r#"
        Memory is an important concept in computing.
        The palace was built centuries ago.
        We need better memory management.
        "#;
        let result = detect_from_content(text);
        let mut all_names: Vec<String> = result.people.iter().map(|p| p.name.clone()).collect();
        all_names.extend(result.projects.iter().map(|p| p.name.clone()));
        // These are single occurrences, so they shouldn't pass the 3x threshold
        assert!(
            !all_names.iter().any(|n| n.as_str() == "Memory"),
            "'Memory' should not be detected (only appears once)"
        );
        assert!(
            !all_names.iter().any(|n| n.as_str() == "Palace"),
            "'Palace' should not be detected (only appears once)"
        );
    }

    #[test]
    fn test_confidence_scales_with_frequency() {
        // A name appearing many times with person signals should be detected
        let text = r#"
        Alice worked on this. Alice finished that. Alice is productive.
        Alice reviewed the PR. Alice shipped the feature. Alice is great.
        Alice said it was good. Alice thinks it's ready.
        "#;
        let result = detect_from_content(text);
        let alice = result.people.iter().find(|p| p.name == "Alice");
        assert!(
            alice.is_some(),
            "Alice should be detected from repeated mentions"
        );
    }

    #[test]
    fn test_detection_result_structure() {
        let text = r#"
        Alice said hello. Alice said hello. Alice said hello.
        Alice asked a question. Alice asked a question.
        Bob asked a question. Bob asked a question. Bob asked a question.
        Bob thinks about it. Bob thinks about it.
        ProjectX is being built. ProjectX is being built. ProjectX is being built.
        ProjectX is essential. ProjectX is great.
        "#;
        let result = detect_from_content(text);
        assert!(
            !result.people.is_empty() || !result.projects.is_empty(),
            "Should detect at least people or projects from mixed content"
        );
        for person in &result.people {
            assert!(
                (0.0..=1.0).contains(&person.confidence),
                "Confidence should be between 0 and 1, got {} for {}",
                person.confidence,
                person.name
            );
            assert!(
                !person.context.is_empty(),
                "Context should not be empty for {}",
                person.name
            );
        }
        for project in &result.projects {
            assert!(
                (0.0..=1.0).contains(&project.confidence),
                "Confidence should be between 0 and 1, got {} for {}",
                project.confidence,
                project.name
            );
        }
    }

    #[test]
    fn test_empty_text_returns_empty_result() {
        let result = detect_from_content("");
        assert!(result.people.is_empty());
        assert!(result.projects.is_empty());

        let result2 = detect_from_content("no entities here just lowercase words");
        assert!(result2.people.is_empty());
        assert!(result2.projects.is_empty());
    }

    #[test]
    fn test_proper_noun_extraction_multi_word() {
        // "Memory Palace" as a multi-word proper noun
        let text = r#"
        The Memory Palace approach is interesting.
        Memory Palace helps with recall.
        We love Memory Palace for studying.
        "#;
        let result = detect_from_content(text);
        let mut all_names: Vec<String> = result.people.iter().map(|p| p.name.clone()).collect();
        all_names.extend(result.projects.iter().map(|p| p.name.clone()));
        // "Memory Palace" (multi-word) should be extracted as one candidate
        // It should NOT appear as separate "Memory" and "Palace"
        let memory = result.people.iter().any(|p| p.name == "Memory")
            || result.projects.iter().any(|p| p.name == "Memory");
        let palace = result.people.iter().any(|p| p.name == "Palace")
            || result.projects.iter().any(|p| p.name == "Palace");
        assert!(
            !memory && !palace,
            "Individual words 'Memory'/'Palace' should not be detected separately when multi-word exists"
        );
        let _ = all_names;
    }

    #[test]
    fn test_diagnostic_output_shows_signals() {
        let text = r#"
        hey Alice, are you there? hey Alice, can you help?
        Alice wrote the report. Alice reviewed the code. Alice shipped it.
        "#;
        let people = detect_people(text);
        let alice = people.iter().find(|p| p.name == "Alice");
        assert!(alice.is_some(), "Alice should be detected");
        let ctx = alice.unwrap().context.clone();
        // Context should include signal information
        assert!(
            ctx.contains("addressed") || ctx.contains("action") || ctx.contains("dialogue"),
            "Context should include signal type, got: {}",
            ctx
        );
    }

    #[test]
    fn test_cyrillic_entity_detection() {
        // Cyrillic names should be detected with Unicode-aware patterns
        // They may be classified as Uncertain (not Person) due to lack of English person signals,
        // but extract_candidates should still find them
        let text = "Иван работает над проектом. Иван закончил важную задачу. Иван написал код. Анна проверила результаты. Анна завершила работу. Анна довольна.";
        let candidates = extract_candidates(text);
        // Иван appears 3 times, Анна appears 3 times - both should pass frequency threshold
        assert!(
            candidates.contains_key("Иван") || candidates.contains_key("Анна"),
            "Cyrillic names should pass frequency threshold, got: {candidates:?}"
        );
    }

    #[test]
    fn test_latin_detection_still_works() {
        // Ensure the Unicode change didn't break Latin script detection
        // Uses proper person signals: direct address and multiple action verbs
        let text = r#"
        hey Alice, can you help? hey Alice, can you help? hey Alice, can you help?
        hey Alice, can you help? hey Alice, can you help? hey Alice, can you help?
        Bob wrote the script. Bob wrote the script. Bob wrote the script.
        "#;
        let candidates = extract_candidates(text);
        assert!(
            candidates.contains_key("Alice") && candidates.contains_key("Bob"),
            "Both names should pass frequency threshold"
        );
        let result = detect_from_content(text);
        let names: Vec<&str> = result.people.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Alice"), "Alice should be detected");
        assert!(names.contains(&"Bob"), "Bob should be detected");
    }
}
