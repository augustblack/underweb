use std::collections::BTreeMap;

use anyhow::{Context, Result};
use landlock::{
    ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
    path_beneath_rules,
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use crate::{Policy, system_read_paths};

pub fn lockdown(policy: &Policy) -> Result<()> {
    filesystem(policy)?;
    syscalls()?;
    Ok(())
}

fn filesystem(policy: &Policy) -> Result<()> {
    let abi = ABI::V3;
    let mut read = system_read_paths();
    read.extend(policy.read_only.iter().cloned());
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))?
        .create()?
        .add_rules(path_beneath_rules(&read, AccessFs::from_read(abi)))?
        .add_rules(path_beneath_rules(
            &policy.read_write,
            AccessFs::from_all(abi),
        ))?
        .add_rules(path_beneath_rules(
            ["/dev/null", "/dev/urandom"],
            AccessFs::from_read(abi),
        ))?
        .restrict_self()
        .context("landlock")?;
    if status.ruleset == RulesetStatus::NotEnforced {
        anyhow::bail!("this kernel has no Landlock support");
    }
    Ok(())
}

fn arch() -> TargetArch {
    if cfg!(target_arch = "aarch64") {
        TargetArch::aarch64
    } else {
        TargetArch::x86_64
    }
}

fn syscalls() -> Result<()> {
    let eq = |arg, op, v| SeccompCondition::new(arg, SeccompCmpArgLen::Qword, op, v);
    let mut deny: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    // no new programs
    deny.insert(libc::SYS_execve, vec![]);
    deny.insert(libc::SYS_execveat, vec![]);
    // only Unix sockets (socketpair for internal use); no IP, no raw sockets
    deny.insert(
        libc::SYS_socket,
        vec![SeccompRule::new(vec![eq(
            0,
            SeccompCmpOp::Ne,
            libc::AF_UNIX as u64,
        )?])?],
    );
    // threads yes, new processes no
    deny.insert(
        libc::SYS_clone,
        vec![SeccompRule::new(vec![eq(
            0,
            SeccompCmpOp::MaskedEq(libc::CLONE_THREAD as u64),
            0,
        )?])?],
    );
    #[cfg(target_arch = "x86_64")]
    {
        deny.insert(libc::SYS_fork, vec![]);
        deny.insert(libc::SYS_vfork, vec![]);
    }
    deny.insert(libc::SYS_ptrace, vec![]);

    let main: BpfProgram = SeccompFilter::new(
        deny,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch(),
    )?
    .try_into()?;
    // clone3 can't be filtered by flags; ENOSYS makes libc fall back to clone
    let clone3: BpfProgram = SeccompFilter::new(
        BTreeMap::from([(libc::SYS_clone3, vec![])]),
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
        arch(),
    )?
    .try_into()?;
    seccompiler::apply_filter(&clone3)?;
    seccompiler::apply_filter(&main)?;
    Ok(())
}
