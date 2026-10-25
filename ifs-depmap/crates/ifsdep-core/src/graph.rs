//! Strongly connected components (Tarjan, iterative so deep include chains
//! can't overflow the stack).

/// `adj[n]` lists the successors of node `n`. Returns every SCC.
pub fn tarjan_scc(adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = adj.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut out = Vec::new();
    let mut next = 0usize;

    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        // (node, position in its successor list)
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;

        while let Some(&mut (v, ref mut pos)) = work.last_mut() {
            if *pos < adj[v].len() {
                let w = adj[v][*pos];
                *pos += 1;
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut comp = Vec::new();
                    loop {
                        let w = stack.pop().unwrap();
                        on_stack[w] = false;
                        comp.push(w);
                        if w == v {
                            break;
                        }
                    }
                    out.push(comp);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
        for c in &mut v {
            c.sort();
        }
        v.sort();
        v
    }

    #[test]
    fn finds_components() {
        // 0→1→2→0, 2→3, 3→4→3, 5 alone
        let adj = vec![vec![1], vec![2], vec![0, 3], vec![4], vec![3], vec![]];
        assert_eq!(sorted(tarjan_scc(&adj)), vec![vec![0, 1, 2], vec![3, 4], vec![5]]);
    }

    #[test]
    fn long_chain_does_not_overflow() {
        let n = 200_000;
        let adj: Vec<Vec<usize>> = (0..n).map(|i| if i + 1 < n { vec![i + 1] } else { vec![0] }).collect();
        let scc = tarjan_scc(&adj);
        assert_eq!(scc.len(), 1);
        assert_eq!(scc[0].len(), n);
    }
}
