use chrono::{DateTime, Utc};

use super::types::OutboundProblem;
use crate::AgentFiles;
use residuum_a2a::client::tracker::read_open_tasks;
use residuum_config::WorkspaceLayout;

pub(super) struct OutboundRead {
    pub problems: Vec<OutboundProblem>,
    pub next_notice_at: Option<DateTime<Utc>>,
    pub problem: Option<String>,
}

impl OutboundRead {
    pub(super) fn none() -> Self {
        Self {
            problems: Vec::new(),
            next_notice_at: None,
            problem: None,
        }
    }
}

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
