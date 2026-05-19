use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub type ProjectId = Uuid;
pub type JobId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub root: PathBuf,
    pub created_at: DateTime<Utc>,
}

impl Project {
    pub fn new(name: impl Into<String>, root: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            root,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignRequest {
    pub prompt: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default = "default_max_repair_rounds")]
    pub max_repair_rounds: u8,
    #[serde(default)]
    pub use_yosys: bool,
    #[serde(default = "default_generate_waveform")]
    pub generate_waveform: bool,
    #[serde(default)]
    pub generate_kicad: bool,
    #[serde(default, alias = "rag_context")]
    pub retrieved_context: Vec<RetrievedContext>,
}

fn default_language() -> String {
    "systemverilog".to_string()
}

fn default_target() -> String {
    "simulation".to_string()
}

fn default_max_repair_rounds() -> u8 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievedContext {
    pub source: String,
    pub title: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateRequest {
    pub top: Option<String>,
    pub synthesis_top: Option<String>,
    pub rtl_files: Vec<String>,
    pub testbench_files: Vec<String>,
    #[serde(default)]
    pub use_yosys: bool,
    #[serde(default = "default_generate_waveform")]
    pub generate_waveform: bool,
    #[serde(default)]
    pub generate_kicad: bool,
}

fn default_generate_waveform() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairRequest {
    pub job_id: JobId,
    #[serde(default)]
    pub max_repair_rounds: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub project_id: ProjectId,
    pub kind: JobKind,
    pub status: JobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Job {
    pub fn new(project_id: ProjectId, kind: JobKind) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            project_id,
            kind,
            status: JobStatus::Queued,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Design,
    Simulate,
    Repair,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobCreated {
    pub job_id: JobId,
    pub project_id: ProjectId,
    pub status: JobStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobEvent {
    pub job_id: JobId,
    pub sequence: u64,
    pub level: EventLevel,
    pub message: String,
    pub at: DateTime<Utc>,
    pub data: Option<serde_json::Value>,
}

impl JobEvent {
    pub fn new(
        job_id: JobId,
        sequence: u64,
        level: EventLevel,
        message: impl Into<String>,
    ) -> Self {
        Self {
            job_id,
            sequence,
            level,
            message: message.into(),
            at: Utc::now(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLevel {
    Info,
    Warning,
    Error,
    Artifact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignArtifact {
    pub path: String,
    pub kind: ArtifactKind,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Rtl,
    Testbench,
    Spec,
    Constraint,
    Report,
    Netlist,
    Waveform,
    Diagram,
    Schematic,
    Script,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignPackage {
    pub summary: String,
    pub artifacts: Vec<DesignArtifact>,
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationSummary {
    pub passed: bool,
    pub tool: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub artifacts: Vec<RunArtifact>,
    #[serde(default)]
    pub analysis: Option<VerificationAnalysis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtifact {
    pub path: String,
    pub kind: ArtifactKind,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveformSignal {
    pub name: String,
    pub width: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveformDigest {
    pub path: String,
    pub source: String,
    pub timescale: Option<String>,
    pub signal_count: usize,
    pub signals: Vec<WaveformSignal>,
    pub transitions_sample: Vec<String>,
    pub ocr_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationAnalysis {
    pub overall_passed: bool,
    pub passed_tools: Vec<String>,
    pub failing_tools: Vec<String>,
    pub artifact_paths: Vec<String>,
    pub waveform: Option<WaveformDigest>,
    pub findings: Vec<String>,
    pub next_actions: Vec<String>,
    pub structured_prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairSuggestion {
    pub reason: String,
    pub artifacts: Vec<DesignArtifact>,
    pub should_retry: bool,
}
