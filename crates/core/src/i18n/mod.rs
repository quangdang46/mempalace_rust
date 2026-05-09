//! Internationalization (i18n) support for MemPalace.
//!
//! Provides locale-aware entity detection, UI strings, and language-specific patterns.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Locale configuration loaded from JSON files.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Locale {
    /// Human-readable name of the locale (e.g., "English", "Português (Brasil)")
    pub name: String,
    /// BCP 47 language code (e.g., "en", "pt-BR", "ru")
    pub code: String,
    /// UI strings for CLI output
    #[serde(default)]
    pub cli: CliStrings,
    /// Entity detection patterns for this locale
    #[serde(default)]
    pub entity: EntityPatterns,
    /// Dialect-specific instructions
    #[serde(default)]
    pub dialect: DialectConfig,
}

/// CLI UI strings.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CliStrings {
    #[serde(default = "default_init_complete")]
    pub init_complete: String,
    #[serde(default = "default_mine_complete")]
    pub mine_complete: String,
    #[serde(default = "default_search_no_results")]
    pub search_no_results: String,
    #[serde(default = "default_palace_not_found")]
    pub palace_not_found: String,
    #[serde(default = "default_corpus_empty")]
    pub corpus_empty: String,
}

fn default_init_complete() -> String {
    "Initialization complete".to_string()
}

fn default_mine_complete() -> String {
    "Mining complete".to_string()
}

fn default_search_no_results() -> String {
    "No results found".to_string()
}

fn default_palace_not_found() -> String {
    "Palace not found. Run 'mempalace init <dir>' first.".to_string()
}

fn default_corpus_empty() -> String {
    "No content found in the specified directory.".to_string()
}

/// Entity detection patterns for a locale.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EntityPatterns {
    /// Verbs that indicate person entities (e.g., "said", "asked", "told")
    #[serde(default)]
    pub person_verbs: Vec<String>,
    /// Verbs that indicate project entities (e.g., "built", "created", "developed")
    #[serde(default)]
    pub project_verbs: Vec<String>,
    /// Pronouns for this language
    #[serde(default)]
    pub pronouns: Vec<String>,
    /// Common stopwords to exclude from entity detection
    #[serde(default)]
    pub stopwords: Vec<String>,
    /// Regex pattern for candidate entities (capitalized words)
    #[serde(default = "default_candidate_pattern")]
    pub candidate_pattern: String,
    /// Characters that mark word boundaries in this script
    #[serde(default)]
    pub boundary_chars: String,
    /// Pattern for versioned entities (e.g., "Project-v2.1")
    #[serde(default = "default_versioned_pattern")]
    pub versioned_pattern: String,
}

fn default_candidate_pattern() -> String {
    "[A-Z][a-z]{2,}".to_string()
}

fn default_versioned_pattern() -> String {
    "[A-Z][a-z]{2,}-v?\\d+(?:\\.\\d+)*".to_string()
}

/// Dialect-specific configuration.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DialectConfig {
    /// Instructions for AAAK compression in this language
    #[serde(default = "default_compression_instructions")]
    pub compression_instructions: String,
}

fn default_compression_instructions() -> String {
    "Compress your memories into AAAK format for efficient storage and retrieval. AAAK is a lossless shorthand dialect designed for AI agents.".to_string()
}

/// Locale manager that loads and provides locale configurations.
#[derive(Debug, Clone)]
pub struct LocaleManager {
    locales: HashMap<String, Locale>,
    default_locale: String,
}

impl LocaleManager {
    /// Create a new locale manager, loading all available locales from the i18n directory.
    pub fn new() -> Result<Self, LocaleError> {
        let mut locales = HashMap::new();

        // Load built-in locales
        Self::load_builtin_locales(&mut locales)?;

        Ok(Self {
            locales,
            default_locale: "en".to_string(),
        })
    }

    /// Load built-in locale files from the i18n directory.
    fn load_builtin_locales(locales: &mut HashMap<String, Locale>) -> Result<(), LocaleError> {
        // Try to load from embedded assets or filesystem
        let locale_files = vec!["en.json", "pt-br.json", "ru.json"];

        for file_name in locale_files {
            // For now, we'll use the embedded JSON we created
            // In a real implementation, this would load from the filesystem or embedded assets
            if let Ok(content) = Self::load_locale_content(file_name) {
                let locale: Locale = serde_json::from_str(&content)
                    .map_err(|e| LocaleError::ParseError(file_name.to_string(), e))?;

                // Store with both exact code and case-insensitive variant
                let code_lower = locale.code.to_lowercase();
                locales.insert(locale.code.clone(), locale.clone());
                locales.insert(code_lower, locale);
            }
        }

        Ok(())
    }

    /// Load locale content from embedded data.
    fn load_locale_content(file_name: &str) -> Result<String, LocaleError> {
        // Embedded locale data
        match file_name {
            "en.json" => Ok(Self::embedded_en()),
            "pt-br.json" => Ok(Self::embedded_pt_br()),
            "ru.json" => Ok(Self::embedded_ru()),
            _ => Err(LocaleError::NotFound(file_name.to_string())),
        }
    }

    /// Embedded English locale.
    fn embedded_en() -> String {
        r#"{
  "name": "English",
  "code": "en",
  "status_drawers": "{count} drawers filed",
  "status_wings": "{count} wings",
  "status_rooms": "{count} rooms",
  "error_not_found": "Not found: {item}",
  "error_no_palace": "No palace found at {path}",
  "error_invalid_input": "Invalid input: {reason}",
  "entity": {
    "person_verbs": ["said", "asked", "told", "replied", "answered", "spoke", "mentioned", "noted", "suggested", "recommended", "decided", "agreed", "thought", "believed", "remembered", "forgot", "realized", "discovered", "found", "saw", "heard", "learned", "knew", "understood", "wondered", "hoped", "feared", "worried", "doubted", "suspected", "expected", "predicted", "assumed", "guessed", "estimated", "calculated", "measured", "counted", "checked", "verified", "confirmed", "denied", "rejected", "accepted", "approved", "authorized", "permitted", "allowed", "forbade", "prohibited", "blocked", "stopped", "started", "began", "finished", "completed", "ended", "paused", "resumed", "continued", "proceeded", "advanced", "progressed", "moved", "changed", "altered", "modified", "adjusted", "fixed", "repaired", "restored", "recovered", "saved", "loaded", "read", "wrote", "created", "built", "made", "did", "has", "had", "was", "were", "been", "have"],
    "project_verbs": ["built", "created", "developed", "designed", "implemented", "wrote", "coded", "programmed", "engineered", "maintained", "managed", "deployed", "released", "published", "shipped", "launched", "started", "founded", "co-founded", "owns", "runs", "operates", "leads", "directs", "manages", "coordinates", "organizes", "sponsors", "supports", "backed", "invested", "funded", "contributed", "participates"],
    "pronouns": ["he", "him", "his", "himself", "she", "her", "hers", "herself", "they", "them", "their", "themselves", "it", "its", "itself", "we", "us", "our", "ours", "ourselves", "you", "your", "yours", "yourself", "i", "me", "my", "myself"],
    "stopwords": ["the", "a", "an", "and", "or", "but", "if", "because", "as", "what", "when", "where", "who", "which", "how", "why", "this", "that", "these", "those", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had", "do", "does", "did", "will", "would", "could", "should", "may", "might", "must", "shall", "can", "need"],
    "candidate_pattern": "[A-Z][a-z]{2,}",
    "boundary_chars": "",
    "versioned_pattern": "[A-Z][a-z]{2,}-v?\\d+(?:\\.\\d+)*"
  },
  "cli": {
    "init_complete": "Initialization complete",
    "mine_complete": "Mining complete",
    "search_no_results": "No results found",
    "palace_not_found": "Palace not found. Run 'mempalace init <dir>' first.",
    "corpus_empty": "No content found in the specified directory."
  },
  "dialect": {
    "compression_instructions": "Compress your memories into AAAK format for efficient storage and retrieval. AAAK is a lossless shorthand dialect designed for AI agents."
  }
}"#.to_string()
    }

    /// Embedded Portuguese (Brazil) locale.
    fn embedded_pt_br() -> String {
        r#"{
  "name": "Português (Brasil)",
  "code": "pt-BR",
  "status_drawers": "{count} gavetas arquivadas",
  "status_wings": "{count} alas",
  "status_rooms": "{count} quartos",
  "error_not_found": "Não encontrado: {item}",
  "error_no_palace": "Palácio não encontrado em {path}",
  "error_invalid_input": "Entrada inválida: {reason}",
  "entity": {
    "person_verbs": ["disse", "perguntou", "contou", "respondeu", "respondeu", "falou", "mencionou", "notou", "sugeriu", "recomendou", "decidiu", "concordou", "pensou", "acreditou", "lembrou", "esqueceu", "percebeu", "descobriu", "encontrou", "viu", "ouviu", "aprendeu", "sabia", "entendeu", "se perguntou", "esperou", "temeu", "preocupou", "duvidou", "suspeitou", "esperava", "previu", "assumiu", "adivinhou", "estimou", "calculou", "mediu", "contou", "verificou", "confirmou", "negou", "rejeitou", "aceitou", "aprovou", "autorizou", "permitiu", "permitiu", "proibiu", "proibiu", "bloqueou", "parou", "começou", "iniciou", "terminou", "completou", "acabou", "pausou", "retomou", "continuou", "procedeu", "avançou", "progrediu", "moveu", "mudou", "alterou", "modificou", "ajustou", "consertou", "reparou", "restaurou", "recuperou", "salvou", "carregou", "leu", "escreveu", "criou", "construiu", "fez", "fez", "tem", "teve", "foi", "foram", "sido", "tem"],
    "project_verbs": ["construiu", "criou", "desenvolveu", "desenhou", "implementou", "escreveu", "codou", "programou", "engenheirou", "manteve", "gerenciou", "deployou", "lançou", "publicou", "enviou", "lançou", "começou", "fundou", "co-fundou", "possui", "roda", "opera", "lidera", "direciona", "gerencia", "coordena", "organiza", "patrocina", "suporta", "apoiou", "investiu", "financiou", "contribuiu", "participa"],
    "pronouns": ["ele", "o", "seu", "si mesmo", "ela", "a", "seus", "si mesma", "eles", "os", "seus", "si mesmos", "isso", "seu", "si mesmo", "nós", "nos", "nosso", "nossos", "nós mesmos", "você", "seu", "seus", "si mesmo", "eu", "me", "meu", "eu mesmo"],
    "stopwords": ["o", "a", "um", "uma", "e", "ou", "mas", "se", "porque", "como", "o que", "quando", "onde", "quem", "qual", "como", "por que", "isto", "isso", "estes", "esses", "é", "são", "era", "foram", "ser", "sido", "sendo", "tem", "têm", "teve", "fazer", "faz", "fez", "vai", "iria", "poderia", "deveria", "pode", "poder", "deve", "precisa"],
    "candidate_pattern": "[A-Z][a-z]{2,}",
    "boundary_chars": "",
    "versioned_pattern": "[A-Z][a-z]{2,}-v?\\d+(?:\\.\\d+)*"
  },
  "cli": {
    "init_complete": "Inicialização completa",
    "mine_complete": "Mineração completa",
    "search_no_results": "Nenhum resultado encontrado",
    "palace_not_found": "Palácio não encontrado. Execute 'mempalace init <dir>' primeiro.",
    "corpus_empty": "Nenhum conteúdo encontrado no diretório especificado."
  },
  "dialect": {
    "compression_instructions": "Comprima suas memórias no formato AAAK para armazenamento e recuperação eficientes. AAAK é um dialeto abreviado sem perdas projetado para agentes de IA."
  }
}"#.to_string()
    }

    /// Embedded Russian locale.
    fn embedded_ru() -> String {
        r#"{
  "name": "Русский",
  "code": "ru",
  "status_drawers": "{count} ящиков подано",
  "status_wings": "{count} крыльев",
  "status_rooms": "{count} комнат",
  "error_not_found": "Не найдено: {item}",
  "error_no_palace": "Дворец не найден в {path}",
  "error_invalid_input": "Неверный ввод: {reason}",
  "entity": {
    "person_verbs": ["сказал", "спросил", "рассказал", "ответил", "ответил", "говорил", "упомянул", "заметил", "предложил", "рекомендовал", "решил", "согласился", "думал", "верил", "помнил", "забыл", "осознал", "обнаружил", "нашел", "увидел", "услышал", "узнал", "знал", "понял", "задумался", "надеялся", "боялся", "волновался", "сомневался", "подозревал", "ожидал", "предсказал", "предположил", "догадался", "оценил", "вычислил", "измерил", "считал", "проверил", "подтвердил", "отрицал", "отклонил", "принял", "одобрил", "авторизовал", "разрешил", "позволил", "запретил", "запретил", "заблокировал", "остановил", "начал", "начал", "закончил", "завершил", "закончил", "приостановил", "возобновил", "продолжил", "продвинулся", "продвинулся", "продвинулся", "переместился", "изменил", "изменил", "изменил", "скорректировал", "исправил", "починил", "восстановил", "восстановил", "сохранил", "загрузил", "прочитал", "написал", "создал", "построил", "сделал", "сделал", "имеет", "имел", "был", "были", "было", "имеет"],
    "project_verbs": ["построил", "создал", "разработал", "спроектировал", "реализовал", "написал", "закодировал", "запрограммировал", "инженерил", "поддерживал", "управлял", "развернул", "выпустил", "опубликовал", "отправил", "запустил", "начал", "основал", "соосновал", "владеет", "запускает", "оперирует", "руководит", "направляет", "управляет", "координирует", "организует", "спонсирует", "поддерживает", "поддержал", "инвестировал", "финансировал", "внес", "участвует"],
    "pronouns": ["он", "его", "ему", "себя", "она", "ее", "ей", "себя", "они", "их", "им", "себя", "это", "его", "себя", "мы", "нас", "нам", "себя", "вы", "вас", "вам", "себя", "я", "меня", "мне", "себя"],
    "stopwords": ["в", "на", "и", "или", "но", "если", "потому что", "как", "что", "когда", "где", "кто", "который", "как", "почему", "это", "то", "эти", "те", "есть", "являются", "был", "были", "быть", "был", "будучи", "имеет", "имеют", "имел", "делать", "делает", "сделал", "будет", "был бы", "мог бы", "должен", "может", "может", "должен", "нужно"],
    "candidate_pattern": "[А-Я][а-я]{2,}",
    "boundary_chars": "",
    "versioned_pattern": "[А-Я][а-я]{2,}-v?\\d+(?:\\.\\d+)*"
  },
  "cli": {
    "init_complete": "Инициализация завершена",
    "mine_complete": "Майнинг завершен",
    "search_no_results": "Результатов не найдено",
    "palace_not_found": "Дворец не найден. Сначала запустите 'mempalace init <dir>'.",
    "corpus_empty": "Содержимое не найдено в указанной директории."
  },
  "dialect": {
    "compression_instructions": "Сожмите свои воспоминания в формат AAAK для эффективного хранения и поиска. AAAK - это диалект сокращений без потерь, созданный для агентов ИИ."
  }
}"#.to_string()
    }

    /// Get a locale by its BCP 47 code (case-insensitive).
    pub fn get_locale(&self, code: &str) -> Option<&Locale> {
        self.locales.get(code).or_else(|| {
            // Try case-insensitive lookup
            self.locales.get(&code.to_lowercase())
        })
    }

    /// Get the default locale.
    pub fn get_default(&self) -> &Locale {
        self.get_locale(&self.default_locale)
            .expect("Default locale should always be available")
    }

    /// Get a locale, falling back to default if not found.
    pub fn get_locale_or_default(&self, code: &str) -> &Locale {
        self.get_locale(code).unwrap_or_else(|| self.get_default())
    }

    /// Get all available locale codes.
    pub fn available_locales(&self) -> Vec<String> {
        self.locales.keys().cloned().collect()
    }

    /// Resolve a BCP 47 language code to the best matching locale.
    /// Handles case-insensitivity and partial matches (e.g., "pt" → "pt-BR").
    pub fn resolve_locale(&self, code: &str) -> &Locale {
        let code_lower = code.to_lowercase();

        // Exact match
        if let Some(locale) = self.get_locale(&code_lower) {
            return locale;
        }

        // Try primary language subtag (e.g., "pt" from "pt-BR")
        if let Some(primary) = code_lower.split('-').next() {
            // Find any locale starting with this primary language
            for locale_code in self.available_locales() {
                if locale_code.to_lowercase().starts_with(primary) {
                    if let Some(locale) = self.get_locale(&locale_code) {
                        return locale;
                    }
                }
            }
        }

        // Fallback to default
        self.get_default()
    }
}

impl Default for LocaleManager {
    fn default() -> Self {
        Self::new().expect("Failed to create locale manager")
    }
}

/// Errors that can occur when working with locales.
#[derive(Debug, thiserror::Error)]
pub enum LocaleError {
    #[error("Locale not found: {0}")]
    NotFound(String),

    #[error("Failed to parse locale '{0}': {1}")]
    ParseError(String, serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_locale_manager_creation() {
        let manager = LocaleManager::new().unwrap();
        assert!(!manager.available_locales().is_empty());
    }

    #[test]
    fn test_get_locale() {
        let manager = LocaleManager::new().unwrap();

        // Test exact match
        assert!(manager.get_locale("en").is_some());
        assert!(manager.get_locale("pt-BR").is_some());
        assert!(manager.get_locale("ru").is_some());

        // Test case-insensitive
        assert!(manager.get_locale("EN").is_some());
        assert!(manager.get_locale("pt-br").is_some());
        assert!(manager.get_locale("RU").is_some());

        // Test non-existent
        assert!(manager.get_locale("de").is_none());
    }

    #[test]
    fn test_resolve_locale() {
        let manager = LocaleManager::new().unwrap();

        // Exact match
        let locale = manager.resolve_locale("en");
        assert_eq!(locale.code, "en");

        // Case-insensitive
        let locale = manager.resolve_locale("PT-BR");
        assert_eq!(locale.code, "pt-BR");

        // Partial match (primary language)
        let locale = manager.resolve_locale("pt");
        assert!(locale.code.starts_with("pt"));

        // Fallback to default
        let locale = manager.resolve_locale("de");
        assert_eq!(locale.code, "en");
    }

    #[test]
    fn test_entity_patterns() {
        let manager = LocaleManager::new().unwrap();
        let locale = manager.get_locale("en").unwrap();

        assert!(!locale.entity.person_verbs.is_empty());
        assert!(!locale.entity.project_verbs.is_empty());
        assert!(!locale.entity.pronouns.is_empty());
        assert!(!locale.entity.stopwords.is_empty());
    }

    #[test]
    fn test_cli_strings() {
        let manager = LocaleManager::new().unwrap();
        let locale = manager.get_locale("en").unwrap();

        assert!(!locale.cli.init_complete.is_empty());
        assert!(!locale.cli.mine_complete.is_empty());
        assert!(!locale.cli.search_no_results.is_empty());
    }
}
