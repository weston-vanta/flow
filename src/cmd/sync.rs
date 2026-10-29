//! `flow inventory --sync`: brings other systems in step with flow's hosts. herdr's saved SSH
//! machines on this machine are the one target: every known host but this one and the gone ones
//! gets a profile labelled with its name that shows its herdr session flow runs in, so one local
//! herdr window shows every host's workspaces and agents. A profile is flow's when its label is
//! a known host's name; flow never touches any other. A gone host's profile goes only when it
//! still has the target flow would have given it. A host that did not answer the last refresh is
//! not added, since `herdr machine add` has to reach it, and its profile is kept as it is.

use std::collections::BTreeSet;
use std::process::ExitCode;

use anyhow::{Context, bail};
use serde::Serialize;

use super::{Consent, Refused};
use crate::herdr::{self, Machine};
use crate::host;
use crate::inventory::{self, HostInventory};
use crate::paths::StateDir;
use crate::runner::Runner;
use crate::state::config::{Config, Host};

/// One change to herdr's saved machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Step {
    Add {
        host: String,
        target: String,
        session: String,
    },
    Remove {
        host: String,
        profile_id: String,
        target: String,
        session: String,
        /// The host is gone, rather than its profile out of date.
        gone: bool,
    },
}

/// What `--sync` would change, in the order it applies it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Hosts with no up-to-date profile that are not added: they did not answer the last
    /// refresh.
    pub skipped: Vec<String>,
}

/// The plan that gives every host of `inventories` but the local one and the gone ones a
/// profile among `machines` labelled with its name, for its ssh alias and herdr session
/// `session`, host by host in name order. A host's profiles that differ are removed, beside a
/// current one too; when there is none they go before the new one is added, since herdr names
/// each machine once. A gone host's profile with the target flow would have given it is
/// removed, unless a live host has the same name.
pub fn plan(
    inventories: &[HostInventory],
    local_env_id: Option<&str>,
    session: &str,
    machines: &[Machine],
) -> Plan {
    let mut sorted: Vec<&HostInventory> = inventories.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let live: BTreeSet<&str> = sorted
        .iter()
        .filter(|inv| !inv.gone)
        .map(|inv| inv.name.as_str())
        .collect();
    let mut plan = Plan::default();
    for inv in sorted {
        let host = inv.host();
        let labelled = || machines.iter().filter(|machine| machine.label == host.name);
        if inv.gone {
            if live.contains(host.name.as_str()) {
                continue;
            }
            for machine in labelled().filter(|machine| machine.target == host.ssh_alias) {
                plan.steps.push(remove(&host, machine, true));
            }
            continue;
        }
        if host::is_local(&host, local_env_id) {
            continue;
        }
        let (current, stale): (Vec<&Machine>, Vec<&Machine>) = labelled()
            .partition(|machine| machine.target == host.ssh_alias && machine.session == session);
        let replace = current.is_empty();
        if replace && !(inv.reachable && inv.should_probe()) {
            plan.skipped.push(host.name.clone());
            continue;
        }
        for machine in stale {
            plan.steps.push(remove(&host, machine, false));
        }
        if !replace {
            continue;
        }
        plan.steps.push(Step::Add {
            host: host.name.clone(),
            target: host.ssh_alias.clone(),
            session: session.to_owned(),
        });
    }
    plan
}

fn remove(host: &Host, machine: &Machine, gone: bool) -> Step {
    Step::Remove {
        host: host.name.clone(),
        profile_id: machine.id.clone(),
        target: machine.target.clone(),
        session: machine.session.clone(),
        gone,
    }
}

impl Step {
    /// The plan's line for this step.
    fn line(&self) -> String {
        match self {
            Step::Add {
                host,
                target,
                session,
            } => format!(
                " - add host {host} ({target}, session {session}) to herdr's saved machines.\n"
            ),
            Step::Remove {
                host, gone: true, ..
            } => format!(" - remove gone host {host} from herdr's saved machines.\n"),
            Step::Remove {
                host,
                target,
                session,
                gone: false,
                ..
            } => format!(
                " - remove host {host} ({target}, session {session}) from herdr's saved \
                 machines.\n"
            ),
        }
    }

    /// What the step does, as its failure names it.
    fn doing(&self) -> String {
        match self {
            Step::Add { host, .. } => format!("adding host {host}"),
            Step::Remove {
                host, gone: true, ..
            } => format!("removing gone host {host}"),
            Step::Remove { host, .. } => format!("removing host {host}"),
        }
    }

    fn argv(&self) -> Vec<String> {
        match self {
            Step::Add {
                host,
                target,
                session,
            } => herdr::add_machine_argv(target, host, session),
            Step::Remove { profile_id, .. } => herdr::remove_machine_argv(profile_id),
        }
    }
}

/// Reads herdr's saved machines on this machine, prints the plan, and applies it once the user
/// agrees, or at once with `yes`. With `json` the plan is printed as JSON, and applied only with
/// `yes`, herdr's output then going to stderr. Without a terminal or `yes`, it exits 1 having
/// applied nothing. A step that fails stops the rest and fails the command, naming how many
/// were applied.
pub fn run(
    state: &StateDir,
    config: &Config,
    runner: &dyn Runner,
    json: bool,
    yes: bool,
) -> anyhow::Result<ExitCode> {
    let hosts = inventory::live_hosts(state)?;
    let here = host::local(&hosts).map_err(|err| Refused(format!("{err:#}")))?;
    let machines = herdr::machines(runner, here)?;
    let plan = plan(
        &inventory::load_all(state)?,
        host::local_env_id().as_deref(),
        &config.herdr_session,
        &machines,
    );
    for name in &plan.skipped {
        eprintln!(
            "not adding host {name}: it did not answer the last refresh, and herdr machine add \
             has to reach it"
        );
    }
    if json {
        super::print_json(&plan)?;
        if yes {
            apply(runner, here, &plan.steps, Out::Stderr)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    let except = if plan.skipped.is_empty() {
        ""
    } else {
        ", except for the hosts not added"
    };
    if plan.steps.is_empty() {
        super::print(&format!(
            "herdr machines already match the inventory{except}\n"
        ))?;
        return Ok(ExitCode::SUCCESS);
    }
    let lines: String = plan.steps.iter().map(Step::line).collect();
    super::print(&format!("Herdr:\n{lines}"))?;
    match super::confirm(yes)? {
        Consent::Given => {
            apply(runner, here, &plan.steps, Out::Through)?;
            super::print(&format!("herdr machines now match the inventory{except}\n"))?;
            Ok(ExitCode::SUCCESS)
        }
        Consent::Declined => {
            super::print("nothing applied\n")?;
            Ok(ExitCode::SUCCESS)
        }
        Consent::NoTerminal => {
            eprintln!("rerun with --yes to apply");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Where herdr's own output goes.
#[derive(Clone, Copy)]
enum Out {
    /// stdout to stdout and stderr to stderr.
    Through,
    /// Both to stderr, keeping stdout for JSON.
    Stderr,
}

/// Applies `steps` in order, stopping at the first that fails, and failing with what it was
/// doing, how many steps were applied, and, when it was adding a host whose old profile was
/// already removed, that the host is left without one.
fn apply(runner: &dyn Runner, here: &Host, steps: &[Step], out: Out) -> anyhow::Result<()> {
    for (applied, step) in steps.iter().enumerate() {
        let argv = step.argv();
        let command = argv.join(" ");
        let failed = || {
            let mut failed = format!(
                "{}; applied {applied} of {} steps before it",
                step.doing(),
                steps.len()
            );
            if let Some(host) = left_without_profile(&steps[..applied], step) {
                failed.push_str(&format!(
                    ", and host {host} is left without a herdr machine"
                ));
            }
            failed
        };
        let output = runner
            .run(here, &argv, None)
            .with_context(|| format!("running `{command}` {}", failed()))?;
        match out {
            Out::Through => super::print(&output.stdout)?,
            Out::Stderr => eprint!("{}", output.stdout),
        }
        eprint!("{}", output.stderr);
        if output.status != 0 {
            bail!("`{command}` exited {} {}", output.status, failed());
        }
    }
    Ok(())
}

/// The host `step` adds when one of the `applied` steps removed its old profile.
fn left_without_profile<'a>(applied: &[Step], step: &'a Step) -> Option<&'a str> {
    let Step::Add { host, .. } = step else {
        return None;
    };
    applied
        .iter()
        .any(|done| matches!(done, Step::Remove { host: removed, gone: false, .. } if removed == host))
        .then_some(host.as_str())
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;

    fn record(name: &str, reachable: bool, gone: bool) -> HostInventory {
        let host = Host {
            name: name.to_owned(),
            env_id: format!("env-{name}"),
            ssh_alias: format!("{name}.example"),
            local_marker_env: None,
        };
        HostInventory {
            reachable,
            gone,
            ..HostInventory::new(&host, Timestamp::UNIX_EPOCH)
        }
    }

    fn machine(id: &str, label: &str, target: &str, session: &str) -> Machine {
        Machine {
            id: id.to_owned(),
            label: label.to_owned(),
            target: target.to_owned(),
            session: session.to_owned(),
        }
    }

    fn add(host: &str) -> Step {
        Step::Add {
            host: host.to_owned(),
            target: format!("{host}.example"),
            session: "default".to_owned(),
        }
    }

    #[test]
    fn plan_adds_live_hosts_and_skips_the_local_one() {
        let inventories = [record("host-b", true, false), record("host-a", true, false)];

        let plan = plan(&inventories, Some("env-host-a"), "default", &[]);

        assert_eq!(plan.steps, [add("host-b")]);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn plan_removes_a_gone_hosts_profile_only_when_flow_would_have_made_it() {
        let inventories = [record("host-d", true, true), record("host-g", true, true)];
        let machines = [
            machine("p1", "host-d", "host-d.example", "flow"),
            machine("p2", "host-g", "elsewhere.example", "default"),
        ];

        let plan = plan(&inventories, None, "default", &machines);

        assert_eq!(
            plan.steps,
            [Step::Remove {
                host: "host-d".to_owned(),
                profile_id: "p1".to_owned(),
                target: "host-d.example".to_owned(),
                session: "flow".to_owned(),
                gone: true,
            }]
        );
    }

    #[test]
    fn plan_keeps_unreachable_hosts_as_they_are() {
        let mut stopped = record("host-s", true, false);
        stopped.ona_phase = Some("ENVIRONMENT_PHASE_STOPPED".to_owned());
        let inventories = [
            record("host-c", false, false),
            stopped,
            record("host-u", false, false),
        ];
        let machines = [machine("p1", "host-u", "host-u.example", "flow")];

        let plan = plan(&inventories, None, "default", &machines);

        assert!(plan.steps.is_empty(), "{plan:?}");
        assert_eq!(plan.skipped, ["host-c", "host-s", "host-u"]);
    }

    #[test]
    fn plan_replaces_an_out_of_date_profile_and_keeps_a_current_one() {
        let inventories = [record("host-e", true, false), record("host-f", true, false)];
        let machines = [
            machine("p1", "host-e", "host-e.example", "default"),
            machine("p2", "host-f", "host-f.example", "flow"),
        ];

        let plan = plan(&inventories, None, "default", &machines);

        assert_eq!(
            plan.steps,
            [
                Step::Remove {
                    host: "host-f".to_owned(),
                    profile_id: "p2".to_owned(),
                    target: "host-f.example".to_owned(),
                    session: "flow".to_owned(),
                    gone: false,
                },
                add("host-f"),
            ]
        );
    }

    #[test]
    fn plan_removes_stale_profiles_beside_a_current_one() {
        let inventories = [
            record("host-e", true, false),
            record("host-c", false, false),
        ];
        let machines = [
            machine("p1", "host-e", "host-e.example", "default"),
            machine("p2", "host-e", "old-e.example", "default"),
            machine("p3", "host-c", "host-c.example", "default"),
            machine("p4", "host-c", "host-c.example", "flow"),
        ];

        let plan = plan(&inventories, None, "default", &machines);

        let removed: Vec<&str> = plan
            .steps
            .iter()
            .map(|step| match step {
                Step::Remove {
                    profile_id,
                    gone: false,
                    ..
                } => profile_id.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(removed, ["p4", "p2"]);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn plan_never_touches_profiles_of_no_known_host() {
        let inventories = [record("host-e", true, false)];
        let machines = [
            machine("p1", "host-e", "host-e.example", "default"),
            machine("p2", "my-box", "box.example", "work"),
        ];

        assert_eq!(
            plan(&inventories, None, "default", &machines),
            Plan::default()
        );
    }

    #[test]
    fn plan_leaves_a_gone_name_a_live_host_took_over() {
        let inventories = [record("host-d", true, true), record("host-d", true, false)];
        let machines = [machine("p1", "host-d", "host-d.example", "default")];

        assert_eq!(
            plan(&inventories, None, "default", &machines),
            Plan::default()
        );
    }
}
