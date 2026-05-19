use crate::state::AppState;
use agent_core::{
    AgentEvent, AgentEventSink, DesignOrchestrator, HeuristicRepairAgent, HybridDesignAgent,
    RepairAgent,
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    Json,
};
use domain::{
    CreateProjectRequest, DesignRequest, EventLevel, Job, JobCreated, JobId, JobKind, JobStatus,
    Project, ProjectId, RepairRequest, SimulateRequest, SimulationSummary,
};
use eda_runner::{EdaRunner, SimulationPlan};
use futures::{stream, Stream, StreamExt};
use serde_json::json;
use std::{convert::Infallible, path::PathBuf, sync::Arc};
use tokio_stream::wrappers::BroadcastStream;

pub async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

pub async fn health() -> Json<serde_json::Value> {
    let availability = EdaRunner::default().availability().await;
    Json(json!({
        "status": "ok",
        "tools": availability
    }))
}

pub async fn create_project(
    State(state): State<AppState>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<Json<Project>, ApiError> {
    let id = uuid::Uuid::new_v4();
    let name = request
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("chip-project-{}", &id.to_string()[..8]));
    let root = state.workspaces_root.join(id.to_string());
    tokio::fs::create_dir_all(root.join("src")).await?;
    tokio::fs::create_dir_all(root.join("tb")).await?;
    tokio::fs::create_dir_all(root.join("runs")).await?;
    tokio::fs::create_dir_all(root.join("reports")).await?;

    let project = Project {
        id,
        name,
        root,
        created_at: chrono::Utc::now(),
    };

    state.insert_project(project.clone()).await;
    Ok(Json(project))
}

pub async fn get_project(
    State(state): State<AppState>,
    Path(project_id): Path<ProjectId>,
) -> Result<Json<Project>, ApiError> {
    state
        .get_project(project_id)
        .await
        .map(Json)
        .ok_or(ApiError::not_found("project not found"))
}

pub async fn start_design(
    State(state): State<AppState>,
    Path(project_id): Path<ProjectId>,
    Json(request): Json<DesignRequest>,
) -> Result<Json<JobCreated>, ApiError> {
    let project = state
        .get_project(project_id)
        .await
        .ok_or(ApiError::not_found("project not found"))?;

    let job = Job::new(project_id, JobKind::Design);
    let response = JobCreated {
        job_id: job.id,
        project_id,
        status: job.status.clone(),
    };
    state.insert_job(job.clone()).await;
    state
        .push_event(job.id, EventLevel::Info, "设计任务已排队", None)
        .await;

    tokio::spawn(run_design_job(state.clone(), project, job.id, request));

    Ok(Json(response))
}

pub async fn start_simulation(
    State(state): State<AppState>,
    Path(project_id): Path<ProjectId>,
    Json(request): Json<SimulateRequest>,
) -> Result<Json<JobCreated>, ApiError> {
    let project = state
        .get_project(project_id)
        .await
        .ok_or(ApiError::not_found("project not found"))?;

    let job = Job::new(project_id, JobKind::Simulate);
    let response = JobCreated {
        job_id: job.id,
        project_id,
        status: job.status.clone(),
    };
    state.insert_job(job.clone()).await;
    state
        .push_event(job.id, EventLevel::Info, "仿真任务已排队", None)
        .await;

    tokio::spawn(run_simulation_job(state.clone(), project, job.id, request));

    Ok(Json(response))
}

pub async fn start_repair(
    State(state): State<AppState>,
    Path(project_id): Path<ProjectId>,
    Json(request): Json<RepairRequest>,
) -> Result<Json<JobCreated>, ApiError> {
    let _project = state
        .get_project(project_id)
        .await
        .ok_or(ApiError::not_found("project not found"))?;
    let source_job = state
        .get_job(request.job_id)
        .await
        .ok_or(ApiError::not_found("source job not found"))?;
    if source_job.project_id != project_id {
        return Err(ApiError::bad_request(
            "source job does not belong to project",
        ));
    }

    let job = Job::new(project_id, JobKind::Repair);
    let response = JobCreated {
        job_id: job.id,
        project_id,
        status: job.status.clone(),
    };
    state.insert_job(job.clone()).await;
    state
        .push_event(
            job.id,
            EventLevel::Info,
            "修复任务已排队",
            Some(json!({ "source_job_id": request.job_id, "max_repair_rounds": request.max_repair_rounds })),
        )
        .await;

    tokio::spawn(run_repair_job(state.clone(), job.id, request.job_id));

    Ok(Json(response))
}

pub async fn get_job(
    State(state): State<AppState>,
    Path(job_id): Path<JobId>,
) -> Result<Json<Job>, ApiError> {
    state
        .get_job(job_id)
        .await
        .map(Json)
        .ok_or(ApiError::not_found("job not found"))
}

pub async fn job_events(
    State(state): State<AppState>,
    Path(job_id): Path<JobId>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (history, receiver) = state
        .subscribe_events(job_id)
        .await
        .ok_or(ApiError::not_found("job not found"))?;

    let history_stream = stream::iter(history.into_iter().map(sse_event));
    let live_stream = BroadcastStream::new(receiver).filter_map(|event| async move {
        match event {
            Ok(event) => Some(sse_event(event)),
            Err(_) => None,
        }
    });

    Ok(Sse::new(history_stream.chain(live_stream)).keep_alive(KeepAlive::default()))
}

pub async fn list_artifacts(
    State(state): State<AppState>,
    Path(project_id): Path<ProjectId>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let project = state
        .get_project(project_id)
        .await
        .ok_or(ApiError::not_found("project not found"))?;

    let mut files = Vec::new();
    collect_files(&project.root, &project.root, &mut files).await?;

    Ok(Json(json!({ "project_id": project_id, "files": files })))
}

pub async fn read_project_file(
    State(state): State<AppState>,
    Path((project_id, path)): Path<(ProjectId, String)>,
) -> Result<Response, ApiError> {
    let project = state
        .get_project(project_id)
        .await
        .ok_or(ApiError::not_found("project not found"))?;

    let relative = sanitize_path(&path)?;
    let full_path = project.root.join(relative);
    let bytes = tokio::fs::read(&full_path).await?;
    let content_type = match full_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
    {
        Some(ext) if ext == "svg" => "image/svg+xml; charset=utf-8",
        Some(ext) if ext == "dot" => "text/vnd.graphviz; charset=utf-8",
        Some(ext) if ext == "md" => "text/markdown; charset=utf-8",
        Some(ext) if ext == "json" => "application/json; charset=utf-8",
        Some(ext)
            if ext == "ps1" || ext == "v" || ext == "sv" || ext == "kicad_sch" || ext == "blif" =>
        {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    };

    let mut response = (StatusCode::OK, bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    Ok(response)
}

async fn run_design_job(state: AppState, project: Project, job_id: JobId, request: DesignRequest) {
    state.set_job_status(job_id, JobStatus::Running).await;
    state
        .push_event(
            job_id,
            EventLevel::Info,
            "设计任务已开始",
            Some(json!({ "prompt": request.prompt.clone() })),
        )
        .await;

    let orchestrator = DesignOrchestrator::new(
        HybridDesignAgent::default(),
        HeuristicRepairAgent::default(),
        EdaRunner::default(),
    );

    let event_state = state.clone();
    let sink: AgentEventSink = Arc::new(move |event: AgentEvent| {
        let event_state = event_state.clone();
        tokio::spawn(async move {
            event_state
                .push_event(job_id, event.level, event.message, event.data)
                .await;
        });
    });

    let result = orchestrator
        .run_design_flow(job_id, &project.root, &request, sink)
        .await;

    match result {
        Ok(output) => {
            let passed = output.summaries.iter().all(|summary| summary.passed);
            state
                .push_event(
                    job_id,
                    EventLevel::Info,
                    "设计任务已完成",
                    Some(json!({
                        "passed": passed,
                        "artifact_count": output.package.artifacts.len(),
                        "repair": output.repair,
                    })),
                )
                .await;
            state
                .set_job_status(
                    job_id,
                    if passed {
                        JobStatus::Passed
                    } else {
                        JobStatus::Failed
                    },
                )
                .await;
        }
        Err(error) => {
            state
                .push_event(
                    job_id,
                    EventLevel::Error,
                    "设计任务失败",
                    Some(json!({ "error": error.to_string() })),
                )
                .await;
            state.set_job_status(job_id, JobStatus::Failed).await;
        }
    }
}

async fn run_simulation_job(
    state: AppState,
    project: Project,
    job_id: JobId,
    request: SimulateRequest,
) {
    state.set_job_status(job_id, JobStatus::Running).await;
    state
        .push_event(
            job_id,
            EventLevel::Info,
            "仿真任务已开始",
            Some(json!(&request)),
        )
        .await;

    let plan = SimulationPlan {
        project_root: project.root.clone(),
        top: request.top,
        synthesis_top: request.synthesis_top,
        rtl_files: request.rtl_files.iter().map(PathBuf::from).collect(),
        testbench_files: request.testbench_files.iter().map(PathBuf::from).collect(),
        use_yosys: request.use_yosys,
        generate_waveform: request.generate_waveform,
        generate_kicad: request.generate_kicad,
    };

    match EdaRunner::default().run_digital_simulation(&plan).await {
        Ok(summaries) => {
            let passed = summaries.iter().all(|summary| summary.passed);
            for summary in &summaries {
                let status = if summary.passed { "通过" } else { "失败" };
                state
                    .push_event(
                        job_id,
                        if summary.passed {
                            EventLevel::Info
                        } else {
                            EventLevel::Error
                        },
                        format!("{} 完成：通过={}", summary.tool, status),
                        Some(json!(summary)),
                    )
                    .await;
            }
            if let Some(analysis) = summaries
                .iter()
                .find_map(|summary| summary.analysis.as_ref())
            {
                state
                    .push_event(
                        job_id,
                        if analysis.overall_passed {
                            EventLevel::Info
                        } else {
                            EventLevel::Warning
                        },
                        "已生成结构化验证分析",
                        Some(json!(analysis)),
                    )
                    .await;
            }
            state
                .set_job_status(
                    job_id,
                    if passed {
                        JobStatus::Passed
                    } else {
                        JobStatus::Failed
                    },
                )
                .await;
        }
        Err(error) => {
            state
                .push_event(
                    job_id,
                    EventLevel::Error,
                    "仿真任务失败",
                    Some(json!({ "error": error.to_string() })),
                )
                .await;
            state.set_job_status(job_id, JobStatus::Failed).await;
        }
    }
}

async fn run_repair_job(state: AppState, job_id: JobId, source_job_id: JobId) {
    state.set_job_status(job_id, JobStatus::Running).await;
    state
        .push_event(
            job_id,
            EventLevel::Info,
            "修复任务已开始",
            Some(json!({ "source_job_id": source_job_id })),
        )
        .await;

    let Some(events) = state.get_events(source_job_id).await else {
        state
            .push_event(job_id, EventLevel::Error, "未找到源任务事件", None)
            .await;
        state.set_job_status(job_id, JobStatus::Failed).await;
        return;
    };

    let summaries = events
        .into_iter()
        .filter_map(|event| event.data)
        .filter_map(|data| serde_json::from_value::<SimulationSummary>(data).ok())
        .collect::<Vec<_>>();

    if summaries.is_empty() {
        state
            .push_event(job_id, EventLevel::Error, "源任务中未找到仿真摘要", None)
            .await;
        state.set_job_status(job_id, JobStatus::Failed).await;
        return;
    }

    match HeuristicRepairAgent::default().suggest(&summaries).await {
        Ok(suggestion) => {
            state
                .push_event(
                    job_id,
                    EventLevel::Warning,
                    "已生成修复建议",
                    Some(json!(suggestion)),
                )
                .await;
            state.set_job_status(job_id, JobStatus::Passed).await;
        }
        Err(error) => {
            state
                .push_event(
                    job_id,
                    EventLevel::Error,
                    "修复任务失败",
                    Some(json!({ "error": error.to_string() })),
                )
                .await;
            state.set_job_status(job_id, JobStatus::Failed).await;
        }
    }
}

fn sse_event(job_event: domain::JobEvent) -> Result<Event, Infallible> {
    let sequence = job_event.sequence.to_string();
    let sse = Event::default().event("job_event").id(sequence);
    let sse = sse
        .json_data(job_event)
        .unwrap_or_else(|_| Event::default().event("job_event").data("{}"));
    Ok(sse)
}

async fn collect_files(
    root: &std::path::Path,
    current: &std::path::Path,
    files: &mut Vec<String>,
) -> std::io::Result<()> {
    let mut stack = vec![current.to_path_buf()];

    while let Some(dir_path) = stack.pop() {
        let mut dir = tokio::fs::read_dir(&dir_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    Ok(())
}

fn sanitize_path(path: &str) -> Result<PathBuf, ApiError> {
    let path = PathBuf::from(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ApiError::bad_request("路径必须保留在项目工作区内"));
    }
    Ok(path)
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": self.message }));
        (self.status, body).into_response()
    }
}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: value.to_string(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: value.to_string(),
        }
    }
}
