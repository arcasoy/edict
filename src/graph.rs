//! Generic directed-graph algorithms over `usize` node ids. Pure.

use std::collections::{BTreeSet, VecDeque};

/// Strongly connected components via Tarjan's algorithm.
/// Returns every SCC, each sorted ascending, in no particular order.
pub fn sccs(n: usize, adj: &[BTreeSet<usize>]) -> Vec<Vec<usize>> {
    struct State<'a> {
        adj: &'a [BTreeSet<usize>],
        index: usize,
        indices: Vec<Option<usize>>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        out: Vec<Vec<usize>>,
    }

    fn strong(s: &mut State, v: usize) {
        s.indices[v] = Some(s.index);
        s.low[v] = s.index;
        s.index += 1;
        s.stack.push(v);
        s.on_stack[v] = true;

        let succs: Vec<usize> = s.adj[v].iter().copied().collect();
        for w in succs {
            match s.indices[w] {
                None => {
                    strong(s, w);
                    s.low[v] = s.low[v].min(s.low[w]);
                }
                Some(wi) if s.on_stack[w] => s.low[v] = s.low[v].min(wi),
                _ => {}
            }
        }

        if Some(s.low[v]) == s.indices[v] {
            let mut comp = Vec::new();
            while let Some(w) = s.stack.pop() {
                s.on_stack[w] = false;
                comp.push(w);
                if w == v {
                    break;
                }
            }
            comp.sort_unstable();
            s.out.push(comp);
        }
    }

    let mut s = State {
        adj,
        index: 0,
        indices: vec![None; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        out: Vec::new(),
    };
    for v in 0..n {
        if s.indices[v].is_none() {
            strong(&mut s, v);
        }
    }
    s.out
}

/// Shortest cycle through `start` using only nodes in `members`.
/// Returns the node sequence `[start, a, b, ..., start]`, or empty if none.
pub fn shortest_cycle(
    adj: &[BTreeSet<usize>],
    members: &BTreeSet<usize>,
    start: usize,
) -> Vec<usize> {
    let mut prev: Vec<Option<usize>> = vec![None; adj.len()];
    let mut queue = VecDeque::new();
    for &w in &adj[start] {
        if members.contains(&w) && prev[w].is_none() {
            prev[w] = Some(start);
            queue.push_back(w);
        }
    }
    while let Some(v) = queue.pop_front() {
        if v == start {
            let mut path = vec![start];
            let mut cur = prev[start].expect("start has a predecessor");
            while cur != start {
                path.push(cur);
                cur = prev[cur].expect("path is connected");
            }
            path.push(start);
            path.reverse();
            return path;
        }
        for &w in &adj[v] {
            if members.contains(&w) && prev[w].is_none() {
                prev[w] = Some(v);
                queue.push_back(w);
            }
        }
    }
    Vec::new()
}

/// True if the graph has no cycles (self-loops count as cycles).
pub fn is_acyclic(n: usize, adj: &[BTreeSet<usize>]) -> bool {
    (0..n).all(|v| !adj[v].contains(&v)) && sccs(n, adj).iter().all(|c| c.len() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(n: usize, edges: &[(usize, usize)]) -> Vec<BTreeSet<usize>> {
        let mut adj = vec![BTreeSet::new(); n];
        for &(a, b) in edges {
            adj[a].insert(b);
        }
        adj
    }

    #[test]
    fn finds_scc_and_cycle() {
        let adj = graph(4, &[(0, 1), (1, 2), (2, 0), (2, 3)]);
        let comps = sccs(4, &adj);
        assert!(comps.contains(&vec![0, 1, 2]));
        let members: BTreeSet<usize> = [0, 1, 2].into();
        assert_eq!(shortest_cycle(&adj, &members, 0), vec![0, 1, 2, 0]);
        assert!(!is_acyclic(4, &adj));
    }

    #[test]
    fn dag_is_acyclic() {
        let adj = graph(3, &[(0, 1), (1, 2), (0, 2)]);
        assert!(is_acyclic(3, &adj));
    }
}
