use crate::{DesignAgent, RepairAgent};
use anyhow::{anyhow, Result};
use domain::{
    ArtifactKind, DesignArtifact, DesignPackage, DesignRequest, EventLevel, JobEvent, JobId,
    RepairSuggestion, SimulationSummary,
};
use eda_runner::{EdaRunner, SimulationPlan};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct AgentEvent {
    pub level: EventLevel,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

pub type AgentEventSink = Arc<dyn Fn(AgentEvent) + Send + Sync>;

pub struct DesignOrchestrator<D, R>
where
    D: DesignAgent,
    R: RepairAgent,
{
    design_agent: D,
    repair_agent: R,
    eda_runner: EdaRunner,
}

impl<D, R> DesignOrchestrator<D, R>
where
    D: DesignAgent,
    R: RepairAgent,
{
    pub fn new(design_agent: D, repair_agent: R, eda_runner: EdaRunner) -> Self {
        Self {
            design_agent,
            repair_agent,
            eda_runner,
        }
    }

    pub async fn run_design_flow(
        &self,
        job_id: JobId,
        project_root: &Path,
        request: &DesignRequest,
        sink: AgentEventSink,
    ) -> Result<DesignFlowOutput> {
        emit(&sink, EventLevel::Info, "正在生成设计产物", None);
        let package = self.design_agent.generate(request).await?;
        write_artifacts(project_root, &package.artifacts).await?;
        emit(
            &sink,
            EventLevel::Artifact,
            "设计产物已写入",
            Some(json!({ "count": package.artifacts.len(), "summary": package.summary })),
        );

        let plan = package_to_plan(project_root, &package, request)?;
        emit(
            &sink,
            EventLevel::Info,
            "正在运行仿真反馈",
            Some(json!({
                "rtl_files": plan.rtl_files.iter().map(|path| path.to_string_lossy().to_string()).collect::<Vec<_>>(),
                "testbench_files": plan.testbench_files.iter().map(|path| path.to_string_lossy().to_string()).collect::<Vec<_>>()
            })),
        );

        let mut summaries = match self.eda_runner.run_digital_simulation(&plan).await {
            Ok(summaries) => summaries,
            Err(error) => {
                let summary = SimulationSummary {
                    passed: false,
                    tool: "eda-runner".to_string(),
                    command: "digital simulation".to_string(),
                    exit_code: None,
                    stdout: String::new(),
                    stderr: error.to_string(),
                    artifacts: Vec::new(),
                    analysis: None,
                };
                vec![summary]
            }
        };

        for summary in &summaries {
            emit(
                &sink,
                if summary.passed {
                    EventLevel::Info
                } else {
                    EventLevel::Error
                },
                format!(
                    "{} 完成：通过={}",
                    summary.tool,
                    if summary.passed { "是" } else { "否" }
                ),
                Some(json!(summary)),
            );
        }

        if let Some(analysis) = summaries
            .iter()
            .find_map(|summary| summary.analysis.as_ref())
        {
            emit(
                &sink,
                if analysis.overall_passed {
                    EventLevel::Info
                } else {
                    EventLevel::Warning
                },
                "已生成结构化验证分析",
                Some(json!(analysis)),
            );
        }

        let mut repair = None;
        if summaries.iter().any(|summary| !summary.passed) && request.max_repair_rounds > 0 {
            emit(
                &sink,
                EventLevel::Warning,
                "正在请求反馈代理生成修复建议",
                None,
            );
            let suggestion = self.repair_agent.suggest(&summaries).await?;
            emit(
                &sink,
                EventLevel::Warning,
                "已生成修复建议",
                Some(json!(suggestion)),
            );
            repair = Some(suggestion);
        }

        emit(
            &sink,
            EventLevel::Info,
            "设计流程完成",
            Some(json!({ "job_id": job_id })),
        );

        Ok(DesignFlowOutput {
            package,
            summaries: std::mem::take(&mut summaries),
            repair,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DesignFlowOutput {
    pub package: DesignPackage,
    pub summaries: Vec<SimulationSummary>,
    pub repair: Option<RepairSuggestion>,
}

fn emit(
    sink: &AgentEventSink,
    level: EventLevel,
    message: impl Into<String>,
    data: Option<serde_json::Value>,
) {
    sink(AgentEvent {
        level,
        message: message.into(),
        data,
    });
}

async fn write_artifacts(project_root: &Path, artifacts: &[DesignArtifact]) -> Result<()> {
    for artifact in artifacts {
        let relative = sanitize_relative_path(&artifact.path)?;
        let path = project_root.join(relative);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, artifact.content.as_bytes()).await?;
    }

    Ok(())
}

fn package_to_plan(
    project_root: &Path,
    package: &DesignPackage,
    request: &DesignRequest,
) -> Result<SimulationPlan> {
    let rtl_files = artifacts_by_kind(package, ArtifactKind::Rtl)?;
    let testbench_files = artifacts_by_kind(package, ArtifactKind::Testbench)?;
    let top = infer_top(&testbench_files);
    let synthesis_top = infer_top(&rtl_files);

    Ok(SimulationPlan {
        project_root: project_root.to_path_buf(),
        top,
        synthesis_top,
        rtl_files,
        testbench_files,
        use_yosys: request.use_yosys,
        generate_waveform: request.generate_waveform,
        generate_kicad: request.generate_kicad,
    })
}

fn artifacts_by_kind(package: &DesignPackage, kind: ArtifactKind) -> Result<Vec<PathBuf>> {
    let files = package
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == kind)
        .map(|artifact| sanitize_relative_path(&artifact.path))
        .collect::<Result<Vec<_>>>()?;

    if files.is_empty() {
        return Err(anyhow!(
            "package does not contain required artifact kind: {kind:?}"
        ));
    }

    Ok(files)
}

fn sanitize_relative_path(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(anyhow!(
            "artifact path must be relative and stay inside project: {path:?}"
        ));
    }
    Ok(path)
}

fn infer_top(rtl_files: &[PathBuf]) -> Option<String> {
    rtl_files
        .first()
        .and_then(|path| path.file_stem())
        .map(|stem| stem.to_string_lossy().to_string())
}

#[allow(dead_code)]
fn _job_event_example(job_id: JobId) -> JobEvent {
    JobEvent::new(job_id, 0, EventLevel::Info, "example")
}
