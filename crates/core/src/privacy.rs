//! privacy.rs — Secret/PII redaction at ingest (mp-031, ADR-12)
//!
//! Strips well-known secret patterns from drawer content **before** it is
//! stored in the palace. Hard prerequisite for jcode adoption: tool-result
//! blocks regularly contain `OPENAI_API_KEY=sk-...`, OAuth tokens, JWTs, and
//! private keys after the model inspects shell output. Storing those is a
//! P0 security blocker.
//!
//! The redactor recognizes these kinds (each tagged with a `RedactionKind`
//! used in the `<REDACTED:KIND>` placeholder):
//!
//! | Kind             | What it catches                                              |
//! |------------------|--------------------------------------------------------------|
//! | `OPENAI_KEY`     | `sk-…`, `sk-proj-…`                                          |
//! | `ANTHROPIC_KEY`  | `sk-ant-…`                                                   |
//! | `GITHUB_TOKEN`   | `gh{p,o,s,r,u}_…`                                            |
//! | `SLACK_TOKEN`    | `xox{b,p,a,r,s}-…`                                           |
//! | `AWS_AKID`       | `AKIA…` access-key IDs                                       |
//! | `AWS_SECRET`     | 40-char base64 adjacent to `aws_secret_access_key=…`         |
//! | `JWT`            | `eyJ…\.eyJ…\.…` three-segment JWTs                           |
//! | `RSA_PRIVATE`    | `-----BEGIN (?:RSA|OPENSSH|EC|DSA )?PRIVATE KEY-----` blocks |
//! | `BEARER`         | `Authorization: Bearer …`                                    |
//! | `PASSWORD_VAR`   | `(?:password|passwd|pwd)\s*[:=]\s*…`                         |
//!
//! Each match is replaced with `<REDACTED:KIND>` so AAAK compression and
//! search continue to work meaningfully. The placeholder format is chosen
//! so it cannot itself match any of the secret patterns — placeholders never
//! get double-redacted.
//!
//! ## Configuration
//!
//! [`RedactionConfig`] supports two knobs:
//!
//! - `allow_patterns` — regex strings. Any byte range matched by an
//!   allow-pattern is excluded from redaction. Used for test fixtures
//!   (e.g. `sk-test-MOCK_*`) and any palace-specific allow-list.
//! - `disabled_kinds` — opt out of specific [`RedactionKind`]s entirely
//!   (per-palace policy override).
//!
//! ## Performance
//!
//! Every drawer body passes through `redact()` so patterns must compile
//! once. We use [`std::sync::OnceLock`] to cache `Vec<(Kind, Regex)>` for
//! the lifetime of the process. Allow-list patterns are compiled per call
//! since they are user-supplied and palace-scoped.
//!
//! See ADR-12 in `docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md` and the
//! mempalace analysis in `docs/research/06_mempalace_repo_analysis.md`
//! §3.4.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

/// Outcome of the LLM-API-key consent gate (mr-2k4g).
///
/// The gate is invoked at the entry of every LLM `complete()` call when the
/// provider's `api_key` was sourced from a process environment variable
/// (the `*_API_KEY` env-fallback path), rather than from explicit user
/// configuration. This protects the user from an LLM call silently
/// transmitting data to an external endpoint without their consent.
///
/// The check is intentionally cheap: a single env-var read plus a bool
/// lookup on the persisted config. It runs on every LLM call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsentStatus {
    /// Consent granted — either via the persisted flag, or via the
    /// `MEMPALACE_LLM_CONSENT` environment override for this process.
    Granted,
    /// The provider would use an env-fallback key but consent has not been
    /// recorded. The caller must return an error and surface remediation
    /// guidance to the user.
    Required,
    /// The provider's key came from explicit config (not env-fallback), so
    /// the consent gate does not apply and `complete()` may proceed.
    NotRequired,
}

/// Check whether the LLM consent gate is satisfied for an env-fallback key.
///
/// Precedence (highest first):
/// 1. **`MEMPALACE_LLM_CONSENT` env override** — set to one of
///    `true` / `1` / `yes` / `on` (case-insensitive, whitespace-trimmed) to
///    grant one-shot consent for the lifetime of the process. Used by CI
///    jobs and tests that have a key in the environment but no persisted
///    config yet.
/// 2. **Persisted `llm_consent_given` flag** — granted at some prior point
///    by `mpr config record-llm-consent`. Sticks across runs.
/// 3. Otherwise: `Required`.
///
/// `provider` and `base_url` are accepted for symmetry / future use
/// (e.g. surfacing a more specific warning per-provider) but the current
/// implementation does not gate on them. The caller is responsible for
/// deciding *whether* the key came from env-fallback before calling this
/// function.
pub fn check_env_consent(persisted: bool, _provider: &str, _base_url: &str) -> ConsentStatus {
    // 1. Env override — sticky for the lifetime of the process.
    if let Ok(v) = std::env::var("MEMPALACE_LLM_CONSENT") {
        let s = v.trim().to_ascii_lowercase();
        if matches!(s.as_str(), "true" | "1" | "yes" | "on") {
            return ConsentStatus::Granted;
        }
    }
    // 2. Persisted flag from prior `mpr config record-llm-consent`.
    if persisted {
        return ConsentStatus::Granted;
    }
    // 3. No override, no persisted grant → consent must be obtained.
    ConsentStatus::Required
}

/// Categories of secrets the redactor knows about.
///
/// The variant name is interpolated verbatim into the `<REDACTED:KIND>`
/// placeholder, so do not rename without considering downstream consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RedactionKind {
    /// OpenAI API keys (`sk-...` / `sk-proj-...`).
    OpenAiKey,
    /// Anthropic API keys (`sk-ant-...`).
    AnthropicKey,
    /// GitHub personal/oauth/server/refresh/user tokens (`gh{p,o,s,r,u}_...`).
    GitHubToken,
    /// Slack tokens (`xox{b,p,a,r,s}-...`).
    SlackToken,
    /// AWS access-key IDs (`AKIA...`).
    AwsAkid,
    /// AWS secret access keys (40-char base64 in env-export context).
    AwsSecret,
    /// JSON Web Tokens (`eyJ...\.eyJ...\....`).
    Jwt,
    /// PEM-encoded private keys (RSA/OpenSSH/EC/DSA/generic).
    RsaPrivate,
    /// `Authorization: Bearer ...` HTTP credentials.
    Bearer,
    /// `password=`, `passwd:`, `pwd =` style assignments.
    PasswordVar,
}

impl RedactionKind {
    /// Stable string used in the `<REDACTED:KIND>` placeholder.
    pub fn placeholder_label(self) -> &'static str {
        match self {
            RedactionKind::OpenAiKey => "OPENAI_KEY",
            RedactionKind::AnthropicKey => "ANTHROPIC_KEY",
            RedactionKind::GitHubToken => "GITHUB_TOKEN",
            RedactionKind::SlackToken => "SLACK_TOKEN",
            RedactionKind::AwsAkid => "AWS_AKID",
            RedactionKind::AwsSecret => "AWS_SECRET",
            RedactionKind::Jwt => "JWT",
            RedactionKind::RsaPrivate => "RSA_PRIVATE",
            RedactionKind::Bearer => "BEARER",
            RedactionKind::PasswordVar => "PASSWORD_VAR",
        }
    }

    fn placeholder(self) -> String {
        format!("<REDACTED:{}>", self.placeholder_label())
    }
}

/// Per-call (typically per-palace) tuning of the redactor.
#[derive(Debug, Clone, Default)]
pub struct RedactionConfig {
    /// Regex strings whose matches are *exempt* from redaction. Used for
    /// known-safe test fixtures and palace-specific allow-lists. Invalid
    /// regexes are silently skipped (logged via `tracing::warn`) so a typo
    /// in one entry does not disable the whole privacy filter.
    pub allow_patterns: Vec<String>,
    /// Kinds to skip entirely for this call. Useful when an integration
    /// stores e.g. legitimate `Bearer …` debug strings and the false-positive
    /// rate is unacceptable.
    pub disabled_kinds: HashSet<RedactionKind>,
}

/// Outcome of a redaction pass.
#[derive(Debug, Clone)]
pub struct RedactionResult {
    /// Text with every detected secret replaced by `<REDACTED:KIND>`.
    pub redacted_text: String,
    /// Byte ranges (in the **original** input) that were redacted, with
    /// the kind that matched. Useful for callers that want to surface a
    /// "X secrets stripped" report (e.g. `mpr doctor`).
    pub hits: Vec<(RedactionKind, Range<usize>)>,
}

/// Redact a drawer body using the default config.
///
/// Equivalent to `redact_with_config(text, &RedactionConfig::default())`.
pub fn redact(text: &str) -> RedactionResult {
    redact_with_config(text, &RedactionConfig::default())
}

/// Redact a drawer body with explicit configuration.
pub fn redact_with_config(text: &str, config: &RedactionConfig) -> RedactionResult {
    if text.is_empty() {
        return RedactionResult {
            redacted_text: String::new(),
            hits: Vec::new(),
        };
    }

    // 1. Compile (or fetch cached) secret patterns.
    let patterns = secret_patterns();

    // 2. Compute allow-list ranges from user-supplied regexes. Failures are
    //    swallowed (warn-logged) so a single bad regex never disables the
    //    privacy filter — refusing to redact would be worse than skipping
    //    the bad allow-pattern.
    let allow_ranges = compile_allow_ranges(text, &config.allow_patterns);

    // 3. Collect every match across every kind, skipping disabled kinds.
    let mut all_hits: Vec<(RedactionKind, Range<usize>)> = Vec::new();
    for (kind, regex) in patterns {
        if config.disabled_kinds.contains(kind) {
            continue;
        }
        for m in regex.find_iter(text) {
            all_hits.push((*kind, m.range()));
        }
    }

    // 4. Drop matches that fall fully inside an allow-list range.
    all_hits.retain(|(_, range)| !is_inside_any(range, &allow_ranges));

    // 5. Sort by start. On tie, prefer the longer match — this lets
    //    `sk-ant-...` win over `sk-...` when both regexes hit the same
    //    span, and lets RSA blocks subsume any incidental sub-matches.
    all_hits.sort_by(|a, b| {
        a.1.start.cmp(&b.1.start).then_with(|| {
            (b.1.end - b.1.start)
                .cmp(&(a.1.end - a.1.start))
                .then_with(|| order_of(a.0).cmp(&order_of(b.0)))
        })
    });

    // 6. Drop overlaps: a later hit whose start is < the previous hit's end
    //    is shadowed by the earlier one.
    let mut deduped: Vec<(RedactionKind, Range<usize>)> = Vec::new();
    let mut last_end = 0usize;
    for (kind, range) in all_hits {
        if range.start < last_end {
            continue;
        }
        last_end = range.end;
        deduped.push((kind, range));
    }

    // 7. Stitch the redacted text. Walking the byte slice is safe because
    //    every regex above operates on byte offsets that align with UTF-8
    //    boundaries (all secret patterns are ASCII).
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for (kind, range) in &deduped {
        // Defensive bounds check — should not trigger in practice but
        // protects against any future regex producing non-monotonic spans.
        if range.start < cursor || range.end > text.len() {
            continue;
        }
        out.push_str(&text[cursor..range.start]);
        out.push_str(&kind.placeholder());
        cursor = range.end;
    }
    out.push_str(&text[cursor..]);

    RedactionResult {
        redacted_text: out,
        hits: deduped,
    }
}

/// Stable priority for ties on identical (start, length) — ensures
/// deterministic output across runs. Lower number = preferred.
fn order_of(kind: RedactionKind) -> u8 {
    match kind {
        RedactionKind::RsaPrivate => 0,
        RedactionKind::AnthropicKey => 1,
        RedactionKind::OpenAiKey => 2,
        RedactionKind::GitHubToken => 3,
        RedactionKind::SlackToken => 4,
        RedactionKind::AwsAkid => 5,
        RedactionKind::AwsSecret => 6,
        RedactionKind::Jwt => 7,
        RedactionKind::Bearer => 8,
        RedactionKind::PasswordVar => 9,
    }
}

fn is_inside_any(range: &Range<usize>, allow_ranges: &[Range<usize>]) -> bool {
    allow_ranges
        .iter()
        .any(|r| range.start >= r.start && range.end <= r.end)
}

fn compile_allow_ranges(text: &str, allow_patterns: &[String]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for pat in allow_patterns {
        match Regex::new(pat) {
            Ok(re) => {
                for m in re.find_iter(text) {
                    ranges.push(m.range());
                }
            }
            Err(err) => {
                tracing::warn!(
                    target: "mempalace::privacy",
                    pattern = %pat,
                    error = %err,
                    "skipping invalid allow-list regex"
                );
            }
        }
    }
    ranges
}

/// Compiled secret patterns, lazy-initialized on first call.
fn secret_patterns() -> &'static [(RedactionKind, Regex)] {
    static PATTERNS: OnceLock<Vec<(RedactionKind, Regex)>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            // Order matters where two patterns could overlap on the same
            // span. ANTHROPIC_KEY (`sk-ant-...`) must come before the
            // generic OPENAI_KEY (`sk-...`); same for `sk-proj-...`.
            //
            // RSA blocks are matched first with a DOTALL-mode pattern so
            // anything inside the PEM frame is subsumed and never flagged
            // as JWT/base64/etc.
            let entries: Vec<(RedactionKind, &str)> = vec![
                // RSA / OpenSSH / EC / DSA / generic PEM private-key blocks.
                // `(?s)` => `.` matches `\n`. Non-greedy body so adjacent
                // PEM blocks don't fuse.
                (
                    RedactionKind::RsaPrivate,
                    r"(?s)-----BEGIN (?:RSA |OPENSSH |EC |DSA |ENCRYPTED |PGP )?PRIVATE KEY(?:[ A-Z]*)-----.*?-----END (?:RSA |OPENSSH |EC |DSA |ENCRYPTED |PGP )?PRIVATE KEY(?:[ A-Z]*)-----",
                ),
                // Anthropic keys — must match before generic `sk-…`.
                (
                    RedactionKind::AnthropicKey,
                    r"sk-ant-[A-Za-z0-9_\-]{90,}",
                ),
                // OpenAI project keys — must match before generic `sk-…`.
                (
                    RedactionKind::OpenAiKey,
                    r"sk-proj-[A-Za-z0-9_\-]{40,}",
                ),
                // OpenAI legacy keys.
                (RedactionKind::OpenAiKey, r"sk-[a-zA-Z0-9]{48,}"),
                // GitHub tokens (PAT, OAuth, server-to-server, refresh, user).
                (
                    RedactionKind::GitHubToken,
                    r"gh[opsru]_[A-Za-z0-9]{36,}",
                ),
                // Slack tokens.
                (
                    RedactionKind::SlackToken,
                    r"xox[bpars]-[A-Za-z0-9-]{10,}",
                ),
                // AWS access-key IDs — fixed `AKIA` prefix + 16 chars.
                (RedactionKind::AwsAkid, r"AKIA[0-9A-Z]{16}"),
                // AWS secret access keys — context-aware to avoid flagging
                // raw 40-char base64 paragraphs. Captures the entire
                // `aws_secret_access_key=…` assignment so the assignment
                // line itself is rewritten as a single placeholder.
                (
                    RedactionKind::AwsSecret,
                    r#"(?i)aws[_\-]?secret[_\-]?access[_\-]?key\s*[:=]\s*['"]?[A-Za-z0-9/+=]{40}['"]?"#,
                ),
                // JWTs — three base64url segments separated by dots. We
                // require the `eyJ` prefix on the header segment (real
                // JWTs always start with `eyJ` because that's
                // base64url("{\"")) to keep false positives down.
                (
                    RedactionKind::Jwt,
                    r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{5,}\b",
                ),
                // Authorization: Bearer …
                (
                    RedactionKind::Bearer,
                    r"(?i)Authorization\s*:?\s*Bearer\s+[A-Za-z0-9._\-/+=]{20,}",
                ),
                // password / passwd / pwd assignments.
                (
                    RedactionKind::PasswordVar,
                    r#"(?i)(?:password|passwd|pwd)\s*[:=]\s*['"]?[^\s'"<>]{8,}"#,
                ),
            ];

            entries
                .into_iter()
                .map(|(kind, pat)| {
                    let re = Regex::new(pat).unwrap_or_else(|err| {
                        // A bad built-in pattern is a programmer error —
                        // panic loudly in tests / debug; in release log and
                        // fall back to a never-matching pattern so the
                        // process keeps going (better than corrupting
                        // every drawer with a panic on every ingest).
                        tracing::error!(
                            target: "mempalace::privacy",
                            kind = ?kind,
                            error = %err,
                            "built-in privacy regex failed to compile; falling back to never-matching pattern"
                        );
                        Regex::new(r"$.^").expect("never-matching regex compiles")
                    });
                    (kind, re)
                })
                .collect()
        })
        .as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock;

    // ---------- Pattern coverage: each canonical example should match ----

    #[test]
    fn redacts_openai_legacy_key() {
        let key = "sk-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234";
        let res = redact(key);
        assert_eq!(res.redacted_text, "<REDACTED:OPENAI_KEY>");
        assert_eq!(res.hits.len(), 1);
        assert_eq!(res.hits[0].0, RedactionKind::OpenAiKey);
    }

    #[test]
    fn redacts_openai_project_key() {
        let key = "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789-_ABCDEFGHIJKL";
        let res = redact(key);
        assert!(res.redacted_text.contains("<REDACTED:OPENAI_KEY>"));
        assert_eq!(res.hits[0].0, RedactionKind::OpenAiKey);
    }

    #[test]
    fn redacts_anthropic_key() {
        // 95 chars after `sk-ant-` to clear the 90+ floor.
        let body = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-_abcdefghijklmnopqrstuvwxyz1234";
        let key = format!("sk-ant-{}", body);
        let res = redact(&key);
        assert!(
            res.redacted_text.contains("<REDACTED:ANTHROPIC_KEY>"),
            "expected anthropic redaction, got: {}",
            res.redacted_text
        );
        assert_eq!(res.hits[0].0, RedactionKind::AnthropicKey);
    }

    #[test]
    fn anthropic_wins_over_openai_on_same_span() {
        // sk-ant-... starts with `sk-` so a naive ordering would tag it as
        // OPENAI_KEY. ANTHROPIC_KEY must win.
        let body = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-_abcdefghijklmnopqrstuvwxyz1234";
        let key = format!("sk-ant-{}", body);
        let res = redact(&key);
        assert!(res.redacted_text.contains("ANTHROPIC_KEY"));
        assert!(!res.redacted_text.contains("OPENAI_KEY"));
    }

    #[test]
    fn redacts_github_token() {
        let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789AB";
        let res = redact(token);
        assert_eq!(res.redacted_text, "<REDACTED:GITHUB_TOKEN>");
    }

    #[test]
    fn redacts_slack_token() {
        let token = "xoxb-1234567890-abcdef-XYZ";
        let res = redact(token);
        assert!(res.redacted_text.contains("<REDACTED:SLACK_TOKEN>"));
    }

    #[test]
    fn redacts_aws_access_key_id() {
        let token = "AKIAIOSFODNN7EXAMPLE";
        let res = redact(token);
        assert_eq!(res.redacted_text, "<REDACTED:AWS_AKID>");
    }

    #[test]
    fn redacts_aws_secret_in_assignment_context() {
        let line = "aws_secret_access_key=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let res = redact(line);
        assert!(
            res.redacted_text.contains("<REDACTED:AWS_SECRET>"),
            "got: {}",
            res.redacted_text
        );
        // The whole assignment is rewritten — the raw secret must be gone.
        assert!(!res.redacted_text.contains("wJalrXUtnFEMI"));
    }

    /// False-positive guard: a raw 40-char base64 paragraph (e.g. a hash
    /// or commit SHA chain) **must not** be redacted as AWS_SECRET unless
    /// it's adjacent to the env-var key.
    #[test]
    fn raw_40char_base64_is_not_aws_secret() {
        let bare = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let res = redact(bare);
        assert!(
            !res.redacted_text.contains("AWS_SECRET"),
            "false positive on bare 40-char base64: {}",
            res.redacted_text
        );
        // Specifically: no AWS_SECRET hit.
        assert!(!res
            .hits
            .iter()
            .any(|(k, _)| matches!(k, RedactionKind::AwsSecret)));
    }

    #[test]
    fn redacts_jwt() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let res = redact(jwt);
        assert!(
            res.redacted_text.contains("<REDACTED:JWT>"),
            "got: {}",
            res.redacted_text
        );
    }

    #[test]
    fn redacts_rsa_private_key_block() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQ\nfakefakefake\n-----END RSA PRIVATE KEY-----";
        let res = redact(pem);
        assert_eq!(res.redacted_text, "<REDACTED:RSA_PRIVATE>");
        assert_eq!(res.hits.len(), 1);
        assert_eq!(res.hits[0].0, RedactionKind::RsaPrivate);
    }

    #[test]
    fn redacts_openssh_private_key_block() {
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";
        let res = redact(pem);
        assert!(res.redacted_text.contains("<REDACTED:RSA_PRIVATE>"));
    }

    #[test]
    fn redacts_bearer_authorization_header() {
        let line = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz1234567890";
        let res = redact(line);
        assert!(res.redacted_text.contains("<REDACTED:BEARER>"));
        // The raw token must be gone.
        assert!(!res.redacted_text.contains("abcdefghijklmnop"));
    }

    #[test]
    fn redacts_password_assignment() {
        let line = r#"password = "supersecret123""#;
        let res = redact(line);
        assert!(
            res.redacted_text.contains("<REDACTED:PASSWORD_VAR>"),
            "got: {}",
            res.redacted_text
        );
        assert!(!res.redacted_text.contains("supersecret123"));
    }

    // ---------- Allow-list semantics ----------

    #[test]
    fn allow_list_short_circuits_test_fixtures() {
        let text = "OPENAI_API_KEY=sk-test-MOCK_abcdefghijklmnopqrstuvwxyz0123456789ABCDEF";
        let mut config = RedactionConfig::default();
        config.allow_patterns.push(r"sk-test-MOCK_\w+".to_string());
        let res = redact_with_config(text, &config);
        // Allow-list covers the entire key span — no redaction.
        assert!(
            !res.redacted_text.contains("REDACTED"),
            "allow-listed text should pass through: {}",
            res.redacted_text
        );
        assert!(res.redacted_text.contains("sk-test-MOCK_"));
    }

    #[test]
    fn allow_list_does_not_protect_unrelated_secrets() {
        // Allow-list shields one secret; another in the same body still
        // redacts.
        let text = "test=sk-test-MOCK_abcdefghijklmnopqrstuvwxyz0123456789ABCDEF\nreal=sk-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234";
        let mut config = RedactionConfig::default();
        config.allow_patterns.push(r"sk-test-MOCK_\w+".to_string());
        let res = redact_with_config(text, &config);
        assert!(res.redacted_text.contains("sk-test-MOCK_"));
        assert!(res.redacted_text.contains("<REDACTED:OPENAI_KEY>"));
    }

    #[test]
    fn invalid_allow_pattern_is_skipped_not_fatal() {
        let text = "sk-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234";
        let mut config = RedactionConfig::default();
        // Unbalanced bracket — invalid regex.
        config.allow_patterns.push(r"[unbalanced".to_string());
        let res = redact_with_config(text, &config);
        // Filter still ran.
        assert!(res.redacted_text.contains("<REDACTED:OPENAI_KEY>"));
    }

    // ---------- Disabled-kinds semantics ----------

    #[test]
    fn disabled_kind_is_skipped() {
        let text = "ghp_abcdefghijklmnopqrstuvwxyz0123456789AB";
        let mut config = RedactionConfig::default();
        config.disabled_kinds.insert(RedactionKind::GitHubToken);
        let res = redact_with_config(text, &config);
        assert_eq!(res.redacted_text, text);
        assert!(res.hits.is_empty());
    }

    #[test]
    fn disabling_one_kind_does_not_affect_others() {
        let text = "ghp_abcdefghijklmnopqrstuvwxyz0123456789AB and AKIAIOSFODNN7EXAMPLE";
        let mut config = RedactionConfig::default();
        config.disabled_kinds.insert(RedactionKind::GitHubToken);
        let res = redact_with_config(text, &config);
        assert!(res.redacted_text.contains("ghp_"));
        assert!(res.redacted_text.contains("<REDACTED:AWS_AKID>"));
    }

    // ---------- Placeholder safety ----------

    /// `<REDACTED:KIND>` placeholders must not match any secret pattern.
    /// This guards against double-redaction artifacts in re-ingested text.
    #[test]
    fn placeholders_never_double_redact() {
        let placeholders = [
            "<REDACTED:OPENAI_KEY>",
            "<REDACTED:ANTHROPIC_KEY>",
            "<REDACTED:GITHUB_TOKEN>",
            "<REDACTED:SLACK_TOKEN>",
            "<REDACTED:AWS_AKID>",
            "<REDACTED:AWS_SECRET>",
            "<REDACTED:JWT>",
            "<REDACTED:RSA_PRIVATE>",
            "<REDACTED:BEARER>",
            "<REDACTED:PASSWORD_VAR>",
        ];
        for ph in placeholders {
            let res = redact(ph);
            assert_eq!(
                res.redacted_text, ph,
                "placeholder {} got falsely re-redacted to {}",
                ph, res.redacted_text
            );
            assert!(res.hits.is_empty(), "placeholder {} produced hits", ph);
        }
    }

    /// Round-tripping a stored placeholder through `redact()` must be a
    /// fixed point.
    #[test]
    fn redaction_is_idempotent() {
        let text = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz1234567890\nsk-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234";
        let once = redact(text).redacted_text;
        let twice = redact(&once).redacted_text;
        assert_eq!(once, twice, "redact() should be idempotent");
    }

    // ---------- Multi-secret bodies ----------

    #[test]
    fn redacts_multiple_secrets_in_one_body() {
        let body = "config:\n  github=ghp_abcdefghijklmnopqrstuvwxyz0123456789AB\n  aws=AKIAIOSFODNN7EXAMPLE\n  openai=sk-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234";
        let res = redact(body);
        assert!(res.redacted_text.contains("<REDACTED:GITHUB_TOKEN>"));
        assert!(res.redacted_text.contains("<REDACTED:AWS_AKID>"));
        assert!(res.redacted_text.contains("<REDACTED:OPENAI_KEY>"));
        assert_eq!(res.hits.len(), 3);
    }

    #[test]
    fn redacts_nothing_in_clean_text() {
        let text = "This is a normal sentence about the project. We chose Postgres over SQLite because of concurrency.";
        let res = redact(text);
        assert_eq!(res.redacted_text, text);
        assert!(res.hits.is_empty());
    }

    #[test]
    fn empty_input_returns_empty() {
        let res = redact("");
        assert_eq!(res.redacted_text, "");
        assert!(res.hits.is_empty());
    }

    /// Hits report byte ranges into the **original** input.
    #[test]
    fn hits_record_original_byte_ranges() {
        let prefix = "key=";
        let key = "sk-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234";
        let text = format!("{}{}", prefix, key);
        let res = redact(&text);
        assert_eq!(res.hits.len(), 1);
        let (kind, range) = &res.hits[0];
        assert_eq!(*kind, RedactionKind::OpenAiKey);
        assert_eq!(range.start, prefix.len());
        assert_eq!(range.end, prefix.len() + key.len());
    }

    // ---------- mr-2k4g: LLM consent gate ----------

    /// Persisted flag = true is the strongest in-band signal: granted.
    #[test]
    fn test_check_env_consent_persisted_true_grants() {
        let _guard = test_env_lock().lock().unwrap();
        std::env::remove_var("MEMPALACE_LLM_CONSENT");
        assert_eq!(
            check_env_consent(true, "openai", "https://api.openai.com"),
            ConsentStatus::Granted
        );
    }

    /// Persisted flag = false with no env override → consent is required.
    /// This is the default-first-run behavior; the user must run
    /// `mpr config record-llm-consent` (or set the env var) before
    /// env-fallback LLM calls succeed.
    #[test]
    fn test_check_env_consent_persisted_false_requires() {
        let _guard = test_env_lock().lock().unwrap();
        std::env::remove_var("MEMPALACE_LLM_CONSENT");
        assert_eq!(
            check_env_consent(false, "anthropic", "https://api.anthropic.com"),
            ConsentStatus::Required
        );
    }

    /// The env override grants consent even when the persisted flag is
    /// false. This is the "I know what I'm doing" path for CI / tests.
    #[test]
    fn test_check_env_consent_env_true_overrides_persisted_false() {
        let _guard = test_env_lock().lock().unwrap();
        std::env::set_var("MEMPALACE_LLM_CONSENT", "true");
        assert_eq!(
            check_env_consent(false, "openai", "https://api.openai.com"),
            ConsentStatus::Granted
        );
        std::env::remove_var("MEMPALACE_LLM_CONSENT");
    }

    /// All four documented truthy spellings must grant consent. Common
    /// typos like `"True "` (mixed case, surrounding whitespace) should
    /// also work because we trim + lowercase.
    #[test]
    fn test_check_env_consent_env_truthy_variants_grant() {
        let _guard = test_env_lock().lock().unwrap();
        for truthy in ["1", "yes", "on", "True", "  YES  ", "On"] {
            std::env::set_var("MEMPALACE_LLM_CONSENT", truthy);
            assert_eq!(
                check_env_consent(false, "openai", "https://api.openai.com"),
                ConsentStatus::Granted,
                "env value {:?} should grant consent",
                truthy
            );
        }
        std::env::remove_var("MEMPALACE_LLM_CONSENT");
    }

    /// When the env var is unset, the persisted flag is the sole
    /// determinant. We don't conflate "not granted" with "required" —
    /// that's the caller's distinction to make.
    #[test]
    fn test_check_env_consent_env_unset_returns_prior_state() {
        let _guard = test_env_lock().lock().unwrap();
        std::env::remove_var("MEMPALACE_LLM_CONSENT");
        // Persisted true → Granted.
        assert_eq!(
            check_env_consent(true, "openai", "https://api.openai.com"),
            ConsentStatus::Granted
        );
        // Persisted false → Required.
        assert_eq!(
            check_env_consent(false, "openai", "https://api.openai.com"),
            ConsentStatus::Required
        );
    }
}
