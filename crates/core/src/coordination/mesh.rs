use crate::types::{AuditEntry, MeshPeer, SyncFilter};
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

const DEFAULT_SHARED_SCOPES: &[&str] = &[
    "memories",
    "actions",
    "semantic",
    "procedural",
    "relations",
    "graph:nodes",
    "graph:edges",
];

pub struct Mesh {
    peers: HashMap<String, MeshPeer>,
    audit_log: Vec<AuditEntry>,
    auth_token: Option<String>,
}

impl Mesh {
    pub fn new(auth_token: Option<String>) -> Self {
        Self {
            peers: HashMap::new(),
            audit_log: Vec::new(),
            auth_token,
        }
    }

    pub fn register(
        &mut self,
        url: &str,
        name: &str,
        shared_scopes: Option<Vec<String>>,
        sync_filter: Option<SyncFilter>,
    ) -> Result<MeshPeer> {
        Self::validate_url(url)?;

        if self.peers.values().any(|p| p.url == url) {
            return Err(anyhow::anyhow!("peer already registered"));
        }

        let peer = MeshPeer {
            id: format!("peer-{}", uuid::Uuid::new_v4()),
            url: url.to_string(),
            name: name.to_string(),
            status: "disconnected".to_string(),
            shared_scopes: shared_scopes.unwrap_or_else(|| {
                DEFAULT_SHARED_SCOPES
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            }),
            sync_filter,
            last_sync_at: None,
        };

        self.record_audit(
            "mesh_sync",
            "mem::mesh-register",
            vec![peer.id.clone()],
            serde_json::json!({
                "action": "mesh.register",
                "peerId": peer.id,
                "name": peer.name,
                "url": peer.url,
                "sharedScopes": peer.shared_scopes,
            }),
        );

        self.peers.insert(peer.id.clone(), peer.clone());
        Ok(peer)
    }

    pub fn list_peers(&self) -> Vec<&MeshPeer> {
        self.peers.values().collect()
    }

    pub fn remove_peer(&mut self, peer_id: &str) -> Result<()> {
        self.peers
            .remove(peer_id)
            .ok_or_else(|| anyhow::anyhow!("peer not found"))?;
        self.record_audit(
            "mesh_sync",
            "mem::mesh-remove",
            vec![peer_id.to_string()],
            serde_json::json!({"action": "mesh.remove"}),
        );
        Ok(())
    }

    pub fn sync_requires_auth(&self) -> bool {
        self.auth_token.is_none()
    }

    pub fn audit_log(&self) -> &[AuditEntry] {
        &self.audit_log
    }

    fn validate_url(url: &str) -> Result<()> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(anyhow::anyhow!("URL blocked: only http/https allowed"));
        }

        let without_scheme = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .unwrap_or(url);

        let host_port = without_scheme.split('/').next().unwrap_or("");
        let host = host_port.split(':').next().unwrap_or("").to_lowercase();

        if host.is_empty() || host == "localhost" {
            return Err(anyhow::anyhow!(
                "URL blocked: private/local address not allowed"
            ));
        }

        if Self::is_private_ip(&host) {
            return Err(anyhow::anyhow!(
                "URL blocked: private/local address not allowed"
            ));
        }

        Ok(())
    }

    fn is_private_ip(host: &str) -> bool {
        if host == "127.0.0.1" || host == "::1" || host == "0.0.0.0" {
            return true;
        }
        if host.starts_with("10.") || host.starts_with("192.168.") {
            return true;
        }
        if let Some(rest) = host.strip_prefix("172.") {
            if let Ok(first) = rest.split('.').next().unwrap_or("").parse::<u8>() {
                if (16..=31).contains(&first) {
                    return true;
                }
            }
        }
        if host == "169.254.169.254" {
            return true;
        }
        if host.starts_with("fe80:") || host.starts_with("fc00:") || host.starts_with("fd") {
            return true;
        }
        if let Some(v4) = host.strip_prefix("::ffff:") {
            return Self::is_private_ip(v4);
        }
        false
    }

    fn record_audit(
        &mut self,
        operation: &str,
        function_id: &str,
        target_ids: Vec<String>,
        details: serde_json::Value,
    ) {
        let entry = AuditEntry {
            id: format!("aud-{}", uuid::Uuid::new_v4()),
            timestamp: Utc::now(),
            operation: operation.to_string(),
            user_id: None,
            function_id: function_id.to_string(),
            target_ids,
            details: details
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, v))
                .collect(),
            quality_score: None,
        };
        self.audit_log.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_peer() {
        let mut mesh = Mesh::new(None);
        let peer = mesh
            .register("https://peer.example.com/mempalace", "peer-1", None, None)
            .unwrap();

        assert_eq!(peer.name, "peer-1");
        assert_eq!(peer.status, "disconnected");
        assert!(peer.shared_scopes.contains(&"memories".to_string()));
        assert_eq!(mesh.list_peers().len(), 1);
    }

    #[test]
    fn test_register_duplicate_url() {
        let mut mesh = Mesh::new(None);
        mesh.register("https://peer.example.com/mempalace", "peer-1", None, None)
            .unwrap();
        let result = mesh.register("https://peer.example.com/mempalace", "peer-2", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_register_blocks_localhost() {
        let mut mesh = Mesh::new(None);
        let result = mesh.register("http://localhost:8080/mempalace", "peer-1", None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked"));
    }

    #[test]
    fn test_register_blocks_private_ip_10x() {
        let mut mesh = Mesh::new(None);
        let result = mesh.register("http://10.0.0.1:8080/mempalace", "peer-1", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_register_blocks_private_ip_192x() {
        let mut mesh = Mesh::new(None);
        let result = mesh.register("http://192.168.1.1:8080/mempalace", "peer-1", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_register_blocks_private_ip_172x() {
        let mut mesh = Mesh::new(None);
        let result = mesh.register("http://172.16.0.1:8080/mempalace", "peer-1", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_register_blocks_172_31x() {
        let mut mesh = Mesh::new(None);
        let result = mesh.register(
            "http://172.31.255.255:8080/mempalace",
            "peer-1",
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_register_allows_172_15x() {
        let mut mesh = Mesh::new(None);
        let result = mesh.register("http://172.15.0.1:8080/mempalace", "peer-1", None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_register_blocks_169_254() {
        let mut mesh = Mesh::new(None);
        let result = mesh.register("http://169.254.169.254/mempalace", "peer-1", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_peer() {
        let mut mesh = Mesh::new(None);
        let peer = mesh
            .register("https://peer.example.com/mempalace", "peer-1", None, None)
            .unwrap();
        mesh.remove_peer(&peer.id).unwrap();
        assert!(mesh.list_peers().is_empty());
    }

    #[test]
    fn test_remove_nonexistent_peer() {
        let mut mesh = Mesh::new(None);
        let result = mesh.remove_peer("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_sync_requires_auth() {
        let mesh_no_auth = Mesh::new(None);
        assert!(mesh_no_auth.sync_requires_auth());

        let mesh_with_auth = Mesh::new(Some("secret-token".to_string()));
        assert!(!mesh_with_auth.sync_requires_auth());
    }

    #[test]
    fn test_register_records_audit() {
        let mut mesh = Mesh::new(None);
        mesh.register("https://peer.example.com/mempalace", "peer-1", None, None)
            .unwrap();
        assert_eq!(mesh.audit_log().len(), 1);
        assert_eq!(mesh.audit_log()[0].function_id, "mem::mesh-register");
    }

    #[test]
    fn test_custom_shared_scopes() {
        let mut mesh = Mesh::new(None);
        let peer = mesh
            .register(
                "https://peer.example.com/mempalace",
                "peer-1",
                Some(vec!["memories".to_string(), "actions".to_string()]),
                None,
            )
            .unwrap();
        assert_eq!(peer.shared_scopes.len(), 2);
    }

    #[test]
    fn test_register_with_sync_filter() {
        let mut mesh = Mesh::new(None);
        let peer = mesh
            .register(
                "https://peer.example.com/mempalace",
                "peer-1",
                None,
                Some(SyncFilter {
                    project: Some("my-project".to_string()),
                }),
            )
            .unwrap();
        assert_eq!(
            peer.sync_filter.as_ref().unwrap().project.as_deref(),
            Some("my-project")
        );
    }
}
