//! **Install, supervise, start at boot, and remove.**
//!
//! Rank 4 is the only one `tokenade install` does NOT lay down: it is asked
//! for by name, because it can break something. An agent pointed at a proxy
//! that does not answer does not degrade, it refuses.
//!
//! Hence the shape of this file: nothing is declared installed before
//! `confirm_it_serves` has verified it, and removal lives next to install. An
//! installer without its removal leaves a configuration pointing at a missing
//! binary, and the user reads the failure as their agent breaking down.

use super::*;

pub(crate) fn do_install(agent: Agent, listen: &str, autostart: bool, enable: bool) -> i32 {
    let Some(home) = home() else {
        eprintln!("tokenade llm-proxy: no home directory");
        return 1;
    };
    // The matrix decides whether this is worth doing at ALL, before a byte is
    // written. Installing in front of Claude Code would add a second authority
    // on payloads its hooks already own, and installing in front of an agent
    // nobody measured would be a claim rather than a coverage.
    let entry = agent_matrix::entry(agent);
    let role = entry.role(agent.coverage());
    if install::recipe(agent).is_none() {
        // No FILE to write. That is not the same fact as "out of reach", and
        // printing one sentence for both was hiding a whole class: measured
        // 2026-08-19, `cursor-agent` dials whatever `CURSOR_API_ENDPOINT`
        // names — a listener on this machine got its
        // `aiserver.v1.DashboardService/GetMe` — and its only other route is
        // the `-e` flag. There is nothing to install and there IS something
        // to do, so say which one it is and hand over the exact line.
        if let agent_matrix::Reach::EnvVar(var) = entry.reach {
            let listen = install::listen_for(&home, agent, DEFAULT_LISTEN, None);
            eprintln!(
                "tokenade llm-proxy: no config file to write for {} — it is reached through the \n\
                 {var} environment variable, and this product does not edit shell rc files.\n\
                 \n\
                 Set it yourself, in the shell you launch {} from:\n\
                 \x20 export {var}={}\n\
                 \n\
                 and run the proxy against whatever {var} pointed at before:\n\
                 \x20 tokenade llm-proxy --listen {listen} --upstream <the URL it used>\n\
                 \n\
                 {}",
                agent.as_str(),
                agent.as_str(),
                base_url_for(&listen),
                role.describe()
            );
            return 3;
        }
        // Two more shapes of "no file", and neither is "nothing to install".
        // Printing one sentence for all four is what made three cells read
        // `n/a` for three different facts: one agent nobody had installed, one
        // whose endpoint is nailed shut, and one already covered by a row that
        // exists. An operator can act on exactly one of those.
        if let agent_matrix::Reach::Inherited { from, how } = entry.reach {
            eprintln!(
                "tokenade llm-proxy: nothing to write for {} — it is covered by {}'s install.\n\
                 \n\
                 {how}\n\
                 \n\
                 Install that one instead, and this agent goes through it:\n\
                 \x20 tokenade llm-proxy install --agent {}\n\
                 \n\
                 A second file here would put two proxies in front of one process \n\
                 and count the same bytes twice.",
                agent.as_str(),
                from.as_str(),
                from.as_str(),
            );
            return 4;
        }
        if let agent_matrix::Reach::NoneFound(where_) = entry.reach {
            eprintln!(
                "tokenade llm-proxy: no way in for {} — {where_}.\n\
                 \n\
                 {}",
                agent.as_str(),
                entry.evidence
            );
            return 5;
        }
        eprintln!(
            "tokenade llm-proxy: nothing to install for {} — {}",
            agent.as_str(),
            role.describe()
        );
        return 2;
    }

    // A port per agent, allocated here rather than defaulted in the parser.
    // Pointing a second agent at the first one's port produces a config that
    // looks right and sends every request to the wrong provider; the port is
    // what tells the single proxy process which agent it is serving.
    let listen = &install::listen_for(
        &home,
        agent,
        DEFAULT_LISTEN,
        (listen != DEFAULT_LISTEN).then_some(listen),
    );
    // Asking for the install by hand cancels an earlier refusal: it is the
    // same person changing their mind, and the list must not say no on their
    // behalf.
    install::clear_declined(&home, agent);
    // Already installed and still tracked: reuse the record. Without this,
    // adding `--autostart` to an install made without it was refused ("the
    // agent already points at … this proxy") — measured 2026-10-06, Codex on
    // a Windows VM — and the only way to get it was to uninstall everything.
    let kept = install::load_records(&home)
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.agent == agent.as_str())
        .filter(|_| !install::bypassed(agent, &home, &base_url_for(listen)));
    let record = if let Some(r) = kept {
        println!(
            "{}: already installed → this proxy on {listen}, forwarding to {}",
            agent.as_str(),
            shown(&r.upstream)
        );
        r
    } else {
        let plan = match install::plan(agent, &home, &base_url_for(listen)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("tokenade llm-proxy: {e}");
                return 2;
            }
        };
        // Say what will happen BEFORE it happens, including where the traffic
        // will go. The upstream is the one thing the user cannot see in their
        // own config afterwards, because we are about to overwrite it.
        println!(
            "{}: {} → this proxy on {listen}, forwarding to {}",
            agent.as_str(),
            plan.path.display(),
            shown(&plan.upstream)
        );
        match install::install(&home, &plan) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("tokenade llm-proxy: {e}");
                return 1;
            }
        }
    };
    println!("  {}", role.describe());

    if autostart {
        match autostart_for(&home, listen, &record.upstream, Some(&record.agent)) {
            Ok((path, cmds)) => {
                println!("\nwrote {}", path.display());
                if enable {
                    if let Err(rc) = apply_supervisor(&cmds, "start at boot", listen) {
                        return rc;
                    }
                } else {
                    println!("run these to make it start at boot:");
                    for c in cmds {
                        println!("  {c}");
                    }
                    println!("  (or re-run with --enable and this product runs them)");
                }
            }
            Err(e) => {
                eprintln!("tokenade llm-proxy: autostart not set up: {e}");
                return 1;
            }
        }
    } else {
        println!(
            "\nthe proxy is NOT running yet. Start it with:\n  \
             tokenade llm-proxy --listen {listen} --upstream {}\n\
             or re-run this with --autostart, because a base URL pointing at a proxy \
             that is not there fails every turn.",
            shown(&record.upstream)
        );
    }
    0
}

/// Run a supervisor list, then ASK the supervisor whether it took.
///
/// The asking is not belt-and-braces. `schtasks /create` exits 0 on a failure
/// — measured 2026-08-18 — so a run that believes exit codes reports an
/// autostart nobody registered, which is the "works but does not tell you"
/// zero this whole ladder is about.
pub(crate) fn apply_supervisor(cmds: &[String], what: &str, listen: &str) -> Result<(), i32> {
    let Some(platform) = autostart::Platform::current() else {
        eprintln!(
            "tokenade llm-proxy: no supervisor known for {}",
            std::env::consts::OS
        );
        return Err(2);
    };
    if cmds.iter().any(|c| autostart::needs_admin(c)) {
        eprintln!("tokenade llm-proxy: starting at boot here needs administrator rights; tokenade never asks for them. Run these yourself:");
        for c in cmds {
            eprintln!("  {c}");
        }
        return Err(1);
    }
    for c in cmds {
        match autostart::run_supervisor_command(c) {
            Ok(()) => println!("  ran: {c}"),
            Err(e) => {
                eprintln!("tokenade llm-proxy: {e}");
                return Err(1);
            }
        }
    }
    let check = autostart::verify_command(platform, listen);
    let known = autostart::run_supervisor_command(&check).is_ok();
    // Removal is the one case where the query FAILING is the good answer.
    let removing = what == "stop";
    if known == removing {
        eprintln!(
            "tokenade llm-proxy: the commands all succeeded and `{check}` disagrees — \
             the supervisor did not {} this unit",
            if removing { "forget" } else { "register" }
        );
        return Err(1);
    }
    println!(
        "  confirmed by `{check}`: the supervisor {} this unit",
        if removing { "no longer knows" } else { "knows" }
    );
    if !removing {
        confirm_it_serves(platform, listen)?;
    }
    Ok(())
}

/// Registered is not running, and the enable path used to stop at registered.
///
/// The second question exists because the first one answered `enabled` over a
/// unit that was `failed` — see `autostart::running_command`. It is asked only
/// on the start path, and only where the answer is verified rather than
/// guessed; `running_command` returns `None` elsewhere and this is a no-op.
///
/// Two readings a second apart rather than one: `Type=simple` means the
/// supervisor returns from `enable --now` as soon as it has forked, before the
/// proxy has tried to bind, so a single look right after can read `active` on
/// a process that is already on its way out.
pub(crate) fn confirm_it_serves(platform: autostart::Platform, listen: &str) -> Result<(), i32> {
    let Some(is_active) = autostart::running_command(platform, listen) else {
        return Ok(());
    };
    let mut seen = String::new();
    let mut consecutive = 0;
    for _ in 0..12 {
        seen = autostart::supervisor_says(&is_active).unwrap_or_default();
        if seen == "active" {
            consecutive += 1;
            if consecutive == 2 {
                // **What the supervisor knows, and what it does not.**
                //
                // `is-active` vouches for the UNIT, not the address: a unit
                // whose `ExecStart` is broken can stay "active" on a process
                // from the previous version, listening on another port. The
                // real output of an upgrade thus said "it is serving" three
                // lines above "nothing answered on 127.0.0.1:8788 within 8s"
                // — two true sentences, only one readable. This one now
                // promises only what it has verified.
                println!("  confirmed by `{is_active}`: the unit is running");
                return Ok(());
            }
        } else {
            consecutive = 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(700));
    }
    let why = autostart::failure_reason_command(platform, listen)
        .and_then(|c| autostart::supervisor_says(&c))
        .filter(|w| !w.is_empty())
        .map(|w| format!(" — the supervisor says: {w}"))
        .unwrap_or_default();
    eprintln!(
        "tokenade llm-proxy: the unit is registered and NOT serving \
         (`{is_active}` says `{}`){why}. The usual cause is something else already \
         holding the listen address; the unit keeps retrying, so freeing it is enough. \
         `tokenade llm-proxy autostart --remove --enable` takes the unit back off.",
        if seen.is_empty() { "nothing" } else { &seen }
    );
    Err(1)
}

/// `autostart_unit` appends the restart commands only when the unit's text
/// changed.
fn unit_changed(cmds: &[String], listen: &str) -> bool {
    autostart::Platform::current()
        .is_some_and(|p| cmds.len() > autostart::enable_commands(p, listen).len())
}

pub(crate) fn autostart_for(
    home: &std::path::Path,
    listen: &str,
    upstream: &str,
    agent: Option<&str>,
) -> Result<(PathBuf, Vec<String>), String> {
    let platform = autostart::Platform::current()
        .ok_or_else(|| format!("no supervisor known for {}", std::env::consts::OS))?;
    autostart_unit(home, platform, listen, upstream, agent)
}

/// Removes the old unit, the one that carried no port.
///
/// Silent when there is none: that is the case for every install made after
/// this change, and a migration that talks about nothing is noise on every
/// upgrade.
fn retire_the_portless_unit(platform: autostart::Platform, home: &std::path::Path) {
    // `""` gives no port, so `service_name` returns the historical name.
    let legacy = autostart::unit_path(platform, home, "");
    if !legacy.exists() {
        return;
    }
    for c in autostart::disable_commands(platform, "") {
        let _ = autostart::run_supervisor_command(&c);
    }
    match std::fs::remove_file(&legacy) {
        Ok(()) => println!(
            "  ✓ removed the old unit ({}): there is now one service per proxy",
            legacy.display()
        ),
        Err(e) => eprintln!(
            "  ⚠ the old unit {} could not be removed: {e}",
            legacy.display()
        ),
    }
}

pub(crate) fn autostart_unit(
    home: &std::path::Path,
    platform: autostart::Platform,
    listen: &str,
    upstream: &str,
    agent: Option<&str>,
) -> Result<(PathBuf, Vec<String>), String> {
    // **This path is WRITTEN into a unit file, so it must outlive the binary
    // that writes it.** `env::current_exe()` reads `/proc/self/exe`, and
    // Linux appends ` (deleted)` as soon as the file has been replaced — which
    // is what `tokenade upgrade` does right before getting here. Observed
    // plainly on a real machine after 1.1.14:
    //
    //   ExecStart=~/.local/bin/tokenade (deleted) llm-proxy …
    //
    // The unit can no longer start; the old process survives on the old port,
    // `systemctl is-active` answers "active", and every following upgrade
    // probes the NEW port, gets nothing, and undoes the agent's configuration
    // announcing "not installed" three lines below "it is serving".
    //
    // `stable_exe` is the declarer written for this — "the binary a PERSISTED
    // configuration must name" — and its own comment tells of the same
    // mistake made by `mcp_wrap`. Third time.
    let exe = std::path::PathBuf::from(tokenade_core::platform::stable_exe());
    let svc = autostart::Service {
        exe,
        listen: listen.to_string(),
        upstream: upstream.to_string(),
        agent: agent.map(str::to_string),
    };
    // **The historical, portless unit must go.**
    //
    // Earlier installs wrote `tokenade-llm-proxy.service` on behalf of every
    // agent — the last one overwrote the previous one's upstream. Now that
    // there is one unit PER PORT, that one is a duplicate that restarts with
    // every session and fights the right one for a port.
    retire_the_portless_unit(platform, home);
    let (path, changed) =
        autostart::write_unit_reporting(platform, home, &svc).map_err(|e| e.to_string())?;
    // **Rewriting the unit repairs nothing until someone re-reads it.**
    // `enable --now` leaves an already-active process in place, whatever
    // command line it carries: a machine whose `ExecStart` named a deleted
    // binary kept its old proxy, on the old port, on every upgrade. When the
    // content changed, we restart.
    let mut cmds = autostart::enable_commands(platform, listen);
    if changed {
        cmds.extend(autostart::restart_commands(platform, listen));
    }
    Ok((path, cmds))
}

pub(crate) fn do_autostart(
    listen: &str,
    upstream: &str,
    remove: bool,
    enable: bool,
    agent: Option<&str>,
) -> i32 {
    let Some(home) = home() else {
        eprintln!("tokenade llm-proxy: no home directory");
        return 1;
    };
    let Some(platform) = autostart::Platform::current() else {
        eprintln!(
            "tokenade llm-proxy: no supervisor known for {} — the proxy would have to be \
             started by hand after every reboot",
            std::env::consts::OS
        );
        return 2;
    };
    let path = autostart::unit_path(platform, &home, listen);
    if remove {
        // The file goes only after the supervisor has been told, and the
        // command that tells it is printed rather than run: removing a unit
        // out from under a running service leaves a process nothing can name.
        let cmds = autostart::disable_commands(platform, listen);
        if enable {
            // `--enable` on `--remove` means the same thing it means on the
            // way in: run the list instead of printing it. The verb is still
            // the opposite one, and the list is the disable list.
            if let Err(rc) = apply_supervisor(&cmds, "stop", listen) {
                return rc;
            }
        } else {
            println!("stop it first:");
            for c in cmds {
                println!("  {c}");
            }
            println!("  (or re-run with --enable and this product runs them)");
        }
        match std::fs::remove_file(&path) {
            Ok(()) => println!("\nremoved {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("\nno unit at {}", path.display())
            }
            Err(e) => {
                eprintln!("tokenade llm-proxy: {} — {e}", path.display());
                return 1;
            }
        }
        return 0;
    }
    // The agent of this port: the one named, otherwise the one the install
    // put on this port. Without it, the proxy records its savings under
    // `unknown` — measured 2026-10-06 on a Windows VM, Codex app: three
    // `unknown` rows.
    let agent = agent.map(str::to_string).or_else(|| {
        install::load_records(&home)
            .unwrap_or_default()
            .into_iter()
            .find(|r| autostart::service_name(&r.listen) == autostart::service_name(listen))
            .map(|r| r.agent)
    });
    match autostart_unit(&home, platform, listen, upstream, agent.as_deref()) {
        Ok((path, cmds)) => {
            println!("wrote {}", path.display());
            if enable {
                if let Err(rc) = apply_supervisor(&cmds, "start at boot", listen) {
                    return rc;
                }
            } else {
                println!("run these to make it start at boot:");
                for c in cmds {
                    println!("  {c}");
                }
                println!("  (or re-run with --enable and this product runs them)");
            }
            0
        }
        Err(e) => {
            eprintln!("tokenade llm-proxy: {e}");
            1
        }
    }
}

/// What the automatic install did, agent by agent.
#[derive(Debug)]
pub(crate) enum AutoOutcome {
    /// Installed and VERIFIED: the proxy answers on its port.
    Installed {
        agent: Agent,
        listen: String,
        gains: usize,
    },
    /// Already there. Nothing to do, and above all nothing to rewrite.
    Already(Agent),
    /// The user said no once. We do not ask again.
    Declined(Agent),
    /// Installed then TAKEN BACK: the proxy did not answer, so the agent's
    /// configuration was put back as it was.
    RolledBack { agent: Agent, why: String },
}

/// How long the proxy gets to answer before everything is taken back.
///
/// A supervisor that was just registered does not start instantly — systemd,
/// launchd and the Windows scheduler each have their latency. Eight seconds
/// cover all three with margin, measured on the "proxy survives a kill"
/// column where recovery takes 2 s on Linux and 97 s at worst on Windows after
/// a SIGKILL — but here we are not recovering from a kill, we are starting
/// cold.
pub(crate) const HEALTH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);

/// Install the proxy where it lights up a dark cell, without asking anything.
///
/// This is the heart of the move from opt-in to default, and it rests on a
/// single rule: **NEVER leave an agent pointing at a port nobody listens
/// on.** A base URL rewritten to a missing proxy is an agent that fails on
/// every turn — and since this install is no longer requested by the user,
/// they would have neither the output in front of them nor the slightest
/// reason to connect the two. So we write, we start, we ASK the proxy whether
/// it answers, and if it does not we put the original configuration back.
///
/// Agents that `features::gains` leaves empty are not touched: those are the
/// ones the proxy brings nothing to, and one more channel on a payload already
/// covered is the double authority the one-authority rule forbids.
/// Does the port answer before the deadline? Probed, not inferred.
fn answers_before(listen: &str, budget: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if our_proxy_health(listen).is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    false
}

/// Does the answering proxy serve the binary we just installed?
///
/// `None` (no `version` field) answers NO: it is a proxy older than the field
/// itself, so at best the previous version. Answering "yes" when in doubt
/// would carry over exactly the failure this field exists to see.
pub(crate) fn serving_this_build(served: Option<&str>) -> bool {
    served == Some(env!("CARGO_PKG_VERSION"))
}

/// Restart the service holding this port, then wait for it to answer.
///
/// Restarting is the ONLY correct move here: the supervisor re-reads the unit
/// and executes the current inode, whereas killing the process would leave
/// the port with nothing and rewriting the unit would not change a line (its
/// `ExecStart` is already right — it is the process's MEMORY that is old).
fn restart_stale_proxy(listen: &str) {
    // Only the INSTALLED binary gets to decide.
    //
    // The unit executes `stable_exe()`: restarting therefore puts it back on
    // THAT binary, not on the one making the request. A tokenade run from a
    // `target/release` would see its version differ from the one the proxy
    // serves and would restart a perfectly up-to-date service — every day,
    // since the restart cannot make the two converge.
    if !tokenade_core::platform::running_the_installed_binary() {
        return;
    }
    let Some(platform) = autostart::Platform::current() else {
        return;
    };
    for c in autostart::restart_commands(platform, listen) {
        let _ = autostart::run_supervisor_command(&c);
    }
    let _ = answers_before(listen, HEALTH_DEADLINE);
}

pub(crate) fn ensure_installed_for(agents: &[Agent]) -> Vec<AutoOutcome> {
    let Some(home) = home() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // The (listen, upstream) pairs already applied in THIS pass — see below.
    let mut applied: Vec<(String, String)> = Vec::new();
    for &agent in agents {
        let gains = super::features::gains(agent);
        if gains.is_empty() {
            continue;
        }
        if install::is_declined(&home, agent) {
            out.push(AutoOutcome::Declined(agent));
            continue;
        }
        // The port FIRST, because deciding "already installed" needs it.
        let listen = install::listen_for(&home, agent, DEFAULT_LISTEN, None);
        let has_record = install::load_records(&home)
            .unwrap_or_default()
            .iter()
            .any(|r| r.agent == agent.as_str());
        // A record proves a CONFIG was written, not that anything serves it.
        //
        // This branch used to return `Already` on the record alone, and that
        // made the daily auto-install a no-op on exactly the machines that
        // needed it. The supervisor did not exist before 2026-08-31
        // (`d208b23e`): every install older than that wrote the agent's config
        // to point at a port and left nothing listening on it. Measured on a
        // dogfood box 2026-09-02 — record dated 08-25, no systemd unit, no
        // listener, machine up 8 days, and `llm-proxy-autocheck` stamped the
        // day before: the check ran, said "already installed", and walked past
        // an agent whose config had been pointing at a dead port for a week.
        //
        // So ask the port. If something of ours answers, leave it alone —
        // including a proxy the user started by hand. If nothing does, fall
        // through and install for real: the write is idempotent, the
        // supervisor gets laid down, and the health check below either
        // confirms it or rolls the config back exactly as it was.
        //
        // "It answers" is not enough either: the answering process may serve
        // the PREVIOUS binary. `upgrade` replaces the inode and touches no
        // daemon, so a proxy started three versions ago keeps answering `ok`
        // while serving stale code — measured on this machine on 2026-09-15,
        // three active services whose `/proc/<pid>/exe` read `… (deleted)`
        // after the move to 1.1.16. The version announced by the health route
        // decides: if it is not ours, we restart the service and ask again.
        // Installed on a route the agent no longer takes (Qwen's selected
        // model carries its own base URL): undo that install exactly, then
        // install again on the route it does take.
        let has_record = if has_record && install::bypassed(agent, &home, &base_url_for(&listen)) {
            install::uninstall(&home, agent).is_err()
        } else {
            has_record
        };
        if has_record {
            if let Some(served) = our_proxy_health(&listen) {
                // The unit is rewritten on every pass: one written before
                // 1.2.0 ran without `--agent`, and only a rewrite — with the
                // restart `autostart_unit` asks for when the text changed —
                // gives the running proxy its agent.
                let record = install::load_records(&home)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|r| r.agent == agent.as_str());
                let rewrote = record
                    .and_then(|r| autostart_for(&home, &listen, &r.upstream, Some(&r.agent)).ok())
                    .is_some_and(|(_, cmds)| {
                        unit_changed(&cmds, &listen)
                            && apply_supervisor(&cmds, "start at boot", &listen).is_ok()
                    });
                if !rewrote && !serving_this_build(served.as_deref()) {
                    restart_stale_proxy(&listen);
                }
                out.push(AutoOutcome::Already(agent));
                continue;
            }
        }
        // Two different repairs, and telling them apart is the whole point.
        //
        // When a record already exists the agent's config ALREADY points at
        // us, and `install::plan` refuses exactly that ("the agent already
        // points at …, which is this proxy") — rightly, since re-pointing it
        // would make the proxy its own upstream. So a machine whose config is
        // ours and whose port is dead could be repaired by nothing: `install`
        // refused out loud, and this function swallowed the same refusal in
        // silence. What is missing there is not the config, it is something
        // SERVING it, so we keep the record and go straight to the supervisor.
        let (record, wrote_config) = if has_record {
            let Some(existing) = install::load_records(&home)
                .unwrap_or_default()
                .into_iter()
                .find(|r| r.agent == agent.as_str())
            else {
                continue;
            };
            (existing, false)
        } else {
            let Ok(plan) = install::plan(agent, &home, &base_url_for(&listen)) else {
                continue;
            };
            match install::install(&home, &plan) {
                Ok(r) => (r, true),
                Err(_) => continue,
            }
        };
        // The supervisor first, the check second. The order matters: we cannot
        // ask the proxy whether it answers before asking someone to start it.
        let mut why: Option<String> = None;
        // **One supervisor, one unit — and N agents.**
        //
        // This block ran once PER AGENT on a single unit file: on a real
        // machine, the same five lines (`daemon-reload`, `enable --now`,
        // `enable-linger`, two confirmations) came out twice in a row, for
        // qwen-code then for grok. And the second silently overwrote the
        // first's upstream — the observed unit carried
        // `--upstream https://api.x.ai/v1`, grok's.
        //
        // So the same pair is no longer re-applied. A DIFFERENT pair is
        // re-applied and SAYS so: it is a conflict, not a repetition, and
        // keeping quiet left one agent pointing at another's upstream.
        // **One service per PORT, so two agents only get in each other's way
        // if they share the port.** This guard dates from when a single unit
        // served everyone and the last agent overwrote the previous one's
        // upstream; now that the port is in the service name, the only
        // possible collision is two agents on the same listen address with
        // different upstreams — and that one still deserves to be said.
        let want = (listen.clone(), record.upstream.clone());
        let already_done = applied.contains(&want);
        if let Some((_, other_upstream)) =
            applied.iter().find(|(l, u)| *l == want.0 && *u != want.1)
        {
            println!(
                "  ⚠ two agents on {} with different upstreams: {} wants {}, \
                 already set for {other_upstream}",
                want.0,
                agent.as_str(),
                want.1
            );
        }
        if !already_done {
            match autostart_for(&home, &listen, &record.upstream, Some(&record.agent)) {
                Ok((_, cmds)) => {
                    if apply_supervisor(&cmds, "start at boot", &listen).is_err() {
                        why = Some("the supervisor did not get the proxy running".into());
                    } else {
                        applied.push(want);
                    }
                }
                Err(e) => why = Some(format!("autostart could not be written: {e}")),
            }
        }
        if why.is_none() {
            // **A second chance, preceded by a restart.**
            //
            // The unit's content is a poor signal for "is the running process
            // the right one?": a pass that fixes the `ExecStart` makes it
            // identical to the next one, so the next one restarts nothing
            // while the old process is still there. Seen while repairing a
            // real machine — unit rewritten on the first call, old process of
            // the previous version still on the old port on the second.
            //
            // The port is the only judge. If it does not answer, we restart
            // and ask again: that repairs the stale process, the crashed
            // service and the fixed-but-never-reread unit, without having to
            // tell which of the three it was.
            if !answers_before(&listen, HEALTH_DEADLINE) {
                let restart = autostart::Platform::current()
                    .map(|p| autostart::restart_commands(p, &listen))
                    .unwrap_or_default();
                let relaunched = !restart.is_empty()
                    && restart
                        .iter()
                        .all(|c| autostart::run_supervisor_command(c).is_ok());
                if !(relaunched && answers_before(&listen, HEALTH_DEADLINE)) {
                    why = Some(format!(
                        "nothing answered on {listen} within {}s, restart included",
                        HEALTH_DEADLINE.as_secs() * 2
                    ));
                }
            }
        }
        match why {
            None => out.push(AutoOutcome::Installed {
                agent,
                listen,
                gains: gains.len(),
            }),
            Some(why) => {
                // The rollback: `uninstall` puts the agent's file back as it
                // was, byte for byte — that is the "reversible install"
                // column, green on all 24 agents.
                //
                // ONLY when this pass is what wrote it. On the supervisor-only
                // repair the config was already the user's state before we ran,
                // and "put it back as it was" would mean tearing out a working
                // configuration because a service failed to start — undoing
                // something we did not do.
                if wrote_config {
                    let _ = install::uninstall(&home, agent);
                }
                out.push(AutoOutcome::RolledBack { agent, why });
            }
        }
    }
    out
}

pub(crate) fn do_uninstall(agent: Agent) -> i32 {
    let Some(home) = home() else {
        eprintln!("tokenade llm-proxy: no home directory");
        return 1;
    };
    // An explicit refusal is remembered. The proxy installs itself where it
    // lights up a dark cell; without this trace, the next automatic repair
    // would put back what the user just removed.
    install::mark_declined(&home, agent);
    match install::uninstall(&home, agent) {
        Ok(Some(r)) => {
            println!(
                "{}: {} restored to {}",
                agent.as_str(),
                r.path.display(),
                shown(&r.upstream)
            );
            if let Some(p) = autostart::Platform::current() {
                println!("\nthe proxy may still be running. Stop it with:");
                for c in autostart::disable_commands(p, &r.listen) {
                    println!("  {c}");
                }
            }
            0
        }
        Ok(None) => {
            println!("{}: nothing was installed", agent.as_str());
            0
        }
        Err(e) => {
            eprintln!("tokenade llm-proxy: {e}");
            1
        }
    }
}
