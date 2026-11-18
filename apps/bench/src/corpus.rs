//! Frozen synthetic corpora.
//!
//! Both corpora are deterministic generators. The small 10,000-command corpus
//! is hashed on every quick run. The one-million-entry file-index profile is
//! generated and hashed on demand: a million lines are never committed.

use crate::json;
use crate::sha256::Sha256;
use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};

pub const COMMAND_CORPUS_VERSION: &str = "ansa-commands-10k-v1";
pub const COMMAND_CORPUS_COUNT: usize = 10_000;
pub const COMMAND_CORPUS_SEED: u64 = 0x5eed_0012_2026_1118;

pub const FILE_INDEX_VERSION: &str = "ansa-file-index-1m-v1";
pub const FILE_INDEX_COUNT: usize = 1_000_000;
pub const FILE_INDEX_SEED: u64 = 0x1f11_e001_2026_1118;

const COMMAND_MANIFEST: &str = include_str!("../../../fixtures/bench/corpus-v1.manifest.json");
const FILE_INDEX_MANIFEST: &str =
    include_str!("../../../fixtures/bench/file-index-v1.manifest.json");

const PROJECTS: [&str; 16] = [
    "Atlas", "Beacon", "Cobalt", "Delta", "Ember", "Falcon", "Granite", "Harbor", "Indigo",
    "Juniper", "Kestrel", "Lumen", "Meridian", "Nimbus", "Orchid", "Pioneer",
];
const ACTIONS: [&str; 12] = [
    "Open",
    "Connect",
    "Review",
    "Archive",
    "Deploy",
    "Inspect",
    "Sync",
    "Export",
    "Restore",
    "Monitor",
    "Configure",
    "Audit",
];
const OBJECTS: [&str; 12] = [
    "Terminal",
    "Dashboard",
    "Workspace",
    "Pipeline",
    "Notebook",
    "Registry",
    "Console",
    "Timeline",
    "Browser",
    "Clipboard",
    "Settings",
    "Report",
];
const KEYWORDS: [&str; 8] = [
    "benchmark",
    "synthetic",
    "command",
    "workstation",
    "project",
    "tool",
    "local",
    "fixture",
];

const TEAMS: [&str; 12] = [
    "engineering",
    "design",
    "operations",
    "research",
    "finance",
    "support",
    "security",
    "platform",
    "product",
    "legal",
    "people",
    "data",
];
const FOLDERS: [&str; 12] = [
    "Documents",
    "Projects",
    "Shared",
    "Archive",
    "Notes",
    "Reports",
    "Specs",
    "Plans",
    "Inbox",
    "Scratch",
    "Reference",
    "Deliverables",
];
const EXTENSIONS: [&str; 8] = ["txt", "md", "rs", "ts", "json", "pdf", "csv", "log"];
const FILE_STEMS: [&str; 12] = [
    "meeting-notes",
    "release-checklist",
    "design-review",
    "budget-draft",
    "roadmap",
    "incident-report",
    "onboarding-guide",
    "test-plan",
    "summary",
    "changelog",
    "readme",
    "status-update",
];

/// Queries used by the warm query sweep. They are fixed so two runs on the same
/// corpus are comparable.
pub const COMMAND_QUERIES: [&str; 8] = [
    "",
    "benchmark",
    "terminal",
    "Atlas Release",
    "b00013",
    "cmd09999",
    "no-such-command-xyz",
    "Meridian Configure Report",
];

/// One record of the frozen command corpus, before conversion to a domain type.
#[derive(Debug, Clone)]
pub struct CommandRecord {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub keywords: Vec<String>,
    pub alias: Option<String>,
    pub path: String,
}

/// Returns the deterministic record at `index`.
pub fn command_record(index: usize) -> CommandRecord {
    let mixed =
        COMMAND_CORPUS_SEED.wrapping_add((index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let project = PROJECTS[((mixed >> 7) % PROJECTS.len() as u64) as usize];
    let action = ACTIONS[((mixed >> 17) % ACTIONS.len() as u64) as usize];
    let object = OBJECTS[((mixed >> 29) % OBJECTS.len() as u64) as usize];
    let keyword = KEYWORDS[((mixed >> 41) % KEYWORDS.len() as u64) as usize];
    let title = format!("{project} {action} {object} {index:05}");
    let subtitle = format!("Synthetic benchmark command {index:05}");
    let alias = index.is_multiple_of(13).then(|| format!("b{index:05}"));
    let path = format!("C:\\Program Files\\Ansa Bench\\{project}\\{object}-{index:05}.exe");
    CommandRecord {
        id: format!("bench:command-{index:05}"),
        title,
        subtitle,
        keywords: vec![
            "benchmark".to_string(),
            keyword.to_string(),
            format!("cmd{index:05}"),
        ],
        alias,
        path,
    }
}

/// Serializes one command record as canonical JSONL (no trailing newline).
pub fn command_line(index: usize) -> String {
    let record = command_record(index);
    let keywords = record
        .keywords
        .iter()
        .map(|keyword| json::string(keyword))
        .collect::<Vec<_>>()
        .join(",");
    let alias = record
        .alias
        .as_deref()
        .map_or_else(|| "null".to_string(), json::string);
    format!(
        "{{\"id\":{},\"title\":{},\"subtitle\":{},\"keywords\":[{}],\"alias\":{},\"kind\":{{\"type\":\"application\",\"path\":{}}}}}",
        json::string(&record.id),
        json::string(&record.title),
        json::string(&record.subtitle),
        keywords,
        alias,
        json::string(&record.path),
    )
}

/// Generates the entire frozen 10,000-command corpus as JSONL.
pub fn command_corpus_jsonl() -> String {
    let mut output = String::with_capacity(COMMAND_CORPUS_COUNT * 240);
    for index in 0..COMMAND_CORPUS_COUNT {
        output.push_str(&command_line(index));
        output.push('\n');
    }
    output
}

/// Builds the domain commands used by the engine.
pub fn command_catalog() -> Vec<Command> {
    (0..COMMAND_CORPUS_COUNT)
        .map(|index| {
            let record = command_record(index);
            let mut command = Command::new(
                CommandId::new(record.id).expect("generated command ID is valid"),
                record.title,
                CommandKind::Builtin(BuiltinCommand::About),
            )
            .with_subtitle(record.subtitle)
            .with_keywords(record.keywords);
            if let Some(alias) = record.alias {
                command = command.with_alias(alias);
            }
            command
        })
        .collect()
}

/// Result of verifying a frozen corpus against its checked-in manifest.
#[derive(Debug, Clone)]
pub struct CorpusIdentity {
    pub version: String,
    pub count: usize,
    pub sha256: String,
    pub manifest_sha256: String,
    pub bytes: u64,
}

impl CorpusIdentity {
    pub fn matches(&self) -> bool {
        self.sha256 == self.manifest_sha256
    }
}

/// Hashes the command corpus and compares it with the frozen manifest.
pub fn verify_command_corpus() -> CorpusIdentity {
    let bytes = command_corpus_jsonl();
    CorpusIdentity {
        version: COMMAND_CORPUS_VERSION.to_string(),
        count: COMMAND_CORPUS_COUNT,
        sha256: crate::sha256::digest_hex(bytes.as_bytes()),
        manifest_sha256: manifest_string(COMMAND_MANIFEST, "sha256").unwrap_or_default(),
        bytes: bytes.len() as u64,
    }
}

/// One synthetic file-index entry.
#[derive(Debug, Clone)]
pub struct FileIndexEntry {
    pub path: String,
    pub title: String,
    pub size_bytes: u64,
    pub modified_unix: u64,
}

/// Returns the deterministic file-index entry at `index`.
pub fn file_index_entry(index: usize) -> FileIndexEntry {
    let mixed = FILE_INDEX_SEED.wrapping_add((index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let team = TEAMS[((mixed >> 7) % TEAMS.len() as u64) as usize];
    let folder = FOLDERS[((mixed >> 17) % FOLDERS.len() as u64) as usize];
    let extension = EXTENSIONS[((mixed >> 29) % EXTENSIONS.len() as u64) as usize];
    let stem = FILE_STEMS[((mixed >> 41) % FILE_STEMS.len() as u64) as usize];
    let path = format!("C:\\Users\\bench\\{folder}\\{team}\\{stem}-{index:07}.{extension}");
    let title = format!("{stem} {team} {index:07}");
    FileIndexEntry {
        path,
        title,
        size_bytes: 512 + ((index as u64).wrapping_mul(37) % 48_000),
        modified_unix: 1_700_000_000 + ((index as u64).wrapping_mul(97) % 20_000_000),
    }
}

/// Serializes one file-index entry as canonical JSONL (no trailing newline).
pub fn file_index_line(index: usize) -> String {
    let entry = file_index_entry(index);
    format!(
        "{{\"path\":{},\"title\":{},\"size\":{},\"modified\":{}}}",
        json::string(&entry.path),
        json::string(&entry.title),
        entry.size_bytes,
        entry.modified_unix,
    )
}

/// Streams the one-million-entry corpus through SHA-256 and returns
/// `(sha256, serialized_bytes)`. It never retains the million entries.
pub fn hash_file_index(count: usize) -> (String, u64) {
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    for index in 0..count {
        let mut line = file_index_line(index);
        line.push('\n');
        bytes += line.len() as u64;
        hasher.update(line.as_bytes());
    }
    (crate::sha256::to_hex(&hasher.finish()), bytes)
}

/// Builds an in-memory file index and hashes exactly the same serialized lines.
///
/// This is a synthetic harness baseline, not the production file indexer.
pub fn build_file_index(count: usize) -> (Vec<FileIndexEntry>, String, u64) {
    let mut entries = Vec::with_capacity(count);
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    for index in 0..count {
        let line = file_index_line(index);
        bytes += line.len() as u64 + 1;
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
        entries.push(file_index_entry(index));
    }
    (entries, crate::sha256::to_hex(&hasher.finish()), bytes)
}

/// The frozen manifest strings, exposed for reporting and tests.
#[cfg(test)]
pub fn command_manifest_text() -> &'static str {
    COMMAND_MANIFEST
}

pub fn file_index_manifest_text() -> &'static str {
    FILE_INDEX_MANIFEST
}

/// Extracts a quoted string field from the tiny frozen manifests.
pub fn manifest_string(manifest: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let start = manifest.find(&needle)? + needle.len();
    let rest = &manifest[start..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Extracts an unsigned integer field from the tiny frozen manifests.
#[cfg(test)]
pub fn manifest_u64(manifest: &str, field: &str) -> Option<u64> {
    let needle = format!("\"{field}\"");
    let start = manifest.find(&needle)? + needle.len();
    let rest = &manifest[start..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    let digits = rest
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_corpus_is_deterministic_and_frozen() {
        let first = command_corpus_jsonl();
        let second = command_corpus_jsonl();
        assert_eq!(first, second);
        assert_eq!(first.lines().count(), COMMAND_CORPUS_COUNT);
        assert!(first.ends_with('\n'));

        let identity = verify_command_corpus();
        assert_eq!(identity.version, COMMAND_CORPUS_VERSION);
        assert_eq!(identity.count, COMMAND_CORPUS_COUNT);
        assert_eq!(
            manifest_u64(command_manifest_text(), "bytes"),
            Some(identity.bytes)
        );
        assert!(
            identity.matches(),
            "corpus hash {} does not match frozen manifest {}; bump the version \
             instead of editing generated text",
            identity.sha256,
            identity.manifest_sha256
        );
        assert_eq!(
            manifest_string(command_manifest_text(), "version").as_deref(),
            Some(COMMAND_CORPUS_VERSION)
        );
    }

    #[test]
    fn command_rows_have_distinct_ids_and_file_like_metadata() {
        let mut ids = std::collections::HashSet::new();
        for index in [0usize, 1, 42, 9_999] {
            let record = command_record(index);
            assert!(ids.insert(record.id.clone()));
            assert!(record.id.starts_with("bench:command-"));
            assert!(record.path.ends_with(".exe"));
            assert!(record.title.contains(&format!("{index:05}")));
        }
    }

    #[test]
    fn file_index_profile_is_deterministic() {
        let (first_hash, first_bytes) = hash_file_index(1000);
        let (second_hash, second_bytes) = hash_file_index(1000);
        assert_eq!(first_hash, second_hash);
        assert_eq!(first_bytes, second_bytes);
        assert!(first_bytes > 0);

        let (entries, built_hash, built_bytes) = build_file_index(1000);
        assert_eq!(entries.len(), 1000);
        assert_eq!(built_hash, first_hash);
        assert_eq!(built_bytes, first_bytes);
        assert!(entries[0].path.starts_with("C:\\Users\\bench\\"));
        assert_eq!(
            manifest_string(file_index_manifest_text(), "version").as_deref(),
            Some(FILE_INDEX_VERSION)
        );
    }
}
