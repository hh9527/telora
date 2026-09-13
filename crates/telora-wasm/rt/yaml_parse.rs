//! YAML events become a postorder graph; aliases share completed nodes only.
use crate::json_parse::{Node, Plan};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    format,
    string::{String, ToString},
    vec::Vec,
};
use saphyr_parser::{Event, Parser, ScalarStyle};

enum Frame {
    Array(usize, Vec<u32>),
    Object(usize, Vec<(u32, u32)>, Option<u32>),
}

pub(crate) fn parse(input: &str) -> Result<Plan, String> {
    // The iterator scanner counts Unicode scalar values consistently. The
    // string scanner in saphyr-parser 0.0.12 mixes byte and character offsets.
    let offsets: Vec<_> = input
        .char_indices()
        .map(|(offset, _)| offset)
        .chain(core::iter::once(input.len()))
        .collect();
    let mut names = BTreeSet::new();
    let mut previous_end = 0;
    let mut plan = Plan {
        nodes: Vec::new(),
        root: 0,
    };
    let mut frames = Vec::new();
    let mut anchors = BTreeMap::new();
    let mut root = None;
    let mut documents = 0;
    for event in Parser::new_from_iter(input.chars()) {
        let (event, span) = event.map_err(|error| error.to_string())?;
        let anchor = match &event {
            Event::Scalar(_, _, anchor, _)
            | Event::MappingStart(anchor, _)
            | Event::SequenceStart(anchor, _) => *anchor,
            _ => 0,
        };
        if anchor != 0 {
            // The parser confirms an anchor here. Inspect only the consumed node
            // properties between events, excluding comments and scalar contents.
            let properties = input
                .get(previous_end..*offsets.get(span.start.index()).ok_or("invalid YAML span")?)
                .ok_or("invalid YAML anchor span")?;
            let mut name = None;
            for line in properties.lines() {
                let content = line.split('#').next().unwrap_or("");
                if let Some((_, tail)) = content.rsplit_once('&') {
                    let end = tail
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
                        .unwrap_or(tail.len());
                    if end == 0 {
                        return Err("invalid YAML anchor".into());
                    }
                    name = Some(tail[..end].to_string());
                }
            }
            let name = name.ok_or("YAML anchor must name a value")?;
            if !names.insert(name) {
                return Err("duplicate YAML anchor".into());
            }
        }
        if !matches!(
            event,
            Event::Nothing
                | Event::StreamStart
                | Event::StreamEnd
                | Event::DocumentStart(_)
                | Event::DocumentEnd
        ) {
            previous_end = *offsets.get(span.end.index()).ok_or("invalid YAML span")?;
        }
        if frames.len() > 512 {
            return Err("YAML nesting limit".into());
        }
        let (node, anchor) = match event {
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentEnd => continue,
            Event::DocumentStart(_) => {
                documents += 1;
                if documents > 1 {
                    return Err("multiple YAML documents are not supported".into());
                }
                continue;
            }
            Event::SequenceStart(anchor, tag) => {
                if tag.is_some() {
                    return Err("custom YAML tags are not supported".into());
                }
                frames.push(Frame::Array(anchor, Vec::new()));
                continue;
            }
            Event::MappingStart(anchor, tag) => {
                if tag.is_some() {
                    return Err("custom YAML tags are not supported".into());
                }
                frames.push(Frame::Object(anchor, Vec::new(), None));
                continue;
            }
            Event::SequenceEnd => {
                let Some(Frame::Array(anchor, items)) = frames.pop() else {
                    return Err("invalid YAML sequence".into());
                };
                (Node::Array(items), anchor)
            }
            Event::MappingEnd => {
                let Some(Frame::Object(anchor, entries, None)) = frames.pop() else {
                    return Err("invalid YAML mapping".into());
                };
                (mapping(&plan, entries)?, anchor)
            }
            Event::Scalar(text, style, anchor, tag) => {
                let node = if let Some(tag) = tag {
                    if tag.is_yaml_core_schema() && tag.suffix == "binary" {
                        Node::Bytes(crate::yaml_scalar::binary(&text)?)
                    } else {
                        return Err("custom YAML tags are not supported".into());
                    }
                } else if style == ScalarStyle::Plain {
                    crate::yaml_scalar::scalar(&text)?
                } else {
                    Node::String(text.into_owned())
                };
                (node, anchor)
            }
            Event::Alias(anchor) => {
                let id = *anchors.get(&anchor).ok_or("unknown or cyclic YAML alias")?;
                attach(&mut frames, &mut root, id)?;
                continue;
            }
        };
        let id = u32::try_from(plan.nodes.len()).map_err(|_| "YAML node count exceeds wasm32")?;
        plan.nodes.push(node);
        if anchor != 0 {
            anchors.insert(anchor, id);
        }
        attach(&mut frames, &mut root, id)?;
    }
    plan.root = if let Some(root) = root {
        root
    } else {
        plan.nodes.push(Node::Null);
        0
    };
    Ok(plan)
}

fn attach(frames: &mut [Frame], root: &mut Option<u32>, id: u32) -> Result<(), String> {
    match frames.last_mut() {
        Some(Frame::Array(_, items)) => items.push(id),
        Some(Frame::Object(_, entries, key)) => {
            if let Some(key) = key.take() {
                entries.push((key, id));
            } else {
                *key = Some(id);
            }
        }
        None => {
            if root.replace(id).is_some() {
                return Err("multiple YAML roots".into());
            }
        }
    }
    Ok(())
}

fn mapping(plan: &Plan, entries: Vec<(u32, u32)>) -> Result<Node, String> {
    let mut fields = BTreeMap::new();
    let mut merged = Vec::new();
    let mut explicit = BTreeSet::new();
    for (key, value) in entries {
        let Node::String(key) = &plan.nodes[key as usize] else {
            return Err("YAML mapping keys must be Strings".into());
        };
        if key.is_empty() {
            return Err("empty YAML mapping key".into());
        }
        if !explicit.insert(key.clone()) {
            return Err(format!("duplicate YAML key {key:?}"));
        }
        if key == "<<" {
            merge(plan, value, &mut merged)?;
        } else {
            fields.insert(key.clone(), value);
        }
    }
    let mut seen = BTreeSet::new();
    for (key, value) in merged {
        if explicit.contains(&key) {
            continue;
        }
        if !seen.insert(key.clone()) {
            return Err(format!("duplicate effective YAML merge key {key:?}"));
        }
        fields.insert(key, value);
    }
    Ok(Node::Object(fields.into_iter().collect()))
}

fn merge(plan: &Plan, id: u32, output: &mut Vec<(String, u32)>) -> Result<(), String> {
    match &plan.nodes[id as usize] {
        Node::Object(fields) => output.extend(fields.iter().cloned()),
        Node::Array(items) => {
            for id in items {
                let Node::Object(fields) = &plan.nodes[*id as usize] else {
                    return Err("YAML merge sequence items must be mappings".into());
                };
                output.extend(fields.iter().cloned());
            }
        }
        _ => return Err("YAML merge value must be a mapping or sequence of mappings".into()),
    }
    Ok(())
}
