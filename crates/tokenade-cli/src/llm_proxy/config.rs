//! Where the proxy listens, what it talks to, and what it admits it cannot do.
//!
//! The proxy has two deployments and one property that matters more than
//! either: **the remote mode must be EXPLICIT, never a silent fallback**, and
//! the startup banner must say which capabilities are live. The reason is the
//! one this whole document keeps returning to — a user cannot tell "it does not
//! work" from "it works and does not tell me", and both show up as a zero.

use std::fmt::Write as _;

/// Where this proxy is running, which decides what it is allowed to offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deployment {
    /// On the user's own machine. Everything is available and the API key never
    /// leaves the host.
    Local,
    /// In a container, possibly shared. The key transits, and the web
    /// capabilities are OFF: one non-residential egress IP cannot carry the volume,
    /// and when it is rate-limited the quality collapses for every user at once.
    Remote,
}

/// Why a configuration was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A listen address reachable from outside the machine, in local mode. The
    /// refusal is the feature: silently accepting it is how a proxy holding
    /// somebody's API key ends up bound to 0.0.0.0 because a container example
    /// was copied onto a laptop.
    NonLoopbackWithoutRemote(String),
    /// An upstream that is not an absolute http(s) URL.
    BadUpstream(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonLoopbackWithoutRemote(a) => write!(
                f,
                "refusing to listen on {a}: that address is reachable from off this machine, \
                 and this proxy holds your API key. Pass the remote deployment explicitly if \
                 that is what you meant."
            ),
            Self::BadUpstream(u) => write!(f, "upstream must be an http(s) URL, got {u:?}"),
        }
    }
}

/// One proxy instance.
#[derive(Debug, Clone)]
pub struct Config {
    /// `host:port` to bind.
    pub listen: String,
    /// Base URL of the provider, e.g. `https://api.anthropic.com`. The request
    /// target the agent sent is appended to it verbatim, so a provider that
    /// moves a route does not need a change here.
    pub upstream: String,
    pub deployment: Deployment,
    /// Whether outgoing tool results may be rewritten at all. Off is a
    /// perfectly good mode: the exact `usage` accounting is worth having
    /// on its own, and it is the only rung that gets the provider's own numbers
    /// rather than an estimate.
    pub compaction: bool,
    /// Seconds of SILENCE upstream before giving up. Not a deadline on the
    /// answer — a model may think for minutes.
    pub idle_timeout_secs: u64,
    /// What this provider charges for reading and for WRITING its cache.
    ///
    /// Carried in the config rather than in a table inside `cache_gate`,
    /// because these differ per provider and per model and a baked-in table
    /// would rot exactly the way the per-agent delivery caps did.
    pub prices: super::cache_gate::Prices,
    /// Which agent this proxy is standing in front of, when the operator says.
    ///
    /// It decides whether folding is allowed AT ALL (one authority per payload), and the answer
    /// is not "yes" by default: Claude Code's hooks already own every tool
    /// result, so a proxy in front of it must relay and count and nothing else.
    /// `None` means nobody said, and the proxy then falls back to the runtime
    /// signal — the `X-Tokenade-Channel` header — which is the only evidence
    /// available when the operator did not name an agent.
    pub agent: Option<tokenade_core::detect::Agent>,
    /// The style preamble to prepend to the outgoing system prompt, when the
    /// operator has one set and this agent has no prompt-time hook of its own.
    ///
    /// `None` is the common case and means "change nothing about what the model
    /// is told". Rank 1 does this with a `UserPromptSubmit`-shaped hook;
    /// eleven agents on the roster have no such event, and for them this is the
    /// only moment the preamble can reach the model at all.
    pub system_preamble: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8787".to_string(),
            upstream: String::new(),
            deployment: Deployment::Local,
            compaction: true,
            idle_timeout_secs: 600,
            // Anthropic's published ratios against a plain input token: a
            // cached read is a tenth, a cache write is a quarter more than
            // full price. They are the harshest of the wires we speak, so a
            // gate tuned on them declines rather than over-approves when the
            // operator has not said which provider this is.
            prices: super::cache_gate::Prices {
                input: 1.0,
                cache_read: 0.1,
                cache_write: 1.25,
            },
            agent: None,
            system_preamble: None,
        }
    }
}

impl Config {
    /// Refuse anything that would be a silent downgrade.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(self.upstream.starts_with("http://") || self.upstream.starts_with("https://")) {
            return Err(ConfigError::BadUpstream(self.upstream.clone()));
        }
        if self.deployment == Deployment::Local && !listens_on_loopback(&self.listen) {
            return Err(ConfigError::NonLoopbackWithoutRemote(self.listen.clone()));
        }
        Ok(())
    }

    /// Whether the user's web capabilities are offered on this deployment.
    pub fn web_capabilities(&self) -> bool {
        self.deployment == Deployment::Local
    }

    /// The startup banner.
    ///
    /// Every line is a capability with a verdict. A banner that lists only what
    /// works reads exactly like a banner on a broken install, so the OFF lines
    /// carry their reason.
    pub fn banner(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "[tokenade llm-proxy] listening on {} · upstream {}",
            self.listen,
            upstream_label(&self.upstream)
        );
        let _ = writeln!(
            s,
            "  deployment      {}",
            match self.deployment {
                Deployment::Local => "LOCAL — your key does not leave this machine",
                Deployment::Remote =>
                    "REMOTE — your key transits this container (asked for explicitly)",
            }
        );
        let _ = writeln!(
            s,
            "  agent           {}",
            match self.agent {
                Some(a) => format!(
                    "{} — {}",
                    a.as_str(),
                    super::agent_matrix::entry(a).role(a.coverage()).describe()
                ),
                None => "not declared — folding falls back to the X-Tokenade-Channel header, \
                         and an agent whose hooks already fold will be folded for TWICE unless \
                         it sets one"
                    .to_string(),
            }
        );
        let _ = writeln!(
            s,
            "  compaction      {}",
            if self.compaction {
                "ON — tool results already in the history are folded on the way out"
            } else {
                "OFF — nothing is rewritten; accounting only"
            }
        );
        let _ = writeln!(
            s,
            "  accounting      ON — exact, read from the provider's own usage event"
        );
        let _ = writeln!(
            s,
            "  web search      {}",
            if self.web_capabilities() {
                "ON"
            } else {
                "OFF — a shared egress IP gets rate-limited for everyone at once"
            }
        );
        let _ = writeln!(
            s,
            "  request bodies  NEVER persisted · logs redacted · keys relayed, not stored"
        );
        let _ = write!(
            s,
            "  note            rank 4 does not refund the turn a raw result first arrived on; \
             it removes its cost from every later turn. A hook catches it before the first send."
        );
        s
    }
}

/// True when this bind address can only be reached from this machine.
///
/// `pub(crate)` so that any listener this binary opens asks the same
/// question; `every_listener_asks` checks that they do.
///
/// Parsed rather than string-matched on "localhost": `127.0.0.1`, `[::1]` and
/// the whole `127.0.0.0/8` block are loopback, and `0.0.0.0` is emphatically
/// not, however local the port looks.
pub(crate) fn listens_on_loopback(addr: &str) -> bool {
    use std::net::{IpAddr, SocketAddr};
    // A bare `:port` or a hostname needs resolving; only decide `true` on
    // something we positively parsed as a loopback IP.
    if let Ok(sa) = addr.parse::<SocketAddr>() {
        return sa.ip().is_loopback();
    }
    let host = match addr.rsplit_once(':') {
        Some((h, _)) => h,
        None => addr,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// An upstream safe to print: scheme and host, never the path or query.
///
/// Some providers take the key as a query parameter (`?key=…`), so a banner
/// that echoed the configured URL would print a credential to a terminal, into
/// a scrollback, and into whatever collects the container's stdout.
fn upstream_label(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?']).next().unwrap_or(rest);
    let scheme = if url.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{host}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            upstream: "https://api.anthropic.com".into(),
            ..Config::default()
        }
    }

    #[test]
    fn a_default_config_is_loopback_and_valid() {
        assert!(cfg().validate().is_ok());
        assert!(listens_on_loopback("127.0.0.1:8787"));
        assert!(listens_on_loopback("[::1]:8787"));
        assert!(listens_on_loopback("localhost:8787"));
        assert!(listens_on_loopback("127.9.9.9:1"));
    }

    #[test]
    fn binding_off_machine_in_local_mode_is_refused_not_downgraded() {
        // "Remote mode must be EXPLICIT, never a silent
        // fallback." A proxy that accepted this would be holding an API key
        // on a listener the network can reach, because an example was copied.
        for addr in ["0.0.0.0:8787", "192.168.1.10:8787", "[::]:8787"] {
            let c = Config {
                listen: addr.into(),
                ..cfg()
            };
            assert_eq!(
                c.validate(),
                Err(ConfigError::NonLoopbackWithoutRemote(addr.into())),
                "{addr} must be refused in local mode"
            );
            // The SAME address is fine once the operator has said so.
            let explicit = Config {
                deployment: Deployment::Remote,
                ..c
            };
            assert!(explicit.validate().is_ok());
        }
    }

    #[test]
    fn an_upstream_that_is_not_a_url_is_refused() {
        for bad in ["api.anthropic.com", "", "ftp://x"] {
            let c = Config {
                upstream: bad.into(),
                ..cfg()
            };
            assert_eq!(c.validate(), Err(ConfigError::BadUpstream(bad.into())));
        }
    }

    #[test]
    fn the_banner_names_every_capability_including_the_off_ones() {
        let local = cfg().banner();
        assert!(local.contains("LOCAL"));
        assert!(local.contains("web search      ON"));
        assert!(local.contains("compaction      ON"));
        // The honest limit is IN the banner, not only in the docs — the product doc
        // states it, and the banner is the doc every user reads.
        assert!(local.contains("does not refund the turn"));

        let remote = Config {
            deployment: Deployment::Remote,
            listen: "0.0.0.0:8787".into(),
            compaction: false,
            ..cfg()
        }
        .banner();
        assert!(remote.contains("REMOTE"));
        assert!(remote.contains("web search      OFF"));
        assert!(remote.contains("compaction      OFF"));
        // An OFF line that does not say why is indistinguishable from a bug.
        assert!(remote.contains("rate-limited"));
    }

    #[test]
    fn the_banner_never_prints_a_key_carried_in_the_upstream_url() {
        let c = Config {
            upstream: "https://generativelanguage.googleapis.com/v1beta?key=AIzaSyTOPSECRET".into(),
            ..cfg()
        };
        let b = c.banner();
        assert!(!b.contains("AIzaSyTOPSECRET"), "{b}");
        assert!(b.contains("https://generativelanguage.googleapis.com"));
    }
}

#[cfg(test)]
mod every_listener_asks {
    use super::listens_on_loopback;

    /// Every listener this binary opens must ask the same question about its
    /// bind address.
    ///
    /// A SOURCE guard, because the failure it prevents is a NEW listener added
    /// without the check — which is how a second one (since removed) came to be
    /// missing it: it took its address from the environment verbatim and bound
    /// it, while the rule that refuses a reachable address lived here. The sweep
    /// covers every production file of the crate, so the next listener is
    /// checked wherever it lands, not only where the last one lived.
    #[test]
    fn every_production_bind_asks_whether_the_address_is_local() {
        let root = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src");
        let sources = tokenade_core::source_guard::rust_sources(&root);
        let mut checked = 0usize;
        for (path, src) in &sources {
            let name = path.strip_prefix(&root).unwrap_or(path).display();
            for (n, line) in src.lines().enumerate() {
                let t = line.trim_start();
                if t.starts_with("//") || !t.contains("TcpListener::bind") {
                    continue;
                }
                // A loopback literal needs no gate: it cannot be reached from
                // off the machine whatever the environment says.
                if line.contains("127.0.0.1") || line.contains("[::1]") {
                    continue;
                }
                checked += 1;
                let upto = src.find(line).unwrap_or(0);
                let start = src[..upto].rfind("\nfn ").unwrap_or(0);
                let window = &src[start..upto];
                assert!(
                    window.contains("listens_on_loopback") || window.contains("validate"),
                    "{name}:{} binds a configurable address without asking \
                     `listens_on_loopback` (or a validate() that does):\n  {t}",
                    n + 1
                );
            }
        }
        // The sweep must be doing work: a rename of `TcpListener::bind` would
        // otherwise report a clean pass over nothing.
        assert!(
            checked >= 1,
            "only {checked} configurable bind(s) found — this guard has gone blind"
        );
    }

    /// The rule itself, on the addresses the second listener accepts.
    #[test]
    fn a_reachable_address_is_not_mistaken_for_a_local_one() {
        for addr in ["0.0.0.0:8919", "192.168.1.10:8919", "[::]:8919"] {
            assert!(
                !listens_on_loopback(addr),
                "{addr} must not read as local-only"
            );
        }
        for addr in ["127.0.0.1:8919", "localhost:8919", "[::1]:8919"] {
            assert!(listens_on_loopback(addr), "{addr} is loopback");
        }
    }
}
