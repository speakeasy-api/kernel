use std::collections::{HashMap, HashSet};

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::scheduler::{validate_dag, SchedulerError};
use super::types::{Priority, Task, TaskStatus};

/// Input to the planning agent — what the user wants done.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecompositionRequest {
    pub session_id: Uuid,
    pub project_description: String,
    pub base_ref: String,
    pub merge_target_ref: String,
}

/// A single planned task from the planning agent's output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedTask {
    pub title: String,
    pub description: String,
    pub priority: Priority,
    pub depends_on_titles: Vec<String>,
    pub parent_title: Option<String>,
}

/// The full decomposition output from the planning agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecompositionResult {
    pub tasks: Vec<PlannedTask>,
}

/// Errors during decomposition validation and persistence.
#[derive(Debug, thiserror::Error)]
pub enum DecompositionError {
    #[error("Duplicate task title: {0}")]
    DuplicateTitle(String),
    #[error(
        "Unknown dependency reference: task '{task}' depends on '{dependency}' which doesn't exist"
    )]
    UnknownDependency { task: String, dependency: String },
    #[error(
        "Unknown parent reference: task '{task}' references parent '{parent}' which doesn't exist"
    )]
    UnknownParent { task: String, parent: String },
    #[error("Dependency cycle detected")]
    CycleDetected,
    #[error("Database error: {0}")]
    DbError(#[from] rusqlite::Error),
}

pub fn validate_decomposition(result: &DecompositionResult) -> Result<(), DecompositionError> {
    let mut seen_titles: HashSet<&str> = HashSet::with_capacity(result.tasks.len());
    for task in &result.tasks {
        if !seen_titles.insert(task.title.as_str()) {
            return Err(DecompositionError::DuplicateTitle(task.title.clone()));
        }
    }

    for task in &result.tasks {
        for dep_title in &task.depends_on_titles {
            if !seen_titles.contains(dep_title.as_str()) {
                return Err(DecompositionError::UnknownDependency {
                    task: task.title.clone(),
                    dependency: dep_title.clone(),
                });
            }
        }
        if let Some(parent) = &task.parent_title {
            if !seen_titles.contains(parent.as_str()) {
                return Err(DecompositionError::UnknownParent {
                    task: task.title.clone(),
                    parent: parent.clone(),
                });
            }
        }
    }

    let mut title_to_id: HashMap<&str, Uuid> = HashMap::with_capacity(result.tasks.len());
    for task in &result.tasks {
        title_to_id.insert(task.title.as_str(), Uuid::new_v4());
    }

    let now = Utc::now();
    let temporary_tasks = result
        .tasks
        .iter()
        .map(|task| Task {
            id: *title_to_id
                .get(task.title.as_str())
                .expect("title should exist in decomposition map"),
            session_id: Uuid::nil(),
            title: task.title.clone(),
            description: task.description.clone(),
            status: TaskStatus::Pending,
            priority: task.priority,
            assigned_agent: None,
            parent_task: None,
            depends_on: Vec::new(),
            worktree_branch: None,
            base_ref: String::new(),
            base_commit: String::new(),
            merge_target_ref: String::new(),
            outcome: None,
            engagement_override: None,
            cost_usd: 0.0,
            created_at: now,
            updated_at: now,
        })
        .collect::<Vec<_>>();

    let mut edges: Vec<(Uuid, Uuid)> = Vec::new();
    for task in &result.tasks {
        let task_id = *title_to_id
            .get(task.title.as_str())
            .expect("title should exist in decomposition map");
        for dependency_title in &task.depends_on_titles {
            let dep_id = *title_to_id
                .get(dependency_title.as_str())
                .expect("dependency title was validated to exist");
            edges.push((task_id, dep_id));
        }
    }

    match validate_dag(&temporary_tasks, &edges) {
        Ok(_) => Ok(()),
        Err(SchedulerError::CycleDetected(_)) => Err(DecompositionError::CycleDetected),
        Err(SchedulerError::DbError(err)) => Err(DecompositionError::DbError(err)),
    }
}

pub fn persist_decomposition(
    conn: &Connection,
    request: &DecompositionRequest,
    result: &DecompositionResult,
) -> Result<Vec<Task>, DecompositionError> {
    validate_decomposition(result)?;

    let mut title_to_id: HashMap<&str, Uuid> = HashMap::with_capacity(result.tasks.len());
    for task in &result.tasks {
        title_to_id.insert(task.title.as_str(), Uuid::new_v4());
    }

    let now = Utc::now();
    let mut tasks = Vec::with_capacity(result.tasks.len());
    for planned in &result.tasks {
        let id = *title_to_id
            .get(planned.title.as_str())
            .expect("title should exist in decomposition map");
        let depends_on = planned
            .depends_on_titles
            .iter()
            .map(|title| {
                *title_to_id
                    .get(title.as_str())
                    .expect("dependency title was validated to exist")
            })
            .collect();
        let parent_task = planned.parent_title.as_ref().map(|title| {
            *title_to_id
                .get(title.as_str())
                .expect("parent title was validated to exist")
        });

        tasks.push(Task {
            id,
            session_id: request.session_id,
            title: planned.title.clone(),
            description: planned.description.clone(),
            status: TaskStatus::Pending,
            priority: planned.priority,
            assigned_agent: None,
            parent_task,
            depends_on,
            worktree_branch: None,
            base_ref: request.base_ref.clone(),
            base_commit: String::new(),
            merge_target_ref: request.merge_target_ref.clone(),
            outcome: None,
            engagement_override: None,
            cost_usd: 0.0,
            created_at: now,
            updated_at: now,
        });
    }

    let tx = conn.unchecked_transaction()?;
    for task in &tasks {
        super::db::insert_task_in_transaction(&tx, task)?;
    }
    tx.commit()?;

    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::tasks::db;

    fn planned(
        title: &str,
        depends_on_titles: Vec<&str>,
        parent_title: Option<&str>,
        priority: Priority,
    ) -> PlannedTask {
        PlannedTask {
            title: title.to_string(),
            description: format!("description: {title}"),
            priority,
            depends_on_titles: depends_on_titles.into_iter().map(str::to_string).collect(),
            parent_title: parent_title.map(str::to_string),
        }
    }

    #[test]
    fn validate_rejects_duplicate_titles() {
        let result = DecompositionResult {
            tasks: vec![
                planned("same", vec![], None, Priority::Medium),
                planned("same", vec![], None, Priority::High),
            ],
        };

        let err = validate_decomposition(&result).unwrap_err();
        assert!(matches!(err, DecompositionError::DuplicateTitle(title) if title == "same"));
    }

    #[test]
    fn validate_rejects_dependency_cycles() {
        let result = DecompositionResult {
            tasks: vec![
                planned("a", vec!["b"], None, Priority::Medium),
                planned("b", vec!["a"], None, Priority::Medium),
            ],
        };

        let err = validate_decomposition(&result).unwrap_err();
        assert!(matches!(err, DecompositionError::CycleDetected));
    }

    #[test]
    fn persist_materializes_title_references() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();

        let session_id = Uuid::new_v4();
        let request = DecompositionRequest {
            session_id,
            project_description: "ship a feature".to_string(),
            base_ref: "main".to_string(),
            merge_target_ref: "main".to_string(),
        };
        let result = DecompositionResult {
            tasks: vec![
                planned("root", vec![], None, Priority::High),
                planned("child", vec!["root"], Some("root"), Priority::Medium),
            ],
        };

        let persisted = persist_decomposition(&conn, &request, &result).unwrap();
        assert_eq!(persisted.len(), 2);
        assert!(persisted
            .iter()
            .all(|task| task.status == TaskStatus::Pending));

        let all_tasks = db::list_tasks(&conn, session_id, None).unwrap();
        assert_eq!(all_tasks.len(), 2);
        let root = all_tasks.iter().find(|task| task.title == "root").unwrap();
        let child = all_tasks.iter().find(|task| task.title == "child").unwrap();
        assert_eq!(child.parent_task, Some(root.id));
        assert_eq!(child.depends_on, vec![root.id]);
    }
}
