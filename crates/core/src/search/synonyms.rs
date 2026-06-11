//! Synonym expansion for search queries.
//!
//! 72 hardcoded synonym groups used to expand queries at index/search time.
//! BM25 weight for synonym-matched terms: 0.7 (per mempalace/src/state/search-index.ts:98).

use std::collections::HashSet;

/// BM25 weight applied to documents matched only via synonyms (vs direct term match).
pub const SYNONYM_BM25_WEIGHT: f32 = 0.7;

/// All 46 synonym groups. Each inner slice contains words treated as interchangeable.
pub const SYNONYM_GROUPS: &[&[&str]] = &[
    &["auth", "authentication", "authn", "authenticating"],
    &["authz", "authorization", "authorizing"],
    &["k8s", "kubernetes", "kube"],
    &["pg", "postgres", "postgresql"],
    &["js", "javascript"],
    &["ts", "typescript"],
    &["py", "python"],
    &["rs", "rust"],
    &["go", "golang"],
    &["ml", "machinelearning", "machine-learning"],
    &["ai", "artificialintelligence"],
    &["nlp", "naturallanguageprocessing"],
    &["db", "database", "datastore"],
    &["env", "environment", "envvar"],
    &["config", "configuration", "cfg", "conf"],
    &["fn", "function", "func", "method"],
    &["var", "variable", "val"],
    &["param", "parameter", "arg", "argument"],
    &["err", "error", "exception"],
    &["msg", "message", "messaging"],
    &["req", "request"],
    &["res", "response", "resp"],
    &["svc", "service"],
    &["sys", "system"],
    &["lib", "library"],
    &["pkg", "package"],
    &["repo", "repository"],
    &["ci", "continuousintegration", "continuous-integration"],
    &[
        "cd",
        "continuousdeployment",
        "continuousdelivery",
        "continuous-deployment",
    ],
    &["ui", "userinterface"],
    &["ux", "userexperience"],
    &[
        "api",
        "applicationprogramminginterface",
        "endpoint",
        "endpoints",
    ],
    &["cli", "commandline", "command-line-interface"],
    &["gui", "graphicaluserinterface"],
    &["vm", "virtualmachine"],
    &["ide", "integrateddevelopmentenvironment"],
    &["os", "operatingsystem"],
    &["fs", "filesystem", "file-system"],
    &["ssr", "serversiderendering", "server-side-rendering"],
    &["spa", "singlepageapplication", "single-page-application"],
    &["ssg", "staticstegenerator", "static-site-generator"],
    &["http", "hypertexttransferprotocol"],
    &["tcp", "transmissioncontrolprotocol"],
    &["udp", "userdatagramprotocol"],
    &["dns", "domainnamesystem", "domain-name-system"],
    &["tls", "transportlayersecurity", "ssl"],
    &["ssh", "secureshell"],
    &["crud", "createreadupdatedelete"],
    &[
        "perf",
        "performance",
        "latency",
        "throughput",
        "slow",
        "bottleneck",
    ],
    &[
        "optim",
        "optimization",
        "optimizing",
        "optimise",
        "query-optimization",
    ],
    &["deps", "dependencies", "dependency"],
    &["impl", "implementation", "implementing"],
    &["test", "testing", "tests"],
    &["doc", "documentation", "docs"],
    &["infra", "infrastructure"],
    &["deploy", "deployment", "deploying"],
    &["cache", "caching", "cached"],
    &["log", "logging", "logs"],
    &["monitor", "monitoring"],
    &["observe", "observability"],
    &["sec", "security", "secure"],
    &["validate", "validation", "validating"],
    &["migrate", "migration", "migrations"],
    &["debug", "debugging"],
    &["container", "containerization", "docker"],
    &["crash", "crashloop", "crashloopbackoff"],
    &["webhook", "webhooks", "callback"],
    &["middleware", "mw"],
    &["paginate", "pagination"],
    &["serialize", "serialization"],
    &["encrypt", "encryption"],
    &["hash", "hashing"],

    // ── Preference / opinion ──
    &["prefer", "preference", "prefers", "preferred", "rather", "would-rather"],
    &["like", "likes", "liked", "enjoy", "enjoys", "love", "loves"],
    &["want", "wants", "wanted", "wish", "wishes", "would-like"],
    &["think", "thinks", "thought", "believe", "believes", "feel", "feels"],
    &["choose", "chooses", "chose", "choice", "pick", "picks", "select", "selects"],
    &["opinion", "opinions", "view", "views", "perspective"],
    // ── Comparison ──
    &["better", "best", "worse", "worst", "compare", "comparison", "versus", "vs"],
    // ── Reasoning ──
    &["reason", "reasons", "because", "since", "why", "explain", "explanation"],
];

/// Look up all synonyms for a given word (case-insensitive).
/// Returns a static slice of synonym words, or an empty slice if not found.
pub fn get_synonyms(word: &str) -> &'static [&'static str] {
    let lower = word.to_lowercase();
    for group in SYNONYM_GROUPS {
        for &term in *group {
            if term.eq_ignore_ascii_case(&lower) {
                return group;
            }
        }
    }
    &[]
}

/// Expand a query by appending synonyms for each input token.
///
/// Returns the original tokens followed by their synonyms (deduplicated).
/// Tokens not matching any synonym group are returned unchanged.
pub fn expand_query(tokens: &[&str]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<String> = Vec::new();

    for token in tokens {
        if seen.insert(token.to_lowercase()) {
            result.push(token.to_string());
        }
        let syns = get_synonyms(token);
        for &syn in syns {
            if seen.insert(syn.to_lowercase()) {
                result.push(syn.to_string());
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synonym_count() {
        assert!(
            SYNONYM_GROUPS.len() >= 46 + 7 + 7,
            "SYNONYM_GROUPS must have at least 46 groups (mempalace source had 46, we have {})",
            SYNONYM_GROUPS.len()
        );
    }

    #[test]
    fn test_get_synonyms_known() {
        let syns = get_synonyms("auth");
        assert!(
            syns.contains(&"authentication"),
            "auth should have authentication as synonym"
        );
        assert!(syns.contains(&"authn"), "auth should have authn as synonym");
    }

    #[test]
    fn test_get_synonyms_case_insensitive() {
        let syns_lower = get_synonyms("auth");
        let syns_upper = get_synonyms("Auth");
        let syns_mixed = get_synonyms("AUTH");
        assert_eq!(syns_lower, syns_upper);
        assert_eq!(syns_upper, syns_mixed);
    }

    #[test]
    fn test_get_synonyms_unknown() {
        let syns = get_synonyms("xyzzy");
        assert!(syns.is_empty(), "unknown word should return empty slice");
    }

    #[test]
    fn test_bm25_weight_is_07() {
        assert_eq!(SYNONYM_BM25_WEIGHT, 0.7, "SYNONYM_BM25_WEIGHT must be 0.7");
    }

    #[test]
    fn test_expand_query_dedup() {
        let tokens = &["auth", "authn"];
        let expanded = expand_query(tokens);
        // auth, authn, authentication, authn (dup), authenticating (dup of auth)
        // Unique: auth, authn, authentication, authenticating
        let unique_count = expanded
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(
            unique_count,
            expanded.len(),
            "expand_query should not produce duplicates"
        );
    }

    #[test]
    fn test_expand_query_no_match() {
        let tokens = &["xyzzy"];
        let expanded = expand_query(tokens);
        assert_eq!(
            expanded,
            vec!["xyzzy"],
            "unknown token should be returned as-is"
        );
    }

    #[test]
    fn test_expand_query_k8s() {
        let tokens = &["k8s"];
        let expanded = expand_query(tokens);
        assert!(expanded.contains(&"kubernetes".to_string()));
        assert!(expanded.contains(&"kube".to_string()));
    }

    #[test]
    fn test_expand_query_multiple_tokens() {
        let tokens = &["auth", "db"];
        let expanded = expand_query(tokens);
        assert!(expanded.contains(&"authentication".to_string()));
        assert!(expanded.contains(&"database".to_string()));
    }

    #[test]
    fn test_get_synonyms_pg() {
        let syns = get_synonyms("pg");
        assert!(syns.contains(&"postgres"));
        assert!(syns.contains(&"postgresql"));
    }

    #[test]
    fn test_get_synonyms_api() {
        let syns = get_synonyms("api");
        assert!(syns.contains(&"endpoint"));
        assert!(syns.contains(&"endpoints"));
    }
}
