use super::context::{DagNode, DagPlan};

/// Builds a deterministic dependency graph from the router's preserved actions.
/// The graph is orchestration metadata; it never changes the upstream payload.
pub fn plan(actions: &[String]) -> DagPlan {
    let nodes = actions
        .iter()
        .enumerate()
        .map(|(index, action)| DagNode {
            id: format!("step-{:02}", index + 1),
            action: action.clone(),
            depends_on: index
                .checked_sub(1)
                .map(|previous| vec![format!("step-{:02}", previous + 1)])
                .unwrap_or_default(),
            status: "pending".to_string(),
        })
        .collect();
    DagPlan { version: 1, nodes }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubtaskResult {
    pub node_id: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReduceError {
    UnknownNode(String),
    EmptyResult(String),
    MissingDependency(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduleError {
    NodeFailed(String),
    MissingDependency(String),
}

/// Runs nodes in dependency order and returns only attributed results.
/// The executor is deliberately injected so transport, policy and auth remain
/// owned by the caller rather than hidden inside the reducer.
pub fn execute<F>(plan: &mut DagPlan, mut executor: F) -> Result<Vec<SubtaskResult>, ScheduleError>
where
    F: FnMut(&DagNode, &[SubtaskResult]) -> Result<String, String>,
{
    let mut results = Vec::with_capacity(plan.nodes.len());
    for index in 0..plan.nodes.len() {
        let node = plan.nodes[index].clone();
        if node.depends_on.iter().any(|dependency| {
            !results
                .iter()
                .any(|result: &SubtaskResult| &result.node_id == dependency)
        }) {
            plan.nodes[index].status = "blocked".to_string();
            return Err(ScheduleError::MissingDependency(node.id));
        }
        let content = match executor(&node, &results) {
            Ok(content) if !content.trim().is_empty() => content,
            Ok(_) => {
                plan.nodes[index].status = "failed".to_string();
                return Err(ScheduleError::NodeFailed(node.id));
            }
            Err(_) => {
                plan.nodes[index].status = "failed".to_string();
                return Err(ScheduleError::NodeFailed(node.id));
            }
        };
        plan.nodes[index].status = "completed".to_string();
        results.push(SubtaskResult {
            node_id: node.id,
            content,
        });
    }
    Ok(results)
}

/// Combines only attributed subtask outputs. It does not synthesize missing content.
pub fn reduce(plan: &DagPlan, results: &[SubtaskResult]) -> Result<String, ReduceError> {
    let mut output = Vec::new();
    for node in &plan.nodes {
        let result = results.iter().find(|item| item.node_id == node.id);
        let Some(result) = result else {
            return Err(ReduceError::MissingDependency(node.id.clone()));
        };
        if result.content.trim().is_empty() {
            return Err(ReduceError::EmptyResult(node.id.clone()));
        }
        output.push(format!("[{}:{}]\n{}", node.id, node.action, result.content));
    }
    for result in results {
        if !plan.nodes.iter().any(|node| node.id == result.node_id) {
            return Err(ReduceError::UnknownNode(result.node_id.clone()));
        }
    }
    Ok(output.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_ordered_dependencies() {
        let graph = plan(&["acquire".into(), "inspect".into(), "verify".into()]);
        assert_eq!(graph.nodes[1].depends_on, ["step-01"]);
        assert_eq!(graph.pending_count(), 3);
    }

    #[test]
    fn reducer_requires_attributed_complete_results() {
        let graph = plan(&["inspect".into(), "verify".into()]);
        let results = vec![SubtaskResult {
            node_id: "step-01".into(),
            content: "ok".into(),
        }];
        assert_eq!(
            reduce(&graph, &results),
            Err(ReduceError::MissingDependency("step-02".into()))
        );
    }

    #[test]
    fn scheduler_stops_on_failed_node() {
        let mut graph = plan(&["inspect".into(), "verify".into()]);
        let result = execute(&mut graph, |node, _| {
            if node.action == "verify" {
                Err("provider block".into())
            } else {
                Ok("inspection".into())
            }
        });
        assert_eq!(result, Err(ScheduleError::NodeFailed("step-02".into())));
        assert_eq!(graph.nodes[0].status, "completed");
        assert_eq!(graph.nodes[1].status, "failed");
    }
}
