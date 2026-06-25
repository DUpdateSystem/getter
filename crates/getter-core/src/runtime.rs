//! In-memory Phase D runtime for getter-owned update/download/install tasks.
//!
//! This module implements the ADR-0011 task/action/runtime-notification shape.
//! It deliberately keeps task state in the current process only: no SQLite task
//! persistence, no daemon, no recovery, and no replayable event log. Product
//! embedders can hold this runtime as a process-lifetime singleton and expose
//! [`RuntimeNotification`] through a push stream.

use crate::{PackageId, UpdateAction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageVersionLuaObject {
    pub object_id: String,
    pub dependency_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedActionPlan {
    pub package_id: PackageId,
    #[serde(default)]
    pub actions: Vec<UpdateAction>,
    pub lua_object: PackageVersionLuaObject,
}

impl SealedActionPlan {
    fn has_install_action(&self) -> bool {
        self.actions
            .iter()
            .any(|action| matches!(action, UpdateAction::Install { .. }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedAction {
    pub action_id: String,
    pub package_id: PackageId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub package_id: PackageId,
    pub status: RuntimeTaskStatus,
    pub phase: TaskPhase,
    #[serde(default)]
    pub progress: Option<TaskProgress>,
    pub capabilities: TaskCapabilities,
    #[serde(default)]
    pub current_diagnostic: Option<TaskDiagnostic>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTaskStatus {
    Queued,
    Running,
    Paused,
    Failed,
    Completed,
    Canceled,
}

impl RuntimeTaskStatus {
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPhase {
    pub category: TaskPhaseCategory,
    #[serde(default)]
    pub reason: Option<TaskPhaseReason>,
}

impl TaskPhase {
    pub const fn new(category: TaskPhaseCategory) -> Self {
        Self {
            category,
            reason: None,
        }
    }

    pub const fn with_reason(category: TaskPhaseCategory, reason: TaskPhaseReason) -> Self {
        Self {
            category,
            reason: Some(reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhaseCategory {
    Queued,
    Download,
    WaitingUser,
    Install,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhaseReason {
    InstallHandoff,
    PackageLocked,
    UserRejected,
    DownloadFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskProgress {
    pub unit: TaskProgressUnit,
    pub current: u64,
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskProgressUnit {
    Percent,
    Bit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCapabilities {
    pub cancel: bool,
    pub pause: bool,
    pub resume: bool,
    pub retry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDiagnostic {
    pub code: String,
    pub message: String,
    pub severity: DiagnosticSeverity,
}

impl TaskDiagnostic {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            severity: DiagnosticSeverity::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeNotification {
    TaskChanged { task: TaskSnapshot },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserResult {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCleanMode {
    /// Remove completed and canceled tasks only.
    Default,
    /// Remove failed tasks only.
    Failed,
    /// Remove completed, canceled, and failed tasks.
    AllInactive,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("update action is no longer available: {0}")]
    ActionNotFound(String),
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error("task is not waiting for a user result: {0}")]
    TaskNotWaitingForUser(String),
    #[error("task cannot be paused in its current state: {0}")]
    PauseNotSupported(String),
    #[error("task cannot be resumed in its current state: {0}")]
    ResumeNotSupported(String),
    #[error("task cannot be retried in its current state: {0}")]
    RetryNotSupported(String),
    #[error("task cannot be canceled in its current state: {0}")]
    CancelNotSupported(String),
    #[error("task is active and must be canceled before removal: {0}")]
    TaskActive(String),
}

impl RuntimeError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ActionNotFound(_) => "action.not_found",
            Self::TaskNotFound(_) => "task.not_found",
            Self::TaskNotWaitingForUser(_) => "task.not_waiting_for_user",
            Self::PauseNotSupported(_) => "task.pause_not_supported",
            Self::ResumeNotSupported(_) => "task.resume_not_supported",
            Self::RetryNotSupported(_) => "task.retry_not_supported",
            Self::CancelNotSupported(_) => "task.cancel_not_supported",
            Self::TaskActive(_) => "task.active",
        }
    }
}

type RuntimeNotificationSink = Box<dyn FnMut(RuntimeNotification) + Send>;

#[derive(Default)]
pub struct GetterRuntime {
    next_action_id: u64,
    next_task_id: u64,
    logical_clock: u64,
    actions: HashMap<String, SealedActionPlan>,
    tasks: BTreeMap<String, RuntimeTask>,
    package_locks: HashMap<PackageId, String>,
    notification_sink: Option<RuntimeNotificationSink>,
}

impl GetterRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_notification_sink(
        &mut self,
        sink: impl FnMut(RuntimeNotification) + Send + 'static,
    ) {
        self.notification_sink = Some(Box::new(sink));
    }

    pub fn issue_action(&mut self, plan: SealedActionPlan) -> IssuedAction {
        self.next_action_id += 1;
        let action_id = format!("action-{}", self.next_action_id);
        let package_id = plan.package_id.clone();
        self.actions.insert(action_id.clone(), plan);
        IssuedAction {
            action_id,
            package_id,
        }
    }

    pub fn submit_action(&mut self, action_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        let plan = self
            .actions
            .remove(action_id)
            .ok_or_else(|| RuntimeError::ActionNotFound(action_id.to_owned()))?;
        self.next_task_id += 1;
        let task_id = format!("task-{}", self.next_task_id);
        let task = RuntimeTask::new(task_id.clone(), plan, self.tick());
        let snapshot = task.snapshot();
        self.tasks.insert(task_id, task);
        self.notify_task_changed(snapshot.clone());
        Ok(snapshot)
    }

    pub fn task(&self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        Ok(self.task_ref(task_id)?.snapshot())
    }

    pub fn tasks(&self) -> Vec<TaskSnapshot> {
        self.tasks.values().map(RuntimeTask::snapshot).collect()
    }

    pub fn active_tasks(&self) -> Vec<TaskSnapshot> {
        self.tasks
            .values()
            .filter(|task| task.status.is_active())
            .map(RuntimeTask::snapshot)
            .collect()
    }

    pub fn tasks_for_package(&self, package_id: &PackageId) -> Vec<TaskSnapshot> {
        self.tasks
            .values()
            .filter(|task| &task.plan.package_id == package_id)
            .map(RuntimeTask::snapshot)
            .collect()
    }

    pub fn start_task(&mut self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        {
            let clock = self.tick();
            let task = self.task_mut(task_id)?;
            match task.status {
                RuntimeTaskStatus::Queued => task.start_download(clock),
                RuntimeTaskStatus::Running => {}
                _ => return Err(RuntimeError::PauseNotSupported(task_id.to_owned())),
            }
        }
        self.snapshot_and_notify(task_id)
    }

    pub fn set_download_progress(
        &mut self,
        task_id: &str,
        current_bits: u64,
        total_bits: Option<u64>,
    ) -> Result<TaskSnapshot, RuntimeError> {
        {
            let clock = self.tick();
            let task = self.task_mut(task_id)?;
            if !task.is_running_download() {
                return Err(RuntimeError::PauseNotSupported(task_id.to_owned()));
            }
            task.progress = Some(TaskProgress {
                unit: TaskProgressUnit::Bit,
                current: current_bits,
                total: total_bits,
            });
            task.updated_at = clock;
        }
        self.snapshot_and_notify(task_id)
    }

    pub fn complete_download(&mut self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        let needs_install = {
            let task = self.task_ref(task_id)?;
            task.plan.has_install_action()
        };

        if needs_install {
            self.enter_install_handoff(task_id)
        } else {
            {
                let clock = self.tick();
                let task = self.task_mut(task_id)?;
                if !matches!(
                    task.status,
                    RuntimeTaskStatus::Queued | RuntimeTaskStatus::Running
                ) {
                    return Err(RuntimeError::RetryNotSupported(task_id.to_owned()));
                }
                task.complete(clock);
            }
            self.snapshot_and_notify(task_id)
        }
    }

    pub fn pause_task(&mut self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        {
            let clock = self.tick();
            let task = self.task_mut(task_id)?;
            if !task.is_running_download() {
                return Err(RuntimeError::PauseNotSupported(task_id.to_owned()));
            }
            task.status = RuntimeTaskStatus::Paused;
            task.updated_at = clock;
        }
        self.snapshot_and_notify(task_id)
    }

    pub fn resume_task(&mut self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        {
            let clock = self.tick();
            let task = self.task_mut(task_id)?;
            if task.status != RuntimeTaskStatus::Paused {
                return Err(RuntimeError::ResumeNotSupported(task_id.to_owned()));
            }
            task.status = RuntimeTaskStatus::Running;
            task.phase = TaskPhase::new(TaskPhaseCategory::Download);
            task.updated_at = clock;
        }
        self.snapshot_and_notify(task_id)
    }

    pub fn user_result(
        &mut self,
        task_id: &str,
        result: UserResult,
        reason: Option<&str>,
    ) -> Result<TaskSnapshot, RuntimeError> {
        {
            let task = self.task_ref(task_id)?;
            if !task.is_waiting_user() {
                return Err(RuntimeError::TaskNotWaitingForUser(task_id.to_owned()));
            }
        }
        self.unlock_package_for_task(task_id);
        {
            let clock = self.tick();
            let task = self.task_mut(task_id)?;
            match result {
                UserResult::Accepted => task.complete(clock),
                UserResult::Rejected => task.fail(
                    clock,
                    TaskPhase::with_reason(
                        TaskPhaseCategory::Failed,
                        TaskPhaseReason::UserRejected,
                    ),
                    TaskDiagnostic::error(
                        "user.rejected",
                        reason.unwrap_or("User rejected the pending step"),
                    ),
                    RetryResume::InstallHandoff,
                ),
            }
        }
        self.snapshot_and_notify(task_id)
    }

    pub fn cancel_task(&mut self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        {
            let task = self.task_ref(task_id)?;
            if !matches!(
                task.status,
                RuntimeTaskStatus::Queued | RuntimeTaskStatus::Running | RuntimeTaskStatus::Paused
            ) {
                return Err(RuntimeError::CancelNotSupported(task_id.to_owned()));
            }
        }
        self.unlock_package_for_task(task_id);
        {
            let clock = self.tick();
            let task = self.task_mut(task_id)?;
            task.cancel(clock);
        }
        self.snapshot_and_notify(task_id)
    }

    pub fn retry_task(&mut self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        let retry_resume = {
            let task = self.task_ref(task_id)?;
            if task.status != RuntimeTaskStatus::Failed {
                return Err(RuntimeError::RetryNotSupported(task_id.to_owned()));
            }
            task.retry_resume
        };
        match retry_resume {
            RetryResume::Download => {
                {
                    let clock = self.tick();
                    let task = self.task_mut(task_id)?;
                    task.start_download(clock);
                }
                self.snapshot_and_notify(task_id)
            }
            RetryResume::InstallHandoff => self.enter_install_handoff(task_id),
        }
    }

    pub fn remove_task(&mut self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        let task = self.task_ref(task_id)?;
        if task.status.is_active() {
            return Err(RuntimeError::TaskActive(task_id.to_owned()));
        }
        let removed = self
            .tasks
            .remove(task_id)
            .expect("task existence checked before remove");
        Ok(removed.snapshot())
    }

    pub fn clean_tasks(&mut self, mode: TaskCleanMode) -> Vec<TaskSnapshot> {
        let task_ids: Vec<String> = self
            .tasks
            .iter()
            .filter_map(|(task_id, task)| {
                let remove = match mode {
                    TaskCleanMode::Default => matches!(
                        task.status,
                        RuntimeTaskStatus::Completed | RuntimeTaskStatus::Canceled
                    ),
                    TaskCleanMode::Failed => task.status == RuntimeTaskStatus::Failed,
                    TaskCleanMode::AllInactive => !task.status.is_active(),
                };
                remove.then(|| task_id.clone())
            })
            .collect();

        task_ids
            .into_iter()
            .filter_map(|task_id| {
                let removed = self.tasks.remove(&task_id)?;
                Some(removed.snapshot())
            })
            .collect()
    }

    fn enter_install_handoff(&mut self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        let package_id = self.task_ref(task_id)?.plan.package_id.clone();
        if self.package_locks.contains_key(&package_id) {
            {
                let clock = self.tick();
                let task = self.task_mut(task_id)?;
                task.fail(
                    clock,
                    TaskPhase::with_reason(
                        TaskPhaseCategory::Install,
                        TaskPhaseReason::PackageLocked,
                    ),
                    TaskDiagnostic::error(
                        "package.locked",
                        format!(
                            "Package {} is already being modified by another task",
                            package_id
                        ),
                    ),
                    RetryResume::InstallHandoff,
                );
            }
            return self.snapshot_and_notify(task_id);
        }

        self.package_locks.insert(package_id, task_id.to_owned());
        {
            let clock = self.tick();
            let task = self.task_mut(task_id)?;
            task.enter_waiting_install(clock);
        }
        self.snapshot_and_notify(task_id)
    }

    fn unlock_package_for_task(&mut self, task_id: &str) {
        self.package_locks
            .retain(|_, locked_by_task| locked_by_task != task_id);
    }

    fn snapshot_and_notify(&mut self, task_id: &str) -> Result<TaskSnapshot, RuntimeError> {
        let snapshot = self.task(task_id)?;
        self.notify_task_changed(snapshot.clone());
        Ok(snapshot)
    }

    fn notify_task_changed(&mut self, task: TaskSnapshot) {
        if let Some(sink) = self.notification_sink.as_mut() {
            sink(RuntimeNotification::TaskChanged { task });
        }
    }

    fn task_ref(&self, task_id: &str) -> Result<&RuntimeTask, RuntimeError> {
        self.tasks
            .get(task_id)
            .ok_or_else(|| RuntimeError::TaskNotFound(task_id.to_owned()))
    }

    fn task_mut(&mut self, task_id: &str) -> Result<&mut RuntimeTask, RuntimeError> {
        self.tasks
            .get_mut(task_id)
            .ok_or_else(|| RuntimeError::TaskNotFound(task_id.to_owned()))
    }

    fn tick(&mut self) -> u64 {
        self.logical_clock += 1;
        self.logical_clock
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryResume {
    Download,
    InstallHandoff,
}

#[derive(Debug, Clone)]
struct RuntimeTask {
    id: String,
    plan: SealedActionPlan,
    status: RuntimeTaskStatus,
    phase: TaskPhase,
    progress: Option<TaskProgress>,
    current_diagnostic: Option<TaskDiagnostic>,
    retry_resume: RetryResume,
    updated_at: u64,
}

impl RuntimeTask {
    fn new(id: String, plan: SealedActionPlan, updated_at: u64) -> Self {
        Self {
            id,
            plan,
            status: RuntimeTaskStatus::Queued,
            phase: TaskPhase::new(TaskPhaseCategory::Queued),
            progress: None,
            current_diagnostic: None,
            retry_resume: RetryResume::Download,
            updated_at,
        }
    }

    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            task_id: self.id.clone(),
            package_id: self.plan.package_id.clone(),
            status: self.status,
            phase: self.phase.clone(),
            progress: self.progress.clone(),
            capabilities: self.capabilities(),
            current_diagnostic: self.current_diagnostic.clone(),
            updated_at: self.updated_at,
        }
    }

    fn capabilities(&self) -> TaskCapabilities {
        TaskCapabilities {
            cancel: matches!(
                self.status,
                RuntimeTaskStatus::Queued | RuntimeTaskStatus::Running | RuntimeTaskStatus::Paused
            ),
            pause: self.is_running_download(),
            resume: self.status == RuntimeTaskStatus::Paused,
            retry: self.status == RuntimeTaskStatus::Failed,
        }
    }

    fn is_running_download(&self) -> bool {
        self.status == RuntimeTaskStatus::Running
            && self.phase.category == TaskPhaseCategory::Download
    }

    fn is_waiting_user(&self) -> bool {
        self.status == RuntimeTaskStatus::Running
            && self.phase
                == TaskPhase::with_reason(
                    TaskPhaseCategory::WaitingUser,
                    TaskPhaseReason::InstallHandoff,
                )
    }

    fn start_download(&mut self, updated_at: u64) {
        self.status = RuntimeTaskStatus::Running;
        self.phase = TaskPhase::new(TaskPhaseCategory::Download);
        self.progress = Some(TaskProgress {
            unit: TaskProgressUnit::Bit,
            current: 0,
            total: None,
        });
        self.current_diagnostic = None;
        self.updated_at = updated_at;
    }

    fn enter_waiting_install(&mut self, updated_at: u64) {
        self.status = RuntimeTaskStatus::Running;
        self.phase = TaskPhase::with_reason(
            TaskPhaseCategory::WaitingUser,
            TaskPhaseReason::InstallHandoff,
        );
        self.progress = None;
        self.current_diagnostic = None;
        self.updated_at = updated_at;
    }

    fn complete(&mut self, updated_at: u64) {
        self.status = RuntimeTaskStatus::Completed;
        self.phase = TaskPhase::new(TaskPhaseCategory::Completed);
        self.progress = None;
        self.current_diagnostic = None;
        self.updated_at = updated_at;
    }

    fn cancel(&mut self, updated_at: u64) {
        self.status = RuntimeTaskStatus::Canceled;
        self.phase = TaskPhase::new(TaskPhaseCategory::Canceled);
        self.progress = None;
        self.current_diagnostic = None;
        self.updated_at = updated_at;
    }

    fn fail(
        &mut self,
        updated_at: u64,
        phase: TaskPhase,
        diagnostic: TaskDiagnostic,
        retry_resume: RetryResume,
    ) {
        self.status = RuntimeTaskStatus::Failed;
        self.phase = phase;
        self.progress = None;
        self.current_diagnostic = Some(diagnostic);
        self.retry_resume = retry_resume;
        self.updated_at = updated_at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PackageKind, UpdateAction};
    use std::sync::{Arc, Mutex};

    #[test]
    fn submit_consumes_action_id_and_pushes_snapshot() {
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let sink_notifications = notifications.clone();
        let mut runtime = GetterRuntime::new();
        runtime.set_notification_sink(move |notification| {
            sink_notifications.lock().unwrap().push(notification);
        });
        let action = runtime.issue_action(plan("android/org.fdroid.fdroid"));

        let task = runtime.submit_action(&action.action_id).unwrap();

        assert_eq!(task.task_id, "task-1");
        assert_eq!(task.status, RuntimeTaskStatus::Queued);
        assert_eq!(task.capabilities.cancel, true);
        let error = runtime.submit_action(&action.action_id).unwrap_err();
        assert_eq!(error.code(), "action.not_found");
        assert_eq!(notifications.lock().unwrap().len(), 1);
    }

    #[test]
    fn fake_install_waits_for_generic_user_result() {
        let mut runtime = GetterRuntime::new();
        let task_id = submit_plan(&mut runtime, plan("android/org.fdroid.fdroid"));

        runtime.start_task(&task_id).unwrap();
        let waiting = runtime.complete_download(&task_id).unwrap();

        assert_eq!(waiting.status, RuntimeTaskStatus::Running);
        assert_eq!(
            waiting.phase,
            TaskPhase::with_reason(
                TaskPhaseCategory::WaitingUser,
                TaskPhaseReason::InstallHandoff
            )
        );
        assert_eq!(waiting.capabilities.pause, false);
        assert_eq!(waiting.capabilities.resume, false);
        assert_eq!(waiting.capabilities.cancel, true);

        let completed = runtime
            .user_result(&task_id, UserResult::Accepted, None)
            .unwrap();
        assert_eq!(completed.status, RuntimeTaskStatus::Completed);
        assert_eq!(
            completed.phase,
            TaskPhase::new(TaskPhaseCategory::Completed)
        );
    }

    #[test]
    fn rejected_user_result_fails_and_retry_reuses_same_task() {
        let mut runtime = GetterRuntime::new();
        let task_id = submit_plan(&mut runtime, plan("android/org.fdroid.fdroid"));
        runtime.start_task(&task_id).unwrap();
        runtime.complete_download(&task_id).unwrap();

        let failed = runtime
            .user_result(&task_id, UserResult::Rejected, Some("not now"))
            .unwrap();

        assert_eq!(failed.status, RuntimeTaskStatus::Failed);
        assert_eq!(failed.capabilities.retry, true);
        assert_eq!(
            failed.current_diagnostic.as_ref().unwrap().code,
            "user.rejected"
        );

        let retried = runtime.retry_task(&task_id).unwrap();
        assert_eq!(retried.task_id, task_id);
        assert_eq!(retried.status, RuntimeTaskStatus::Running);
        assert_eq!(
            retried.phase,
            TaskPhase::with_reason(
                TaskPhaseCategory::WaitingUser,
                TaskPhaseReason::InstallHandoff
            )
        );
    }

    #[test]
    fn user_result_outside_waiting_user_is_an_error_without_mutation() {
        let mut runtime = GetterRuntime::new();
        let task_id = submit_plan(&mut runtime, plan("android/org.fdroid.fdroid"));
        let before = runtime.task(&task_id).unwrap();

        let error = runtime
            .user_result(&task_id, UserResult::Accepted, None)
            .unwrap_err();

        assert_eq!(error.code(), "task.not_waiting_for_user");
        assert_eq!(runtime.task(&task_id).unwrap(), before);
    }

    #[test]
    fn pause_resume_are_download_phase_controls_only() {
        let mut runtime = GetterRuntime::new();
        let task_id = submit_plan(&mut runtime, plan("android/org.fdroid.fdroid"));
        runtime.start_task(&task_id).unwrap();
        runtime
            .set_download_progress(&task_id, 42, Some(100))
            .unwrap();

        let paused = runtime.pause_task(&task_id).unwrap();
        assert_eq!(paused.status, RuntimeTaskStatus::Paused);
        assert_eq!(paused.capabilities.resume, true);
        assert_eq!(paused.capabilities.cancel, true);

        let resumed = runtime.resume_task(&task_id).unwrap();
        assert_eq!(resumed.status, RuntimeTaskStatus::Running);
        assert_eq!(resumed.phase, TaskPhase::new(TaskPhaseCategory::Download));

        runtime.complete_download(&task_id).unwrap();
        let error = runtime.pause_task(&task_id).unwrap_err();
        assert_eq!(error.code(), "task.pause_not_supported");
    }

    #[test]
    fn same_package_install_lock_fails_later_task_without_waiting() {
        let mut runtime = GetterRuntime::new();
        let first_task = submit_plan(&mut runtime, plan("android/org.fdroid.fdroid"));
        let second_task = submit_plan(&mut runtime, plan("android/org.fdroid.fdroid"));

        runtime.start_task(&first_task).unwrap();
        runtime.complete_download(&first_task).unwrap();
        runtime.start_task(&second_task).unwrap();
        let failed = runtime.complete_download(&second_task).unwrap();

        assert_eq!(failed.status, RuntimeTaskStatus::Failed);
        assert_eq!(
            failed.phase,
            TaskPhase::with_reason(TaskPhaseCategory::Install, TaskPhaseReason::PackageLocked)
        );
        assert_eq!(
            failed.current_diagnostic.as_ref().unwrap().code,
            "package.locked"
        );

        runtime
            .user_result(&first_task, UserResult::Accepted, None)
            .unwrap();
        let retried = runtime.retry_task(&second_task).unwrap();
        assert_eq!(retried.status, RuntimeTaskStatus::Running);
        assert_eq!(
            retried.phase,
            TaskPhase::with_reason(
                TaskPhaseCategory::WaitingUser,
                TaskPhaseReason::InstallHandoff
            )
        );
    }

    #[test]
    fn cancel_remove_and_clean_follow_in_memory_task_lifecycle() {
        let mut runtime = GetterRuntime::new();
        let canceled = submit_plan(&mut runtime, plan("android/org.fdroid.fdroid"));
        let failed = submit_plan(&mut runtime, plan("android/com.termux"));
        let completed = submit_plan(&mut runtime, plan_without_install("generic/example"));

        runtime.cancel_task(&canceled).unwrap();
        runtime.start_task(&failed).unwrap();
        runtime.complete_download(&failed).unwrap();
        runtime
            .user_result(&failed, UserResult::Rejected, None)
            .unwrap();
        runtime.start_task(&completed).unwrap();
        runtime.complete_download(&completed).unwrap();

        let removed_by_default = runtime.clean_tasks(TaskCleanMode::Default);
        let removed_ids: Vec<_> = removed_by_default
            .into_iter()
            .map(|task| task.task_id)
            .collect();
        assert!(removed_ids.contains(&canceled));
        assert!(removed_ids.contains(&completed));
        assert!(!removed_ids.contains(&failed));
        assert_eq!(
            runtime.task(&failed).unwrap().status,
            RuntimeTaskStatus::Failed
        );

        let removed_failed = runtime.remove_task(&failed).unwrap();
        assert_eq!(removed_failed.status, RuntimeTaskStatus::Failed);
        assert_eq!(runtime.task(&failed).unwrap_err().code(), "task.not_found");
    }

    #[test]
    fn active_tasks_cannot_be_removed_and_can_be_canceled_while_paused() {
        let mut runtime = GetterRuntime::new();
        let task_id = submit_plan(&mut runtime, plan("android/org.fdroid.fdroid"));
        runtime.start_task(&task_id).unwrap();
        runtime.pause_task(&task_id).unwrap();

        let error = runtime.remove_task(&task_id).unwrap_err();
        assert_eq!(error.code(), "task.active");

        let canceled = runtime.cancel_task(&task_id).unwrap();
        assert_eq!(canceled.status, RuntimeTaskStatus::Canceled);
        runtime.remove_task(&task_id).unwrap();
    }

    fn submit_plan(runtime: &mut GetterRuntime, plan: SealedActionPlan) -> String {
        let action = runtime.issue_action(plan);
        runtime.submit_action(&action.action_id).unwrap().task_id
    }

    fn plan(package_id: &str) -> SealedActionPlan {
        SealedActionPlan {
            package_id: package_id.parse().unwrap(),
            lua_object: PackageVersionLuaObject {
                object_id: format!("lua:{package_id}:1.0"),
                dependency_digest: "sha256-test".to_owned(),
            },
            actions: vec![
                UpdateAction::Download {
                    url: "https://example.invalid/app.apk".to_owned(),
                    file_name: "app.apk".to_owned(),
                },
                UpdateAction::Install {
                    installer: "android_package".to_owned(),
                    file: "app.apk".to_owned(),
                },
            ],
        }
    }

    fn plan_without_install(package_id: &str) -> SealedActionPlan {
        SealedActionPlan {
            package_id: PackageId::new(PackageKind::Generic, package_id.split('/').nth(1).unwrap())
                .unwrap(),
            lua_object: PackageVersionLuaObject {
                object_id: format!("lua:{package_id}:1.0"),
                dependency_digest: "sha256-test".to_owned(),
            },
            actions: vec![UpdateAction::Download {
                url: "https://example.invalid/file.bin".to_owned(),
                file_name: "file.bin".to_owned(),
            }],
        }
    }
}
