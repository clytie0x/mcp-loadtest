//! Choosing `lost-update`'s write and read tools from the server's
//! `tools/list` when the flags leave them out (ADR 0035).
//!
//! A write tool is picked only when the server marks it not read-only, not
//! destructive (`destructiveHint: false`) and closed-world
//! (`openWorldHint: false`), and [`build_write_args`] can fill its input
//! schema with `{marker}` in a string. A read tool is picked only when the
//! server marks it read-only, not destructive, and it takes no arguments.
//! Among qualifying tools, names that read like a creation (`create_`,
//! `add_`, ...) or a full listing (`read_`, `list_`, ...) come first; the
//! server's listed order breaks ties.

use std::fmt::Write as _;

use serde_json::{Map, Value};

use mcp_loadtest::protocol::mcp::Tool;
use mcp_loadtest::scenario::lost_update::{MARKER_LEN, MARKER_PLACEHOLDER};

use crate::cmd_deadlock::{annotation, printable};

/// Tools listed when none qualifies; the rest are counted.
const LISTED_TOOLS_CAP: usize = 20;

/// Schema nesting the arguments builder follows before it gives up.
const MAX_DEPTH: usize = 8;

/// Name prefixes (lowercased) of write tools that add a record, preferred
/// over other qualifying writes, which may update a record that must exist.
const WRITE_PREFIXES: &[&str] = &[
    "create", "add", "append", "insert", "write", "save", "record", "store", "put", "push",
];

/// Name prefixes (lowercased) of read tools that plausibly return
/// everything, preferred over other read-only tools such as a status check.
const READ_PREFIXES: &[&str] = &["read", "list", "dump", "export", "get_all", "getall"];

/// What `lost-update` will run, after any automatic choice.
#[derive(Debug)]
pub struct LostUpdateTools {
    /// The write tool.
    pub write_tool: String,
    /// Its arguments, with `{marker}` in at least one string.
    pub write_args: Value,
    /// The read tool.
    pub read_tool: String,
    /// `true` when the read tool was picked here: nobody vouched that it
    /// lists everything, so a loss must be confirmed before it counts.
    pub read_picked: bool,
    /// One line per automatic choice, for stderr.
    pub notices: Vec<String>,
}

/// Fill in whatever the user left out. `write_args` without `write_tool` is
/// the caller's usage error and is not handled here. A named write tool
/// without arguments gets them built from its schema, whatever its hints:
/// naming it is the user's choice. Missing tools are picked by hints only.
///
/// # Errors
///
/// A named write tool the server does not list or whose schema cannot be
/// filled, or no qualifying tool where one must be picked; the message lists
/// the server's tools and why each does not qualify.
pub fn resolve(
    tools: &[Tool],
    write_tool: Option<&str>,
    write_args: Option<Value>,
    read_tool: Option<&str>,
) -> Result<LostUpdateTools, String> {
    let mut notices = Vec::new();
    let mut unmet = Unmet::default();

    let write = match (write_tool, write_args) {
        (Some(name), Some(args)) => Some((name.to_owned(), args)),
        (Some(name), None) => {
            let Some(tool) = tools.iter().find(|tool| tool.name == name) else {
                return Err(format!(
                    "--write-tool `{}` is not in the server's tools/list, so its arguments \
                     cannot be built from its schema. Pass --write-args '{{...}}' with {} in a \
                     string.",
                    printable(name),
                    MARKER_PLACEHOLDER
                ));
            };
            let args = build_write_args(&tool.input_schema).map_err(|why| {
                format!(
                    "no --write-args given and the arguments of `{}` cannot be built from its \
                     input schema: {}. Pass --write-args '{{...}}' with {} in a string.",
                    printable(name),
                    printable(&why),
                    MARKER_PLACEHOLDER
                )
            })?;
            notices.push(format!(
                "no --write-args given; writing to `{}` with arguments built from its input \
                 schema: {}",
                printable(name),
                printable(&args.to_string())
            ));
            Some((name.to_owned(), args))
        }
        (None, _) => match pick_write_tool(tools) {
            Some((tool, args)) => {
                notices.push(format!(
                    "no --write-tool given; writing with `{}` (the server marks it not \
                     read-only, not destructive and closed-world) and arguments built from its \
                     input schema: {}. These are the server's own hints; pass --write-tool to \
                     choose.",
                    printable(&tool.name),
                    printable(&args.to_string())
                ));
                Some((tool.name.clone(), args))
            }
            None => {
                unmet.write = true;
                None
            }
        },
    };

    let read = match read_tool {
        Some(name) => Some(name.to_owned()),
        None => match pick_read_tool(tools) {
            Some(tool) => {
                notices.push(format!(
                    "no --read-tool given; reading with `{}` (the server marks it read-only and \
                     it takes no arguments). Before a loss fails the run, it is confirmed by a \
                     second read; pass --read-tool to choose.",
                    printable(&tool.name)
                ));
                Some(tool.name.clone())
            }
            None => {
                unmet.read = true;
                None
            }
        },
    };

    match (write, read) {
        (Some((write_tool, write_args)), Some(read)) => {
            if !notices.is_empty() {
                notices.push("Writes are real: point the server at a disposable store.".to_owned());
            }
            Ok(LostUpdateTools {
                write_tool,
                write_args,
                read_tool: read,
                read_picked: read_tool.is_none(),
                notices,
            })
        }
        _ => Err(no_qualifying_tools_message(tools, &unmet)),
    }
}

/// The write tool `lost-update` uses when `--write-tool` is omitted, with
/// the arguments built for it: the first qualifying tool (see
/// [`write_rejection`]), names that read like a creation first.
#[must_use]
pub fn pick_write_tool(tools: &[Tool]) -> Option<(&Tool, Value)> {
    let mut qualifying: Vec<(&Tool, Value)> = tools
        .iter()
        .filter(|tool| write_hint_rejection(tool).is_none())
        .filter_map(|tool| Some((tool, build_write_args(&tool.input_schema).ok()?)))
        .collect();
    // Stable: listed order within each tier.
    qualifying.sort_by_key(|(tool, _)| !has_prefix(&tool.name, WRITE_PREFIXES));
    qualifying.into_iter().next()
}

/// The read tool `lost-update` uses when `--read-tool` is omitted: the
/// first qualifying tool (see [`read_rejection`]), names that read like a
/// listing first.
#[must_use]
pub fn pick_read_tool(tools: &[Tool]) -> Option<&Tool> {
    let mut qualifying: Vec<&Tool> = tools
        .iter()
        .filter(|tool| read_rejection(tool).is_none())
        .collect();
    qualifying.sort_by_key(|tool| !has_prefix(&tool.name, READ_PREFIXES));
    qualifying.into_iter().next()
}

/// Why `tool` cannot be picked as the write tool, or `None` if it can.
#[must_use]
pub fn write_rejection(tool: &Tool) -> Option<String> {
    write_hint_rejection(tool).or_else(|| {
        build_write_args(&tool.input_schema)
            .err()
            .map(|why| format!("arguments cannot be built: {why}"))
    })
}

/// The annotation half of [`write_rejection`]. MCP's defaults for an
/// absent `destructiveHint` and `openWorldHint` are `true`, so both must be
/// explicitly `false`.
fn write_hint_rejection(tool: &Tool) -> Option<String> {
    if !has_annotations(tool) {
        return Some(NO_HINTS.to_owned());
    }
    if annotation(tool, "readOnlyHint") == Some(true) {
        return Some("marked read-only".to_owned());
    }
    match annotation(tool, "destructiveHint") {
        Some(false) => {}
        Some(true) => return Some("marked destructive".to_owned()),
        None => return Some("not marked non-destructive (destructiveHint: false)".to_owned()),
    }
    match annotation(tool, "openWorldHint") {
        Some(false) => None,
        Some(true) => Some("marked open-world".to_owned()),
        None => Some("not marked closed-world (openWorldHint: false)".to_owned()),
    }
}

/// Why `tool` cannot be picked as the read tool, or `None` if it can.
///
/// It must take no arguments at all, optional ones included: an optional
/// `limit` or `page` could make the read list only part of the store.
#[must_use]
pub fn read_rejection(tool: &Tool) -> Option<String> {
    if !has_annotations(tool) {
        return Some(NO_HINTS.to_owned());
    }
    if annotation(tool, "readOnlyHint") != Some(true) {
        return Some("not marked read-only".to_owned());
    }
    if annotation(tool, "destructiveHint") == Some(true) {
        return Some("marked destructive".to_owned());
    }
    // Anything beyond a plain object schema (a `$ref`, `allOf`, `if` ...) may
    // require arguments this check cannot see, so it does not qualify.
    const PLAIN: &[&str] = &[
        "type",
        "properties",
        "required",
        "additionalProperties",
        "$schema",
        "$id",
        "title",
        "description",
    ];
    if let Some(schema) = tool.input_schema.as_object()
        && schema.keys().any(|key| !PLAIN.contains(&key.as_str()))
    {
        return Some("its input schema may require arguments".to_owned());
    }
    let required = match tool.input_schema.get("required") {
        None | Some(Value::Null) => false,
        Some(Value::Array(required)) => !required.is_empty(),
        Some(_) => true,
    };
    let properties = match tool.input_schema.get("properties") {
        None | Some(Value::Null) => false,
        Some(Value::Object(properties)) => !properties.is_empty(),
        Some(_) => true,
    };
    (required || properties).then(|| "takes arguments".to_owned())
}

const NO_HINTS: &str = "the server gives no hints (annotations) for it";

fn has_annotations(tool: &Tool) -> bool {
    tool.annotations
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|hints| !hints.is_empty())
}

fn has_prefix(name: &str, prefixes: &[&str]) -> bool {
    let name = name.to_ascii_lowercase();
    prefixes.iter().any(|prefix| name.starts_with(prefix))
}

/// Arguments for a write tool, built from its input schema: `{marker}` in
/// every required string, and the smallest valid value for every other
/// required property. Optional properties are left out.
///
/// Handles nested objects and arrays (an array gets one element), `enum`
/// and `const` (the first value), numeric bounds, and local `$ref`s.
/// Refuses what it cannot fill with confidence: `anyOf`, `oneOf`, `allOf`,
/// `not`, `if`, strings with a `format` or `pattern` or a length that rules
/// out a marker, arrays that need more than one element, and tuples.
///
/// # Errors
///
/// Why the schema cannot be filled, naming the property; also when no
/// required string is left to carry the marker.
pub fn build_write_args(schema: &Value) -> Result<Value, String> {
    let mut builder = ArgsBuilder {
        root: schema,
        marked: false,
    };
    let args = builder.value(schema, "", 0)?;
    if !args.is_object() {
        return Err("the input schema does not describe an object".to_owned());
    }
    if !builder.marked {
        return Err(format!("no required string can carry {MARKER_PLACEHOLDER}"));
    }
    Ok(args)
}

struct ArgsBuilder<'a> {
    root: &'a Value,
    /// Whether a string so far carries the marker.
    marked: bool,
}

impl ArgsBuilder<'_> {
    fn value(&mut self, schema: &Value, path: &str, depth: usize) -> Result<Value, String> {
        let at = display_path(path);
        if depth > MAX_DEPTH {
            return Err(format!("{at} is nested too deeply"));
        }
        let Some(obj) = schema.as_object() else {
            return Err(format!("{at} has no usable schema"));
        };
        if let Some(reference) = obj.get("$ref") {
            let target = reference
                .as_str()
                .and_then(|reference| reference.strip_prefix('#'))
                .and_then(|pointer| self.root.pointer(pointer))
                .ok_or_else(|| format!("{at} refers to a schema that is not in this one"))?;
            return self.value(target, path, depth + 1);
        }
        for keyword in ["anyOf", "oneOf", "allOf", "not", "if"] {
            if obj.contains_key(keyword) {
                return Err(format!("{at} uses `{keyword}`"));
            }
        }
        if let Some(value) = obj.get("const") {
            return Ok(value.clone());
        }
        if let Some(values) = obj.get("enum") {
            return values
                .as_array()
                .and_then(|values| values.first())
                .cloned()
                .ok_or_else(|| format!("{at} has an empty or malformed `enum`"));
        }
        match schema_type(obj).ok_or_else(|| format!("{at} has no type"))? {
            "object" => self.object(obj, path, depth),
            "array" => self.array(obj, path, depth),
            "string" => self.string(obj, &at),
            "integer" => integer(obj, &at),
            "number" => number(obj, &at),
            "boolean" => Ok(match obj.get("default") {
                Some(Value::Bool(default)) => Value::Bool(*default),
                _ => Value::Bool(false),
            }),
            "null" => Ok(Value::Null),
            other => Err(format!("{at} has the unknown type `{other}`")),
        }
    }

    fn object(
        &mut self,
        obj: &Map<String, Value>,
        path: &str,
        depth: usize,
    ) -> Result<Value, String> {
        let at = display_path(path);
        let required: Vec<&str> = match obj.get("required") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(names)) => names
                .iter()
                .map(|name| {
                    name.as_str()
                        .ok_or_else(|| format!("{at} has a malformed `required`"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(format!("{at} has a malformed `required`")),
        };
        let min_properties = obj.get("minProperties").and_then(Value::as_u64);
        if min_properties.is_some_and(|min| min > required.len() as u64) {
            return Err(format!(
                "{at} needs more properties than it requires by name"
            ));
        }
        let properties = obj.get("properties").and_then(Value::as_object);
        let mut out = Map::new();
        for name in required {
            let child = if path.is_empty() {
                name.to_owned()
            } else {
                format!("{path}.{name}")
            };
            let schema = properties
                .and_then(|properties| properties.get(name))
                .ok_or_else(|| format!("{} is required but has no schema", display_path(&child)))?;
            out.insert(name.to_owned(), self.value(schema, &child, depth + 1)?);
        }
        Ok(Value::Object(out))
    }

    fn array(
        &mut self,
        obj: &Map<String, Value>,
        path: &str,
        depth: usize,
    ) -> Result<Value, String> {
        let at = display_path(path);
        if obj.contains_key("prefixItems") || obj.get("items").is_some_and(Value::is_array) {
            return Err(format!("{at} is a tuple"));
        }
        let min_items = obj.get("minItems").and_then(Value::as_u64).unwrap_or(0);
        if min_items > 1 {
            return Err(format!("{at} needs {min_items} items"));
        }
        if obj.get("maxItems").and_then(Value::as_u64) == Some(0) {
            return Ok(Value::Array(Vec::new()));
        }
        let Some(items) = obj.get("items").filter(|items| items.is_object()) else {
            return if min_items == 0 {
                Ok(Value::Array(Vec::new()))
            } else {
                Err(format!("{at} needs an item but does not describe one"))
            };
        };
        let marked = self.marked;
        match self.value(items, &format!("{path}[0]"), depth + 1) {
            Ok(item) => Ok(Value::Array(vec![item])),
            // An empty array is valid too; it just carries no marker.
            Err(_) if min_items == 0 => {
                self.marked = marked;
                Ok(Value::Array(Vec::new()))
            }
            Err(why) => Err(why),
        }
    }

    fn string(&mut self, obj: &Map<String, Value>, at: &str) -> Result<Value, String> {
        if let Some(format) = obj.get("format") {
            return Err(format!(
                "{at} must be a string in the `{}` format",
                format.as_str().unwrap_or("?")
            ));
        }
        if obj.contains_key("pattern") {
            return Err(format!("{at} must match a pattern"));
        }
        let max = obj.get("maxLength").and_then(Value::as_u64);
        let min = obj.get("minLength").and_then(Value::as_u64);
        if max.is_some_and(|max| max < MARKER_LEN as u64)
            || min.is_some_and(|min| min > MARKER_LEN as u64)
        {
            return Err(format!("{at} cannot hold a {MARKER_LEN}-character marker"));
        }
        self.marked = true;
        Ok(Value::String(MARKER_PLACEHOLDER.to_owned()))
    }
}

/// `type`, or the first non-null one of a list, or what `properties` or
/// `items` imply.
fn schema_type(obj: &Map<String, Value>) -> Option<&str> {
    match obj.get("type") {
        Some(Value::String(kind)) => Some(kind),
        Some(Value::Array(kinds)) => {
            let mut kinds = kinds.iter().filter_map(Value::as_str);
            let first = kinds.clone().next();
            kinds.find(|kind| *kind != "null").or(first)
        }
        Some(_) => None,
        None if obj.contains_key("properties") => Some("object"),
        None if obj.contains_key("items") => Some("array"),
        None => None,
    }
}

/// The smallest integer within the bounds that is non-negative if it can
/// be, or the default.
fn integer(obj: &Map<String, Value>, at: &str) -> Result<Value, String> {
    if let Some(default) = obj
        .get("default")
        .filter(|value| value.is_i64() || value.is_u64())
    {
        return Ok(default.clone());
    }
    let bounds = Bounds::of(obj);
    let lower = bounds.lower.map(|(bound, exclusive)| {
        if exclusive {
            bound.floor() + 1.0
        } else {
            bound.ceil()
        }
    });
    let upper = bounds.upper.map(|(bound, exclusive)| {
        if exclusive {
            bound.ceil() - 1.0
        } else {
            bound.floor()
        }
    });
    let mut candidate = match (lower, upper) {
        (Some(lower), _) => lower,
        (None, Some(upper)) if upper < 0.0 => upper,
        (None, _) => 0.0,
    };
    // Round towards the bound that set the candidate onto the nearest
    // multiple: up from a lower bound, down from a negative upper one.
    if let Some(step) = obj.get("multipleOf").and_then(Value::as_f64) {
        if step <= 0.0 {
            return Err(format!("{at} has no integer this builder can choose"));
        }
        candidate = if lower.is_some() || candidate >= 0.0 {
            (candidate / step).ceil() * step
        } else {
            (candidate / step).floor() * step
        };
    }
    if upper.is_some_and(|upper| candidate > upper)
        || lower.is_some_and(|lower| candidate < lower)
        || candidate.fract() != 0.0
        || !(-9.0e15..=9.0e15).contains(&candidate)
    {
        return Err(format!("{at} has no integer this builder can choose"));
    }
    // In range checked above, so the conversion is exact.
    #[allow(clippy::cast_possible_truncation)]
    Ok(Value::from(candidate as i64))
}

/// Like [`integer`], for `number`.
fn number(obj: &Map<String, Value>, at: &str) -> Result<Value, String> {
    if let Some(default) = obj.get("default").filter(|value| value.is_number()) {
        return Ok(default.clone());
    }
    let bounds = Bounds::of(obj);
    let candidate = match (bounds.lower, bounds.upper) {
        (Some((lower, false)), _) => lower,
        (Some((lower, true)), _) => lower + 1.0,
        (None, Some((upper, false))) if upper < 0.0 => upper,
        (None, Some((upper, true))) if upper <= 0.0 => upper - 1.0,
        (None, _) => 0.0,
    };
    let fits = match bounds.upper {
        Some((upper, false)) => candidate <= upper,
        Some((upper, true)) => candidate < upper,
        None => true,
    };
    if !fits || obj.contains_key("multipleOf") || !candidate.is_finite() {
        return Err(format!("{at} has no number this builder can choose"));
    }
    serde_json::Number::from_f64(candidate)
        .map(Value::Number)
        .ok_or_else(|| format!("{at} has no number this builder can choose"))
}

/// Numeric bounds as `(value, exclusive)`, reading both the draft-06+
/// numeric `exclusiveMinimum` and draft-04's boolean form.
struct Bounds {
    lower: Option<(f64, bool)>,
    upper: Option<(f64, bool)>,
}

impl Bounds {
    fn of(obj: &Map<String, Value>) -> Self {
        let bound = |inclusive: &str, exclusive: &str| match (
            obj.get(inclusive).and_then(Value::as_f64),
            obj.get(exclusive),
        ) {
            (_, Some(Value::Number(n))) => n.as_f64().map(|n| (n, true)),
            (Some(n), Some(Value::Bool(true))) => Some((n, true)),
            (Some(n), _) => Some((n, false)),
            (None, _) => None,
        };
        Self {
            lower: bound("minimum", "exclusiveMinimum"),
            upper: bound("maximum", "exclusiveMaximum"),
        }
    }
}

fn display_path(path: &str) -> String {
    if path.is_empty() {
        "the arguments object".to_owned()
    } else {
        format!("`{path}`")
    }
}

/// Which automatic choices found nothing.
#[derive(Default)]
struct Unmet {
    write: bool,
    read: bool,
}

/// The error when a tool must be picked and none qualifies: the rule, the
/// server's tools with the reason each does not qualify, and how to choose.
fn no_qualifying_tools_message(tools: &[Tool], unmet: &Unmet) -> String {
    let role = match (unmet.write, unmet.read) {
        (true, true) => "write tool or read tool",
        (true, false) => "write tool",
        _ => "read tool",
    };
    let mut message = format!("no tool qualifies to be picked automatically as the {role}.");
    if unmet.write {
        message.push_str(
            "\nA write tool qualifies only when the server marks it not read-only, not \
             destructive (destructiveHint: false) and closed-world (openWorldHint: false), and \
             its required arguments can be filled with a string among them to carry {marker}.",
        );
    }
    if unmet.read {
        message.push_str(
            "\nA read tool qualifies only when the server marks it read-only and not \
             destructive, and it takes no arguments.",
        );
    }
    if tools.is_empty() {
        message.push_str("\nThe server lists no tools.");
    } else {
        message.push_str("\nThe server's tools:");
        for tool in tools.iter().take(LISTED_TOOLS_CAP) {
            // Without hints, the reason is the same for both roles.
            let reasons = if has_annotations(tool) {
                let mut reasons = Vec::new();
                if unmet.write {
                    reasons.push(format!(
                        "write: {}",
                        write_rejection(tool).unwrap_or_else(|| "qualifies".to_owned())
                    ));
                }
                if unmet.read {
                    reasons.push(format!(
                        "read: {}",
                        read_rejection(tool).unwrap_or_else(|| "qualifies".to_owned())
                    ));
                }
                reasons.join("; ")
            } else {
                NO_HINTS.to_owned()
            };
            let _ = write!(
                message,
                "\n  - {}: {}",
                printable(&tool.name),
                printable(&reasons)
            );
        }
        if tools.len() > LISTED_TOOLS_CAP {
            let _ = write!(
                message,
                "\n  ... and {} more",
                tools.len() - LISTED_TOOLS_CAP
            );
        }
    }
    message.push_str(
        "\nHint: pass --write-tool NAME (its arguments are built from its schema unless you \
         pass --write-args '{...}') and --read-tool NAME.",
    );
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WRITE_HINTS: &str =
        r#"{"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false}"#;

    fn tools(raw: Value) -> Vec<Tool> {
        serde_json::from_value(raw).expect("tool list")
    }

    fn write_hints() -> Value {
        serde_json::from_str(WRITE_HINTS).expect("hints")
    }

    fn tool(name: &str, schema: Value, hints: Value) -> Value {
        json!({"name": name, "inputSchema": schema, "annotations": hints})
    }

    /// server-memory's `create_entities` (2026.8.31 and 2.0.0).
    fn create_entities() -> Value {
        tool(
            "create_entities",
            json!({
                "type": "object",
                "properties": {"entities": {"type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "entityType": {"type": "string"},
                        "observations": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["name", "entityType", "observations"]
                }}},
                "required": ["entities"]
            }),
            json!({"readOnlyHint": false, "destructiveHint": false,
                   "idempotentHint": false, "openWorldHint": false}),
        )
    }

    #[test]
    fn builds_server_memory_and_taskmanager_shapes() {
        let listed = tools(json!([create_entities()]));
        assert_eq!(
            build_write_args(&listed[0].input_schema),
            Ok(json!({"entities": [{
                "name": "{marker}", "entityType": "{marker}", "observations": ["{marker}"]
            }]}))
        );
        // mcp-taskmanager 1.1.1's `request_planning`; `splitDetails` is optional.
        let request_planning = json!({
            "type": "object",
            "properties": {
                "originalRequest": {"type": "string"},
                "splitDetails": {"type": "string"},
                "tasks": {"type": "array", "items": {
                    "type": "object",
                    "properties": {"title": {"type": "string"}, "description": {"type": "string"}},
                    "required": ["title", "description"]
                }}
            },
            "required": ["originalRequest", "tasks"]
        });
        assert_eq!(
            build_write_args(&request_planning),
            Ok(json!({
                "originalRequest": "{marker}",
                "tasks": [{"title": "{marker}", "description": "{marker}"}]
            }))
        );
    }

    #[test]
    fn builds_minimal_values_for_other_types() {
        let schema = json!({
            "type": "object",
            "properties": {
                "text": {"type": ["string", "null"]},
                "kind": {"type": "string", "enum": ["note", "task"]},
                "version": {"const": 2},
                "count": {"type": "integer", "minimum": 1},
                "even": {"type": "integer", "minimum": 1, "multipleOf": 2},
                "below": {"type": "integer", "maximum": -3, "multipleOf": 2},
                "above": {"type": "integer", "exclusiveMinimum": 3},
                "negative": {"type": "integer", "maximum": -2},
                "ratio": {"type": "number", "exclusiveMinimum": 0.5, "maximum": 2},
                "page": {"type": "integer", "default": 7},
                "flag": {"type": "boolean"},
                "on": {"type": "boolean", "default": true},
                "nothing": {"type": "null"},
                "tags": {"type": "array"},
                "meta": {"type": "object"},
                "entry": {"$ref": "#/$defs/entry"},
                "optional": {"type": "string"}
            },
            "required": ["text", "kind", "version", "count", "even", "below", "above",
                         "negative", "ratio",
                         "page", "flag", "on", "nothing", "tags", "meta", "entry"],
            "$defs": {"entry": {"properties": {"id": {"type": "string"}}, "required": ["id"]}}
        });
        assert_eq!(
            build_write_args(&schema),
            Ok(json!({
                "text": "{marker}", "kind": "note", "version": 2, "count": 1, "even": 2,
                "below": -4, "above": 4,
                "negative": -2, "ratio": 1.5, "page": 7, "flag": false, "on": true,
                "nothing": null, "tags": [], "meta": {}, "entry": {"id": "{marker}"}
            }))
        );
    }

    #[test]
    fn rejects_schemas_it_cannot_fill_with_a_reason() {
        let cases = [
            (
                json!({"type": "object", "properties": {"n": {"type": "integer"}},
                    "required": ["n"]}),
                "no required string can carry {marker}",
            ),
            (
                json!({"type": "object", "properties": {"s": {"type": "string"}}}),
                "no required string",
            ),
            (
                json!({"type": "object", "properties": {"s": {"type": "string",
                    "format": "date-time"}}, "required": ["s"]}),
                "`s` must be a string in the `date-time` format",
            ),
            (
                json!({"type": "object", "properties": {"s": {"type": "string",
                    "pattern": "^[a-z]+$"}}, "required": ["s"]}),
                "`s` must match a pattern",
            ),
            (
                json!({"type": "object", "properties": {"s": {"type": "string",
                    "maxLength": 8}}, "required": ["s"]}),
                "`s` cannot hold a 22-character marker",
            ),
            (
                json!({"type": "object", "properties": {"v": {"anyOf": [{"type": "string"},
                    {"type": "integer"}]}}, "required": ["v"]}),
                "`v` uses `anyOf`",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "array", "minItems": 2,
                    "items": {"type": "string"}}}, "required": ["a"]}),
                "`a` needs 2 items",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "array",
                    "prefixItems": [{"type": "string"}]}}, "required": ["a"]}),
                "`a` is a tuple",
            ),
            (
                json!({"type": "object", "properties": {}, "required": ["ghost"]}),
                "`ghost` is required but has no schema",
            ),
            (
                json!({"type": "object", "properties": {"x": {}}, "required": ["x"]}),
                "`x` has no type",
            ),
            (
                json!({"type": "object", "properties": {"r": {"$ref": "other.json#/x"}},
                    "required": ["r"]}),
                "`r` refers to a schema that is not in this one",
            ),
            (
                json!({"type": "object", "properties": {"n": {"type": "integer",
                    "minimum": 5, "maximum": 3}, "s": {"type": "string"}},
                    "required": ["n", "s"]}),
                "`n` has no integer",
            ),
            (
                json!({"type": "object", "required": "s"}),
                "malformed `required`",
            ),
            (json!({"type": "string"}), "does not describe an object"),
            (json!(true), "has no usable schema"),
        ];
        for (schema, expected) in cases {
            let error = build_write_args(&schema).expect_err(&schema.to_string());
            assert!(error.contains(expected), "{schema}: {error}");
        }
    }

    #[test]
    fn a_self_referencing_schema_stops_at_the_depth_limit() {
        let schema = json!({
            "$ref": "#/$defs/node",
            "$defs": {"node": {"type": "object",
                "properties": {"child": {"$ref": "#/$defs/node"}}, "required": ["child"]}}
        });
        let error = build_write_args(&schema).expect_err("infinite recursion");
        assert!(error.contains("nested too deeply"), "{error}");
    }

    #[test]
    fn an_optional_array_whose_items_cannot_be_built_is_left_empty() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "links": {"type": "array", "items": {"type": "object",
                    "properties": {"url": {"type": "string", "format": "uri"},
                                   "label": {"type": "string"}},
                    "required": ["label", "url"]}}
            },
            "required": ["name", "links"]
        });
        assert_eq!(
            build_write_args(&schema),
            Ok(json!({"name": "{marker}", "links": []}))
        );
        // The item's `label` is filled before `url` fails; that partial
        // marker must not count, so a schema with no other string fails.
        let schema = json!({
            "type": "object",
            "properties": {"links": schema["properties"]["links"].clone()},
            "required": ["links"]
        });
        assert!(build_write_args(&schema).is_err());
    }

    #[test]
    fn write_pick_requires_explicit_safe_hints_and_buildable_args() {
        let string_arg =
            json!({"type": "object", "properties": {"s": {"type": "string"}}, "required": ["s"]});
        let listed = tools(json!([
            {"name": "unannotated", "inputSchema": string_arg},
            tool("read_only", string_arg.clone(),
                 json!({"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false})),
            tool("destructive", string_arg.clone(),
                 json!({"readOnlyHint": false, "destructiveHint": true, "openWorldHint": false})),
            tool("destructive_absent", string_arg.clone(),
                 json!({"readOnlyHint": false, "openWorldHint": false})),
            tool("open_world", string_arg.clone(),
                 json!({"readOnlyHint": false, "destructiveHint": false, "openWorldHint": true})),
            tool("open_world_absent", string_arg.clone(),
                 json!({"readOnlyHint": false, "destructiveHint": false})),
            tool("string_hint", string_arg.clone(),
                 json!({"readOnlyHint": false, "destructiveHint": "false",
                        "openWorldHint": false})),
            tool("no_string", json!({"type": "object"}), write_hints()),
            tool("update_entity", string_arg.clone(), write_hints()),
            tool("add_item", string_arg.clone(), write_hints()),
            tool("create_item", string_arg, write_hints())
        ]));
        let (picked, args) = pick_write_tool(&listed).expect("a tool qualifies");
        assert_eq!(
            picked.name, "add_item",
            "creation names first, then listed order"
        );
        assert_eq!(args, json!({"s": "{marker}"}));
        // Without a creation name, the first qualifying tool in listed order.
        assert_eq!(
            pick_write_tool(&listed[..9]).map(|(tool, _)| tool.name.as_str()),
            Some("update_entity")
        );
        assert!(pick_write_tool(&listed[..8]).is_none());

        let reasons: Vec<String> = listed[..8]
            .iter()
            .map(|tool| write_rejection(tool).expect("rejected"))
            .collect();
        assert_eq!(
            reasons,
            [
                "the server gives no hints (annotations) for it",
                "marked read-only",
                "marked destructive",
                "not marked non-destructive (destructiveHint: false)",
                "marked open-world",
                "not marked closed-world (openWorldHint: false)",
                "not marked non-destructive (destructiveHint: false)",
                "arguments cannot be built: no required string can carry {marker}",
            ]
        );
    }

    #[test]
    fn read_pick_requires_read_only_no_arguments_and_prefers_listings() {
        let read_only = json!({"readOnlyHint": true});
        let listed = tools(json!([
            {"name": "unannotated_list", "inputSchema": {"type": "object"}},
            tool("list_writable", json!({}), json!({"readOnlyHint": false})),
            tool("list_wipe", json!({}),
                 json!({"readOnlyHint": true, "destructiveHint": true})),
            tool("list_required", json!({"type": "object", "required": ["q"],
                 "properties": {"q": {"type": "string"}}}), read_only.clone()),
            tool("list_page", json!({"type": "object",
                 "properties": {"limit": {"type": "integer"}}}), read_only.clone()),
            tool("list_malformed", json!({"type": "object", "properties": []}),
                 read_only.clone()),
            tool("get_status", json!({"type": "object", "properties": {}}), read_only.clone()),
            tool("read_graph", json!({"type": "object", "properties": {}, "required": []}),
                 read_only.clone()),
            tool("listAll", json!({}), read_only.clone()),
            tool("list_ref", json!({"$ref": "#/$defs/q",
                 "$defs": {"q": {"type": "object", "required": ["q"]}}}), read_only.clone()),
            tool("list_all_of", json!({"type": "object",
                 "allOf": [{"required": ["q"]}]}), read_only)
        ]));
        for composed in &listed[9..] {
            assert_eq!(
                read_rejection(composed).as_deref(),
                Some("its input schema may require arguments"),
                "{}",
                composed.name
            );
        }
        assert_eq!(
            pick_read_tool(&listed).map(|tool| tool.name.as_str()),
            Some("read_graph"),
            "a listing name first, then listed order"
        );
        assert_eq!(
            pick_read_tool(&listed[..7]).map(|tool| tool.name.as_str()),
            Some("get_status"),
            "any qualifying read when no name reads like a listing"
        );
        assert!(pick_read_tool(&listed[..6]).is_none());
        let reasons: Vec<String> = listed[..6]
            .iter()
            .map(|tool| read_rejection(tool).expect("rejected"))
            .collect();
        assert_eq!(
            reasons,
            [
                "the server gives no hints (annotations) for it",
                "not marked read-only",
                "marked destructive",
                "takes arguments",
                "takes arguments",
                "takes arguments",
            ]
        );
    }

    #[test]
    fn resolve_picks_server_memory_tools() {
        let listed = tools(json!([
            create_entities(),
            tool(
                "delete_entities",
                json!({"type": "object", "properties": {"entityNames":
                 {"type": "array", "items": {"type": "string"}}}, "required": ["entityNames"]}),
                json!({"readOnlyHint": false, "destructiveHint": true,
                        "openWorldHint": false})
            ),
            tool(
                "read_graph",
                json!({"type": "object", "properties": {}}),
                json!({"readOnlyHint": true, "destructiveHint": false})
            ),
            tool(
                "search_nodes",
                json!({"type": "object", "properties": {"query":
                 {"type": "string"}}, "required": ["query"]}),
                json!({"readOnlyHint": true})
            )
        ]));
        let chosen = resolve(&listed, None, None, None).expect("both picked");
        assert_eq!(chosen.write_tool, "create_entities");
        assert_eq!(chosen.read_tool, "read_graph");
        assert!(chosen.read_picked);
        assert_eq!(chosen.notices.len(), 3, "{:?}", chosen.notices);
        assert!(
            chosen.notices[0].starts_with("no --write-tool given; writing with `create_entities`")
        );
        assert!(chosen.notices[1].starts_with("no --read-tool given; reading with `read_graph`"));
        assert_eq!(
            chosen.notices[2],
            "Writes are real: point the server at a disposable store."
        );
    }

    #[test]
    fn resolve_fills_only_what_is_missing() {
        // mcp-taskmanager annotates nothing: nothing is picked, but a named
        // write tool still gets arguments built from its schema.
        let listed = tools(json!([
            {"name": "request_planning", "inputSchema": {"type": "object",
             "properties": {"originalRequest": {"type": "string"}},
             "required": ["originalRequest"]}},
            {"name": "list_requests", "inputSchema": {"type": "object", "properties": {}}}
        ]));
        let chosen = resolve(
            &listed,
            Some("request_planning"),
            None,
            Some("list_requests"),
        )
        .expect("named tools");
        assert_eq!(chosen.write_args, json!({"originalRequest": "{marker}"}));
        assert!(!chosen.read_picked, "a named read is not rechecked");
        assert!(
            chosen.notices[0].starts_with("no --write-args given; writing to `request_planning`")
        );

        // Given arguments are used as they are.
        let chosen = resolve(
            &listed,
            Some("request_planning"),
            Some(json!({"originalRequest": "x{marker}"})),
            Some("list_requests"),
        )
        .expect("all named");
        assert_eq!(chosen.write_args, json!({"originalRequest": "x{marker}"}));
        assert!(chosen.notices.is_empty());

        let error = resolve(&listed, None, None, Some("list_requests")).expect_err("no write");
        assert!(
            error.starts_with("no tool qualifies to be picked automatically as the write tool."),
            "{error}"
        );
        assert!(
            error.contains("  - request_planning: the server gives no hints"),
            "{error}"
        );
        assert!(
            !error.contains("read:"),
            "only the missing role is explained: {error}"
        );

        let error = resolve(&listed, Some("missing"), None, None).expect_err("not listed");
        assert!(
            error.contains("--write-tool `missing` is not in the server's tools/list"),
            "{error}"
        );

        let error = resolve(&listed, Some("list_requests"), None, Some("list_requests"))
            .expect_err("no string to carry the marker");
        assert!(
            error.contains("cannot be built from its input schema: no required string"),
            "{error}"
        );
    }

    #[test]
    fn no_qualifying_message_escapes_and_caps_the_list() {
        let mut raw = vec![
            json!({"name": "evil\u{1b}[31m", "inputSchema": {"type": "object",
            "properties": {"s\u{7}": {"type": "string", "pattern": "x"}}, "required": ["s\u{7}"]},
            "annotations": serde_json::from_str::<Value>(WRITE_HINTS).expect("hints")}),
        ];
        for n in 0..25 {
            raw.push(json!({"name": format!("t{n}"), "inputSchema": {}}));
        }
        let message = no_qualifying_tools_message(
            &tools(Value::Array(raw)),
            &Unmet {
                write: true,
                read: true,
            },
        );
        assert!(
            message.contains("as the write tool or read tool."),
            "{message}"
        );
        assert!(
            message.contains("  - evil\\u{1b}[31m: write: arguments cannot be built: `s\\u{7}` must match a pattern; read: not marked read-only"),
            "{message}"
        );
        assert!(
            message.contains("\n  - t0: the server gives no hints (annotations) for it\n"),
            "one reason for a tool without hints: {message}"
        );
        assert!(message.contains("\n  ... and 6 more"), "{message}");
        assert!(message.ends_with("and --read-tool NAME."), "{message}");
        assert!(
            no_qualifying_tools_message(
                &[],
                &Unmet {
                    write: false,
                    read: true
                }
            )
            .contains("The server lists no tools.")
        );
    }
}
