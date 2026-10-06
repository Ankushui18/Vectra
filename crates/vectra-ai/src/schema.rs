//! The strict **AI command schema** (Task 9.0, RULE 1).
//!
//! > "The AI output must be strictly typed JSON that conforms to the existing
//! > `Command` schema (`CreateNode`, `SetParameter`, `ApplyOperation`,
//! > `BindSpring`, etc.). No hallucinated APIs, no raw SVG strings, no direct
//! > DOM manipulation."
//!
//! The schema *is* `vectra_core::Command`: there is no second, AI-flavoured
//! command language to keep in sync, no shim enum, no string-prefixed DSL. A
//! model's reply is a JSON **array of command objects**, deserialized by the
//! same `serde` impl the UI's own commands use — so a command the AI can express
//! is exactly a command the user could have clicked, and anything else fails to
//! parse.
//!
//! ```json
//! [
//!   { "type": "SetVariable", "name": "base", "value": 40 },
//!   { "type": "CreateNode", "id": "$new:card",
//!     "name": "card",
//!     "kind": { "Rectangle": { "x": {"Literal": 0}, "y": {"Literal": 0},
//!                              "width": {"Literal": 80}, "height": {"Literal": 60},
//!                              "corner_radius": {"Literal": 0} } } },
//!   { "type": "SetParameter", "node_id": "$new:card", "property": "width",
//!     "value": { "Float": {"Variable": "base"} } }
//! ]
//! ```
//!
//! # The one extension: `$new:<slug>`
//!
//! A `NodeId` in Vectra is a UUID, and a model cannot mint one. Left alone, that
//! forces the model to *invent* an id — the exact hallucination RULE 2 exists to
//! prevent — and the invention would either collide with an existing node or
//! produce `NodeNotFound` at dispatch time.
//!
//! So the schema reserves one token shape, `$new:<slug>` (and the same for
//! `$new:constraint:`, `$new:expression:`, `$new:track:`), for ids that do not
//! exist yet. [`resolve_plan`] mints a real UUID for each distinct placeholder —
//! once per plan, so a created node can be referenced by the commands that
//! follow it — and rewrites every occurrence consistently.
//!
//! This is a *transport* convention, not a new API: the resolved plan is
//! ordinary `Command` values, and nothing downstream ever sees a `$new:` token.
//! (An `OperationId` in Vectra is a `NodeId`, so `$new:` covers it too.)
//!
//! # Two checks, two owners
//!
//! * **[`resolve_plan`]** (this module, the AI layer) refuses tokens that name
//!   nothing in the summary — the Context Law. Only the AI layer has a summary,
//!   so only the AI layer can check it, and it happens *before* dispatch with no
//!   mutation.
//! * **`Engine::dispatch`** (RULE 3) catches everything else — property names,
//!   types, port matching, cycles, arity. The AI layer never duplicates an engine
//!   rule; it only makes sure the model was talking about *this* document.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use vectra_core::{new_constraint_id, new_expression_id, new_node_id, Command, DocumentSummary};

use crate::error::AiError;

/// The placeholder prefix for an id that does not exist yet.
pub const NEW_ID_PREFIX: &str = "$new:";

/// JSON keys that carry an id, and the placeholder namespace each belongs to.
///
/// `id` is command-dependent (`CreateNode` → node, `RemoveConstraint` →
/// constraint), so it is resolved by looking at the sibling `type` field — see
/// [`resolve_id_value`]. Every other key is unambiguous.
fn key_namespace(key: &str) -> Option<IdNamespace> {
    match key {
        "node_id" | "node" | "inputs" => Some(IdNamespace::Node),
        "track_id" => Some(IdNamespace::Track),
        "expression_id" => Some(IdNamespace::Expression),
        "constraint_id" => Some(IdNamespace::Constraint),
        _ => None,
    }
}

/// Which kind of id a placeholder must be minted as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdNamespace {
    Node,
    Track,
    Expression,
    Constraint,
}

impl IdNamespace {
    fn mint(self) -> String {
        match self {
            Self::Node => new_node_id().to_string(),
            Self::Track => vectra_core::TrackId::new().to_string(),
            Self::Expression => new_expression_id().to_string(),
            Self::Constraint => new_constraint_id().to_string(),
        }
    }

    /// The namespace a `$new:` token must carry to be minted in this position
    /// (`$new:card` and `$new:constraint:k1` are both fine for a node slot; the
    /// qualifier is documentation for the model, not a type).
    fn accepts(self, qualifier: &str) -> bool {
        match qualifier {
            "" => true,
            "node" | "op" | "operation" => self == Self::Node,
            "track" => self == Self::Track,
            "expression" | "expr" => self == Self::Expression,
            "constraint" => self == Self::Constraint,
            // A bare slug (`$new:card`) is a name, and names are accepted
            // wherever the *default* namespace of that slot is what the model
            // means. Being strict here would reject `$new:card` on a node slot
            // for no gain, so unqualified slugs are accepted everywhere.
            _ => true,
        }
    }
}

/// What a `Command`'s `id` field refers to, by command tag.
fn id_namespace_of_command(tag: &str) -> IdNamespace {
    match tag {
        // `AddConstraint` carries its rule (and its id) inside `constraint`.
        "RemoveConstraint" | "SetConstraintEnabled" | "AddConstraint" => IdNamespace::Constraint,
        "DefineExpression" | "RemoveExpression" => IdNamespace::Expression,
        "SetMotionTrack" | "RemoveMotionTrack" => IdNamespace::Track,
        // `CreateNode` / `ApplyOperation` / `RemoveOperation` / `RemoveProceduralNode`
        // / `SetOperationEnabled` / `SetProceduralEnabled`: an `OperationId` is a
        // `NodeId` in this engine, and procedural nodes are keyed by node id.
        _ => IdNamespace::Node,
    }
}

/// Extract the model's plan from its reply.
///
/// A model that is *asked* for JSON often wraps it in a fenced block, or in a
/// sentence. Extracting the payload is transport, not schema: the parse that
/// follows is strict, and the extraction never repairs a command. Two shapes are
/// accepted — a bare array, or `{"commands": [...]}` — because both are what a
/// reasonable model emits when told "reply with JSON".
pub fn parse_reply(reply: &str) -> Result<Value, AiError> {
    let trimmed = strip_fences(reply);
    if let Ok(value) = serde_json::from_str::<Value>(trimmed.trim()) {
        return Ok(unwrap_commands(value));
    }
    // Look for the outermost JSON value inside the prose.
    for (open, close) in [('[', ']'), ('{', '}')] {
        if let (Some(start), Some(end)) = (trimmed.find(open), trimmed.rfind(close)) {
            if start < end {
                if let Ok(value) = serde_json::from_str::<Value>(&trimmed[start..=end]) {
                    return Ok(unwrap_commands(value));
                }
            }
        }
    }
    Err(AiError::InvalidJson {
        detail: "no JSON array or object found in the reply".to_string(),
        reply: reply.to_string(),
    })
}

fn unwrap_commands(value: Value) -> Value {
    match &value {
        Value::Object(map) => match map.get("commands") {
            Some(Value::Array(_)) => map.get("commands").cloned().unwrap_or(value),
            _ => value,
        },
        _ => value,
    }
}

fn strip_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Turn a model's reply into commands: extract, resolve ids, parse, and check
/// the plan against the document summary.
///
/// The order matters. Ids are resolved *before* deserialization because a
/// placeholder is not a UUID and would otherwise fail to parse; the context
/// check runs *after* deserialization because it compares typed ids against the
/// summary, and a plan that failed to parse has no ids to check.
pub fn compile_plan(reply: &str, summary: &DocumentSummary) -> Result<Vec<Command>, AiError> {
    let value = parse_reply(reply)?;
    let resolved = resolve_plan(&value, summary)?;
    check_context(&resolved, summary)?;
    Ok(resolved)
}

/// Rewrite `$new:` placeholders into real ids and check every other id against
/// the summary. Returns the resolved commands; `resolved_json` keeps the JSON a
/// UI wants to preview (post-resolution, so what is shown is what will run).
pub fn resolve_plan(value: &Value, summary: &DocumentSummary) -> Result<Vec<Command>, AiError> {
    let (resolved, _) = resolve_plan_with_json(value, summary)?;
    Ok(resolved)
}

/// [`resolve_plan`], also returning the resolved JSON.
pub fn resolve_plan_with_json(
    value: &Value,
    summary: &DocumentSummary,
) -> Result<(Vec<Command>, Vec<Value>), AiError> {
    let array = match value {
        Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    let known: std::collections::BTreeSet<&str> = summary.node_ids();
    let mut minted: BTreeMap<String, String> = BTreeMap::new();
    let mut out = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        let resolved = resolve_item(item, &known, &mut minted, index)?;
        out.push(resolved);
    }
    let mut commands = Vec::with_capacity(out.len());
    for (index, item) in out.iter().enumerate() {
        let command = serde_json::from_value::<Command>(item.clone()).map_err(|error| {
            AiError::UnknownCommand {
                detail: error.to_string(),
                command: item.clone(),
            }
        })?;
        // RULE 1's teeth. `Command` is an internally tagged enum, and serde
        // cannot `deny_unknown_fields` those — so a misspelled field would be
        // *ignored*, and the engine would quietly apply something other than
        // what the model meant. Re-serializing the parsed command gives the
        // canonical shape, and anything the reply carried that the canonical
        // shape does not have is a field the engine has no room for.
        //
        // (`null` is exempt: `value: null` legitimately rides along for the
        // constraint kinds that take no operand, and serde drops it.)
        let canonical =
            serde_json::to_value(&command).map_err(|error| AiError::UnknownCommand {
                detail: error.to_string(),
                command: item.clone(),
            })?;
        let mut unknown = Vec::new();
        unknown_fields(item, &canonical, "", &mut unknown);
        if !unknown.is_empty() {
            return Err(AiError::UnknownCommand {
                detail: format!(
                    "command {index} has field(s) `{}` that the schema does not define \
                     (the engine would have ignored them)",
                    unknown.join("`, `")
                ),
                command: item.clone(),
            });
        }
        commands.push(command);
    }
    Ok((commands, out))
}

/// Every field a reply carries that the parsed command does not — recursively,
/// because a hallucinated field is just as wrong inside `kind` as at the top.
fn unknown_fields(reply: &Value, canonical: &Value, path: &str, out: &mut Vec<String>) {
    match (reply, canonical) {
        (Value::Object(reply), Value::Object(canonical)) => {
            for (key, value) in reply {
                match canonical.get(key) {
                    Some(nested) => {
                        unknown_fields(value, nested, &format!("{path}.{key}"), out);
                    }
                    None if !value.is_null() => out.push(format!("{path}.{key}")),
                    None => {}
                }
            }
        }
        (Value::Array(reply), Value::Array(canonical)) => {
            for (index, (value, nested)) in reply.iter().zip(canonical).enumerate() {
                unknown_fields(value, nested, &format!("{path}[{index}]"), out);
            }
        }
        _ => {}
    }
}

fn resolve_item(
    item: &Value,
    known: &std::collections::BTreeSet<&str>,
    minted: &mut BTreeMap<String, String>,
    index: usize,
) -> Result<Value, AiError> {
    let Value::Object(map) = item else {
        return Err(AiError::UnknownCommand {
            detail: format!("command {index} is not a JSON object"),
            command: item.clone(),
        });
    };
    let tag = map
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| AiError::UnknownCommand {
            detail: format!("command {index} has no `type` field"),
            command: item.clone(),
        })?
        .to_string();
    let default_ns = id_namespace_of_command(&tag);
    let mut out = Map::with_capacity(map.len());
    for (key, value) in map {
        let rendered = match (key.as_str(), value) {
            (
                "id" | "node_id" | "track_id" | "expression_id" | "constraint_id" | "node",
                Value::String(token),
            ) => {
                let ns = if key == "id" {
                    default_ns
                } else {
                    key_namespace(key).unwrap_or(default_ns)
                };
                Value::String(resolve_token(token, ns, known, minted, &tag)?)
            }
            ("inputs", Value::Array(items)) => Value::Array(
                items
                    .iter()
                    .map(|item| match item {
                        Value::String(token) => Ok(Value::String(resolve_token(
                            token,
                            IdNamespace::Node,
                            known,
                            minted,
                            &tag,
                        )?)),
                        other => Ok(other.clone()),
                    })
                    .collect::<Result<Vec<Value>, AiError>>()?,
            ),
            // Nested payloads that carry ids of their own: a constraint's
            // targets, a procedural node's kind/`source`, a wire's `from`, a
            // keyframe binding's track.
            // The constraint's own `id`, and its targets' `node_id`s, live in
            // the same object: the default namespace is the constraint's, and
            // the explicit `node_id` keys win inside the walk.
            ("constraint", Value::Object(fields)) => Value::Object(resolve_nested(
                fields,
                IdNamespace::Constraint,
                known,
                minted,
                &tag,
            )?),
            ("kind" | "track" | "node" | "from" | "output" | "value", Value::Object(fields)) => {
                let ns = match key.as_str() {
                    "track" => IdNamespace::Track,
                    _ => default_ns,
                };
                Value::Object(resolve_nested(fields, ns, known, minted, &tag)?)
            }
            _ => value.clone(),
        };
        out.insert(key.clone(), rendered);
    }
    Ok(Value::Object(out))
}

/// Walk a nested object, resolving id-bearing keys anywhere inside it.
///
/// The walk is deliberately *generic*: a constraint's `targets[].node_id`, a
/// `{"Procedural": {"node": …}}` parameter, and a procedural `source` node all
/// live at different depths, and a new command that adds one more id field
/// should not need a new case here.
fn resolve_nested(
    map: &Map<String, Value>,
    default_ns: IdNamespace,
    known: &std::collections::BTreeSet<&str>,
    minted: &mut BTreeMap<String, String>,
    tag: &str,
) -> Result<Map<String, Value>, AiError> {
    let mut out = Map::with_capacity(map.len());
    for (key, value) in map {
        let rendered = match (key.as_str(), value) {
            // `Parameter::Expression(id)` on the wire is `{"Expression": "<uuid>"}`
            // — an id that may equally be a `$new:` placeholder, so it is
            // resolved here just like a top-level id. (`{"Variable": "base"}` is
            // a *name*, and is deliberately left alone.)
            ("Expression", Value::String(token)) => Value::String(resolve_token(
                token,
                IdNamespace::Expression,
                known,
                minted,
                tag,
            )?),
            (key, Value::String(token)) if id_key(key) => {
                let ns = key_namespace(key).unwrap_or(default_ns);
                Value::String(resolve_token(token, ns, known, minted, tag)?)
            }
            (_, Value::Array(items)) => {
                let mut resolved = Vec::with_capacity(items.len());
                for item in items {
                    resolved.push(match item {
                        Value::Object(inner) => {
                            Value::Object(resolve_nested(inner, default_ns, known, minted, tag)?)
                        }
                        other => other.clone(),
                    });
                }
                Value::Array(resolved)
            }
            (_, Value::Object(inner)) => {
                Value::Object(resolve_nested(inner, default_ns, known, minted, tag)?)
            }
            _ => value.clone(),
        };
        out.insert(key.clone(), rendered);
    }
    Ok(out)
}

fn id_key(key: &str) -> bool {
    matches!(
        key,
        "id" | "node_id" | "node" | "track_id" | "expression_id" | "constraint_id"
    )
}

fn resolve_token(
    token: &str,
    ns: IdNamespace,
    known: &std::collections::BTreeSet<&str>,
    minted: &mut BTreeMap<String, String>,
    tag: &str,
) -> Result<String, AiError> {
    if let Some(slug) = token.strip_prefix(NEW_ID_PREFIX) {
        let (qualifier, name) = match slug.split_once(':') {
            Some((qualifier, name)) => (qualifier, name),
            None => ("", slug),
        };
        if !ns.accepts(qualifier) {
            return Err(AiError::InvalidPlaceholder {
                token: token.to_string(),
                detail: format!("`$new:{qualifier}:` is not an id for a `{tag}` command"),
            });
        }
        let key = format!("{qualifier}:{name}");
        return Ok(minted.entry(key).or_insert_with(|| ns.mint()).clone());
    }
    if known.contains(token) {
        return Ok(token.to_string());
    }
    Err(AiError::UnknownNodeId {
        id: token.to_string(),
        command: tag.to_string(),
    })
}

/// The Context Law, enforced: every id a plan *references* must either exist in
/// the summary or be created by the plan itself.
///
/// `resolve_plan` already refuses unknown tokens, so this is the second half of
/// the same law and catches what resolving cannot see: a plan that references a
/// node it never creates, and a plan that tries to create a node that is already
/// there (which the engine would reject as `NodeAlreadyExists`, but which is
/// really a *context* mistake — the model used `$new:` for something that
/// exists).
pub fn check_context(commands: &[Command], summary: &DocumentSummary) -> Result<(), AiError> {
    let known: std::collections::BTreeSet<String> =
        summary.nodes.iter().map(|node| node.id.clone()).collect();
    let mut created: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for command in commands {
        if let Command::CreateNode { id, .. } = command {
            let id = id.to_string();
            if known.contains(&id) {
                return Err(AiError::PlanConflict {
                    detail: format!(
                        "`CreateNode` reuses the id of an existing node ({id}); \
                         existing nodes are edited with `SetParameter`, and new ones use `$new:<slug>`"
                    ),
                });
            }
            created.insert(id);
        }
        for reference in referenced_nodes(command) {
            if known.contains(&reference) || created.contains(&reference) {
                continue;
            }
            return Err(AiError::UnknownNodeId {
                id: reference,
                command: command.label(),
            });
        }
    }
    Ok(())
}

/// Every node id a command reads or targets, for the context check.
fn referenced_nodes(command: &Command) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |id: vectra_core::NodeId| out.push(id.to_string());
    match command {
        Command::CreateNode { .. } => {}
        Command::DeleteNode { id } => push(*id),
        Command::SetParameter { node_id, value, .. } => {
            push(*node_id);
            collect_param_nodes(value, &mut out);
        }
        // Task 10.1: a path rewrite targets one node, and both halves of the
        // geometry (the start point and every segment endpoint) can name ports.
        Command::SetPath {
            id,
            start,
            segments,
        } => {
            push(*id);
            collect_parameter_nodes(start, &mut out);
            for segment in segments {
                for param in segment.params() {
                    collect_parameter_nodes(param, &mut out);
                }
            }
        }
        // Task 10.2: a paint stack names one node, and every paint slot in it can
        // reference a port (a gradient's frame, a stroke's width, a solid
        // colour). The AI's context check must see those references, exactly as
        // it sees a path's vertices — a gradient driven by a procedural output is
        // just as much a dependency as a rectangle's width.
        Command::SetAppearances {
            node_id,
            appearances,
        } => {
            push(*node_id);
            for layer in appearances {
                layer
                    .paint
                    .visit_colors(|_, color| collect_parameter_nodes(color, &mut out));
                layer
                    .paint
                    .visit_points(|_, point| collect_parameter_nodes(point, &mut out));
                layer
                    .paint
                    .visit_floats(|_, float| collect_parameter_nodes(float, &mut out));
                if let vectra_core::AppearanceKind::Stroke { width } = &layer.kind {
                    collect_parameter_nodes(width, &mut out);
                }
                collect_parameter_nodes(&layer.opacity, &mut out);
            }
        }
        // Flags, names, layers and artboards name nodes and layers but no
        // *parameters*: there is no source reference for the schema to carry, so
        // the only thing to record is the node id a rename or a visibility
        // toggle touches.
        Command::SetNodeVisible { id, .. }
        | Command::SetNodeLocked { id, .. }
        | Command::RenameNode { id, .. } => push(*id),
        // Task 10.4: a move names two nodes — the one that travels and the group
        // it lands in — and both must be in the AI's context before the command
        // is allowed through, exactly as a rename's id must be. (Since Task 10.5
        // this is the *only* placement command: `ReorderNode` is retired, so a
        // reorder is a move whose parent is the container it is already in.)
        Command::SetNodeParent { id, parent, .. } => {
            push(*id);
            if let Some(parent) = parent {
                push(*parent);
            }
        }
        Command::AssignNodeToLayer { node_id, .. } | Command::DetachNodeFromLayers { node_id } => {
            push(*node_id)
        }
        Command::CreateLayer { .. }
        | Command::DeleteLayer { .. }
        | Command::RenameLayer { .. }
        | Command::SetLayerVisible { .. }
        | Command::SetLayerLocked { .. }
        | Command::ReorderLayer { .. }
        | Command::CreateArtboard { .. }
        | Command::DeleteArtboard { .. }
        | Command::RenameArtboard { .. }
        | Command::SetArtboardBounds { .. }
        | Command::SetArtboardBackground { .. }
        | Command::SetActiveArtboard { .. }
        | Command::SetActiveLayer { .. } => {}
        Command::AddConstraint { constraint } => {
            for target in &constraint.targets {
                push(target.node_id);
            }
        }
        Command::RemoveConstraint { .. } | Command::SetConstraintEnabled { .. } => {}
        Command::BeginDrag { node_id }
        | Command::UpdateDrag { node_id, .. }
        | Command::EndDrag { node_id }
        | Command::RemoveProceduralNode { id: node_id }
        | Command::SetProceduralEnabled { id: node_id, .. }
        | Command::DisconnectProcedural { node_id, .. } => push(*node_id),
        Command::ApplyOperation { inputs, .. } => {
            for input in inputs {
                push(*input);
            }
        }
        Command::RemoveOperation { .. } | Command::SetOperationEnabled { .. } => {}
        Command::BindMotion {
            node_id, binding, ..
        } => {
            push(*node_id);
            match binding {
                vectra_core::MotionBinding::Spring { target, .. } => {
                    collect_parameter_nodes(target, &mut out)
                }
                vectra_core::MotionBinding::StateDriven {
                    true_value,
                    false_value,
                    ..
                } => {
                    collect_parameter_nodes(true_value, &mut out);
                    collect_parameter_nodes(false_value, &mut out);
                }
                vectra_core::MotionBinding::KeyframeTrack { .. } => {}
            }
        }
        Command::SetMotionTrack { .. } | Command::RemoveMotionTrack { .. } => {}
        Command::AddProceduralNode { node } => {
            if let vectra_core::ProceduralKind::Source { node: subject } = &node.kind {
                push(*subject);
            }
            for from in node.wires.values() {
                push(from.node);
            }
            for value in node.operands.values() {
                collect_param_nodes(value, &mut out);
            }
        }
        Command::ConnectProcedural { from, .. } => push(from.node),
        Command::SetProceduralOperand { value, .. } => collect_param_nodes(value, &mut out),
        Command::SetVariable { .. }
        | Command::RemoveVariable { .. }
        | Command::DefineExpression { .. }
        | Command::RemoveExpression { .. } => {}
        Command::Batch { commands } => {
            for inner in commands {
                out.extend(referenced_nodes(inner));
            }
        }
    }
    out
}

fn collect_param_nodes(value: &vectra_core::ParamValue, out: &mut Vec<String>) {
    match value {
        vectra_core::ParamValue::Float(param) => collect_parameter_nodes(param, out),
        vectra_core::ParamValue::Point(param) => collect_parameter_nodes(param, out),
        vectra_core::ParamValue::Color(param) => collect_parameter_nodes(param, out),
    }
}

fn collect_parameter_nodes<T>(param: &vectra_core::Parameter<T>, out: &mut Vec<String>) {
    if let vectra_core::Parameter::Procedural(output) = param {
        out.push(output.node.to_string());
    }
}

/// The shape the UI previews: the resolved plan as JSON.
pub fn plan_json(commands: &[Command]) -> Vec<Value> {
    commands
        .iter()
        .map(|command| serde_json::to_value(command).unwrap_or(Value::Null))
        .collect()
}

/// Render a plan the way the AI panel shows it — one command per line, in the
/// engine's own JSON so what the user reads is what will be dispatched.
pub fn render_plan(commands: &[Value]) -> String {
    commands
        .iter()
        .map(|command| serde_json::to_string(command).unwrap_or_else(|_| "null".to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}
