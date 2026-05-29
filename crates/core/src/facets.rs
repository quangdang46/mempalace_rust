use crate::types::Facet;
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

pub struct FacetStore {
    conn: Connection,
}

impl FacetStore {
    pub fn new(conn: Connection) -> Result<Self> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS facets (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                value TEXT NOT NULL,
                target_id TEXT NOT NULL,
                target_type TEXT NOT NULL,
                dimension TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn tag(&self, target_id: &str, target_type: &str, dimension: &str, value: &str) -> Result<Facet> {
        let valid_types = ["action", "memory", "observation"];
        if !valid_types.contains(&target_type) {
            return Err(anyhow::anyhow!("targetType must be one of: action, memory, observation"));
        }

        let existing = self.list()?;
        for f in &existing {
            if f.target_id == target_id && f.dimension == dimension && f.value == value {
                return Ok(f.clone());
            }
        }

        let now = Utc::now();
        let facet = Facet {
            id: format!("fct-{}", uuid::Uuid::new_v4().to_string()[..8].to_string()),
            name: dimension.to_string(),
            value: value.trim().to_string(),
            target_id: target_id.to_string(),
            target_type: target_type.to_string(),
            dimension: dimension.trim().to_string(),
            created_at: now,
        };

        self.conn.execute(
            "INSERT INTO facets (id, name, value, target_id, target_type, dimension, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![facet.id, facet.name, facet.value, facet.target_id, facet.target_type, facet.dimension, facet.created_at.to_rfc3339()],
        )?;
        Ok(facet)
    }

    pub fn untag(&self, target_id: &str, dimension: &str, value: Option<&str>) -> Result<usize> {
        let all = self.list()?;
        let matches: Vec<&Facet> = all.iter().filter(|f| {
            f.target_id == target_id && f.dimension == dimension &&
            value.map_or(true, |v| f.value == v)
        }).collect();

        for f in &matches {
            self.conn.execute("DELETE FROM facets WHERE id = ?1", params![f.id])?;
        }
        Ok(matches.len())
    }

    pub fn query(&self, match_all: Option<&[String]>, match_any: Option<&[String]>, target_type: Option<&str>, limit: usize) -> Result<Vec<(String, String, Vec<String>)>> {
        if match_all.map_or(true, |a| a.is_empty()) && match_any.map_or(true, |a| a.is_empty()) {
            return Err(anyhow::anyhow!("at least one of matchAll or matchAny is required"));
        }

        let all = self.list()?;
        let filtered: Vec<&Facet> = match target_type {
            Some(t) => all.iter().filter(|f| f.target_type == t).collect(),
            None => all.iter().collect(),
        };

        let mut target_map: HashMap<String, (String, HashSet<String>)> = HashMap::new();
        for f in filtered {
            let key = format!("{}:{}", f.dimension, f.value);
            let entry = target_map.entry(f.target_id.clone()).or_insert_with(|| (f.target_type.clone(), HashSet::new()));
            entry.1.insert(key);
        }

        let mut results: Vec<(String, String, Vec<String>)> = Vec::new();
        for (target_id, (ttype, facet_keys)) in target_map {
            let mut matched: Vec<String> = Vec::new();

            if let Some(all_keys) = match_all {
                if !all_keys.iter().all(|k| facet_keys.contains(k.as_str())) { continue; }
                for k in all_keys {
                    if !matched.contains(k) { matched.push(k.clone()); }
                }
            }

            if let Some(any_keys) = match_any {
                let any_present: Vec<String> = any_keys.iter().filter(|k| facet_keys.contains(k.as_str())).cloned().collect();
                if any_present.is_empty() { continue; }
                for k in any_present {
                    if !matched.contains(&k) { matched.push(k); }
                }
            }

            results.push((target_id, ttype, matched));
        }

        results.truncate(limit);
        Ok(results)
    }

    pub fn get(&self, target_id: &str) -> Result<Vec<(String, Vec<String>)>> {
        let all = self.list()?;
        let target_facets: Vec<&Facet> = all.iter().filter(|f| f.target_id == target_id).collect();

        let mut dim_map: HashMap<String, Vec<String>> = HashMap::new();
        for f in target_facets {
            dim_map.entry(f.dimension.clone()).or_default().push(f.value.clone());
        }

        Ok(dim_map.into_iter().collect())
    }

    pub fn stats(&self, target_type: Option<&str>) -> Result<(Vec<(String, Vec<(String, usize)>)>, usize)> {
        let all = self.list()?;
        let filtered: Vec<Facet> = match target_type {
            Some(t) => all.into_iter().filter(|f| f.target_type == t).collect(),
            None => all,
        };
        let total = filtered.len();

        let mut dim_map: HashMap<String, HashMap<String, usize>> = HashMap::new();
        for f in filtered {
            dim_map.entry(f.dimension).or_default().entry(f.value).and_modify(|c| *c += 1).or_insert(1);
        }

        let dimensions: Vec<(String, Vec<(String, usize)>)> = dim_map
            .into_iter()
            .map(|(dim, values)| (dim, values.into_iter().collect()))
            .collect();

        Ok((dimensions, total))
    }

    pub fn dimensions(&self) -> Result<Vec<(String, usize)>> {
        let all = self.list()?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for f in all {
            *counts.entry(f.dimension).or_insert(0) += 1;
        }
        Ok(counts.into_iter().collect())
    }

    pub fn list(&self) -> Result<Vec<Facet>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, value, target_id, target_type, dimension, created_at FROM facets"
        )?;
        let rows: Vec<rusqlite::Result<Facet>> = stmt.query_map([], |row| {
            Ok(Facet {
                id: row.get(0)?,
                name: row.get(1)?,
                value: row.get(2)?,
                target_id: row.get(3)?,
                target_type: row.get(4)?,
                dimension: row.get(5)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?).unwrap().with_timezone(&Utc),
            })
        })?.collect();
        rows.into_iter().map(|r| r.map_err(|e| anyhow::anyhow!(e))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> FacetStore {
        FacetStore::new(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn test_tag_creates_facet() {
        let store = test_store();
        let facet = store.tag("act-1", "action", "priority", "high").unwrap();
        assert!(facet.id.starts_with("fct-"));
        assert_eq!(facet.dimension, "priority");
        assert_eq!(facet.value, "high");
    }

    #[test]
    fn test_tag_deduplicates() {
        let store = test_store();
        let f1 = store.tag("act-1", "action", "priority", "high").unwrap();
        let f2 = store.tag("act-1", "action", "priority", "high").unwrap();
        assert_eq!(f1.id, f2.id);
    }

    #[test]
    fn test_untag_removes_facets() {
        let store = test_store();
        store.tag("act-1", "action", "priority", "high").unwrap();
        store.tag("act-1", "action", "priority", "low").unwrap();
        let removed = store.untag("act-1", "priority", Some("high")).unwrap();
        assert_eq!(removed, 1);
        let remaining = store.get("act-1").unwrap();
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn test_query_match_all() {
        let store = test_store();
        store.tag("act-1", "action", "priority", "high").unwrap();
        store.tag("act-1", "action", "status", "urgent").unwrap();
        store.tag("act-2", "action", "priority", "high").unwrap();
        let results = store.query(
            Some(&["priority:high".to_string(), "status:urgent".to_string()]),
            None, None, 10
        ).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "act-1");
    }

    #[test]
    fn test_get_returns_dimensions() {
        let store = test_store();
        store.tag("act-1", "action", "priority", "high").unwrap();
        store.tag("act-1", "action", "priority", "low").unwrap();
        let dims = store.get("act-1").unwrap();
        assert_eq!(dims.len(), 1);
        assert_eq!(dims[0].0, "priority");
        assert_eq!(dims[0].1.len(), 2);
    }

    #[test]
    fn test_stats_returns_counts() {
        let store = test_store();
        store.tag("act-1", "action", "priority", "high").unwrap();
        store.tag("act-2", "action", "priority", "high").unwrap();
        store.tag("act-3", "action", "priority", "low").unwrap();
        let (dims, total) = store.stats(None).unwrap();
        assert_eq!(total, 3);
        assert_eq!(dims.len(), 1);
    }

    #[test]
    fn test_dimensions_returns_counts() {
        let store = test_store();
        store.tag("act-1", "action", "priority", "high").unwrap();
        store.tag("act-1", "action", "status", "urgent").unwrap();
        let dims = store.dimensions().unwrap();
        assert_eq!(dims.len(), 2);
    }

    #[test]
    fn test_untag_all_values() {
        let store = test_store();
        store.tag("act-1", "action", "priority", "high").unwrap();
        store.tag("act-1", "action", "priority", "low").unwrap();
        let removed = store.untag("act-1", "priority", None).unwrap();
        assert_eq!(removed, 2);
    }
}
