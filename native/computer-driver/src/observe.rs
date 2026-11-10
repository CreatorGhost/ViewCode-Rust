//! Turns an agent-desktop accessibility tree into the flat, capped control list
//! `observe` returns. It mirrors `observe` in
//! `apps/server/src/computerUse/Xa11yDriverCore.ts`, so both drivers answer the
//! agent the same way: breadth-first so a cap keeps the shallow, structural
//! controls, re-sorted into reading order, each control placed under the
//! nearest heading and numbered among controls sharing its role and label.

use std::collections::{HashMap, VecDeque};

use agent_desktop_core::AccessibilityNode;
use agent_desktop_core::tree::roles::INTERACTIVE_ROLES;
use agent_desktop_core::tree::state;

/// Every value the driver reports is clipped to this many characters.
pub const VALUE_MAX_CHARS: usize = 200;
/// Nodes the walk may visit, whatever the returned cap.
pub const OBSERVE_MAX_VISITED: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedElement {
    pub role: String,
    pub label: String,
    pub value: Option<String>,
    pub section: Option<String>,
    pub instance: Option<String>,
    pub enabled: bool,
    pub focused: bool,
}

/// Each control with the node it was read from, which carries its native identity.
pub struct Observation<'a> {
    pub elements: Vec<(ObservedElement, &'a AccessibilityNode)>,
    pub truncated: bool,
}

pub fn clip(value: &str) -> String {
    if value.chars().count() <= VALUE_MAX_CHARS {
        return value.to_string();
    }
    let mut clipped: String = value.chars().take(VALUE_MAX_CHARS - 1).collect();
    clipped.push('…');
    clipped
}

fn non_empty(value: Option<&String>) -> Option<&str> {
    value
        .map(String::as_str)
        .filter(|text| !text.trim().is_empty())
}

/// The label the agent sees and the identity is checked against.
pub fn label_of(node: &AccessibilityNode) -> String {
    non_empty(node.identity.name.as_ref())
        .or_else(|| non_empty(node.identity.description.as_ref()))
        .unwrap_or("")
        .to_string()
}

fn has_state(node: &AccessibilityNode, token: &str) -> bool {
    node.presentation.states.iter().any(|value| value == token)
}

fn is_editable(node: &AccessibilityNode) -> bool {
    node.role == "textfield" || node.role == "combobox"
}

fn actionable(node: &AccessibilityNode) -> bool {
    !node.presentation.available_actions.is_empty()
        || INTERACTIVE_ROLES.contains(&node.role.as_str())
}

/// Over-matching only hides a value: the adapter's `secure` state, or an
/// editable field labelled like a password.
fn is_secure(node: &AccessibilityNode, label: &str) -> bool {
    if has_state(node, state::SECURE) {
        return true;
    }
    if !is_editable(node) {
        return false;
    }
    let lower = label.to_lowercase();
    ["password", "passcode", "passphrase"]
        .iter()
        .any(|needle| lower.contains(needle))
        || lower
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word == "pin")
}

struct Kept<'a> {
    node: &'a AccessibilityNode,
    path: Vec<usize>,
    label: String,
}

/// Reads the observed controls of `root`'s descendants (the root is the window).
pub fn flatten<'a>(
    root: &'a AccessibilityNode,
    max_elements: usize,
    query: Option<&str>,
) -> Observation<'a> {
    let cap = max_elements.min(OBSERVE_MAX_VISITED);
    let needle = query
        .map(|text| text.trim().to_lowercase())
        .filter(|text| !text.is_empty());
    let mut kept: Vec<Kept<'_>> = Vec::new();
    // Headings are recorded whether or not they match, so every kept control
    // can name the section it sits in.
    let mut headings: Vec<(Vec<usize>, String)> = Vec::new();
    let mut truncated = root.subtree_truncated;
    let mut visited = 0usize;
    let mut queue: VecDeque<(&AccessibilityNode, Vec<usize>)> = root
        .children
        .iter()
        .enumerate()
        .map(|(index, child)| (child, vec![index]))
        .collect();

    while let Some((node, path)) = queue.pop_front() {
        if visited >= OBSERVE_MAX_VISITED {
            truncated = true;
            break;
        }
        visited += 1;
        truncated |= node.subtree_truncated;
        let label = label_of(node);
        if node.role == "heading" && !label.is_empty() {
            headings.push((path.clone(), label.clone()));
        }
        let wanted = (!label.is_empty() || actionable(node))
            && needle.as_deref().is_none_or(|needle| {
                label.to_lowercase().contains(needle)
                    || node
                        .identity
                        .value
                        .as_ref()
                        .is_some_and(|value| value.to_lowercase().contains(needle))
            });
        if wanted {
            if kept.len() >= cap {
                truncated = true;
                break;
            }
            kept.push(Kept {
                node,
                path: path.clone(),
                label,
            });
        }
        for (index, child) in node.children.iter().enumerate() {
            let mut child_path = path.clone();
            child_path.push(index);
            queue.push_back((child, child_path));
        }
    }

    kept.sort_by(|left, right| left.path.cmp(&right.path));
    headings.sort_by(|left, right| left.0.cmp(&right.0));
    let placement = place(&kept, &headings);

    let elements = kept
        .iter()
        .zip(placement)
        .map(|(entry, (section, instance))| {
            let secure = is_secure(entry.node, &entry.label);
            let element = ObservedElement {
                role: entry.node.role.clone(),
                label: clip(&entry.label),
                value: if secure {
                    None
                } else {
                    non_empty(entry.node.identity.value.as_ref()).map(clip)
                },
                section: section.map(|text| clip(&text)),
                instance,
                enabled: !has_state(entry.node, state::DISABLED),
                focused: has_state(entry.node, state::FOCUSED),
            };
            (element, entry.node)
        })
        .collect();
    Observation {
        elements,
        truncated,
    }
}

/// The smallest labelled node whose bounds contain `point`, so approvals and
/// the destructive check can name what a coordinate click hits.
pub fn labelled_at(root: &AccessibilityNode, x: f64, y: f64) -> Option<(String, String)> {
    let mut best: Option<(f64, &AccessibilityNode)> = None;
    let mut stack: Vec<&AccessibilityNode> = root.children.iter().collect();
    let mut visited = 0usize;
    while let Some(node) = stack.pop() {
        visited += 1;
        if visited > OBSERVE_MAX_VISITED {
            break;
        }
        stack.extend(node.children.iter());
        let Some(bounds) = node.presentation.bounds else {
            continue;
        };
        let inside = x >= bounds.x
            && x <= bounds.x + bounds.width
            && y >= bounds.y
            && y <= bounds.y + bounds.height;
        if !inside || label_of(node).is_empty() {
            continue;
        }
        let area = bounds.width * bounds.height;
        if best.is_none_or(|(smallest, _)| area < smallest) {
            best = Some((area, node));
        }
    }
    best.map(|(_, node)| (node.role.clone(), clip(&label_of(node))))
}

/// `section` is the nearest heading before the control in reading order;
/// `instance` ("2 of 8") counts controls sharing its role and label.
fn place(
    kept: &[Kept<'_>],
    headings: &[(Vec<usize>, String)],
) -> Vec<(Option<String>, Option<String>)> {
    let key = |entry: &Kept<'_>| (entry.node.role.clone(), entry.label.clone());
    let mut totals: HashMap<(String, String), usize> = HashMap::new();
    for entry in kept.iter().filter(|entry| !entry.label.is_empty()) {
        *totals.entry(key(entry)).or_default() += 1;
    }
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    let mut heading_index = 0;
    let mut section: Option<String> = None;
    kept.iter()
        .map(|entry| {
            while heading_index < headings.len() && headings[heading_index].0 < entry.path {
                section = Some(headings[heading_index].1.clone());
                heading_index += 1;
            }
            let total = if entry.label.is_empty() {
                0
            } else {
                totals.get(&key(entry)).copied().unwrap_or(0)
            };
            let instance = (total > 1).then(|| {
                let nth = seen.entry(key(entry)).or_default();
                *nth += 1;
                format!("{nth} of {total}")
            });
            let own_heading = entry.node.role == "heading";
            (section.clone().filter(|_| !own_heading), instance)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, name: &str, children: Vec<AccessibilityNode>) -> AccessibilityNode {
        let mut node = AccessibilityNode {
            ref_id: None,
            role: role.to_string(),
            identity: Default::default(),
            presentation: Default::default(),
            children_count: None,
            subtree_truncated: false,
            children,
        };
        if !name.is_empty() {
            node.identity.name = Some(name.to_string());
        }
        node
    }

    fn with_value(mut node: AccessibilityNode, value: &str) -> AccessibilityNode {
        node.identity.value = Some(value.to_string());
        node
    }

    fn with_state(mut node: AccessibilityNode, token: &str) -> AccessibilityNode {
        node.presentation.states.push(token.to_string());
        node
    }

    fn window(children: Vec<AccessibilityNode>) -> AccessibilityNode {
        node("window", "Main", children)
    }

    #[test]
    fn keeps_reading_order_and_places_controls_under_headings() {
        let tree = window(vec![
            node("heading", "Billing", vec![]),
            node("group", "", vec![node("button", "Save", vec![])]),
            node("heading", "Shipping", vec![]),
            node("group", "", vec![node("button", "Save", vec![])]),
        ]);
        let observed = flatten(&tree, 50, None);
        let saves: Vec<_> = observed
            .elements
            .iter()
            .filter(|(element, _)| element.label == "Save")
            .map(|(element, _)| (element.section.clone(), element.instance.clone()))
            .collect();
        assert_eq!(
            saves,
            vec![
                (Some("Billing".into()), Some("1 of 2".into())),
                (Some("Shipping".into()), Some("2 of 2".into())),
            ]
        );
        assert_eq!(observed.elements[0].0.label, "Billing");
        assert_eq!(observed.elements[0].0.section, None);
        assert!(!observed.truncated);
    }

    #[test]
    fn skips_unlabelled_containers_but_keeps_unlabelled_controls() {
        let tree = window(vec![node("group", "", vec![node("textfield", "", vec![])])]);
        let observed = flatten(&tree, 50, None);
        assert_eq!(observed.elements.len(), 1);
        assert_eq!(observed.elements[0].0.role, "textfield");
    }

    #[test]
    fn cap_keeps_shallow_controls_and_reports_truncation() {
        let tree = window(vec![
            node("group", "", vec![node("button", "Deep", vec![])]),
            node("button", "Shallow A", vec![]),
            node("button", "Shallow B", vec![]),
        ]);
        let observed = flatten(&tree, 2, None);
        let labels: Vec<_> = observed
            .elements
            .iter()
            .map(|(e, _)| e.label.as_str())
            .collect();
        assert_eq!(labels, vec!["Shallow A", "Shallow B"]);
        assert!(observed.truncated);
    }

    #[test]
    fn query_matches_label_or_value_past_the_cap() {
        let tree = window(vec![
            node("button", "One", vec![]),
            node("button", "Two", vec![]),
            with_value(node("textfield", "Search", vec![]), "needle here"),
        ]);
        let observed = flatten(&tree, 1, Some("  NEEDLE "));
        assert_eq!(observed.elements.len(), 1);
        assert_eq!(observed.elements[0].0.label, "Search");
    }

    #[test]
    fn hides_secure_values() {
        let tree = window(vec![
            with_state(
                with_value(node("textfield", "Token", vec![]), "hunter2"),
                "secure",
            ),
            with_value(node("textfield", "Account password", vec![]), "hunter2"),
            with_value(node("textfield", "Enter PIN", vec![]), "1234"),
            with_value(node("textfield", "Pinned note", vec![]), "visible"),
        ]);
        let values: Vec<_> = flatten(&tree, 50, None)
            .elements
            .into_iter()
            .map(|(element, _)| element.value)
            .collect();
        assert_eq!(values, vec![None, None, None, Some("visible".into())]);
    }

    #[test]
    fn reads_enabled_and_focused_from_states() {
        let tree = window(vec![
            with_state(node("button", "Off", vec![]), "disabled"),
            with_state(node("textfield", "Here", vec![]), "focused"),
        ]);
        let observed = flatten(&tree, 50, None);
        assert!(!observed.elements[0].0.enabled);
        assert!(observed.elements[1].0.enabled && observed.elements[1].0.focused);
    }

    #[test]
    fn a_cut_subtree_counts_as_truncated() {
        let mut cut = node("group", "", vec![]);
        cut.subtree_truncated = true;
        assert!(flatten(&window(vec![cut]), 50, None).truncated);
    }

    #[test]
    fn names_the_smallest_labelled_node_under_a_point() {
        let at = |mut node: AccessibilityNode, x: f64, y: f64, width: f64, height: f64| {
            node.presentation.bounds = Some(agent_desktop_core::Rect {
                x,
                y,
                width,
                height,
            });
            node
        };
        let tree = window(vec![at(
            node(
                "group",
                "Toolbar",
                vec![
                    at(node("button", "Delete", vec![]), 10.0, 10.0, 20.0, 20.0),
                    at(node("image", "", vec![]), 12.0, 12.0, 4.0, 4.0),
                ],
            ),
            0.0,
            0.0,
            200.0,
            40.0,
        )]);
        assert_eq!(
            labelled_at(&tree, 13.0, 13.0),
            Some(("button".into(), "Delete".into()))
        );
        assert_eq!(
            labelled_at(&tree, 150.0, 20.0),
            Some(("group".into(), "Toolbar".into()))
        );
        assert_eq!(labelled_at(&tree, 500.0, 500.0), None);
    }

    #[test]
    fn clips_long_values() {
        let long = "x".repeat(VALUE_MAX_CHARS + 5);
        let clipped = clip(&long);
        assert_eq!(clipped.chars().count(), VALUE_MAX_CHARS);
        assert!(clipped.ends_with('…'));
        assert_eq!(clip("short"), "short");
    }
}
