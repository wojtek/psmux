use std::io::{self, Write};
use ratatui::prelude::*;

use crate::types::*;

pub fn active_pane_mut<'a>(node: &'a mut Node, path: &Vec<usize>) -> Option<&'a mut Pane> {
    let mut cur = node;
    for &idx in path.iter() {
        match cur {
            Node::Split { children, .. } => { cur = children.get_mut(idx)?; }
            Node::Leaf(_) => return None,
        }
    }
    match cur { Node::Leaf(p) => Some(p), _ => None }
}

pub fn replace_leaf_with_split(node: &mut Node, path: &Vec<usize>, kind: LayoutKind, new_leaf: Node) {
    if path.is_empty() {
        let old = std::mem::replace(node, Node::Split { kind, sizes: vec![50,50], children: vec![] });
        if let Node::Split { children, .. } = node { children.push(old); children.push(new_leaf); }
        return;
    }
    let mut cur = node;
    for (depth, &idx) in path.iter().enumerate() {
        match cur {
            Node::Split { children, .. } => {
                if depth == path.len()-1 {
                    let leaf = std::mem::replace(&mut children[idx], Node::Split { kind, sizes: vec![50,50], children: vec![] });
                    if let Node::Split { children: c, .. } = &mut children[idx] { c.push(leaf); c.push(new_leaf); }
                    return;
                } else { cur = &mut children[idx]; }
            }
            Node::Leaf(_) => return,
        }
    }
}

pub fn kill_leaf(node: &mut Node, path: &Vec<usize>) {
    *node = remove_node(std::mem::replace(node, Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] }), path);
}

/// Kill a node and all its child processes before dropping it
pub fn kill_node(mut n: Node) {
    match &mut n {
        Node::Leaf(p) => { let _ = p.child.kill(); }
        Node::Split { children, .. } => {
            for child in children.iter_mut() {
                kill_all_children(child);
            }
        }
    }
}

pub fn remove_node(n: Node, path: &Vec<usize>) -> Node {
    match n {
        Node::Leaf(p) => {
            Node::Leaf(p)
        }
        Node::Split { kind, sizes, children } => {
            if path.is_empty() { return Node::Split { kind, sizes, children }; }
            let idx = path[0];
            let mut new_children: Vec<Node> = Vec::new();
            for (i, child) in children.into_iter().enumerate() {
                if i == idx {
                    if path.len() > 1 { new_children.push(remove_node(child, &path[1..].to_vec())); }
                    else {
                        kill_node(child);
                    }
                } else { new_children.push(child); }
            }
            if new_children.len() == 1 { new_children.into_iter().next().unwrap() }
            else {
                let mut eq = vec![100 / new_children.len() as u16; new_children.len()];
                let rem = 100 - eq.iter().sum::<u16>();
                if let Some(last) = eq.last_mut() { *last += rem; }
                Node::Split { kind, sizes: eq, children: new_children }
            }
        }
    }
}

pub fn compute_rects(node: &Node, area: Rect, out: &mut Vec<(Vec<usize>, Rect)>) {
    fn rec(node: &Node, area: Rect, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, Rect)>) {
        match node {
            Node::Leaf(_) => { out.push((path.clone(), area)); }
            Node::Split { kind, sizes, children } => {
                let constraints: Vec<Constraint> = if sizes.len() == children.len() {
                    sizes.iter().map(|p| Constraint::Percentage(*p)).collect()
                } else { vec![Constraint::Percentage((100 / children.len() as u16) as u16); children.len()] };
                let rects = match *kind {
                    LayoutKind::Horizontal => Layout::default().direction(Direction::Horizontal).constraints(constraints).split(area),
                    LayoutKind::Vertical => Layout::default().direction(Direction::Vertical).constraints(constraints).split(area),
                };
                for (i, child) in children.iter().enumerate() { path.push(i); rec(child, rects[i], path, out); path.pop(); }
            }
        }
    }
    let mut path = Vec::new();
    rec(node, area, &mut path, out);
}

/// Resize all panes in the current window to match their computed areas
pub fn resize_all_panes(app: &mut AppState) {
    if app.windows.is_empty() { return; }
    let area = app.last_window_area;
    if area.width == 0 || area.height == 0 { return; }
    
    fn resize_node(node: &mut Node, rects: &[(Vec<usize>, Rect)], path: &mut Vec<usize>) {
        match node {
            Node::Leaf(pane) => {
                if let Some((_, rect)) = rects.iter().find(|(p, _)| p == path) {
                    let inner_height = rect.height.saturating_sub(2).max(1);
                    let inner_width = rect.width.saturating_sub(2).max(1);
                    
                    if pane.last_rows != inner_height || pane.last_cols != inner_width {
                        let _ = pane.master.resize(portable_pty::PtySize { 
                            rows: inner_height, 
                            cols: inner_width, 
                            pixel_width: 0, 
                            pixel_height: 0 
                        });
                        if let Ok(mut parser) = pane.term.lock() {
                            parser.screen_mut().set_size(inner_height, inner_width);
                        }
                        pane.last_rows = inner_height;
                        pane.last_cols = inner_width;
                    }
                }
            }
            Node::Split { children, .. } => {
                for (i, child) in children.iter_mut().enumerate() {
                    path.push(i);
                    resize_node(child, rects, path);
                    path.pop();
                }
            }
        }
    }
    
    // Resize panes in ALL windows, not just the active one
    for win in app.windows.iter_mut() {
        let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
        compute_rects(&win.root, area, &mut rects);
        let mut path = Vec::new();
        resize_node(&mut win.root, &rects, &mut path);
    }
}

pub fn kill_all_children(node: &mut Node) {
    match node {
        Node::Leaf(p) => { let _ = p.child.kill(); }
        Node::Split { children, .. } => { for child in children.iter_mut() { kill_all_children(child); } }
    }
}

/// Returns borders as (path, kind, idx, pixel_pos, total_pixels_along_axis).
pub fn compute_split_borders(node: &Node, area: Rect, out: &mut Vec<(Vec<usize>, LayoutKind, usize, u16, u16)>) {
    fn rec(node: &Node, area: Rect, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, LayoutKind, usize, u16, u16)>) {
        match node {
            Node::Leaf(_) => {}
            Node::Split { kind, sizes, children } => {
                let constraints: Vec<Constraint> = if sizes.len() == children.len() {
                    sizes.iter().map(|p| Constraint::Percentage(*p)).collect()
                } else { vec![Constraint::Percentage((100 / children.len() as u16) as u16); children.len()] };
                let rects = match *kind {
                    LayoutKind::Horizontal => Layout::default().direction(Direction::Horizontal).constraints(constraints).split(area),
                    LayoutKind::Vertical => Layout::default().direction(Direction::Vertical).constraints(constraints).split(area),
                };
                let total_px = match *kind {
                    LayoutKind::Horizontal => area.width,
                    LayoutKind::Vertical => area.height,
                };
                for i in 0..children.len()-1 {
                    let pos = match *kind {
                        LayoutKind::Horizontal => rects[i].x + rects[i].width,
                        LayoutKind::Vertical => rects[i].y + rects[i].height,
                    };
                    out.push((path.clone(), *kind, i, pos, total_px));
                }
                for (i, child) in children.iter().enumerate() { path.push(i); rec(child, rects[i], path, out); path.pop(); }
            }
        }
    }
    let mut path = Vec::new();
    rec(node, area, &mut path, out);
}

pub fn split_sizes_at<'a>(node: &'a Node, path: Vec<usize>, idx: usize) -> Option<(u16,u16)> {
    let mut cur = node;
    for &i in path.iter() {
        match cur { Node::Split { children, .. } => { cur = children.get(i)?; } _ => return None }
    }
    if let Node::Split { sizes, .. } = cur {
        if idx+1 < sizes.len() { Some((sizes[idx], sizes[idx+1])) } else { None }
    } else { None }
}

pub fn adjust_split_sizes(root: &mut Node, d: &DragState, x: u16, y: u16) {
    if let Some(Node::Split { sizes, .. }) = get_split_mut(root, &d.split_path) {
        let total_pct = sizes[d.index] + sizes[d.index+1];
        let min_pct = 5u16;
        // Convert pixel delta to percentage delta
        let pixel_delta: i32 = match d.kind {
            LayoutKind::Horizontal => x as i32 - d.start_x as i32,
            LayoutKind::Vertical => y as i32 - d.start_y as i32,
        };
        let total_px = d.total_pixels.max(1) as i32;
        let pct_delta = (pixel_delta * total_pct as i32) / total_px;
        let left = (d.left_initial as i32 + pct_delta).clamp(min_pct as i32, (total_pct - min_pct) as i32) as u16;
        let right = total_pct - left;
        sizes[d.index] = left;
        sizes[d.index+1] = right;
    }
}

pub fn get_split_mut<'a>(node: &'a mut Node, path: &Vec<usize>) -> Option<&'a mut Node> {
    let mut cur = node;
    for &idx in path.iter() {
        match cur { Node::Split { children, .. } => { cur = children.get_mut(idx)?; } _ => return None }
    }
    Some(cur)
}

pub fn prune_exited(n: Node) -> Option<Node> {
    match n {
        Node::Leaf(mut p) => {
            match p.child.try_wait() {
                Ok(Some(_)) => None,
                _ => Some(Node::Leaf(p)),
            }
        }
        Node::Split { kind, sizes: _sizes, children } => {
            let mut new_children: Vec<Node> = Vec::new();
            for child in children { if let Some(c) = prune_exited(child) { new_children.push(c); } }
            if new_children.is_empty() { None }
            else if new_children.len() == 1 { Some(new_children.remove(0)) }
            else {
                let mut eq = vec![100 / new_children.len() as u16; new_children.len()];
                let rem = 100 - eq.iter().sum::<u16>();
                if let Some(last) = eq.last_mut() { *last += rem; }
                Some(Node::Split { kind, sizes: eq, children: new_children })
            }
        }
    }
}

pub fn path_exists(node: &Node, path: &Vec<usize>) -> bool {
    let mut cur = node;
    for &idx in path.iter() {
        match cur {
            Node::Split { children, .. } => {
                if let Some(next) = children.get(idx) { cur = next; } else { return false; }
            }
            Node::Leaf(_) => return false,
        }
    }
    matches!(cur, Node::Leaf(_) | Node::Split { .. })
}

pub fn first_leaf_path(node: &Node) -> Vec<usize> {
    fn rec(n: &Node, path: &mut Vec<usize>) -> Option<Vec<usize>> {
        match n {
            Node::Leaf(_) => Some(path.clone()),
            Node::Split { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    path.push(i);
                    if let Some(p) = rec(child, path) { return Some(p); }
                    path.pop();
                }
                None
            }
        }
    }
    rec(node, &mut Vec::new()).unwrap_or_default()
}

/// Get the pane ID of the active pane
pub fn get_active_pane_id(node: &Node, path: &[usize]) -> Option<usize> {
    match node {
        Node::Leaf(p) => Some(p.id),
        Node::Split { children, .. } => {
            if let Some(&idx) = path.first() {
                if let Some(child) = children.get(idx) {
                    return get_active_pane_id(child, &path[1..]);
                }
            }
            children.first().and_then(|c| get_active_pane_id(c, &[]))
        }
    }
}

pub fn find_window_index_by_id(app: &AppState, wid: usize) -> Option<usize> {
    app.windows.iter().position(|w| w.id == wid)
}

pub fn focus_pane_by_id(app: &mut AppState, pid: usize) {
    fn rec(node: &Node, path: &mut Vec<usize>, found: &mut Option<Vec<usize>>, pid: usize) {
        match node {
            Node::Leaf(p) => { if p.id == pid { *found = Some(path.clone()); } }
            Node::Split { children, .. } => {
                for (i, c) in children.iter().enumerate() { path.push(i); rec(c, path, found, pid); path.pop(); if found.is_some() { return; } }
            }
        }
    }
    for (wi, w) in app.windows.iter().enumerate() {
        let mut path = Vec::new();
        let mut found = None;
        rec(&w.root, &mut path, &mut found, pid);
        if let Some(p) = found { app.active_idx = wi; let win = &mut app.windows[wi]; win.active_path = p; return; }
    }
}

pub fn focus_pane_by_index(app: &mut AppState, idx: usize) {
    fn collect_pane_paths(node: &Node, path: &mut Vec<usize>, panes: &mut Vec<Vec<usize>>) {
        match node {
            Node::Leaf(_) => { panes.push(path.clone()); }
            Node::Split { children, .. } => {
                for (i, c) in children.iter().enumerate() {
                    path.push(i);
                    collect_pane_paths(c, path, panes);
                    path.pop();
                }
            }
        }
    }
    let win = &mut app.windows[app.active_idx];
    let mut pane_paths = Vec::new();
    let mut path = Vec::new();
    collect_pane_paths(&win.root, &mut path, &mut pane_paths);
    if let Some(path) = pane_paths.get(idx) {
        win.active_path = path.clone();
    }
}

/// Count the number of leaf nodes in a tree
fn count_leaves(node: &Node) -> usize {
    match node {
        Node::Leaf(_) => 1,
        Node::Split { children, .. } => children.iter().map(count_leaves).sum(),
    }
}

/// Reap exited children from the app. Returns (all_empty, any_pruned).
pub fn reap_children(app: &mut AppState) -> io::Result<(bool, bool)> {
    let mut any_pruned = false;
    for i in (0..app.windows.len()).rev() {
        let leaves_before = count_leaves(&app.windows[i].root);
        let root = std::mem::replace(&mut app.windows[i].root, Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] });
        match prune_exited(root) {
            Some(new_root) => {
                let leaves_after = count_leaves(&new_root);
                if leaves_after < leaves_before {
                    any_pruned = true;
                }
                app.windows[i].root = new_root;
                if !path_exists(&app.windows[i].root, &app.windows[i].active_path) {
                    app.windows[i].active_path = first_leaf_path(&app.windows[i].root);
                }
            }
            None => { app.windows.remove(i); any_pruned = true; }
        }
    }
    Ok((app.windows.is_empty(), any_pruned))
}
