//! [`ResiduumA2aHandler`]: wraps `a2a_server::DefaultRequestHandler` with the
//! caller-ownership and one-open-task-per-context rules the SDK doesn't
//! enforce on its own. See `docs/systems-usage/a2a.md`.

use std::sync::Arc;

use a2a::{
    A2AError, AgentCard, CancelTaskRequest, DeleteTaskPushNotificationConfigRequest,
    GetExtendedAgentCardRequest, GetTaskPushNotificationConfigRequest, GetTaskRequest,
    ListTaskPushNotificationConfigsRequest, ListTaskPushNotificationConfigsResponse,
    ListTasksRequest, ListTasksResponse, SendMessageRequest, SendMessageResponse, StreamResponse,
    SubscribeToTaskRequest, Task, TaskPushNotificationConfig, new_task_id,
};
use a2a_server::{DefaultRequestHandler, RequestHandler, ServiceParams, TaskStore as _};
use async_trait::async_trait;
use futures_util::stream::BoxStream;

use super::auth::CALLER_HEADER;
use super::executor::SessionExecutor;
use super::task_store::{
    ADDRESS_METADATA_KEY, CALLER_METADATA_KEY, FileTaskStore, SharedTaskStore,
};

/// Wraps [`DefaultRequestHandler`] with caller ownership checks, `list_tasks`
/// scoped to the caller, follow-up history appends, and one non-terminal
/// task per context — the gaps the SDK leaves to the embedder (see the
/// plan's "SDK gaps we cover ourselves").
pub struct ResiduumA2aHandler {
    inner: DefaultRequestHandler,
    task_store: SharedTaskStore,
}

impl ResiduumA2aHandler {
    /// Wrap `inner`, sharing `task_store` with it so ownership metadata and
    /// the one-open-task-per-context check can be read and written directly,
    /// alongside whatever `inner` itself does with the same store.
    #[must_use]
    pub fn new(inner: DefaultRequestHandler, task_store: SharedTaskStore) -> Self {
        Self { inner, task_store }
    }

    fn caller_of(params: &ServiceParams) -> Result<String, A2AError> {
        params
            .get(CALLER_HEADER)
            .and_then(|v| v.first())
            .cloned()
            .ok_or_else(|| A2AError::invalid_request("missing caller identity"))
    }

    /// Verify `task_id` belongs to the caller in `params`, returning
    /// `task_not_found` (never leaking whether a task exists for someone
    /// else) when it doesn't.
    async fn check_ownership(&self, params: &ServiceParams, task_id: &str) -> Result<(), A2AError> {
        let caller = Self::caller_of(params)?;
        let task = self
            .task_store
            .get(task_id)
            .await?
            .ok_or_else(|| A2AError::task_not_found(task_id))?;
        match FileTaskStore::caller_of(&task) {
            Some(owner) if owner == caller => Ok(()),
            _ => Err(A2AError::task_not_found(task_id)),
        }
    }

    /// Prepare a `send_message`/`send_streaming_message` request: reject a
    /// brand-new task in a context that already has an open one, ensure the
    /// message carries a known `task_id`, append it to an existing task's
    /// history (the SDK doesn't), and check ownership when the task already
    /// exists. Returns the resolved caller, the task id now set on `req`,
    /// and whether the task is brand new (for [`Self::record_ownership`]
    /// after the inner handler creates it).
    async fn prepare_send(
        &self,
        params: &ServiceParams,
        req: &mut SendMessageRequest,
    ) -> Result<(String, String, bool), A2AError> {
        let caller = Self::caller_of(params)?;

        if req.message.task_id.is_none()
            && let Some(context_id) = req.message.context_id.clone()
            && let Some(open) = self.task_store.open_task_in_context(&context_id).await
        {
            return Err(A2AError::invalid_request(format!(
                "context {context_id} already has an open task ({}); send a follow-up with that \
                 task_id, or start a new context",
                open.id
            )));
        }

        let task_id = req.message.task_id.clone().unwrap_or_else(new_task_id);
        req.message.task_id = Some(task_id.clone());

        match self.task_store.get(&task_id).await? {
            Some(mut existing) => {
                match FileTaskStore::caller_of(&existing) {
                    Some(owner) if owner == caller => {}
                    _ => return Err(A2AError::task_not_found(&task_id)),
                }
                existing
                    .history
                    .get_or_insert_with(Vec::new)
                    .push(req.message.clone());
                self.task_store.update(existing).await?;
                Ok((caller, task_id, false))
            }
            None => Ok((caller, task_id, true)),
        }
    }

    /// Record `residuum.caller`/`residuum.address` metadata on a task the
    /// inner handler just created. Best-effort: if the task has somehow
    /// already vanished, there's nothing to attribute ownership to.
    async fn record_ownership(&self, task_id: &str, caller: &str) -> Result<(), A2AError> {
        let Some(mut task) = self.task_store.get(task_id).await? else {
            return Ok(());
        };
        let address = SessionExecutor::address_for(caller, &task.context_id);
        let metadata = task
            .metadata
            .get_or_insert_with(std::collections::HashMap::new);
        metadata.insert(
            CALLER_METADATA_KEY.to_string(),
            serde_json::Value::String(caller.to_string()),
        );
        metadata.insert(
            ADDRESS_METADATA_KEY.to_string(),
            serde_json::Value::String(address.to_string()),
        );
        self.task_store.update(task).await?;
        Ok(())
    }
}

#[async_trait]
impl RequestHandler for ResiduumA2aHandler {
    async fn send_message(
        &self,
        params: &ServiceParams,
        mut req: SendMessageRequest,
    ) -> Result<SendMessageResponse, A2AError> {
        let (caller, task_id, is_new) = self.prepare_send(params, &mut req).await?;
        let response = self.inner.send_message(params, req).await?;
        if is_new {
            self.record_ownership(&task_id, &caller).await?;
        }
        Ok(response)
    }

    async fn send_streaming_message(
        &self,
        params: &ServiceParams,
        mut req: SendMessageRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        let (caller, task_id, is_new) = self.prepare_send(params, &mut req).await?;
        let stream = self.inner.send_streaming_message(params, req).await?;
        if is_new {
            self.record_ownership(&task_id, &caller).await?;
        }
        Ok(stream)
    }

    async fn get_task(
        &self,
        params: &ServiceParams,
        req: GetTaskRequest,
    ) -> Result<Task, A2AError> {
        self.check_ownership(params, &req.id).await?;
        self.inner.get_task(params, req).await
    }

    /// Scoped to the caller: only tasks whose `residuum.caller` metadata
    /// matches the request's caller are ever considered, before the SDK's
    /// own context/status filtering and paging (`ListTasks` isn't
    /// caller-scoped in the SDK — see the plan's "SDK gaps" note).
    async fn list_tasks(
        &self,
        params: &ServiceParams,
        req: ListTasksRequest,
    ) -> Result<ListTasksResponse, A2AError> {
        let caller = Self::caller_of(params)?;
        let mut tasks = self.task_store.tasks_for_caller(&caller).await;
        if let Some(context_id) = &req.context_id {
            tasks.retain(|task| task.context_id == *context_id);
        }
        if let Some(status) = &req.status {
            tasks.retain(|task| task.status.state == *status);
        }
        tasks.sort_by(|a, b| a.id.cmp(&b.id));

        let page_size = a2a_server::pagination::resolve_page_size(req.page_size);
        let start = match &req.page_token {
            Some(token) => token
                .parse::<usize>()
                .map_err(|e| A2AError::invalid_params(format!("invalid page token: {e}")))?,
            None => 0,
        };
        let total_size = tasks.len();
        let start = start.min(total_size);
        let end = start.saturating_add(page_size).min(total_size);
        let mut page: Vec<Task> = tasks.get(start..end).unwrap_or_default().to_vec();
        for task in &mut page {
            super::task_store::apply_history_length(task, req.history_length);
        }
        let next_page_token = if end < total_size {
            end.to_string()
        } else {
            String::new()
        };

        Ok(ListTasksResponse {
            tasks: page,
            next_page_token,
            page_size: super::task_store::saturating_i32(page_size),
            total_size: super::task_store::saturating_i32(total_size),
        })
    }

    async fn cancel_task(
        &self,
        params: &ServiceParams,
        req: CancelTaskRequest,
    ) -> Result<Task, A2AError> {
        self.check_ownership(params, &req.id).await?;
        self.inner.cancel_task(params, req).await
    }

    async fn subscribe_to_task(
        &self,
        params: &ServiceParams,
        req: SubscribeToTaskRequest,
    ) -> Result<BoxStream<'static, Result<StreamResponse, A2AError>>, A2AError> {
        self.check_ownership(params, &req.id).await?;
        self.inner.subscribe_to_task(params, req).await
    }

    async fn create_push_config(
        &self,
        params: &ServiceParams,
        req: TaskPushNotificationConfig,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        self.check_ownership(params, &req.task_id).await?;
        self.inner.create_push_config(params, req).await
    }

    async fn get_push_config(
        &self,
        params: &ServiceParams,
        req: GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, A2AError> {
        self.check_ownership(params, &req.task_id).await?;
        self.inner.get_push_config(params, req).await
    }

    async fn list_push_configs(
        &self,
        params: &ServiceParams,
        req: ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, A2AError> {
        self.check_ownership(params, &req.task_id).await?;
        self.inner.list_push_configs(params, req).await
    }

    async fn delete_push_config(
        &self,
        params: &ServiceParams,
        req: DeleteTaskPushNotificationConfigRequest,
    ) -> Result<(), A2AError> {
        self.check_ownership(params, &req.task_id).await?;
        self.inner.delete_push_config(params, req).await
    }

    async fn get_extended_agent_card(
        &self,
        _params: &ServiceParams,
        _req: GetExtendedAgentCardRequest,
    ) -> Result<AgentCard, A2AError> {
        Err(A2AError::unsupported_operation(
            "this agent does not serve an extended agent card",
        ))
    }
}

/// Build a synthetic continuation message for a task left `Submitted` or
/// `Working` across a restart, marked `residuum.synthetic` in its metadata
/// so the session can tell it apart from a real caller message.
#[must_use]
pub fn continuation_request(task: &Task) -> SendMessageRequest {
    let mut message = a2a::Message::new(
        a2a::Role::User,
        vec![a2a::Part::text(
            "[Residuum restarted while this task was in progress. Continue where you left off.]",
        )],
    );
    message.task_id = Some(task.id.clone());
    message.context_id = Some(task.context_id.clone());
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        super::task_store::SYNTHETIC_METADATA_KEY.to_string(),
        serde_json::Value::Bool(true),
    );
    message.metadata = Some(metadata);
    SendMessageRequest {
        message,
        configuration: None,
        metadata: None,
        tenant: None,
    }
}

/// `ServiceParams` carrying just the caller header, for the restart
/// continuation sweep, which calls the handler in-process rather than over
/// HTTP.
#[must_use]
pub fn caller_service_params(caller: &str) -> ServiceParams {
    let mut params = ServiceParams::new();
    params.insert(CALLER_HEADER.to_string(), vec![caller.to_string()]);
    params
}

/// Drive every task left `Submitted`/`Working` across a restart through a
/// synthetic continuation message, so long-running work resumes on its own.
/// Runs each task's continuation concurrently and logs failures — a
/// continuation that can't be delivered still leaves the task itself intact
/// for the next real message to it.
pub async fn resume_in_progress_tasks(
    handler: Arc<ResiduumA2aHandler>,
    task_store: SharedTaskStore,
) {
    let tasks = task_store.in_progress_tasks().await;
    if tasks.is_empty() {
        return;
    }
    tracing::info!(
        count = tasks.len(),
        "resuming a2a tasks left in progress across a restart"
    );
    for task in tasks {
        let handler = Arc::clone(&handler);
        tokio::spawn(async move {
            let Some(caller) = FileTaskStore::caller_of(&task).map(str::to_string) else {
                tracing::warn!(task_id = %task.id, "a2a task left in progress has no recorded caller; cannot resume it");
                return;
            };
            let params = caller_service_params(&caller);
            let req = continuation_request(&task);
            match handler.send_streaming_message(&params, req).await {
                Ok(mut stream) => {
                    use futures_util::StreamExt as _;
                    while let Some(item) = stream.next().await {
                        if let Err(e) = item {
                            tracing::warn!(task_id = %task.id, error = %e, "a2a restart continuation reported an error");
                        }
                    }
                    tracing::info!(task_id = %task.id, "a2a restart continuation finished");
                }
                Err(e) => {
                    tracing::error!(task_id = %task.id, error = %e, "failed to resume a2a task after restart");
                }
            }
        });
    }
}
