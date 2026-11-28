//! The layering passes over a plain digraph: cycle breaking, ranking and
//! ordering.

/// How many down-and-up barycentre sweeps the ordering pass makes.
const SWEEPS: usize = 4;

/// What the layering passes produce, over node indices.
pub(crate) struct Layers {
    /// The nodes of each rank, in order.
    pub(crate) layers: Vec<Vec<usize>>,
    /// For each edge, whether it is a back-edge.
    pub(crate) back: Vec<bool>,
}

/// Cycle breaking, ranking and ordering over a plain digraph of `n` nodes,
/// where a node's index is its declaration order.
pub(crate) fn layer(n: usize, edges: &[(usize, usize)]) -> Layers {
    let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut has_input = vec![false; n];
    for (e, &(a, b)) in edges.iter().enumerate() {
        out_edges[a].push(e);
        has_input[b] = true;
    }

    // 1. Depth-first search from the sources, then from anything a source
    //    does not reach (a cycle with no way in), in declaration order.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        OnStack,
        Done,
    }
    let mut mark = vec![Mark::New; n];
    let mut back = vec![false; edges.len()];
    let mut preorder = Vec::with_capacity(n);
    let roots = (0..n)
        .filter(|&v| !has_input[v])
        .chain(0..n)
        .collect::<Vec<_>>();
    for root in roots {
        if mark[root] != Mark::New {
            continue;
        }
        mark[root] = Mark::OnStack;
        preorder.push(root);
        let mut stack = vec![(root, 0_usize)];
        while let Some((node, next)) = stack.last_mut() {
            let node = *node;
            if let Some(&e) = out_edges[node].get(*next) {
                *next += 1;
                let target = edges[e].1;
                match mark[target] {
                    Mark::New => {
                        mark[target] = Mark::OnStack;
                        preorder.push(target);
                        stack.push((target, 0));
                    }
                    Mark::OnStack => back[e] = true,
                    Mark::Done => {}
                }
            } else {
                mark[node] = Mark::Done;
                stack.pop();
            }
        }
    }

    // 2. Longest path over the forward edges, which form a DAG now.
    let mut indegree = vec![0_usize; n];
    for (e, &(_, b)) in edges.iter().enumerate() {
        if !back[e] {
            indegree[b] += 1;
        }
    }
    let mut rank = vec![0_usize; n];
    let mut queue: std::collections::VecDeque<usize> =
        (0..n).filter(|&v| indegree[v] == 0).collect();
    while let Some(node) = queue.pop_front() {
        for &e in &out_edges[node] {
            if back[e] {
                continue;
            }
            let target = edges[e].1;
            rank[target] = rank[target].max(rank[node] + 1);
            indegree[target] -= 1;
            if indegree[target] == 0 {
                queue.push_back(target);
            }
        }
    }

    // 3. Start from the search's discovery order, then sweep.
    let ranks = rank.iter().max().map_or(0, |&r| r + 1);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); ranks];
    for &node in &preorder {
        layers[rank[node]].push(node);
    }
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (e, &(a, b)) in edges.iter().enumerate() {
        if !back[e] {
            neighbours[a].push(b);
            neighbours[b].push(a);
        }
    }
    let mut cross = vec![0.0_f32; n];
    for layer in &layers {
        let last = layer.len().saturating_sub(1) as f32;
        for (i, &node) in layer.iter().enumerate() {
            cross[node] = i as f32 - last / 2.0;
        }
    }
    for _ in 0..SWEEPS {
        // Down: each rank by its neighbours above. Up: by those below.
        for (r, layer) in layers.iter_mut().enumerate().skip(1) {
            reorder(layer, &neighbours, &rank, &mut cross, |nr| nr < r);
        }
        for (r, layer) in layers.iter_mut().enumerate().rev().skip(1) {
            reorder(layer, &neighbours, &rank, &mut cross, |nr| nr > r);
        }
    }

    Layers { layers, back }
}

/// Sorts one rank by the barycentre of each node's neighbours in the ranks
/// `side` admits, and updates the cross positions to match. A node with no
/// such neighbour keeps its current position as its key, and ties keep their
/// current order: the sort is stable.
fn reorder(
    layer: &mut [usize],
    neighbours: &[Vec<usize>],
    rank: &[usize],
    cross: &mut [f32],
    side: impl Fn(usize) -> bool,
) {
    let key = |node: usize| {
        let (sum, count) = neighbours[node]
            .iter()
            .filter(|&&m| side(rank[m]))
            .fold((0.0_f32, 0_u32), |(s, c), &m| (s + cross[m], c + 1));
        if count == 0 {
            cross[node]
        } else {
            sum / count as f32
        }
    };
    let mut keyed: Vec<(f32, usize)> = layer.iter().map(|&node| (key(node), node)).collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let last = layer.len().saturating_sub(1) as f32;
    for (i, (_, node)) in keyed.into_iter().enumerate() {
        layer[i] = node;
        cross[node] = i as f32 - last / 2.0;
    }
}
