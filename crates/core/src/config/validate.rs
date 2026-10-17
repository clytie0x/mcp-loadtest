//! Semantic validation for [`super::Config`]. Pulled out of `config.rs` so the
//! schema file stays focused on the structs themselves. The free fn shape lets
//! `Config::from_toml_str` call it without an `impl` redirection.

use super::{Config, ConfigError, OAuthFlow, OAuthRegistration};
use crate::version::ProtocolVersion;
use url::Url;

/// Transports recognized by the parser + runtime.
pub(super) const KNOWN_TRANSPORTS: &[&str] = &["stdio", "http", "sse"];

/// Scenario kinds the parser knows about. Mirrors §8 of DESIGN.md and the
/// modules actually shipped under `src/scenario/`. Keep this in sync with the
/// `pub mod` lines in `scenario/mod.rs` so TOML configs documented in the
/// README parse cleanly.
pub(super) const KNOWN_SCENARIOS: &[&str] = &[
    "sustained",
    "deadlock_probe",
    "cold_start",
    "race_check",
    "pattern",
    "version_matrix",
];

/// Run semantic checks on an already-parsed `Config`. Pulled out so tests
/// can poke individual rules without re-parsing.
pub(super) fn validate(cfg: &Config) -> Result<(), ConfigError> {
    // server.transport must be a known string.
    if !KNOWN_TRANSPORTS.contains(&cfg.server.transport.as_str()) {
        return Err(ConfigError::Invalid(format!(
            "server.transport: unknown transport `{}` (expected one of: {})",
            cfg.server.transport,
            KNOWN_TRANSPORTS.join(", ")
        )));
    }

    // Transport-specific field requirements.
    match cfg.server.transport.as_str() {
        "stdio" if cfg.server.command.is_none() => {
            return Err(ConfigError::Invalid(
                "server.command is required when transport = \"stdio\"".into(),
            ));
        }
        "http" | "sse" if cfg.server.url.is_none() => {
            return Err(ConfigError::Invalid(format!(
                "server.url is required when transport = \"{}\"",
                cfg.server.transport
            )));
        }
        _ => {}
    }

    // allowed_hosts entries must be bare hostnames / IP literals — no
    // scheme, port, path, or whitespace. A non-bare entry would never match
    // `Url::host_str()` and is almost certainly an operator mistake (e.g.
    // pasting a full URL). Reject early with an actionable message
    // (SSRF guard, ADR 0012).
    for e in &cfg.server.allowed_hosts {
        let bad = e.is_empty()
            || e.contains("://")
            || e.contains('/')
            || e.contains(':')
            || e.chars().any(char::is_whitespace);
        if bad {
            return Err(ConfigError::Invalid(format!(
                "allowed_hosts entry `{e}` must be a bare hostname (no scheme/port/path)"
            )));
        }
    }

    validate_remote_headers(cfg)?;
    validate_auth(cfg)?;

    // protocol_version must be "auto" or a known revision (ADR 0018).
    if let Some(v) = &cfg.server.protocol_version
        && v != "auto"
    {
        let Some(parsed) = ProtocolVersion::parse(v) else {
            let known: Vec<&str> = ProtocolVersion::ALL.iter().map(|p| p.as_str()).collect();
            return Err(ConfigError::Invalid(format!(
                "server.protocol_version: unsupported revision `{v}` (expected \"auto\" or one of: {})",
                known.join(", ")
            )));
        };
        // Stateless mode ships for stdio + Streamable HTTP only
        // (ADR 0019 scope): SSE is the legacy transport the stateless core
        // retires.
        if parsed.is_stateless() && cfg.server.transport == "sse" {
            return Err(ConfigError::Invalid(format!(
                "server.protocol_version: `{v}` (stateless) is not supported on the `{}` \
                 transport — use \"stdio\" or \"http\" (ADR 0019)",
                cfg.server.transport
            )));
        }
    }

    if matches!(cfg.server.transport.as_str(), "http" | "sse") {
        let raw_url = cfg
            .server
            .url
            .as_deref()
            .expect("remote transports were checked for a URL above");
        validate_remote_endpoint(
            raw_url,
            &cfg.server.transport,
            !cfg.server.headers_from_env.is_empty(),
        )
        .map_err(|message| ConfigError::Invalid(format!("server.url: {message}")))?;
    }

    // scenario.type must be a known scenario kind.
    if !KNOWN_SCENARIOS.contains(&cfg.scenario.kind.as_str()) {
        return Err(ConfigError::Invalid(format!(
            "scenario.type: unknown scenario `{}` (expected one of: {})",
            cfg.scenario.kind,
            KNOWN_SCENARIOS.join(", ")
        )));
    }

    // Thresholds: every Some(_) value must be sensible.
    if let Some(rate) = cfg.thresholds.error_rate
        && !(0.0..=1.0).contains(&rate)
    {
        return Err(ConfigError::Invalid(format!(
            "thresholds.error_rate: must be in [0.0, 1.0], got {rate}"
        )));
    }
    if let Some(growth) = cfg.thresholds.memory_growth_mb
        && (!growth.is_finite() || growth < 0.0)
    {
        return Err(ConfigError::Invalid(format!(
            "thresholds.memory_growth_mb: must be finite and >= 0, got {growth}"
        )));
    }
    if let Some(slope) = cfg.thresholds.rss_leak_mb_per_sec
        && (!slope.is_finite() || slope < 0.0)
    {
        return Err(ConfigError::Invalid(format!(
            "thresholds.rss_leak_mb_per_sec: must be finite and >= 0, got {slope}"
        )));
    }
    // Duration is unsigned (`std::time::Duration`) so it can't be negative;
    // we don't need to check for negative latencies. We do check for the
    // pathological `0` case on the per-call hang timeout which would make
    // every call instantly hang.
    if let Some(d) = cfg.thresholds.hang_timeout
        && d.is_zero()
    {
        return Err(ConfigError::Invalid(
            "thresholds.hang_timeout: must be > 0".to_string(),
        ));
    }

    validate_output(cfg)?;

    Ok(())
}

fn validate_output(cfg: &Config) -> Result<(), ConfigError> {
    const FORMATS: &[&str] = &["terminal", "markdown", "json", "junit", "prometheus"];
    let mut formats = std::collections::BTreeSet::new();
    for format in &cfg.output.formats {
        if !FORMATS.contains(&format.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "output.formats: unknown format `{format}` (expected one of: {})",
                FORMATS.join(", ")
            )));
        }
        if !formats.insert(format.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "output.formats: duplicate format `{format}`"
            )));
        }
    }

    Ok(())
}

fn validate_auth(cfg: &Config) -> Result<(), ConfigError> {
    let Some(auth) = &cfg.server.auth else {
        return Ok(());
    };

    if !matches!(cfg.server.transport.as_str(), "http" | "sse") {
        return Err(ConfigError::Invalid(
            "server.auth is supported only for http and sse transports".into(),
        ));
    }
    if cfg
        .server
        .headers_from_env
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"))
    {
        return Err(ConfigError::Invalid(
            "server.auth is mutually exclusive with server.headers_from_env.Authorization".into(),
        ));
    }
    if auth.max_step_up_retries > 3 {
        return Err(ConfigError::Invalid(
            "server.auth.max_step_up_retries: must be in 0..=3".into(),
        ));
    }
    let mut scopes = std::collections::BTreeSet::new();
    for scope in &auth.scopes {
        if scope.is_empty() || scope.chars().any(char::is_whitespace) {
            return Err(ConfigError::Invalid(format!(
                "server.auth.scopes: `{scope}` must be a non-empty OAuth scope without whitespace"
            )));
        }
        if !scopes.insert(scope) {
            return Err(ConfigError::Invalid(format!(
                "server.auth.scopes: duplicate scope `{scope}`"
            )));
        }
    }

    match &auth.registration {
        OAuthRegistration::PreRegistered {
            client_id,
            client_secret_env,
            ..
        } => {
            if client_id.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "server.auth.client_id: must not be empty".into(),
                ));
            }
            if let Some(env_name) = client_secret_env
                && !is_portable_env_name(env_name)
            {
                return Err(ConfigError::Invalid(format!(
                    "server.auth.client_secret_env: `{env_name}` is not a portable environment-variable name"
                )));
            }
            if auth.flow == OAuthFlow::ClientCredentials && client_secret_env.is_none() {
                return Err(ConfigError::Invalid(
                    "server.auth.client_credentials requires client_secret_env".into(),
                ));
            }
        }
        OAuthRegistration::ClientIdMetadata {
            client_id_metadata_url,
        } => {
            if auth.flow == OAuthFlow::ClientCredentials {
                return Err(ConfigError::Invalid(
                    "server.auth.client_credentials requires registration = \"pre_registered\""
                        .into(),
                ));
            }
            validate_https_or_loopback_url(
                client_id_metadata_url,
                "server.auth.client_id_metadata_url",
            )?;
        }
        OAuthRegistration::Dynamic { client_name } => {
            if auth.flow == OAuthFlow::ClientCredentials {
                return Err(ConfigError::Invalid(
                    "server.auth.client_credentials requires registration = \"pre_registered\""
                        .into(),
                ));
            }
            if client_name
                .as_ref()
                .is_some_and(|name| name.trim().is_empty())
            {
                return Err(ConfigError::Invalid(
                    "server.auth.client_name: must not be empty when provided".into(),
                ));
            }
        }
    }

    Ok(())
}

fn validate_https_or_loopback_url(raw: &str, field: &str) -> Result<(), ConfigError> {
    let parsed = Url::parse(raw)
        .map_err(|error| ConfigError::Invalid(format!("{field}: invalid URL: {error}")))?;
    let loopback = parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        return Err(ConfigError::Invalid(format!(
            "{field}: must use HTTPS (HTTP is allowed only for loopback test fixtures)"
        )));
    }
    if parsed.username() != "" || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(ConfigError::Invalid(format!(
            "{field}: userinfo and fragments are not allowed"
        )));
    }
    Ok(())
}

/// Validate only header names and environment-variable references. Secret
/// values are intentionally not read during config parsing: a config may be
/// validated in a different process/environment from the eventual run.
fn validate_remote_headers(cfg: &Config) -> Result<(), ConfigError> {
    if !cfg.server.headers_from_env.is_empty() && cfg.server.transport == "stdio" {
        return Err(ConfigError::Invalid(
            "server.headers_from_env is only supported for http, sse, and ws transports".into(),
        ));
    }

    let mut names = std::collections::BTreeSet::new();
    for (name, env_name) in &cfg.server.headers_from_env {
        let folded = name.to_ascii_lowercase();
        if !is_http_token(name) {
            return Err(ConfigError::Invalid(format!(
                "server.headers_from_env: `{name}` is not a valid HTTP header name"
            )));
        }
        if is_managed_remote_header(&folded) {
            return Err(ConfigError::Invalid(format!(
                "server.headers_from_env: `{name}` is managed by mcp-loadtest and cannot be overridden"
            )));
        }
        if !names.insert(folded) {
            return Err(ConfigError::Invalid(format!(
                "server.headers_from_env: duplicate header name `{name}` (header names are case-insensitive)"
            )));
        }
        if !is_portable_env_name(env_name) {
            return Err(ConfigError::Invalid(format!(
                "server.headers_from_env: `{env_name}` is not a portable environment-variable name"
            )));
        }
    }
    Ok(())
}

fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Whether an outbound header is owned by HTTP, the MCP protocol, or the
/// transport stack and therefore cannot be supplied through remote-auth
/// environment references.
///
/// Kept in the core config layer so parse-time and transport-time validation
/// share one future-proof denylist.
pub fn is_managed_remote_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "accept"
            | "connection"
            | "content-length"
            | "content-type"
            | "host"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    ) || name.starts_with("mcp-")
        || name.starts_with("sec-websocket-")
}

/// Parse and enforce the security policy shared by config validation and all
/// direct remote-transport constructors.
///
/// The returned diagnostics deliberately describe the violated rule without
/// echoing `raw`, because URL userinfo and query strings can contain
/// credentials. Static remote headers are allowed only over TLS.
pub fn validate_remote_endpoint(
    raw: &str,
    transport: &str,
    has_remote_headers: bool,
) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "remote endpoint is not a valid absolute URL")?;

    if !matches!(transport, "http" | "sse") {
        return Err("remote endpoint transport is unsupported".into());
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err("remote endpoint scheme must be http:// or https://".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("remote endpoint must include a host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("remote endpoint URL userinfo is forbidden".into());
    }
    if url.fragment().is_some() {
        return Err("remote endpoint URL fragments are forbidden".into());
    }
    if has_remote_headers && url.scheme() != "https" {
        return Err("server.headers_from_env requires the https:// scheme".into());
    }

    Ok(url)
}

/// Render a remote endpoint without credential-bearing URL components.
///
/// Userinfo and fragments are removed. If any query is present, the whole
/// query is replaced with the literal marker `redacted`; individual names and
/// values are intentionally not retained. Invalid URLs collapse to a fixed
/// marker so their raw text cannot leak through a report.
pub fn sanitize_remote_endpoint(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return "<invalid remote endpoint>".into();
    };
    if url.set_username("").is_err() || url.set_password(None).is_err() {
        return "<invalid remote endpoint>".into();
    }
    url.set_fragment(None);
    if url.query().is_some() {
        url.set_query(Some("redacted"));
    }
    url.to_string()
}

fn is_portable_env_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
