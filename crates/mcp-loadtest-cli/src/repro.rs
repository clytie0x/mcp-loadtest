//! `--emit-repro`: turn a pipelined `lost_update` finding into a standalone
//! script on the official MCP SDK (ADR 0034).
//!
//! The script is what an upstream report needs: maintainers run it without
//! installing mcp-loadtest. It starts the server over stdio with the same
//! command, args, env and working directory, sends one round of
//! `concurrent` writes at once on one session (`Promise.allSettled` /
//! `asyncio.gather`), each with a unique marker, then one read, and prints
//! how many acknowledged markers survived. It exits 1 when any is missing.
//!
//! Every value from the plan is embedded as an escaped, ASCII-only string
//! literal, so a tool name, argument, env value or server command cannot
//! end a string or a comment and inject code. `write_args` becomes a
//! literal expression in the target language in which `{marker}`,
//! `{worker}` and `{seq}` are spliced in as variables, the way a
//! hand-written repro builds its arguments.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use mcp_loadtest::ProtocolVersion;
use mcp_loadtest::config::Config;
use mcp_loadtest::report::Report;
use mcp_loadtest::scenario::lost_update::MARKER_PLACEHOLDER;

/// Which official MCP SDK the generated script uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReproSdk {
    /// `@modelcontextprotocol/sdk`, as an ES module (`node repro.mjs`).
    Node,
    /// The `mcp` package (`python repro.py`).
    Python,
}

impl FromStr for ReproSdk {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "node" => Ok(Self::Node),
            "python" => Ok(Self::Python),
            other => Err(format!(
                "unknown SDK `{other}`; expected `node` or `python`"
            )),
        }
    }
}

impl ReproSdk {
    /// The SDK a script path implies: `.py` is Python, `.mjs` is Node. A
    /// `.js` file runs as CommonJS under many `package.json` files, so it
    /// needs `--repro-sdk node`.
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "py" => Some(Self::Python),
            "mjs" => Some(Self::Node),
            _ => None,
        }
    }

    /// How to install the SDK and run `path`, for the message after writing.
    fn run_hint(self, path: &Path) -> String {
        match self {
            Self::Node => format!(
                "node {}` with @modelcontextprotocol/sdk installed next to it",
                path.display()
            ),
            Self::Python => format!("python {}` with the mcp package installed", path.display()),
        }
    }
}

/// Where to write the script, and for which SDK.
#[derive(Debug, Clone)]
pub struct ReproRequest {
    /// Output file.
    pub path: PathBuf,
    /// Target SDK.
    pub sdk: ReproSdk,
}

impl ReproRequest {
    /// Build a request, taking the SDK from `sdk` or else from the file
    /// extension.
    pub fn new(path: PathBuf, sdk: Option<ReproSdk>) -> Result<Self> {
        let sdk = match sdk.or_else(|| ReproSdk::from_path(&path)) {
            Some(sdk) => sdk,
            None => bail!(
                "cannot tell the SDK for --emit-repro {} from its extension; \
                 use .mjs or .py, or pass --repro-sdk node|python",
                path.display()
            ),
        };
        // The Node script is an ES module; any other extension can make
        // `node` parse it as CommonJS.
        if sdk == ReproSdk::Node && path.extension().and_then(|e| e.to_str()) != Some("mjs") {
            bail!("a Node repro must end in .mjs, not {}", path.display());
        }
        Ok(Self { path, sdk })
    }

    /// Refuse a plan the target SDK cannot send unchanged. Call it before the
    /// run, so a refusal does not cost one.
    pub fn check(&self, plan: &ReproPlan) -> Result<()> {
        if self.sdk == ReproSdk::Node {
            for (flag, value) in [
                ("write_args", &plan.write_args),
                ("read_args", &plan.read_args),
            ] {
                if let Some(n) = unsafe_js_integer(value) {
                    bail!(
                        "{flag} holds {n}, which JavaScript rounds; \
                         emit a Python repro (.py) instead"
                    );
                }
            }
        }
        Ok(())
    }

    /// Write `plan`'s script if `report` found lost updates on a session the
    /// script can open, and otherwise say why none was written.
    pub fn write_for(&self, plan: &ReproPlan, report: &Report) -> Result<()> {
        if report.scenario_outcome.lost_update_count == 0 {
            eprintln!(
                "No lost update found, so no repro script was written to {}",
                self.path.display()
            );
            return Ok(());
        }
        // An unpinned run negotiates, and may have run stateless; the script
        // always opens the session with `initialize`.
        let negotiated = report.server_info.protocol_version.as_deref();
        if let Some(v) = negotiated.and_then(ProtocolVersion::parse)
            && v.is_stateless()
        {
            eprintln!(
                "The server negotiated {}, which has no initialize handshake, so no repro \
                 script was written to {}; pin `protocol_version = \"2025-11-25\"` in a \
                 `run` config to get one",
                v.as_str(),
                self.path.display()
            );
            return Ok(());
        }
        self.write(plan)
    }

    /// Write `plan`'s script to [`Self::path`] and say how to run it.
    pub fn write(&self, plan: &ReproPlan) -> Result<()> {
        self.check(plan)?;
        std::fs::write(&self.path, render(plan, self.sdk))
            .with_context(|| format!("writing repro script {}", self.path.display()))?;
        eprintln!(
            "Repro script: {}\n  run it with `{}",
            self.path.display(),
            self.sdk.run_hint(&self.path)
        );
        Ok(())
    }
}

/// Everything the script needs: the stdio server and one round of writes.
#[derive(Debug, Clone)]
pub struct ReproPlan {
    /// Server executable.
    pub command: String,
    /// Server arguments.
    pub args: Vec<String>,
    /// Environment added on top of the script's own.
    pub env: BTreeMap<String, String>,
    /// Server working directory, if the run set one.
    pub working_dir: Option<PathBuf>,
    /// Tool that records one write.
    pub write_tool: String,
    /// `write_args` template with `{marker}`, `{worker}` and `{seq}`.
    pub write_args: Value,
    /// Tool whose result must contain every acknowledged marker.
    pub read_tool: String,
    /// Arguments for `read_tool`, passed verbatim.
    pub read_args: Value,
    /// Writes sent at once.
    pub concurrent: u32,
}

impl ReproPlan {
    /// The plan of a `run` config: a pipelined `lost_update` against a stdio
    /// server. Anything else is refused before the run starts, because the
    /// script only reproduces the one-process case.
    pub fn from_config(config: &Config) -> Result<Self> {
        let scenario = &config.scenario;
        if scenario.kind != "lost_update" {
            bail!(
                "--emit-repro needs a lost_update scenario, not `{}`",
                scenario.kind
            );
        }
        let params = &scenario.params;
        if params.get("pipelined").and_then(Value::as_bool) != Some(true) {
            bail!(
                "--emit-repro needs `pipelined = true`: the script reproduces writes \
                 lost inside one server process"
            );
        }
        let server = &config.server;
        // The scripts open a session with `initialize`, which the stateless
        // revisions removed.
        if server.resolved_protocol_version().is_stateless() {
            bail!("--emit-repro needs a protocol revision with an initialize handshake");
        }
        // The SDKs advertise their own revision, so an older pin would not
        // be what the script sends.
        if let Some(pin) = server.protocol_version.as_deref()
            && pin != "auto"
            && pin != ProtocolVersion::DEFAULT_ADVERTISED.as_str()
        {
            bail!(
                "--emit-repro cannot keep `protocol_version = \"{pin}\"`: the SDK scripts \
                 negotiate their own revision; remove the pin or use \"{}\"",
                ProtocolVersion::DEFAULT_ADVERTISED.as_str()
            );
        }
        let command = match (&server.command, server.transport.as_str()) {
            (Some(command), "stdio") => command.clone(),
            _ => bail!("--emit-repro needs a stdio server with a `command`"),
        };
        let text = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("scenario.{key} (string) is required"))
        };
        let concurrent = match params.get("concurrent") {
            None => 10,
            Some(v) => v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| anyhow!("scenario.concurrent must be a positive integer"))?,
        };
        Ok(Self {
            command,
            args: server.args.clone(),
            env: server.env.clone(),
            working_dir: server.working_dir.clone(),
            write_tool: text("write_tool")?,
            write_args: params.get("write_args").cloned().unwrap_or(Value::Null),
            read_tool: text("read_tool")?,
            read_args: params
                .get("read_args")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default())),
            concurrent,
        })
    }
}

/// The script for `plan` on `sdk`.
pub fn render(plan: &ReproPlan, sdk: ReproSdk) -> String {
    match sdk {
        ReproSdk::Node => render_node(plan),
        ReproSdk::Python => render_python(plan),
    }
}

/// Shared header. Tool names appear only as escaped literals, which hold no
/// line break, so they cannot end the comment.
fn header(plan: &ReproPlan, sdk: ReproSdk, comment: &str, run: &str) -> String {
    let (writes, read) = (
        str_lit(&plan.write_tool, sdk),
        str_lit(&plan.read_tool, sdk),
    );
    let paragraphs = [
        "Lost-update repro generated by `mcp-loadtest lost-update --emit-repro`.".to_owned(),
        format!(
            "Starts the server over stdio with the official MCP SDK, sends {} {writes} \
             calls at once on one session, each with a unique marker, then calls {read} \
             once and counts how many acknowledged markers it still shows. Exits 1 if any \
             are missing.",
            plan.concurrent
        ),
        "Every write is real: point the server at a disposable store. The env below is \
         copied from the run; check it for secrets before sharing."
            .to_owned(),
    ];
    let mut out = String::new();
    for paragraph in paragraphs {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            if !line.is_empty() && line.len() + 1 + word.len() > 72 {
                let _ = writeln!(out, "{comment} {line}");
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        let _ = writeln!(out, "{comment} {line}\n{comment}");
    }
    let _ = writeln!(out, "{comment}   {run}");
    out
}

fn render_node(plan: &ReproPlan) -> String {
    let sdk = ReproSdk::Node;
    let mut env = String::from("{ ...process.env");
    for (key, value) in &plan.env {
        let _ = write!(env, ", {}: {}", key_lit(key, sdk), str_lit(value, sdk));
    }
    env.push_str(" }");
    let cwd = match &plan.working_dir {
        Some(dir) => format!("  cwd: {},\n  ", str_lit(&dir.to_string_lossy(), sdk)),
        None => String::new(),
    };
    let mut out = header(
        plan,
        sdk,
        "//",
        "npm install @modelcontextprotocol/sdk && node this-file.mjs",
    );
    let _ = write!(
        out,
        r#"
import {{ randomBytes }} from "node:crypto";
import {{ Client }} from "@modelcontextprotocol/sdk/client/index.js";
import {{ StdioClientTransport }} from "@modelcontextprotocol/sdk/client/stdio.js";

const CALLS = {calls};
const nonce = randomBytes(4).toString("hex");
const marker = (i) => `mlt${{nonce}}w${{String(i).padStart(4, "0")}}s0000x`;
const CONTROL = `mlt${{nonce}}w000cs0000x`;
const inconclusive = (why) => {{
  console.log(`INCONCLUSIVE: ${{why}}`);
  process.exit(2);
}};

const client = new Client({{ name: "mcp-loadtest-repro", version: "1" }});
try {{
  await client.connect(new StdioClientTransport({{
    command: {command},
    args: {args},
    env: {env},
  {cwd}}}));
}} catch (e) {{
  inconclusive(`the server did not start (${{e}}), so nothing can be concluded about lost writes`);
}}

// `w` is the worker ({{worker}}), `m` the marker.
const write = (w, m) => client.callTool({{ name: {write_tool}, arguments: {write_args} }});
const readStore = async (what) => {{
  let result;
  try {{
    result = await client.callTool({{ name: {read_tool}, arguments: {read_args} }});
  }} catch (e) {{
    inconclusive(`the ${{what}} read failed (${{e}}), so nothing can be concluded about lost writes`);
  }}
  if (result.isError) {{
    console.log(JSON.stringify(result.content));
    inconclusive(`the ${{what}} read returned isError, so nothing can be concluded about lost writes`);
  }}
  return JSON.stringify(result);
}};

// A control write, alone, before the concurrent ones: data the store already
// held must survive them too, and the read must be able to show it.
const control = await write("control", CONTROL).catch((e) => ({{ isError: true, content: String(e) }}));
if (control.isError) inconclusive("the control write failed, so the store cannot be checked");
if (!(await readStore("control")).includes(CONTROL)) {{
  inconclusive("the read does not show the control write, so it cannot show lost writes either");
}}

const results = await Promise.allSettled(Array.from({{ length: CALLS }}, (_, i) => write(String(i), marker(i))));
const acknowledged = [];
let refused = 0;
let failed = 0;
let timedOut = 0;
results.forEach((r, i) => {{
  if (r.status === "rejected") {{
    failed += 1;
    // The SDK's RequestTimeout error code.
    if (r.reason?.code === -32001) timedOut += 1;
  }} else if (r.value.isError) refused += 1;
  else acknowledged.push(marker(i));
}});
console.log(`calls: ${{CALLS}} sent, ${{acknowledged.length}} succeeded, ${{refused}} isError, ${{failed}} failed`);
if (acknowledged.length === 0) {{
  inconclusive("no concurrent write was acknowledged, so there is nothing to check");
}}
if (timedOut > 0) {{
  inconclusive(`${{timedOut}} writes timed out and may still change the store, so the read would prove nothing`);
}}
const read = await readStore("final");
await client.close().catch(() => {{}});
const kept = acknowledged.filter((m) => read.includes(m));
const controlKept = read.includes(CONTROL);
console.log(`read: ${{kept.length}} of ${{acknowledged.length}} acknowledged writes are in the result; control write ${{controlKept ? "kept" : "missing"}}`);
const lost = acknowledged.length - kept.length + (controlKept ? 0 : 1);
if (lost > 0) {{
  console.log(`LOST: ${{lost}} acknowledged writes are missing${{controlKept ? "" : ", including the control write"}}`);
  process.exitCode = 1;
}}
"#,
        calls = plan.concurrent,
        command = str_lit(&plan.command, sdk),
        args = list(&plan.args, sdk),
        write_tool = str_lit(&plan.write_tool, sdk),
        write_args = expr(&plan.write_args, sdk, true),
        read_tool = str_lit(&plan.read_tool, sdk),
        read_args = expr(&plan.read_args, sdk, false),
    );
    out
}

fn render_python(plan: &ReproPlan) -> String {
    let sdk = ReproSdk::Python;
    let mut env = String::from("{**os.environ");
    for (key, value) in &plan.env {
        let _ = write!(env, ", {}: {}", str_lit(key, sdk), str_lit(value, sdk));
    }
    env.push('}');
    let cwd = match &plan.working_dir {
        Some(dir) => format!("        cwd={},\n", str_lit(&dir.to_string_lossy(), sdk)),
        None => String::new(),
    };
    let mut out = header(plan, sdk, "#", "pip install mcp && python this-file.py");
    let _ = write!(
        out,
        r#"
import asyncio
import os
import secrets
import sys

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

CALLS = {calls}
# Seconds to wait for one tool call, as the Node SDK does by default; without
# it a call the server never answers hangs the script.
TIMEOUT = 60
NONCE = secrets.token_hex(4)


CONTROL = f"mlt{{NONCE}}w000cs0000x"


def marker(i):
    return f"mlt{{NONCE}}w{{i:04d}}s0000x"


def write_args(w, m):
    # `w` is the worker ({{worker}}), `m` the marker.
    return {write_args}


def is_error(result):
    # mcp 2.x names the field is_error; 1.x names it isError.
    return bool(getattr(result, "is_error", getattr(result, "isError", False)))


async def main():
    server = StdioServerParameters(
        command={command},
        args={args},
        env={env},
{cwd}    )
    async with stdio_client(server) as (r, w), ClientSession(r, w) as session:
        await asyncio.wait_for(session.initialize(), TIMEOUT)
        # A control write, alone, before the concurrent ones: data the store
        # already held must survive them too.
        try:
            control = await asyncio.wait_for(
                session.call_tool({write_tool}, write_args("control", CONTROL)), TIMEOUT
            )
        except Exception as e:  # noqa: BLE001
            print(f"INCONCLUSIVE: the control write failed ({{e}}), so the store cannot be checked")
            return 2
        if is_error(control):
            print("INCONCLUSIVE: the control write returned isError, so the store cannot be checked")
            return 2

        async def read_store(what):
            try:
                result = await asyncio.wait_for(
                    session.call_tool({read_tool}, {read_args}), TIMEOUT
                )
            except Exception as e:  # noqa: BLE001
                print(f"INCONCLUSIVE: the {{what}} read failed ({{e!r}}), so nothing can be concluded")
                return None
            if is_error(result):
                print(result.model_dump_json())
                print(f"INCONCLUSIVE: the {{what}} read returned isError, so nothing can be concluded")
                return None
            return result.model_dump_json()

        # The read must be able to show what the write stored.
        seen = await read_store("control")
        if seen is None:
            return 2
        if CONTROL not in seen:
            print("INCONCLUSIVE: the read does not show the control write, so it cannot show lost writes either")
            return 2
        results = await asyncio.gather(
            *(
                asyncio.wait_for(
                    session.call_tool({write_tool}, write_args(str(i), marker(i))), TIMEOUT
                )
                for i in range(CALLS)
            ),
            return_exceptions=True,
        )
        acknowledged, refused, failed, timed_out = [], 0, 0, 0
        for i, result in enumerate(results):
            if isinstance(result, BaseException):
                failed += 1
                if isinstance(result, asyncio.TimeoutError):
                    timed_out += 1
            elif is_error(result):
                refused += 1
            else:
                acknowledged.append(marker(i))
        print(f"calls: {{CALLS}} sent, {{len(acknowledged)}} succeeded, {{refused}} isError, {{failed}} failed")
        if not acknowledged:
            print("INCONCLUSIVE: no concurrent write was acknowledged, so there is nothing to check")
            return 2
        if timed_out:
            print(
                f"INCONCLUSIVE: {{timed_out}} writes timed out and may still change the store, "
                "so the read would prove nothing"
            )
            return 2
        read = await read_store("final")
        if read is None:
            return 2
    kept = [m for m in acknowledged if m in read]
    control_kept = CONTROL in read
    print(
        f"read: {{len(kept)}} of {{len(acknowledged)}} acknowledged writes are in the result; "
        f"control write {{'kept' if control_kept else 'missing'}}"
    )
    lost = len(acknowledged) - len(kept) + (0 if control_kept else 1)
    if lost > 0:
        suffix = "" if control_kept else ", including the control write"
        print(f"LOST: {{lost}} acknowledged writes are missing{{suffix}}")
        return 1
    return 0


async def run():
    try:
        return await main()
    except Exception as e:  # noqa: BLE001
        # A server that does not start, or a session that breaks outside a
        # tool call, says nothing about lost writes.
        print(f"INCONCLUSIVE: the session failed ({{e!r}}), so nothing can be concluded about lost writes")
        return 2


sys.exit(asyncio.run(run()))
"#,
        calls = plan.concurrent,
        command = str_lit(&plan.command, sdk),
        args = list(&plan.args, sdk),
        env = env,
        write_tool = str_lit(&plan.write_tool, sdk),
        write_args = expr(&plan.write_args, sdk, true),
        read_tool = str_lit(&plan.read_tool, sdk),
        read_args = expr(&plan.read_args, sdk, false),
    );
    out
}

/// A double-quoted string literal valid in both JavaScript and Python, and
/// pure printable ASCII: `"` and `\` are escaped, and every other character
/// outside `' '..='~'` (line breaks, U+2028, non-ASCII) becomes a `\u`
/// escape. Characters above U+FFFF use `\u{..}` in JavaScript and
/// `\U........` in Python, which have no common form.
fn str_lit(s: &str, sdk: ReproSdk) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            c if u32::from(c) <= 0xFFFF => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => match sdk {
                ReproSdk::Node => {
                    let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
                }
                ReproSdk::Python => {
                    let _ = write!(out, "\\U{:08x}", u32::from(c));
                }
            },
        }
    }
    out.push('"');
    out
}

fn list(items: &[String], sdk: ReproSdk) -> String {
    let items: Vec<String> = items.iter().map(|item| str_lit(item, sdk)).collect();
    format!("[{}]", items.join(", "))
}

/// `value` as a literal expression. With `substitute`, a string holding
/// placeholders becomes a concatenation of literals and the script's
/// variables: `m` for `{marker}` and the call index `i` for `{worker}`.
/// The script sends one round, so `{seq}` is `0`.
fn expr(value: &Value, sdk: ReproSdk, substitute: bool) -> String {
    match value {
        Value::Null => match sdk {
            ReproSdk::Node => "null".to_owned(),
            ReproSdk::Python => "None".to_owned(),
        },
        Value::Bool(b) => match (sdk, b) {
            (ReproSdk::Node, b) => b.to_string(),
            (ReproSdk::Python, true) => "True".to_owned(),
            (ReproSdk::Python, false) => "False".to_owned(),
        },
        // serde_json prints integers and finite floats in a form both
        // languages parse.
        Value::Number(n) => n.to_string(),
        Value::String(s) if substitute => template(s, sdk),
        Value::String(s) => str_lit(s, sdk),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(|v| expr(v, sdk, substitute)).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(map) if map.is_empty() => "{}".to_owned(),
        Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(key, v)| format!("{}: {}", key_lit(key, sdk), expr(v, sdk, substitute)))
                .collect();
            // Each language's usual spacing: `{ "a": 1 }` and `{"a": 1}`.
            match sdk {
                ReproSdk::Node => format!("{{ {} }}", fields.join(", ")),
                ReproSdk::Python => format!("{{{}}}", fields.join(", ")),
            }
        }
    }
}

/// The first integer in `value` outside JavaScript's safe range, which a
/// Node script would round before sending.
fn unsafe_js_integer(value: &Value) -> Option<String> {
    const MAX_SAFE: u64 = (1 << 53) - 1;
    match value {
        Value::Number(n) => {
            let unsafe_int = n.as_u64().is_some_and(|u| u > MAX_SAFE)
                || n.as_i64().is_some_and(|i| i.unsigned_abs() > MAX_SAFE);
            unsafe_int.then(|| n.to_string())
        }
        Value::Array(items) => items.iter().find_map(unsafe_js_integer),
        Value::Object(map) => map.values().find_map(unsafe_js_integer),
        _ => None,
    }
}

/// An object key. In a JavaScript object literal a plain `"__proto__"` key
/// sets the prototype instead of adding a field, so it is written computed.
fn key_lit(key: &str, sdk: ReproSdk) -> String {
    match sdk {
        ReproSdk::Node if key == "__proto__" => format!("[{}]", str_lit(key, sdk)),
        _ => str_lit(key, sdk),
    }
}

/// A string with placeholders as a concatenation. One left-to-right scan is
/// equivalent to the engine's successive replacements, because no
/// substituted value contains a brace.
fn template(s: &str, sdk: ReproSdk) -> String {
    let worker = match sdk {
        ReproSdk::Node | ReproSdk::Python => "w",
    };
    let placeholders = [
        (MARKER_PLACEHOLDER, "m"),
        ("{worker}", worker),
        ("{seq}", ""),
    ];
    let mut parts: Vec<String> = Vec::new();
    let mut literal = String::new();
    let mut rest = s;
    'scan: while !rest.is_empty() {
        for (placeholder, var) in placeholders {
            if let Some(after) = rest.strip_prefix(placeholder) {
                if var.is_empty() {
                    literal.push('0');
                } else {
                    if !literal.is_empty() {
                        parts.push(str_lit(&std::mem::take(&mut literal), sdk));
                    }
                    parts.push(var.to_owned());
                }
                rest = after;
                continue 'scan;
            }
        }
        let mut chars = rest.chars();
        if let Some(c) = chars.next() {
            literal.push(c);
        }
        rest = chars.as_str();
    }
    if !literal.is_empty() || parts.is_empty() {
        parts.push(str_lit(&literal, sdk));
    }
    parts.join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan() -> ReproPlan {
        ReproPlan {
            command: "npx".into(),
            args: vec!["-y".into(), "@kazuph/mcp-taskmanager@1.1.1".into()],
            env: BTreeMap::from([("TASK_MANAGER_FILE_PATH".into(), "/tmp/t.json".into())]),
            working_dir: None,
            write_tool: "request_planning".into(),
            write_args: json!({
                "originalRequest": "{marker}",
                "tasks": [{ "title": "{marker}", "description": "{marker}" }]
            }),
            read_tool: "list_requests".into(),
            read_args: json!({}),
            concurrent: 5,
        }
    }

    #[test]
    fn string_literals_are_ascii_and_escape_quotes_breaks_and_astral_chars() {
        let hostile = "a\"b\\c\nd\u{2028}\u{e9}\u{1f600}${x}`";
        assert_eq!(
            str_lit(hostile, ReproSdk::Node),
            r#""a\"b\\c\u000ad\u2028\u00e9\u{1f600}${x}`""#
        );
        assert_eq!(
            str_lit(hostile, ReproSdk::Python),
            r#""a\"b\\c\u000ad\u2028\u00e9\U0001f600${x}`""#
        );
        for sdk in [ReproSdk::Node, ReproSdk::Python] {
            let lit = str_lit(hostile, sdk);
            assert!(lit.chars().all(|c| (' '..='~').contains(&c)), "{lit}");
        }
    }

    #[test]
    fn placeholders_become_variables_and_the_rest_stays_literal() {
        assert_eq!(template("{marker}", ReproSdk::Node), "m");
        assert_eq!(
            template("w{worker}-s{seq}:{marker}!", ReproSdk::Node),
            r#""w" + w + "-s0:" + m + "!""#
        );
        assert_eq!(template("{worker}{marker}", ReproSdk::Python), "w + m");
        assert_eq!(template("", ReproSdk::Python), r#""""#);
        assert_eq!(template("{mark}er}", ReproSdk::Node), r#""{mark}er}""#);
    }

    #[test]
    fn expressions_follow_each_language() {
        let value = json!({ "a": [true, false, null, 1, -2.5], "b": {}, "c": "{marker}" });
        assert_eq!(
            expr(&value, ReproSdk::Node, true),
            r#"{ "a": [true, false, null, 1, -2.5], "b": {}, "c": m }"#
        );
        assert_eq!(
            expr(&value, ReproSdk::Python, true),
            r#"{"a": [True, False, None, 1, -2.5], "b": {}, "c": m}"#
        );
        // Read args are verbatim: no substitution.
        assert_eq!(
            expr(&json!({ "q": "{marker}" }), ReproSdk::Node, false),
            r#"{ "q": "{marker}" }"#
        );
        // Keys are never substituted, and `__proto__` stays a field in JS.
        assert_eq!(
            expr(&json!({ "__proto__": "{marker}" }), ReproSdk::Node, true),
            r#"{ ["__proto__"]: m }"#
        );
        assert_eq!(
            expr(&json!({ "__proto__": 1 }), ReproSdk::Python, true),
            r#"{"__proto__": 1}"#
        );
    }

    #[test]
    fn node_script_uses_the_sdk_and_passes_the_full_env() {
        let script = render(&plan(), ReproSdk::Node);
        for needle in [
            r#"import { Client } from "@modelcontextprotocol/sdk/client/index.js";"#,
            r#"import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";"#,
            "const CALLS = 5;",
            r#"  command: "npx","#,
            r#"  args: ["-y", "@kazuph/mcp-taskmanager@1.1.1"],"#,
            r#"  env: { ...process.env, "TASK_MANAGER_FILE_PATH": "/tmp/t.json" },"#,
            r#"name: "request_planning", arguments: { "originalRequest": m, "tasks": [{ "description": m, "title": m }] }"#,
            r#"name: "list_requests", arguments: {}"#,
            "Promise.allSettled",
            "process.exitCode = 1;",
            "if (result.isError) {",
            "process.exit(2);",
            r#"const control = await write("control", CONTROL)"#,
            r#"(await readStore("control")).includes(CONTROL)"#,
            "if (acknowledged.length === 0) {",
            "const controlKept = read.includes(CONTROL);",
        ] {
            assert!(script.contains(needle), "missing {needle}:\n{script}");
        }
        assert!(!script.contains("cwd:"));
    }

    #[test]
    fn a_proto_env_key_stays_an_own_property_in_node() {
        let mut plan = plan();
        plan.env.insert("__proto__".to_owned(), "x".to_owned());
        let script = render(&plan, ReproSdk::Node);
        assert!(script.contains(r#"["__proto__"]: "x""#), "{script}");
    }

    #[test]
    fn python_script_uses_the_sdk_and_passes_the_full_env() {
        let mut plan = plan();
        plan.working_dir = Some(PathBuf::from("/srv/app"));
        let script = render(&plan, ReproSdk::Python);
        for needle in [
            "from mcp import ClientSession, StdioServerParameters",
            "from mcp.client.stdio import stdio_client",
            "CALLS = 5",
            r#"        command="npx","#,
            r#"        args=["-y", "@kazuph/mcp-taskmanager@1.1.1"],"#,
            r#"        env={**os.environ, "TASK_MANAGER_FILE_PATH": "/tmp/t.json"},"#,
            r#"        cwd="/srv/app","#,
            r#"    return {"originalRequest": m, "tasks": [{"description": m, "title": m}]}"#,
            r#"session.call_tool("request_planning", write_args(str(i), marker(i))), TIMEOUT"#,
            r#"write_args("control", CONTROL)"#,
            r#"seen = await read_store("control")"#,
            "if not acknowledged:",
            "control_kept = CONTROL in read",
            r#"session.call_tool("list_requests", {})"#,
            "asyncio.gather",
            "sys.exit(asyncio.run(run()))",
            "if is_error(result):",
            "return 2",
        ] {
            assert!(script.contains(needle), "missing {needle}:\n{script}");
        }
    }

    #[test]
    fn hostile_values_stay_inside_their_literals() {
        let mut plan = plan();
        plan.command = "sh\"; process.exit(0); //".into();
        plan.write_tool = "x\n// \nprocess.exit(0)".into();
        plan.read_tool = "r\"\"\"\nimport os".into();
        plan.env = BTreeMap::from([("K\"".into(), "v\n}".into())]);
        for sdk in [ReproSdk::Node, ReproSdk::Python] {
            let script = render(&plan, sdk);
            assert!(script.is_ascii());
            // Each hostile value appears only escaped; no injected line.
            assert!(
                !script
                    .lines()
                    .any(|l| l.trim_start().starts_with("process.exit(0)"))
            );
            assert!(!script.lines().any(|l| l.trim_start() == "import os\""));
            assert!(
                script.contains(r#""sh\"; process.exit(0); //""#),
                "{script}"
            );
            assert!(script.contains(r#""x\u000a// \u000aprocess.exit(0)""#));
            assert!(script.contains(r#""K\"": "v\u000a}""#));
        }
    }

    #[test]
    fn sdk_comes_from_the_flag_or_the_extension() {
        let infer = |p: &str| ReproRequest::new(PathBuf::from(p), None).map(|r| r.sdk);
        assert_eq!(infer("repro.mjs").ok(), Some(ReproSdk::Node));
        assert!(infer("repro.js").is_err());
        assert_eq!(infer("repro.py").ok(), Some(ReproSdk::Python));
        assert!(infer("repro.txt").is_err());
        let forced_node = |p: &str| ReproRequest::new(PathBuf::from(p), Some(ReproSdk::Node));
        assert!(forced_node("repro.mjs").is_ok());
        assert!(
            forced_node("repro.js").is_err(),
            "CommonJS under many package.json files"
        );
        let forced = ReproRequest::new(PathBuf::from("repro.txt"), Some(ReproSdk::Python));
        assert_eq!(forced.map(|r| r.sdk).ok(), Some(ReproSdk::Python));
        assert_eq!("node".parse(), Ok(ReproSdk::Node));
        assert!("ruby".parse::<ReproSdk>().is_err());
    }

    #[test]
    fn a_config_must_be_a_pipelined_lost_update_on_stdio() {
        use mcp_loadtest::config::{ScenarioConfig, ServerConfig};
        let params = json!({
            "pipelined": true,
            "concurrent": 4,
            "write_tool": "append",
            "write_args": { "item": "{marker}" },
            "read_tool": "read"
        });
        let mut server = ServerConfig::stdio("python".into(), vec!["store.py".into()]);
        server.working_dir = Some(PathBuf::from("/srv"));
        let config = |kind: &str, params: Value| {
            Config::new(server.clone(), ScenarioConfig::new(kind, params))
        };

        let plan = ReproPlan::from_config(&config("lost_update", params.clone()))
            .expect("pipelined lost_update");
        assert_eq!(plan.concurrent, 4);
        assert_eq!(plan.args, ["store.py"]);
        assert_eq!(plan.working_dir, Some(PathBuf::from("/srv")));
        assert_eq!(plan.read_args, json!({}));

        let mut unpipelined = params.clone();
        unpipelined["pipelined"] = json!(false);
        let err = ReproPlan::from_config(&config("lost_update", unpipelined)).unwrap_err();
        assert!(err.to_string().contains("pipelined"), "{err}");
        assert!(ReproPlan::from_config(&config("race_check", params.clone())).is_err());

        let mut stateless = config("lost_update", params);
        stateless.server.protocol_version = Some("2026-07-28".into());
        let err = ReproPlan::from_config(&stateless).unwrap_err();
        assert!(err.to_string().contains("initialize"), "{err}");

        let mut pinned = stateless;
        pinned.server.protocol_version = Some("2025-03-26".into());
        let err = ReproPlan::from_config(&pinned).unwrap_err();
        assert!(err.to_string().contains("2025-03-26"), "{err}");
        for ok in ["auto", "2025-11-25"] {
            pinned.server.protocol_version = Some(ok.into());
            ReproPlan::from_config(&pinned).expect(ok);
        }
    }

    #[test]
    fn node_refuses_integers_javascript_would_round() {
        let request = |sdk, path| ReproRequest::new(PathBuf::from(path), Some(sdk)).unwrap();
        let mut plan = plan();
        plan.read_args = json!({ "ids": [9_007_199_254_740_991_u64, -9_007_199_254_740_991_i64] });
        request(ReproSdk::Node, "repro.mjs")
            .check(&plan)
            .expect("safe integers");
        plan.read_args = json!({ "ids": [9_007_199_254_740_993_u64] });
        let err = request(ReproSdk::Node, "repro.mjs")
            .check(&plan)
            .unwrap_err();
        assert!(err.to_string().contains("9007199254740993"), "{err}");
        request(ReproSdk::Python, "repro.py")
            .check(&plan)
            .expect("Python keeps big integers");
    }

    #[test]
    fn a_negotiated_stateless_session_gets_no_script() {
        use mcp_loadtest::{ProcessStats, ScenarioMetrics, ScenarioOutcome, ServerInfo};
        let dir = tempfile::tempdir().expect("tempdir");
        let request = ReproRequest::new(dir.path().join("repro.py"), None).unwrap();
        let mut report = Report {
            run_id: "01TEST".into(),
            started_at: std::time::SystemTime::UNIX_EPOCH,
            duration: std::time::Duration::from_secs(1),
            scenario_name: "lost_update".into(),
            server_info: ServerInfo {
                command: "python".into(),
                args: Vec::new(),
                pid: None,
                protocol_version: Some("2026-07-28".into()),
            },
            metrics: ScenarioMetrics::default(),
            process: ProcessStats::default(),
            scenario_outcome: ScenarioOutcome {
                lost_update_count: 1,
                ..ScenarioOutcome::default()
            },
            trace_path: None,
            threshold_violations: Vec::new(),
            coverage: None,
            per_tool: Default::default(),
        };
        request.write_for(&plan(), &report).expect("stateless");
        assert!(!request.path.exists(), "a stateless run got a script");
        report.server_info.protocol_version = Some("2025-11-25".into());
        request.write_for(&plan(), &report).expect("initialize");
        assert!(request.path.exists(), "an initialize run got no script");
    }

    #[test]
    fn startup_failures_are_inconclusive() {
        let node = render(&plan(), ReproSdk::Node);
        assert!(node.contains("the server did not start"), "{node}");
        let python = render(&plan(), ReproSdk::Python);
        assert!(python.contains("the session failed"), "{python}");
        assert!(python.contains("sys.exit(asyncio.run(run()))"), "{python}");
    }

    #[test]
    fn timed_out_writes_stop_before_the_read_and_the_control_uses_the_engine_marker() {
        let node = render(&plan(), ReproSdk::Node);
        assert!(node.contains("r.reason?.code === -32001"), "{node}");
        assert!(node.contains("if (timedOut > 0) {"), "{node}");
        assert!(node.contains("w000cs0000x"), "{node}");
        let python = render(&plan(), ReproSdk::Python);
        assert!(
            python.contains("isinstance(result, asyncio.TimeoutError)"),
            "{python}"
        );
        assert!(python.contains("if timed_out:"), "{python}");
        assert!(python.contains("w000cs0000x"), "{python}");
        for script in [&node, &python] {
            let stop = script.find("writes timed out").expect("timeout stop");
            let read = script
                .find(r#"read_store("final")"#)
                .or_else(|| script.find(r#"readStore("final")"#))
                .expect("final read");
            assert!(
                stop < read,
                "the timeout check must come before the final read"
            );
        }
    }
}
