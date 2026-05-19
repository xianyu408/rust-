use chrono::Utc;
use domain::{EventLevel, Job, JobEvent, JobId, JobStatus, Project, ProjectId};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::{broadcast, RwLock};

#[derive(Clone)]
pub struct AppState {
    pub workspaces_root: PathBuf,
    projects: Arc<RwLock<HashMap<ProjectId, Project>>>,
    jobs: Arc<RwLock<HashMap<JobId, Job>>>,
    events: Arc<RwLock<HashMap<JobId, Vec<JobEvent>>>>,
    senders: Arc<RwLock<HashMap<JobId, broadcast::Sender<JobEvent>>>>,
}

impl AppState {
    pub fn new(workspaces_root: PathBuf) -> Self {
        Self {
            workspaces_root,
            projects: Arc::new(RwLock::new(HashMap::new())),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(HashMap::new())),
            senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn insert_project(&self, project: Project) {
        self.projects.write().await.insert(project.id, project);
    }

    pub async fn get_project(&self, id: ProjectId) -> Option<Project> {
        self.projects.read().await.get(&id).cloned()
    }

    pub async fn insert_job(&self, job: Job) {
        let (sender, _) = broadcast::channel(256);
        self.senders.write().await.insert(job.id, sender);
        self.events.write().await.insert(job.id, Vec::new());
        self.jobs.write().await.insert(job.id, job);
    }

    pub async fn get_job(&self, id: JobId) -> Option<Job> {
        self.jobs.read().await.get(&id).cloned()
    }

    pub async fn set_job_status(&self, id: JobId, status: JobStatus) {
        if let Some(job) = self.jobs.write().await.get_mut(&id) {
            job.status = status;
            job.updated_at = Utc::now();
        }
    }

    pub async fn push_event(
        &self,
        job_id: JobId,
        level: EventLevel,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) {
        let mut events = self.events.write().await;
        let list = events.entry(job_id).or_default();
        let sequence = list.len() as u64 + 1;
        let mut event = JobEvent::new(job_id, sequence, level, message);
        event.data = data;
        list.push(event.clone());
        drop(events);

        if let Some(sender) = self.senders.read().await.get(&job_id) {
            let _ = sender.send(event);
        }
    }

    pub async fn subscribe_events(
        &self,
        job_id: JobId,
    ) -> Option<(Vec<JobEvent>, broadcast::Receiver<JobEvent>)> {
        let history = self.events.read().await.get(&job_id).cloned()?;
        let receiver = self.senders.read().await.get(&job_id)?.subscribe();
        Some((history, receiver))
    }

    pub async fn get_events(&self, job_id: JobId) -> Option<Vec<JobEvent>> {
        self.events.read().await.get(&job_id).cloned()
    }
}
