//! What rank 4 should DO in front of each agent — and where it should stay out.
//!
//! The question this module answers is not "can the proxy run", it is "what is
//! MISSING for this agent that only the proxy can supply". They are different
//! questions and the first one is the wrong one: Claude Code is perfectly
//! reachable by a base URL and the proxy must not fold a single byte for it,
//! because its hooks already own every tool result (one authority per payload). Codex is the
//! opposite case — its hook needs an approval click most users never find, so
//! for an un-approved Codex the proxy is the ONLY channel that can report a
//! gain at all.
//!
//! Three measured axes, and the role falls out of them:
//!
//! 1. **Reach** — is there a base URL to change? Measured by reading what the
//!    agent itself ships, never inferred from a neighbour. Windsurf (now
//!    Devin Desktop) is the standing reminder: it was classed by resemblance
//!    with antigravity and the class was wrong for three days.
//! 2. **Wire** — once we are in the path, is the body one of the three we can
//!    rewrite? Being in the path is worth nothing on a proprietary protocol,
//!    and `wire::detect` will correctly refuse to touch it.
//! 3. **Who already carries** — [`tokenade_core::detect::Agent::coverage`].
//!
//! **Every row here was measured on 2026-08-15 against the agent installed on
//! this machine**, by reading the shipped bundle or binary for the environment
//! names and route strings it carries. An agent that is not installed here gets
//! [`Reach::Unmeasured`], and `Unmeasured` never renders as a capability — the
//! empty cell that this whole document prefers to an optimistic one.
//!
//! The limit of that evidence, stated rather than glossed: finding
//! `ANTHROPIC_BASE_URL` inside an agent's bundle proves the name is in its
//! code, not that the code path is reachable in every mode. In particular an
//! agent talking to a SUBSCRIPTION rather than to an API may ignore it, which
//! is a structural limit of rank 4. Confirming a row costs one run of
//! that agent against a proxy, which is what [`super::server`]'s tests do for
//! the transport and what an operator can do per agent in a minute.

use tokenade_core::detect::{Agent, Coverage};

/// How rank 4 can be put in this agent's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// An environment variable the agent's own shipped code carries.
    EnvVar(&'static str),
    /// A key in the agent's own config file. Named in full because "set the
    /// base url" is not actionable and `model_providers.<id>.base_url` is.
    ConfigKey(&'static str),
    // `NoneFound` — "looked for, and this agent has no base URL at all" — was
    // here and is gone: not one of the fourteen agents measured on 2026-08-15
    // is in that state, so the variant was a shape nothing could take. Bring it
    // back the day a row needs it, together with that row; carrying an
    // unconstructible state is how an enum starts describing intentions rather
    // than measurements.
    /// Measured, and there is nothing to change. The string says WHERE the
    /// endpoint is fixed, because "no way in" without the place is a verdict
    /// nobody can re-check.
    NoneFound(&'static str),
    /// Reached through ANOTHER agent's row, because this product launches that
    /// agent's own binary with that agent's own settings loaded.
    ///
    /// Nothing is written for this row, and that is the point: one payload,
    /// one authority. A second file naming a second port would put two proxies
    /// in front of one process and count the same bytes twice.
    Inherited { from: Agent, how: &'static str },

    /// Not installed here, so not measured. Never rendered as a capability.
    Unmeasured,
}

/// Which provider wire the agent speaks on that path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaks {
    /// One of the three [`super::wire::Wire`] shapes we can rewrite.
    RewritableWire,
    /// A protocol of the vendor's own. We can relay it and count nothing but
    /// bytes; `wire::detect` returns `Unknown` and the body is never touched.
    Proprietary(&'static str),
    Unmeasured,
}

/// Does the rung above carry by itself, or only after a step the USER takes?
///
/// This distinction is the whole reason the matrix is not just
/// [`Coverage`]. `coverage()` answers "which channel exists for this agent",
/// which is a property of the product. Whether that channel is LIVE for a given
/// person is a property of what they clicked, and the two have opposite
/// consequences here: a rung that always carries makes folding a double
/// authority, while a rung nobody enabled makes the proxy the only channel
/// there is — including for the gain figure, which is what a user looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HigherRung {
    // `CarriesOnceInstalled` — "`tokenade install` wires it and it carries
    // from then on" — went on 2026-08-20, and it goes for the same reason
    // `Reach::NoneFound` went before it: nothing could construct it any more,
    // and carrying an unconstructible state is how an enum starts describing
    // intentions rather than measurements.
    //
    // It meant "every payload in this agent's history is already folded", and
    // no agent is in that state: a hook folds the tools it is wired for, when
    // it runs, and anything else in the history is nobody's. Selecting it
    // switched rank 4 off for that agent entirely — it did, for eleven of
    // them, until it was measured.
    //
    // Bring it back the day a row can PROVE it, together with that row.
    /// Needs an action inside the agent that nothing here can verify. Until it
    /// happens, nothing folds this agent's tool results and nothing reports its
    /// gain at all.
    NeedsUserConsent {
        what: &'static str,
        /// False when this is REPORTED rather than measured on this machine.
        /// Kept as a field rather than as prose so the claim can be promoted to
        /// measured later without anybody having to notice it was not.
        measured_here: bool,
    },
    /// Nothing above folds tool results for this agent.
    NoneFolds,
}

/// What the proxy is FOR, on this agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    // `OutOfReach` went with `Reach::NoneFound`, which was its only source.
    // `AccountOnlyHigherRungOwns` went with the `HigherRung` variant that was
    // its only source. It said "a higher rung already folds every tool
    // result, so the proxy must NOT fold" — a per-AGENT answer to a question
    // that is decided per PAYLOAD by `authority::may_fold`.
    /// In the path, but on a wire we do not rewrite. Relay and count.
    AccountOnlyForeignWire,
    /// Nothing above carries here. Fold and count — this is the case rank 4
    /// was built for.
    FoldAndAccount,
    /// A rung above EXISTS but only carries once the user has enabled it, and
    /// nothing on this side can check whether they did. Fold, and let the already-folded rule
    /// do the deciding per payload: a result the hook already folded carries
    /// the marker and is left alone, one it did not is ours. Refusing to fold
    /// here would leave the users who never found the approval step with no
    /// compaction AND no gain figure, which is the exact "works but does not
    /// tell you" zero this whole document is about.
    FoldWhatTheHigherRungDidNotTouch,
    /// In no path at all: measured, and the agent fixes its endpoint where
    /// nothing outside the product can reach it.
    OutOfReach,
    /// Covered by another agent's row, which owns the single write.
    CoveredByAnotherAgent,
    /// Not measured. The operator gets told that, not a guess.
    Unmeasured,
}

impl Role {
    /// One line an operator can act on.
    pub fn describe(self) -> &'static str {
        match self {
            Role::AccountOnlyForeignWire => {
                "run it for the cost figures only: this agent speaks its vendor's own \
                 protocol, which is relayed untouched rather than guessed at"
            }
            Role::FoldAndAccount => {
                "run it for both: nothing above carries here, so the proxy folds the \
                 tool results in the history and reports the exact cost"
            }
            Role::FoldWhatTheHigherRungDidNotTouch => {
                "run it: this agent HAS a higher rung, but only for people who enabled it. \
                 The proxy folds what that rung did not touch (the fold marker decides, \
                 payload by payload) and reports the cost for everyone else"
            }
            Role::OutOfReach => {
                "no way in: this agent fixes the endpoint of the process that calls the \
                 model, and nothing outside the product can move it"
            }
            Role::CoveredByAnotherAgent => {
                "nothing to write here: this product launches another agent's binary with \
                 that agent's settings, so the base URL installed there already covers it"
            }
            Role::Unmeasured => {
                "not measured — this agent is not installed on the machine the matrix \
                 was built on, and a guessed row is worse than an empty one"
            }
        }
    }

    /// Whether the proxy may rewrite bodies for this agent.
    ///
    /// The static half of "one authority per payload". The runtime half is the
    /// `X-Tokenade-Channel` header, which only exists when the agent-side
    /// channel sets it — this answers the same question when nobody said
    /// anything.
    pub fn may_fold(self) -> bool {
        matches!(
            self,
            Role::FoldAndAccount | Role::FoldWhatTheHigherRungDidNotTouch
        )
    }

    /// The same question, but asked for a body we know how to WALK.
    ///
    /// `AccountOnlyForeignWire` meant "we cannot read this wire", not "we are
    /// not allowed to touch it". Since the proxy can fold a schema-less
    /// protobuf body, this row is no longer a prohibition: it describes an
    /// IGNORANCE that no longer exists for this format. The two other
    /// refusals — out of reach, and owned by another agent's row — stay
    /// whole, because they are about rights, not capability.
    ///
    /// The same lesson in the other direction: a gate PER AGENT that was
    /// deciding in place of a test PER PAYLOAD.
    pub fn may_fold_a_wire_we_can_walk(self) -> bool {
        self.may_fold() || matches!(self, Role::AccountOnlyForeignWire)
    }
}

/// One measured row.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub reach: Reach,
    pub speaks: Speaks,
    /// Set when this agent's higher rung only carries after the user enabled
    /// it. The only per-agent fact the matrix adds on top of `coverage()`,
    /// which answers what the PRODUCT offers rather than what a person turned
    /// on.
    pub consent: Option<Consent>,
    /// What the measurement actually saw, so a later reader can re-derive the
    /// row instead of trusting it.
    pub evidence: &'static str,
}

/// A step the user has to take before the rung above does anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent {
    pub what: &'static str,
    /// False when the requirement is REPORTED rather than measured here. Kept
    /// as data so it can be promoted without anybody having to remember it was
    /// ever an assumption.
    pub measured_here: bool,
}

impl Entry {
    /// Which rung above this one actually folds tool results.
    pub fn higher_rung(&self, coverage: Coverage) -> HigherRung {
        if let Some(c) = self.consent {
            return HigherRung::NeedsUserConsent {
                what: c.what,
                measured_here: c.measured_here,
            };
        }
        match coverage {
            // A hook or a plugin folds tool RESULTS — the ones it is wired for,
            // once it actually runs. Neither half of that is "every payload in
            // the history", and treating it as such switched the proxy OFF on
            // eleven of the twenty-four agents.
            //
            // MEASURED 2026-08-20, same request through the same proxy, only
            // `--agent` changed: `claude-code` relayed 142 395 bytes untouched
            // where `codex` relayed 62 445 — a 56% fold this rung refused to
            // perform, on a history whose `WebFetch` result no hook had
            // touched. `web` reads `Partial bin` on claude-code, grok and
            // droid for exactly that payload, so the rung above demonstrably
            // did NOT own it.
            //
            // The reason this was safe to change is that the guard which
            // justified the coarse rule is per-PAYLOAD and already works:
            // `authority::may_fold` asks `fold_marker::already_folded`, and in
            // the same measured request the hook-folded Bash result came out
            // the far side with its marker intact at 128 bytes while the
            // unfolded WebFetch result was folded to 49 831. Rule 2 decides
            // per block; this function only ever decided per agent, and the
            // two answers are not the same question.
            //
            // `CarriesOnceInstalled` is kept as a variant rather than deleted:
            // it is what an agent whose rung above owns EVERY payload would
            // be, and nothing measured is in that state today. Bring it back
            // with a row that proves it.
            Coverage::Hook(_) | Coverage::Plugin => HigherRung::NeedsUserConsent {
                what: "the hook or plugin has to be present AND allowed to run for \
                       this agent; nothing on this side can check that it fired, so \
                       the proxy folds what carries no fold marker and leaves what does",
                measured_here: true,
            },
            _ => HigherRung::NoneFolds,
        }
    }

    /// The role, derived rather than declared — so a row cannot claim a
    /// capability its own measurements do not support.
    pub fn role(&self, coverage: Coverage) -> Role {
        match (self.reach, self.speaks) {
            // Reach decides FIRST for these two: a row with nowhere to write
            // cannot become a capability because its wire happens to be one we
            // could rewrite, and an inherited row must not claim a second one.
            (Reach::NoneFound(_), _) => Role::OutOfReach,
            (Reach::Inherited { .. }, _) => Role::CoveredByAnotherAgent,
            (Reach::Unmeasured, _) | (_, Speaks::Unmeasured) => Role::Unmeasured,
            (_, Speaks::Proprietary(_)) => Role::AccountOnlyForeignWire,
            (_, Speaks::RewritableWire) => match self.higher_rung(coverage) {
                HigherRung::NeedsUserConsent { .. } => Role::FoldWhatTheHigherRungDidNotTouch,
                HigherRung::NoneFolds => Role::FoldAndAccount,
            },
        }
    }
}

/// The measured matrix.
///
/// Ordered like [`Agent`] so the exhaustiveness guard below reads as a diff
/// against the enum rather than as a lookup.
pub fn entry(agent: Agent) -> Entry {
    // The `env(…)` shorthand that used to live here is gone, and its absence
    // is a measurement: every agent it served turned out to have a FILE once
    // somebody ran the agent instead of counting strings in its bundle. The
    // one row still reached by environment alone (cursor) declares it inline,
    // where the evidence for it sits.
    match agent {
        // Hooks own every tool result here. Reachable, and deliberately not
        // folded — this is the agent the user's question is about.
        Agent::ClaudeCode => Entry {
            // The env NAME is what it reads, and `~/.claude/settings.json`
            // carries an `env` block that sets it — a FILE, so this is inside
            // the config-only constraint rather than a way around it.
            reach: Reach::ConfigKey("env.ANTHROPIC_BASE_URL in ~/.claude/settings.json"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "the shipped 2.1.233 bundle carries ANTHROPIC_BASE_URL and 54 \
                       `v1/messages` references; `env` is already in use in the settings file \
                       on this machine. Hooks already fold, so rank 4 is for the numbers only",
        },
        // Reachable, but the path leads to Cursor's OWN backend: zero
        // references to any of the four provider routes in the 2026.08.11
        // build. Relayed untouched, and that is the correct behaviour rather
        // than a gap.
        Agent::Cursor => Entry {
            // Corrected 2026-08-19 by installing the agent and running it.
            // The variable is `CURSOR_API_ENDPOINT`, declared by the CLI's own
            // `--endpoint` option with `.env("CURSOR_API_ENDPOINT")` and the
            // default `https://api2.cursor.sh`; `CURSOR_API_BASE_URL` appears
            // nowhere in the 2026.08.11 build. Pointed at a listener on this
            // machine it sent `POST /auth/exchange_user_api_key` and `POST
            // /aiserver.v1.DashboardService/GetMe`.
            //
            // There is no config file behind it: `cli-config.json` is read for
            // permissions, model and display, and carries no endpoint key. So
            // this row is reachable and NOT installable, which are two
            // different facts and used to print as one.
            //
            // Re-tested 2026-08-20 the other way round, because "the string is
            // absent from the bundle" is weaker evidence than a refusal. A
            // project `.cursor/cli.json` carrying `{"endpoint": …,
            // "apiUrl": …}` — the two names its own `-e` option and its
            // default use — makes the CLI exit 1 with `Unrecognized key(s) in
            // object: 'endpoint', 'apiUrl'`. Its config schema REFUSES them, so
            // the env var is not merely the route we found, it is the only one
            // there is.
            reach: Reach::EnvVar("CURSOR_API_ENDPOINT"),
            speaks: Speaks::Proprietary("Cursor's own aiserver.v1 over Connect"),
            consent: None,
            evidence: "traffic reached a local listener named by CURSOR_API_ENDPOINT, \
                       2026-08-19, on `aiserver.v1.*`; `v1/messages`, `chat/completions`, \
                       `v1/responses` and `:generateContent` are ALL absent from the build",
        },
        // The case that motivates the rung. Its hook needs an approval click,
        // and without it nothing reports a gain at all. Note the reach is a
        // CONFIG KEY, not an env var: the bundle carries no OPENAI_BASE_URL.
        Agent::Codex => Entry {
            reach: Reach::ConfigKey("model_providers.<id>.base_url in ~/.codex/config.toml"),
            speaks: Speaks::RewritableWire,
            // The fact that makes Codex the case this rung exists for. Its hook
            // does not run until the user approves it, and until then NOTHING
            // in the product folds its tool results or reports its gain — the
            // user sees a zero and cannot tell it from a broken install.
            //
            // MEASURED 2026-08-15, after being reported: `~/.codex/config.toml`
            // on this machine carries `hooks.state.<file>:<event>:0:0` entries
            // holding `enabled = true` AND a `trusted_hash = "sha256:…"`. A
            // per-hook trust hash recorded in the agent's own config is an
            // approval gate: an entry without one is not trusted, so it does
            // not run. The consent is in the config, not in a claim.
            consent: Some(Consent {
                what: "each hook carries a trusted_hash in ~/.codex/config.toml \
                       and does not run until the user approves it",
                measured_here: true,
            }),
            evidence: "no OPENAI_BASE_URL in the bundle; `base_url` appears as a config key and \
                       the one route string is `v1/responses` — the Responses wire",
        },
        // Same shape, same reason: `~/.copilot/settings.json` carries the
        // `env` block, and copilot re-serialises the file with the key intact.
        Agent::Copilot => Entry {
            reach: Reach::ConfigKey("env.COPILOT_PROVIDER_BASE_URL in ~/.copilot/settings.json"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "COPILOT_PROVIDER_BASE_URL, OPENAI_BASE_URL and ANTHROPIC_BASE_URL all \
                       present; 36 `chat/completions` and 32 `v1/messages` references; the \
                       settings file is read back re-serialised with the key kept",
        },
        // Reachable by environment too; the CONFIG route is the one we use and
        // therefore the one recorded. Measured 2026-08-16: both configs on this
        // machine declare `"$schema": "https://opencode.ai/config.json"` (kilo
        // is an opencode fork), and both bundles carry `options:{baseURL`.
        Agent::KiloCode => Entry {
            reach: Reach::ConfigKey("provider.<id>.options.baseURL in ~/.config/kilo/kilo.json"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "OPENAI_BASE_URL and KILO_API_URL present; 618 `chat/completions` \
                       references; the opencode config schema, verified live against the \
                       real binary",
        },
        Agent::OpenCode => Entry {
            reach: Reach::ConfigKey(
                "provider.<id>.options.baseURL in ~/.config/opencode/opencode.json",
            ),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "69 `chat/completions` references; `options:{baseURL` in the bundle and \
                       the opencode schema in the shipped config, verified live",
        },
        // Promoted from an env-var cell 2026-08-19, by reading the config the
        // hermes on this machine actually carries rather than counting strings
        // in its venv: line 4 of `~/.hermes/config.yaml` is
        // `  base_url: https://openrouter.ai/api/v1`, under a top-level
        // `model:` key, and `hermes config set` is its own writer for it.
        Agent::Hermes => Entry {
            reach: Reach::ConfigKey("model.base_url in ~/.hermes/config.yaml"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "the key is in the live config in plain sight; ANTHROPIC_BASE_URL is \
                       present in the venv too, and 286 `chat/completions`, 153 \
                       `v1/responses` and 87 `v1/messages` references",
        },
        // Promoted from an env-var cell 2026-08-17. The config key was found
        // by RUNNING the agent's own writer rather than reading its reader:
        // `cline auth --data-dir <tmp> --baseurl <url>` writes
        // `settings/providers.json`, which answers the key path and the file
        // location at once and cannot be wrong about the shape.
        Agent::Cline => Entry {
            reach: Reach::ConfigKey(
                "providers.<lastUsedProvider>.settings.baseUrl in \
                 ~/.cline/settings/providers.json",
            ),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "OPENAI_BASE_URL, ANTHROPIC_BASE_URL and CLINE_API_BASE_URL present; 39 \
                       `chat/completions` references; the config key produced by the agent's \
                       own `auth` writer and verified live against the real binary",
        },
        // Reachable by environment too, but the CONFIG route is the one that
        // matters: writing an env var would need a shell rc file, ruled out by
        // the operator. Measured in `aider/args.py` — `.aider.conf.yml` is
        // searched in the git root, the cwd and the home directory, and every
        // long flag doubles as a key.
        Agent::Aider => Entry {
            reach: Reach::ConfigKey("openai-api-base in ~/.aider.conf.yml"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "litellm underneath, so every provider's base is settable; 481 \
                       `chat/completions` references; `--openai-api-base` is a declared flag \
                       and `.aider.conf.yml` is its config file",
        },
        // Reachable by environment AND by file, and the FILE is what this
        // product uses: openclaw's own `config set` accepts a top-level `env`
        // block, which is where the installer writes. Recorded as a config key
        // for that reason — the row must name the route the installer takes,
        // or the guard tying the two together is comparing different things.
        Agent::OpenClaw => Entry {
            reach: Reach::ConfigKey("env.OPENAI_BASE_URL in ~/.openclaw/openclaw.json"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "OPENAI_BASE_URL, ANTHROPIC_BASE_URL and a dozen per-provider bases \
                       present; 191 `chat/completions` references; the `env` block is accepted \
                       by openclaw's own writer against its CLOSED schema",
        },
        // Reachable BY FILE, and the wire is Google's own, which is NOT one of
        // the three this proxy rewrites. Relayed untouched.
        //
        Agent::Qwen => Entry {
            reach: Reach::ConfigKey("env.OPENAI_BASE_URL in ~/.qwen/settings.json"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "`settings.env` ×40 and OPENAI_BASE_URL ×21 in its own bundle — measured \
                       there rather than inherited from Claude's identical mechanism; 16 \
                       `chat/completions` references",
        },
        // Promoted from an env-var cell 2026-08-17, on the agent's OWN shipped
        // documentation rather than on a string in a bundle: `~/.grok/docs/
        // user-guide/11-custom-models.md` declares `[endpoints]
        // models_base_url` and says grok inherits it into every `[model.*]`
        // section that does not override it. One key redirects the agent.
        Agent::Grok => Entry {
            reach: Reach::ConfigKey("[endpoints] models_base_url in ~/.grok/config.toml"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "XAI_API_BASE_URL and GROK_CLI_CHAT_PROXY_BASE_URL present; 5 \
                       `chat/completions` references; the config key is documented by grok \
                       itself and verified live against the real binary",
        },
        // Promoted from an env-var cell 2026-08-19 by running the agent, not
        // by reading it. `~/.factory/config.json` carries `custom_models[]`,
        // and droid's own filter REQUIRES `base_url` of an entry — so it is
        // the field it dials. Pointed at a listener here, droid sent it `POST
        // /v1/chat/completions`.
        //
        // What this recipe does NOT cover, and the tests say so: droid's own
        // Factory-hosted models. Those go to `https://api.factory.ai` and are
        // redirected only by `FACTORY_API_BASE_URL`, an environment variable
        // with no file behind it — `~/.factory/.env` was written and IGNORED
        // in the same session. Worth knowing what is on the other side of that
        // variable, because it is not proprietary: with it set, droid sent
        // `POST /api/llm/a/v1/messages`, 44 KB, the Anthropic wire under a
        // Factory path prefix.
        Agent::Droid => Entry {
            reach: Reach::ConfigKey("custom_models[].base_url in ~/.factory/config.json"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "traffic reached a local listener named by a custom_models entry, \
                       2026-08-19, on `/v1/chat/completions`; FACTORY_API_BASE_URL redirects \
                       the Factory-hosted models too but has no config file behind it",
        },
        // Devin Desktop — Windsurf's fork, installed here — measured 2026-08-18
        // (the row was Windsurf's until the rename was modelled as its own agent).
        //
        // Reachable BY CONFIG, which is the rare half: the extension declares
        // `Config.INFERENCE_API_SERVER_URL = "codeium.inferenceApiServerUrl"`
        // with the default `https://inference.codeium.com`, reads it through
        // `getConfig` at exactly the point where it resolves and spawns
        // `language_server_linux_x64`, and writes settings globally through
        // `workspace.getConfiguration(...).update(..., true)` — i.e. into
        // `~/.config/Devin/User/settings.json`, a file.
        //
        // And unreachable by WIRE, which is what decides. The whole
        // conversation is Connect/gRPC-web carrying its own protobuf
        // (`exa.language_server_pb` ×4152, connect/grpc-web ×43212, against
        // three stray `chat/completions` in bundled SDKs). None of the wires
        // this proxy understands, so a body is relayed untouched — and the
        // COST FIGURES do not survive either: `usage` is read out of JSON, and
        // there is no JSON here.
        Agent::DevinDesktop => Entry {
            reach: Reach::ConfigKey(
                "codeium.inferenceApiServerUrl in ~/.config/Devin/User/settings.json",
            ),
            speaks: Speaks::Proprietary("Codeium's Connect/protobuf language-server API"),
            consent: None,
            evidence: "Devin Desktop (ex-Windsurf): the setting is read where the language \
                       server is spawned, default `https://inference.codeium.com`; the wire \
                       is `exa.language_server_pb` over Connect — no JSON, so not even the \
                       usage numbers are readable yet",
        },
        // Not installed on the machine the matrix was measured on. An empty
        // cell beats an optimistic one, and this is the same rule that keeps
        // `nu`, `pwsh` and `tcsh` out of rank 2's claim.
        // Measured 2026-08-19: `~/.pi/agent/models-store.json` keeps a
        // `baseUrl` on every model, and `settings.json` next to it names the
        // provider pi dials. Pointed at a listener here, pi sent it `POST
        // /v1/messages` — the Anthropic wire, which this proxy rewrites.
        Agent::Pi => Entry {
            reach: Reach::ConfigKey("<provider>.models[].baseUrl in ~/.pi/agent/models-store.json"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "traffic reached a local listener named by the model store, 2026-08-19, \
                       on `/v1/messages`; the store ships two providers and both carry the key",
        },
        // MEASURED 2026-08-19 on the shipped bundles, which is the same
        // standard the Copilot CLI row above was declared at.
        //
        // Two names, not one, and the current one is undeclared. The built-in
        // `GitHub.copilot-chat` 0.62.0 inside VS Code 1.134 carries the pair
        // `DebugOverrideCapiUrl: "internal.capiUrl"` and
        // `DebugOverrideCapiUrlLegacy: "advanced.debug.overrideCapiUrl"` — so
        // the settings key it reads FIRST appears in no `package.json` and
        // could not have been found by reading the declared settings. The
        // marketplace `GitHub.copilot` 1.388.0 declares the legacy name with
        // the description "Override GitHub Copilot API full URL". Both are
        // written; the current one wins where it exists and the legacy one
        // carries older installs.
        //
        // `debug.overrideProxyUrl` is deliberately NOT written: its own
        // description says "GitHub authentication proxy", so pointing it here
        // would break sign-in rather than route a model call.
        //
        // The wire is OpenAI-shaped: 20 `chat/completions` references in the
        // marketplace bundle, `api.githubcopilot.com` in both.
        //
        // What is NOT measured, and the reason: traffic on a local listener.
        // Copilot calls CAPI only with an entitled account, and there is none
        // on this machine. Launched under xvfb with both keys pointed at
        // 127.0.0.1:8799, VS Code 1.134 made ZERO connections in 150 s. That
        // is the absence of a subscription, not a refusal of the setting, and
        // the row says so rather than claiming a measurement it does not have.
        Agent::CopilotVsCode => Entry {
            reach: Reach::ConfigKey("github.copilot.internal.capiUrl in VS Code's settings.json"),
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "`internal.capiUrl` and the legacy `advanced.debug.overrideCapiUrl` are \
                       both read by the built-in copilot-chat 0.62.0; 20 `chat/completions` \
                       references; no traffic test — Copilot needs an entitled account and \
                       this machine has none",
        },
        // An ORCHESTRATOR, and the reason this variant exists. Measured
        // 2026-08-20 in the shipped `app.asar`: it embeds the Claude Agent SDK
        // (`claude-agent-sdk` ×22), spawns the real binary through
        // `pathToClaudeCodeExecutable`, and passes
        // `CLAUDE_SETTING_SOURCES = ["user","project","local"]`.
        //
        // Confirmed by running that exact invocation rather than by reading it:
        // `claude --setting-sources user,project,local -p` with
        // `env.ANTHROPIC_BASE_URL` set ONLY in `~/.claude/settings.json` sent
        // `POST /v1/messages?beta=true` to a listener on this machine.
        //
        // So the write that covers this agent is claude-code's, and there must
        // not be a second one.
        Agent::T3Code => Entry {
            reach: Reach::Inherited {
                from: Agent::ClaudeCode,
                how: "it spawns the real `claude` binary with settingSources user/project/local, \
                      so ~/.claude/settings.json is the file that redirects it",
            },
            speaks: Speaks::RewritableWire,
            consent: None,
            evidence: "`claude --setting-sources user,project,local -p` reached a local listener \
                       named only by env.ANTHROPIC_BASE_URL in ~/.claude/settings.json, \
                       2026-08-20; t3code passes exactly those sources to the SDK",
        },
        // Measured 2026-08-20 on the installed IDE, and the answer is no.
        // `language_server --help` declares `-api_server_url` and
        // `-cloud_code_endpoint` ("CCPA API URL"), so the endpoint IS a flag —
        // and the only spawn site in the shipped `app.asar` passes both as
        // string literals. `CODEIUM_LANGUAGE_SERVER_BIN`, the one env name that
        // path reads, is guarded by `app.isPackaged` and dead in a release
        // build. `CLOUD_CODE_URL` and `API_SERVER_URL` in the environment moved
        // nothing to a listener on this machine.
        Agent::Antigravity => Entry {
            reach: Reach::NoneFound(
                "the two endpoint flags are passed as literals at the only \
                 `language_server` spawn in resources/app.asar",
            ),
            speaks: Speaks::Unmeasured,
            consent: None,
            evidence: "`--api_server_url https://generativelanguage.googleapis.com` and \
                       `--cloud_code_endpoint https://daily-cloudcode-pa.googleapis.com` are \
                       hardcoded at the spawn; no config key and no env override reached a \
                       local listener, 2026-08-20. Re-checked 2026-08-22 on the unpacked \
                       asar: still literals, no `*config.json` carries an endpoint, and the \
                       19 vendor env names are telemetry. The ONE door left is a FORWARD \
                       proxy — the app passes the shell env through (`shellEnvSync()`) and \
                       the Go server links `net/http/httpproxy` + `ProxyFromEnvironment` and \
                       honours `SSL_CERT_FILE`, so `HTTPS_PROXY` would put us in the path. \
                       Not taken: folding the body then means terminating TLS, which is a \
                       different capability from rewriting a base URL the agent published, \
                       and it is the operator's call",
        },
        Agent::Odysseus | Agent::Unknown => Entry {
            reach: Reach::Unmeasured,
            speaks: Speaks::Unmeasured,
            consent: None,
            evidence: "not installed on the machine this matrix was measured on (2026-08-15)",
        },
    }
}

/// Every agent, in enum order, for the guard and for rendering.
const ALL: &[Agent] = &[
    Agent::ClaudeCode,
    Agent::Cursor,
    Agent::Codex,
    Agent::Copilot,
    Agent::KiloCode,
    Agent::DevinDesktop,
    Agent::OpenCode,
    Agent::Hermes,
    Agent::Cline,
    Agent::Aider,
    Agent::Pi,
    Agent::OpenClaw,
    Agent::Qwen,
    Agent::Antigravity,
    Agent::Grok,
    Agent::T3Code,
    Agent::Droid,
    Agent::CopilotVsCode,
    Agent::Odysseus,
    Agent::Unknown,
];

/// Resolve an agent from the name the product itself prints.
///
/// Matched against `Agent::as_str()` over [`ALL`] rather than a private list,
/// so `--agent claude-code` means here exactly what it means on the dashboard
/// and in `detect`. A second table would be a second thing to keep in step, and
/// this document is a catalogue of twins that drifted.
pub fn by_name(name: &str) -> Option<Agent> {
    let want = name.trim().to_ascii_lowercase();
    // `windsurf` is the row's former name: a record or a launcher from
    // before the split finds the Devin Desktop row again.
    let want = match want.as_str() {
        "windsurf" | "devin" => "devin-desktop".to_string(),
        _ => want,
    };
    ALL.iter()
        .copied()
        .find(|a| a.as_str().eq_ignore_ascii_case(&want) && *a != Agent::Unknown)
}

/// Rank-4 names that are **not** [`Agent`] rows: the product is known, and
/// the shipped app has no base-URL door. Distinct from `unknown agent`,
/// which means we never heard of the name.
///
/// Three measured cases:
///
/// * Claude Desktop — `CLAUDE_AI_URL` exists in the bundle behind
///   `buildType==dev`, so the distributed build cannot be pointed at us.
///   Rank 3 is `tokenade mcp-wrap` on `claude_desktop_config.json`.
/// * Claude Cowork — a *mode* of Desktop, not a separate app. Anthropic's
///   doc: the session runs on Anthropic's servers; local MCP
///   (`claude_desktop_config.json`) is not available. No local model call,
///   so no rank 4; no local MCP launch, so no rank 3 either.
/// * Codex app (ChatGPT.app) — hosts are compiled literals
///   (`chatgpt.com`, `api.openai.com`); zero `OPENAI_BASE_URL` /
///   `overrideBaseUrl` in the binary. Rank 3 on Codex CLI's
///   `~/.codex/config.toml` is a different product.
///
/// Inventing `Agent::ClaudeDesktop` / `ClaudeCowork` / `CodexApp` or a
/// shell hook would claim a channel the host does not expose.
pub fn rank4_refusal(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "claude-desktop" => Some(
            "claude-desktop has no rank-4 door: the shipped app pins claude.ai \
             (CLAUDE_AI_URL is behind buildType==dev). \
             Rank 3 is `tokenade mcp-wrap` on claude_desktop_config.json — \
             `llm-proxy install` cannot invent a hook this host does not expose",
        ),
        "claude-cowork" | "cowork" => Some(
            "claude-cowork has no rank-4 door: it is a Desktop mode whose \
             session runs on Anthropic's servers. \
             Local MCP (`claude_desktop_config.json`) is not available in \
             Cowork, so rank 3 has nothing to wrap either — \
             `llm-proxy install` cannot invent a hook this host does not expose",
        ),
        "codex-app" | "chatgpt-app" => Some(
            "codex-app has no rank-4 door: ChatGPT.app pins chatgpt.com / \
             api.openai.com as compiled literals (no OPENAI_BASE_URL). \
             Rank 3 on ~/.codex/config.toml is Codex \
             CLI, a different product — `llm-proxy install` cannot rewrite \
             this binary",
        ),
        _ => None,
    }
}

/// Render the matrix, one line per agent.
///
/// Written for the operator asking "should I put the proxy in front of this?",
/// which is a different question from "is this agent covered" and had no answer
/// anywhere in the product.
pub fn render() -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for &a in ALL {
        let e = entry(a);
        let role = e.role(a.coverage());
        let _ = writeln!(s, "{:<12} {}", a.as_str(), role.describe());
        // The evidence, not just the verdict. A matrix that shows only its
        // conclusions is one nobody can re-derive, and re-deriving is the only
        // way a wrong row ever gets found.
        let _ = writeln!(s, "{:<12}   measured: {}", "", e.evidence);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_a_row() {
        // The guard that stops a new agent being added without a measurement.
        // Its sibling on rank 1 refuses a tenth hook agent without a test, for
        // the same reason: the gap this product keeps finding is not a wrong
        // row, it is a missing one that nobody notices because the table still
        // renders.
        for &a in ALL {
            let e = entry(a);
            assert!(
                !e.evidence.is_empty(),
                "{} has no evidence line",
                a.as_str()
            );
        }
        assert_eq!(
            ALL.len(),
            20,
            "an Agent variant was added without a row here"
        );
    }

    #[test]
    fn claude_code_is_reachable_and_must_not_be_folded_for() {
        // The user's question, asserted: the proxy CAN sit in front of Claude
        // Code and must not rewrite a byte, because its hooks already own every
        // tool result. Reachability and permission are two different answers
        // and conflating them is how a payload gets two authorities.
        let e = entry(Agent::ClaudeCode);
        // The reach became a CONFIG key on 2026-08-16 — `env.ANTHROPIC_BASE_URL`
        // in `~/.claude/settings.json`, a file rather than a shell rc — and the
        // PERMISSION did not move an inch. That is the point of the test: the
        // two answers are independent, and this agent is now installable and
        // still must not be folded for.
        assert_eq!(
            e.reach,
            Reach::ConfigKey("env.ANTHROPIC_BASE_URL in ~/.claude/settings.json")
        );
        assert!(super::super::install::recipe(Agent::ClaudeCode).is_some());
        assert_eq!(e.speaks, Speaks::RewritableWire);
        let role = e.role(Agent::ClaudeCode.coverage());
        // Was `AccountOnlyHigherRungOwns` — "reachable and must not be folded
        // for" — until 2026-08-20, on the theory that its hooks own every tool
        // result. Measured, they do not: the same history through the same
        // proxy came out 142 395 bytes on this agent and 62 445 on codex, and
        // the 80 kB difference was a `WebFetch` result no hook had touched.
        // The cell that reads `Partial bin` on this agent is that payload.
        assert_eq!(role, Role::FoldWhatTheHigherRungDidNotTouch);
        assert!(role.may_fold());
    }

    #[test]
    fn codex_is_the_case_the_rung_exists_for() {
        // Its hook needs an approval click most users never find. Without it,
        // nothing in the product can report this agent's gain — the proxy can.
        let e = entry(Agent::Codex);
        assert!(matches!(e.reach, Reach::ConfigKey(_)));
        assert_eq!(e.speaks, Speaks::RewritableWire);
        // And the reach is a config key, NOT an env var: the bundle carries no
        // OPENAI_BASE_URL, so telling a Codex user to export one would be an
        // instruction that silently does nothing.
        assert!(
            !matches!(e.reach, Reach::EnvVar(_)),
            "codex takes its base URL from config.toml, not from the environment"
        );

        // And the role is NOT "a higher rung owns this", even though
        // `coverage()` says hooks exist: they only run once approved, and
        // treating an offered channel as a live one would leave every
        // un-approved Codex user with no compaction and no gain figure.
        let role = e.role(Agent::Codex.coverage());
        assert_eq!(role, Role::FoldWhatTheHigherRungDidNotTouch);
        assert!(role.may_fold());
        // Promoted from reported to MEASURED on 2026-08-15: `~/.codex/config.toml`
        // carries a `trusted_hash` per hook entry, which is an approval gate
        // recorded in the agent's own config. The field is data precisely so
        // this promotion did not depend on anybody remembering it was pending.
        assert!(matches!(
            e.higher_rung(Agent::Codex.coverage()),
            HigherRung::NeedsUserConsent {
                measured_here: true,
                ..
            }
        ));
    }

    #[test]
    fn no_agent_claims_a_rung_above_that_owns_every_payload() {
        // The claim `CarriesOnceInstalled` used to make — "the rung above owns
        // EVERY payload here" — is gone from the enum, so the way to assert
        // nobody makes it is to assert what it used to produce: an agent that
        // may not be folded for, on a wire we CAN read and a row we DID
        // measure. That combination is now unreachable, and this says so.
        for &a in ALL {
            let e = entry(a);
            let readable = matches!(e.speaks, Speaks::RewritableWire);
            let measured = !matches!(e.reach, Reach::Unmeasured);
            // One exception, and it is the opposite of the gate this guards
            // against: an inherited row must NOT fold, because the row it
            // defers to does, and folding on both would put two authorities on
            // one payload. Named by its variant rather than by its agent, so a
            // second orchestrator does not have to be remembered here.
            let inherited = matches!(e.reach, Reach::Inherited { .. });
            if readable && measured && !inherited {
                assert!(
                    e.role(a.coverage()).may_fold(),
                    "{} is measured, on a wire we rewrite, and still not folded for — \
                     the per-agent gate is back",
                    a.as_str()
                );
            }
        }
    }

    #[test]
    fn a_proprietary_wire_is_relayed_and_only_counted() {
        // Being in the path is worth nothing if the body is not one we can
        // read. Cursor talks to its own backend; `wire::detect` answers
        // `Unknown` and the body goes through untouched, which is the
        // contract, not a gap.
        {
            let a = Agent::Cursor;
            let e = entry(a);
            assert!(matches!(e.speaks, Speaks::Proprietary(_)), "{}", a.as_str());
            let role = e.role(a.coverage());
            assert_eq!(role, Role::AccountOnlyForeignWire);
            assert!(!role.may_fold());
        }
    }

    #[test]
    fn an_unmeasured_agent_never_renders_as_a_capability() {
        // The rule the whole document rests on: an empty cell beats an
        // optimistic one. A row we did not measure must say so and must not
        // fall through to the permissive answer.
        // Pi left this list on 2026-08-19 — it was measured by running it.
        // The list is the agents still genuinely unmeasured, and it must never
        // be emptied to make the guard pass.
        {
            let a = Agent::Odysseus;
            let e = entry(a);
            assert_eq!(e.reach, Reach::Unmeasured);
            let role = e.role(a.coverage());
            assert_eq!(role, Role::Unmeasured);
            assert!(!role.may_fold(), "{} must not be folded for", a.as_str());
            assert!(role.describe().contains("not measured"));
        }
    }

    #[test]
    fn a_measured_row_with_nowhere_to_write_is_still_not_a_capability() {
        // The two ways a row can be MEASURED and still not installable. Both
        // must stay out of the folding list, for different reasons: one has no
        // path at all, and the other has a path another row already owns —
        // folding on both would be two authorities on one payload.
        //
        // Each variant is also asserted to have at least one user. `NoneFound`
        // was deleted once for being unconstructible, and a variant nothing
        // builds is a claim the compiler cannot check.
        let mut out_of_reach = 0;
        let mut inherited = 0;
        for a in ALL {
            let e = entry(*a);
            let role = e.role(a.coverage());
            match e.reach {
                Reach::NoneFound(where_) => {
                    out_of_reach += 1;
                    assert_eq!(role, Role::OutOfReach, "{}", a.as_str());
                    assert!(!role.may_fold(), "{}", a.as_str());
                    assert!(!where_.is_empty(), "{} must say WHERE", a.as_str());
                }
                Reach::Inherited { from, .. } => {
                    inherited += 1;
                    assert_eq!(role, Role::CoveredByAnotherAgent, "{}", a.as_str());
                    assert!(!role.may_fold(), "{}", a.as_str());
                    // The row it defers to has to be one somebody can install,
                    // or the inheritance points at nothing.
                    assert!(
                        matches!(entry(from).reach, Reach::ConfigKey(_) | Reach::EnvVar(_)),
                        "{} inherits from {}, which is not installable itself",
                        a.as_str(),
                        from.as_str()
                    );
                }
                _ => {}
            }
        }
        assert!(out_of_reach > 0 && inherited > 0);
    }

    #[test]
    fn folding_is_offered_only_where_nothing_above_carries() {
        // The positive half: if this ever returns nothing, the rung is dead
        // code and the matrix has quietly switched it off.
        let folding: Vec<&str> = ALL
            .iter()
            .filter(|a| entry(**a).role(a.coverage()).may_fold())
            .map(|a| a.as_str())
            .collect();
        assert!(
            !folding.is_empty(),
            "no agent is left for rank 4 to fold for — the rung would be decoration"
        );
        // The NEGATIVE half moved on 2026-08-20. It used to name Claude Code
        // as an agent the proxy must not fold for; that was the coarse rule,
        // and it cost fifteen measurable cells. What must stay excluded is the
        // set the proxy genuinely cannot serve: a wire we do not parse, and a
        // row nobody measured. Those are properties of the BODY and of the
        // EVIDENCE, not of whether the agent happens to own a hook.
        for &a in ALL {
            let e = entry(a);
            let may = e.role(a.coverage()).may_fold();
            if matches!(e.speaks, Speaks::Proprietary(_) | Speaks::Unmeasured)
                || matches!(e.reach, Reach::Unmeasured)
            {
                assert!(
                    !may,
                    "{} is folded for on a wire or a row we cannot stand behind",
                    a.as_str()
                );
            }
        }
        // And the guard that makes folding on a hooked agent safe is not
        // optional: rule 2 must leave a marked payload alone.
        assert!(!super::super::authority::may_fold(
            "out\n[tokenade: 34789 tokens folded away — full output: \
             `tokenade expand-ref 8cbb0d6ffff8`]"
        ));
        assert!(super::super::authority::may_fold("a plain tool result"));
    }

    /// Print the matrix. Not a check — an inspection tool, kept in the tree so
    /// the answer to "which agents does the proxy actually serve" comes from
    /// the code rather than from a note somebody wrote down once.
    ///
    /// `cargo test -p tokenade-cli --bin tokenade print_the_matrix -- --ignored --nocapture`
    #[test]
    #[ignore = "inspection, not a check: prints the matrix"]
    fn print_the_matrix() {
        for &a in ALL {
            let e = entry(a);
            println!(
                "{:<12} {:<32?} {:?}",
                a.as_str(),
                e.role(a.coverage()),
                e.reach
            );
        }
    }

    #[test]
    fn every_installable_agent_is_reachable_by_config_and_not_by_environment() {
        // The operator's constraint, tied to the matrix so the two do not
        // drift: an agent we ship an installer for must be reachable through
        // a FILE that can be backed up and restored.
        //
        // It did not disappear on 2026-08-21, it got sharper. The operator
        // allowed the environment variable, and a shell profile is a file:
        // backed up like the others, written between markers, removed to the
        // byte. What stays excluded is what is not a file — mutating the
        // process environment, or the Windows registry.
        for &a in ALL {
            if super::super::install::recipe(a).is_none() {
                continue;
            }
            assert!(
                matches!(entry(a).reach, Reach::ConfigKey(_) | Reach::EnvVar(_)),
                "{} has an installer but is reachable by neither a config key nor a variable",
                a.as_str()
            );
            // NOT `may_fold`. Installing in front of an agent whose tool
            // results a higher rung already owns is legitimate and useful: the
            // proxy still reads the provider's own usage, which is the only
            // exact cost figure in the product. What must never happen is an
            // installer for an agent we never measured.
            assert_ne!(
                entry(a).role(a.coverage()),
                Role::Unmeasured,
                "{} has an installer and no measured role",
                a.as_str()
            );
        }
    }

    #[test]
    fn an_agent_name_round_trips_through_the_products_own_table() {
        for &a in ALL {
            if a == Agent::Unknown {
                continue;
            }
            assert_eq!(by_name(a.as_str()), Some(a), "{}", a.as_str());
        }
        assert_eq!(by_name("CLAUDE-CODE"), Some(Agent::ClaudeCode));
        assert_eq!(by_name(" codex "), Some(Agent::Codex));
        assert_eq!(by_name("not-an-agent"), None);
        // `unknown` is a detection outcome, not something a user can name.
        assert_eq!(by_name("unknown"), None);
        // Claude Desktop is a product mcp-wrap already names — not an
        // `Agent` row, and not a rank-4 install. `by_name` stays None so
        // we never write a base URL; `rank4_refusal` is the classified
        // answer the CLI prints instead of `unknown agent`.
        assert_eq!(by_name("claude-desktop"), None);
        let why = rank4_refusal("claude-desktop").expect("classified");
        assert!(why.contains("mcp-wrap"), "{why}");
        assert_eq!(by_name("claude-cowork"), None);
        assert_eq!(by_name("codex-app"), None);
        let cowork = rank4_refusal("claude-cowork").expect("classified");
        assert!(cowork.contains("Anthropic"), "{cowork}");
        assert!(!cowork.contains("unknown agent"), "{cowork}");
        let app = rank4_refusal("codex-app").expect("classified");
        assert!(
            app.contains("ChatGPT.app") || app.contains("compiled"),
            "{app}"
        );
        assert!(!app.contains("unknown agent"), "{app}");
        // Codex CLI remains an Agent row — not this table.
        assert_eq!(by_name("codex"), Some(Agent::Codex));
        assert!(rank4_refusal("codex").is_none());
        assert!(rank4_refusal("not-an-agent").is_none());
    }

    #[test]
    fn the_rendered_matrix_names_every_agent_once() {
        let out = render();
        for &a in ALL {
            assert!(
                out.contains(a.as_str()),
                "{} missing from render",
                a.as_str()
            );
        }
        // Two lines per agent: the verdict, then the measurement it rests on.
        assert_eq!(out.lines().count(), ALL.len() * 2);
        for &a in ALL {
            assert!(
                out.contains(entry(a).evidence),
                "{} renders a verdict with no evidence under it",
                a.as_str()
            );
        }
    }
}
