use std::collections::HashSet;

use sqlx::AssertSqlSafe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedQueryKind {
    Select,
    Execute,
}

#[derive(Debug)]
pub struct AuditedNamedQuery {
    sql: String,
    parameter_names: Vec<String>,
    kind: NamedQueryKind,
}

impl AuditedNamedQuery {
    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub fn parameter_names(&self) -> &[String] {
        &self.parameter_names
    }

    pub fn kind(&self) -> NamedQueryKind {
        self.kind
    }
}

/// Allowed identifiers for dynamic table/column interpolation.
#[derive(Debug)]
pub struct TrustedSqlIdentifier(String);

impl TrustedSqlIdentifier {
    pub fn from_allowlist<'a>(value: &'a str, allowlist: &[&str]) -> Result<Self, String> {
        if allowlist.iter().any(|candidate| candidate == &value) {
            Ok(Self(value.to_owned()))
        } else {
            Err(format!("untrusted identifier: {value}"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlSafetyBoundaryViolation {
    Unknown,
}

/// Central boundary for explicit SQL safety construction.
#[inline]
pub fn audit_sql(sql: String) -> AssertSqlSafe<String> {
    AssertSqlSafe(sql)
}

/// Return audited SQL for dynamic table/column interpolation.
pub fn audited_identifier_sql(
    fragments: &[&str],
    identifiers: &[TrustedSqlIdentifier],
) -> Result<AssertSqlSafe<String>, String> {
    if fragments.len() != identifiers.len() + 1 {
        return Err("identifier fragments must be separator + identifiers".to_string());
    }

    let mut output = String::new();
    for (idx, fragment) in fragments.iter().enumerate() {
        output.push_str(fragment);
        if let Some(identifier) = identifiers.get(idx) {
            output.push('`');
            output.push_str(identifier.as_str());
            output.push('`');
        }
    }

    Ok(audit_sql(output))
}

/// Central SQL audit for named-parameter query templates.
pub fn audit_named_query(
    name: &str,
    sql: &str,
    kind: NamedQueryKind,
    params: &[&str],
) -> Result<AuditedNamedQuery, String> {
    if has_suspicious_annotation(sql) {
        return Err(format!("{name}: sql contains unsupported annotation or comment"));
    }

    let found = scan_named_params(sql)?;

    if found.is_empty() && !params.is_empty() {
        return Err(format!("{name}: sql has no bind parameters"));
    }

    let mut declared = params.iter().map(|p| (*p).to_string()).collect::<Vec<_>>();
    let mut unique = HashSet::new();
    for declared in &declared {
        if !unique.insert(declared.clone()) {
            return Err(format!("{name}: duplicate declared parameter: {declared}"));
        }
    }

    if declared.len() != found.len() {
        return Err(format!(
            "{name}: expected {} parameter(s), but sql has {}",
            declared.len(),
            found.len()
        ));
    }

    if declared != found {
        return Err(format!("{name}: declared parameters must match SQL placeholders"));
    }

    if inferred_kind(sql) != kind {
        return Err(format!("{name}: statement kind mismatch"));
    }

    let audited_sql = rewrite_named_placeholders(sql)?;

    Ok(AuditedNamedQuery {
        sql: audited_sql,
        parameter_names: declared,
        kind,
    })
}

fn has_suspicious_annotation(sql: &str) -> bool {
    sql.contains("--") || sql.contains("/*") || sql.contains("*/") || sql.contains('#') || sql.contains(';')
}

fn inferred_kind(sql: &str) -> NamedQueryKind {
    let keyword = sql
        .split_whitespace()
        .next()
        .map(|word| word.to_ascii_uppercase())
        .unwrap_or_default();
    if keyword == "SELECT" {
        NamedQueryKind::Select
    } else {
        NamedQueryKind::Execute
    }
}

fn scan_named_params(sql: &str) -> Result<Vec<String>, String> {
    let mut params = Vec::new();
    let mut i = 0;
    let bytes = sql.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b':' {
            if i + 1 >= bytes.len() || !is_identifier_start(bytes[i + 1]) {
                return Err("invalid named placeholder".to_string());
            }

            let mut j = i + 1;
            while j < bytes.len() && is_identifier_char(bytes[j]) {
                j += 1;
            }

            let name = std::str::from_utf8(&bytes[i + 1..j]).map_err(|_| "placeholder decode error")?;
            params.push(name.to_string());
            i = j;
        } else {
            i += 1;
        }
    }

    let mut seen = HashSet::new();
    for p in &params {
        if !seen.insert(p) {
            return Err(format!("duplicated placeholder :{p}"));
        }
    }

    Ok(params)
}

fn rewrite_named_placeholders(sql: &str) -> Result<String, String> {
    let mut output = String::new();
    let mut i = 0;
    let bytes = sql.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b':' {
            if i + 1 >= bytes.len() || !is_identifier_start(bytes[i + 1]) {
                return Err("invalid named placeholder".to_string());
            }
            let mut j = i + 1;
            while j < bytes.len() && is_identifier_char(bytes[j]) {
                j += 1;
            }
            output.push('?');
            i = j;
        } else {
            output.push(char::from(bytes[i]));
            i += 1;
        }
    }

    Ok(output)
}

fn is_identifier_start(ch: u8) -> bool {
    matches!(ch, b'a'..=b'z' | b'A'..=b'Z' | b'_')
}

fn is_identifier_char(ch: u8) -> bool {
    matches!(ch, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimisticLockTable {
    Agents,
    AgentHooks,
    Categories,
    Functions,
    Plugins,
    RecommendedGames,
    Skills,
    Tags,
    Tools,
    Workflows,
}

impl OptimisticLockTable {
    pub const ALL: [Self; 10] = [
        Self::Agents,
        Self::AgentHooks,
        Self::Categories,
        Self::Functions,
        Self::Plugins,
        Self::RecommendedGames,
        Self::Skills,
        Self::Tags,
        Self::Tools,
        Self::Workflows,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::AgentHooks => "agent_hooks",
            Self::Categories => "categories",
            Self::Functions => "functions",
            Self::Plugins => "plugins",
            Self::RecommendedGames => "recommended_games",
            Self::Skills => "skills",
            Self::Tags => "tags",
            Self::Tools => "tools",
            Self::Workflows => "workflows",
        }
    }
}
