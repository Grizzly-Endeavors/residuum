//! `outbound_problems`: the open tasks a running agent sent to remote agents
//! that it has been unable to reach for longer than the tracker's notice
//! threshold, read from the agent's `a2a/outbound.json`.
//!
//! The tracker saves a task before it announces the change, so the file is
//! never behind the event that sent the overview here to read it.

use chrono::{DateTime, Utc};

use super::types::OutboundProblem;
use crate::a2a::client::tracker::read_open_tasks;
use crate::hub::AgentFiles;
use crate::workspace::layout::WorkspaceLayout;

/// What reading an agent's outbound tasks found.
pub(super) struct OutboundRead {
    /// The tasks past the threshold, the longest unreachable first.
    pub problems: Vec<OutboundProblem>,
    /// When the next task still short of the threshold passes it, if one is
    /// in a streak. Nothing announces that moment, so the service waits for
    /// it.
    pub next_notice_at: Option<DateTime<Utc>>,
    /// What could not be read, as one plain sentence, which the service
    /// reports once for as long as it stays unreadable.
    pub problem: Option<String>,
}

impl OutboundRead {
    /// An agent with no tasks being watched: one that isn't running.
    pub(super) fn none() -> Self {
        Self {
            problems: Vec::new(),
            next_notice_at: None,
            problem: None,
        }
    }
}

/// The agent's outbound tasks that are problems as of `now`.
pub(super) async fn read(files: &AgentFiles, now: DateTime<Utc>) -> OutboundRead {
    let path = WorkspaceLayout::new(&files.dir).a2a_outbound_json();
    let tasks = match read_open_tasks(&path).await {
        Ok(tasks) => tasks,
        Err(e) => {
            return OutboundRead {
                problem: Some(format!(
                    "couldn't read the agent's outbound tasks, so none are listed as unreachable: {e:#}"
                )),
                ..OutboundRead::none()
            };
        }
    };

    let mut read = OutboundRead::none();
    for task in tasks {
        let (Some(since), Some(notice_at)) = (task.first_unreachable_at, task.notice_due_at())
        else {
            continue;
        };
        if notice_at <= now {
            read.problems.push(OutboundProblem {
                task_id: task.task_id,
                remote_agent: task.agent,
                status_text: task.last_status_text,
                unreachable_since: since,
            });
        } else {
            read.next_notice_at = Some(
                read.next_notice_at
                    .map_or(notice_at, |at| at.min(notice_at)),
            );
        }
    }
    read.problems
        .sort_by(|a, b| (a.unreachable_since, &a.task_id).cmp(&(b.unreachable_since, &b.task_id)));
    read
}
