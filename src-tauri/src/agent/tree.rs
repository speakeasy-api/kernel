use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use super::types::{AgentStatus, SubAgent, TokenMetrics};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentTree {
    agents: HashMap<Uuid, SubAgent>,
    children: HashMap<Uuid, Vec<Uuid>>,
    root_id: Option<Uuid>,
}

impl AgentTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_root(&mut self, mut agent: SubAgent) {
        info!(agent_id = %agent.id, "setting tree root");
        agent.parent_id = None;
        let root_id = agent.id;
        self.agents.insert(root_id, agent);
        self.root_id = Some(root_id);
    }

    pub fn add_child(&mut self, parent_id: Uuid, mut child: SubAgent) {
        debug!(%parent_id, child_id = %child.id, "adding child agent");
        let child_id = child.id;
        child.parent_id = Some(parent_id);
        self.agents.insert(child_id, child);
        self.children.entry(parent_id).or_default().push(child_id);
    }

    pub fn get(&self, id: &Uuid) -> Option<&SubAgent> {
        self.agents.get(id)
    }

    pub fn get_mut(&mut self, id: &Uuid) -> Option<&mut SubAgent> {
        self.agents.get_mut(id)
    }

    pub fn children_of(&self, parent_id: &Uuid) -> Vec<&SubAgent> {
        self.children
            .get(parent_id)
            .into_iter()
            .flatten()
            .filter_map(|child_id| self.agents.get(child_id))
            .collect()
    }

    pub fn update_status(&mut self, id: &Uuid, status: AgentStatus) {
        debug!(agent_id = %id, new_status = ?status, "updating agent status");
        if let Some(agent) = self.agents.get_mut(id) {
            agent.status = status;
        }
    }

    pub fn rollup_tokens(&self, id: &Uuid) -> TokenMetrics {
        let mut visited = HashSet::new();
        let totals = self.rollup_tokens_recursive(id, &mut visited);
        debug!(agent_id = %id, input = totals.input, output = totals.output, cost_usd = totals.cost_usd, "rollup tokens");
        totals
    }

    pub fn all_agents(&self) -> Vec<&SubAgent> {
        self.agents.values().collect()
    }

    fn rollup_tokens_recursive(&self, id: &Uuid, visited: &mut HashSet<Uuid>) -> TokenMetrics {
        if !visited.insert(*id) {
            return TokenMetrics::default();
        }

        let mut total = self
            .agents
            .get(id)
            .map(|agent| agent.token_usage.clone())
            .unwrap_or_default();

        if let Some(child_ids) = self.children.get(id) {
            for child_id in child_ids {
                total += self.rollup_tokens_recursive(child_id, visited);
            }
        }

        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentRole, CompactedContextRef};

    fn make_agent(id: Uuid, parent_id: Option<Uuid>, status: AgentStatus, input: u64) -> SubAgent {
        SubAgent {
            id,
            parent_id,
            role: AgentRole::Implementation,
            model: "claude-sonnet-4-20250514".to_string(),
            mode: "implementation".to_string(),
            status,
            context: Some(CompactedContextRef {
                summary: "ctx".to_string(),
                token_count: 10,
            }),
            allowed_tools: vec!["read".to_string()],
            token_usage: TokenMetrics {
                input,
                output: input / 2,
                cost_usd: input as f64 / 1000.0,
            },
        }
    }

    #[test]
    fn tree_tracks_parent_child_relationships() {
        let root_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();

        let mut tree = AgentTree::new();
        tree.set_root(make_agent(root_id, None, AgentStatus::Running, 10));
        tree.add_child(
            root_id,
            make_agent(child_id, Some(Uuid::new_v4()), AgentStatus::Spawning, 6),
        );

        let child = tree.get(&child_id).expect("child should exist");
        assert_eq!(child.parent_id, Some(root_id));
        assert_eq!(tree.children_of(&root_id).len(), 1);
    }

    #[test]
    fn rollup_tokens_includes_descendants() {
        let root_id = Uuid::new_v4();
        let child_a_id = Uuid::new_v4();
        let child_b_id = Uuid::new_v4();
        let grandchild_id = Uuid::new_v4();

        let mut tree = AgentTree::new();
        tree.set_root(make_agent(root_id, None, AgentStatus::Running, 10));
        tree.add_child(
            root_id,
            make_agent(child_a_id, Some(root_id), AgentStatus::Complete, 5),
        );
        tree.add_child(
            root_id,
            make_agent(child_b_id, Some(root_id), AgentStatus::Complete, 7),
        );
        tree.add_child(
            child_a_id,
            make_agent(grandchild_id, Some(child_a_id), AgentStatus::Complete, 3),
        );

        let rolled_up = tree.rollup_tokens(&root_id);
        assert_eq!(rolled_up.input, 25);
        assert_eq!(rolled_up.output, 11);
        assert!((rolled_up.cost_usd - 0.025).abs() < f64::EPSILON);
    }

    #[test]
    fn update_status_changes_agent_status() {
        let root_id = Uuid::new_v4();
        let mut tree = AgentTree::new();
        tree.set_root(make_agent(root_id, None, AgentStatus::Spawning, 1));
        tree.update_status(&root_id, AgentStatus::Complete);

        let root = tree.get(&root_id).expect("root should exist");
        assert_eq!(root.status, AgentStatus::Complete);
    }

    #[test]
    fn set_root_and_get_returns_root() {
        let root_id = Uuid::new_v4();
        let mut tree = AgentTree::new();
        tree.set_root(make_agent(
            root_id,
            Some(Uuid::new_v4()),
            AgentStatus::Running,
            10,
        ));

        let root = tree.get(&root_id).expect("root should exist");
        assert_eq!(root.id, root_id);
        assert_eq!(root.parent_id, None);
    }

    #[test]
    fn add_children_and_children_of_returns_direct_children() {
        let root_id = Uuid::new_v4();
        let child_a_id = Uuid::new_v4();
        let child_b_id = Uuid::new_v4();
        let mut tree = AgentTree::new();

        tree.set_root(make_agent(root_id, None, AgentStatus::Running, 10));
        tree.add_child(
            root_id,
            make_agent(child_a_id, Some(Uuid::new_v4()), AgentStatus::Spawning, 2),
        );
        tree.add_child(
            root_id,
            make_agent(child_b_id, Some(Uuid::new_v4()), AgentStatus::Spawning, 3),
        );

        let children = tree.children_of(&root_id);
        assert_eq!(children.len(), 2);
        assert!(children.iter().any(|agent| agent.id == child_a_id));
        assert!(children.iter().any(|agent| agent.id == child_b_id));
    }

    #[test]
    fn nested_children_are_attached_to_expected_parent() {
        let root_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();
        let grandchild_id = Uuid::new_v4();
        let mut tree = AgentTree::new();

        tree.set_root(make_agent(root_id, None, AgentStatus::Running, 5));
        tree.add_child(
            root_id,
            make_agent(child_id, Some(root_id), AgentStatus::Running, 4),
        );
        tree.add_child(
            child_id,
            make_agent(grandchild_id, Some(child_id), AgentStatus::Running, 3),
        );

        let root_children = tree.children_of(&root_id);
        assert_eq!(root_children.len(), 1);
        assert_eq!(root_children[0].id, child_id);

        let child_children = tree.children_of(&child_id);
        assert_eq!(child_children.len(), 1);
        assert_eq!(child_children[0].id, grandchild_id);
    }

    #[test]
    fn rollup_tokens_single_agent_returns_own_usage() {
        let root_id = Uuid::new_v4();
        let mut tree = AgentTree::new();
        tree.set_root(make_agent(root_id, None, AgentStatus::Running, 14));

        let rolled_up = tree.rollup_tokens(&root_id);
        assert_eq!(rolled_up.input, 14);
        assert_eq!(rolled_up.output, 7);
        assert!((rolled_up.cost_usd - 0.014).abs() < f64::EPSILON);
    }

    #[test]
    fn all_agents_returns_every_agent() {
        let root_id = Uuid::new_v4();
        let child_a_id = Uuid::new_v4();
        let child_b_id = Uuid::new_v4();
        let mut tree = AgentTree::new();
        tree.set_root(make_agent(root_id, None, AgentStatus::Running, 1));
        tree.add_child(
            root_id,
            make_agent(child_a_id, Some(root_id), AgentStatus::Running, 2),
        );
        tree.add_child(
            root_id,
            make_agent(child_b_id, Some(root_id), AgentStatus::Running, 3),
        );

        let agents = tree.all_agents();
        assert_eq!(agents.len(), 3);
        assert!(agents.iter().any(|agent| agent.id == root_id));
        assert!(agents.iter().any(|agent| agent.id == child_a_id));
        assert!(agents.iter().any(|agent| agent.id == child_b_id));
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let tree = AgentTree::new();
        assert!(tree.get(&Uuid::new_v4()).is_none());
    }
}
