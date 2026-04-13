use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Source Discovery ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSource {
    pub path: String,
    pub label: String,
    pub read_only: bool,
    pub file_count: u32,
    pub total_bytes: u64,
}

// ── Progress Events (Rust → Frontend) ──

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportPhase {
    Preflight,
    Staging,
    Wiping,
    Distributing,
    Cleanup,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPhaseChange {
    pub phase: ImportPhase,
    pub source_label: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub phase: ImportPhase,
    pub source_label: String,
    pub files_done: u32,
    pub files_total: u32,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current_file: String,
    pub speed_bps: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportWarning {
    pub message: String,
    pub source_label: String,
}

// ── Unknown Files ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownFile {
    pub staged_path: String,
    pub rel_path: String,
    pub extension: String,
    pub filename: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnknownFileAction {
    DeleteFilename,
    DeleteExtension,
    MoveToOther,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownFileDecision {
    pub staged_path: String,
    pub action: UnknownFileAction,
}

// ── Import Result ──

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceResult {
    pub source_label: String,
    pub source_path: String,
    pub files_staged: u32,
    pub bytes_staged: u64,
    pub source_wiped: bool,
    pub read_only: bool,
    pub videos_moved: u32,
    pub photos_moved: u32,
    pub dups_skipped: u32,
    pub unknown_files: u32,
    pub no_files: bool,
    pub earliest_date: Option<String>,
    pub latest_date: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub sources: Vec<SourceResult>,
    pub log_path: Option<String>,
}

// ── Internal pipeline types ──

pub(crate) struct FileEntry {
    pub rel_path: String,
    pub size: u64,
    pub source_hash: [u8; 32],
    pub staged_path: PathBuf,
    pub verified: bool,
}

pub(crate) struct DistributeResult {
    pub videos_moved: u32,
    pub photos_moved: u32,
    pub dups_skipped: u32,
    pub earliest_date: Option<String>,
    pub latest_date: Option<String>,
}
