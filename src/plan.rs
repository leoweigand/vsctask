//! Flattening `dependsOn` into an execution plan.
//!
//! Dependencies form a DAG. Each task is placed in the earliest stage that
//! comes after every task it depends on, so everything in one stage may start
//! together and stages run in order. `dependsOrder: sequence` adds edges
//! between consecutive dependencies, which spreads them across stages.

use crate::model::DependsOrder;
use crate::resolve::{self, Resolved};
use crate::vars::Context;
use crate::workspace::Workspace;
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Serialize)]
pub struct Plan {
    pub root: String,
    /// Stages run in order; the tasks within one stage run together.
    pub stages: Vec<Vec<Resolved>>,
}

pub fn build(ws: &Workspace, root: &str, ctx: &Context) -> Result<Plan> {
    // Collect the reachable subgraph, and the edges dep -> dependent.
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    let mut queue = VecDeque::from([root.to_string()]);

    while let Some(label) = queue.pop_front() {
        if !nodes.insert(label.clone()) {
            continue;
        }
        let task = ws.find(&label)?;
        let deps = task.depends_on.labels();

        for dep in &deps {
            edges.entry((*dep).to_string()).or_default().insert(label.clone());
            queue.push_back((*dep).to_string());
        }
        if task.depends_order == DependsOrder::Sequence {
            for pair in deps.windows(2) {
                edges.entry(pair[0].to_string()).or_default().insert(pair[1].to_string());
            }
        }
    }

    // Kahn's algorithm, tracking depth to get the stage layering.
    let mut indegree: BTreeMap<&str, usize> =
        nodes.iter().map(|n| (n.as_str(), 0usize)).collect();
    for (from, tos) in &edges {
        for to in tos {
            if nodes.contains(from) {
                *indegree.get_mut(to.as_str()).unwrap() += 1;
            }
        }
    }

    let mut depth: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ready: VecDeque<&str> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| *n)
        .collect();
    for n in &ready {
        depth.insert(n, 0);
    }

    let mut ordered: Vec<&str> = Vec::new();
    while let Some(n) = ready.pop_front() {
        ordered.push(n);
        let d = depth[n];
        for to in edges.get(n).into_iter().flatten() {
            let to = to.as_str();
            let e = depth.entry(to).or_insert(0);
            *e = (*e).max(d + 1);
            let deg = indegree.get_mut(to).unwrap();
            *deg -= 1;
            if *deg == 0 {
                ready.push_back(to);
            }
        }
    }

    if ordered.len() != nodes.len() {
        let stuck: Vec<&str> = nodes
            .iter()
            .map(String::as_str)
            .filter(|n| !ordered.contains(n))
            .collect();
        bail!("dependsOn has a cycle involving: {}", stuck.join(", "));
    }

    let depth_count = depth.values().copied().max().map_or(0, |d| d + 1);
    let mut stages: Vec<Vec<Resolved>> = (0..depth_count).map(|_| Vec::new()).collect();
    let file_options = ws.tasks.options.as_ref();

    for label in ordered {
        let task = ws.find(label)?;
        // A task that only declares dependsOn runs nothing itself.
        if resolve::is_composite(&task) {
            continue;
        }
        stages[depth[label]].push(resolve::resolve(&task, file_options, ctx)?);
    }
    stages.retain(|s| !s.is_empty());

    Ok(Plan { root: root.to_string(), stages })
}
