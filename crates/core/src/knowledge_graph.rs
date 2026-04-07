pub struct KnowledgeGraph {
    db_path: std::path::PathBuf,
}

impl KnowledgeGraph {
    pub fn open(db_path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self {
            db_path: db_path.to_path_buf(),
        })
    }

    pub fn add_triple(
        &mut self,
        subject: &str,
        predicate: &str,
        object: &str,
        valid_from: Option<&str>,
    ) -> anyhow::Result<()> {
        let _ = (subject, predicate, object, valid_from);
        Ok(())
    }

    pub fn query_entity(&self, entity: &str, as_of: Option<&str>) -> anyhow::Result<Vec<Triple>> {
        let _ = (entity, as_of);
        Ok(vec![])
    }

    pub fn timeline(&self, entity: &str) -> anyhow::Result<Vec<Triple>> {
        let _ = entity;
        Ok(vec![])
    }

    pub fn invalidate(
        &mut self,
        subject: &str,
        predicate: &str,
        object: &str,
        ended: Option<&str>,
    ) -> anyhow::Result<()> {
        let _ = (subject, predicate, object, ended);
        Ok(())
    }

    pub fn stats(&self) -> anyhow::Result<KgStats> {
        Ok(KgStats {
            total_triples: 0,
            total_entities: 0,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Triple {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
}

#[derive(Debug)]
pub struct KgStats {
    pub total_triples: usize,
    pub total_entities: usize,
}
