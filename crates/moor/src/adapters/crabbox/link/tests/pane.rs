//! The agent's pane on the Box's shared connection.

use super::*;

#[test]
fn the_agent_pane_rides_the_connection_setup_opened() {
    let lab = lab();
    lab.probe_tick().unwrap();
    let setup = lab.proc.calls().into_iter().find(|c| c.tool == Tool::Ssh);
    let argv = lab.driver().connect_argv(&handle()).unwrap();
    assert_eq!(
        option(&argv, "ControlPath"),
        option(&setup.unwrap().args, "ControlPath")
    );
    assert_eq!(option(&argv, "ControlMaster").as_deref(), Some("auto"));
    lab.ssh.lock().unwrap().reply(argv.get(1..).unwrap());
    assert_eq!(lab.connects(), 1, "the pane opens no connection of its own");
}

#[test]
fn the_agent_pane_shows_its_own_ssh_errors() {
    let lab = lab();
    let argv = lab.driver().connect_argv(&handle()).unwrap();
    assert_eq!(flag(&argv, "-E"), None);
    assert_eq!(option(&argv, "LogLevel"), None);
}

#[test]
fn a_pane_on_a_box_with_no_master_opens_one_that_setup_rides() {
    let lab = lab();
    let argv = lab.driver().connect_argv(&handle()).unwrap();
    lab.ssh.lock().unwrap().reply(argv.get(1..).unwrap());
    lab.probe_tick().unwrap();
    assert_eq!(lab.connects(), 1);
}

#[test]
fn a_batch_over_the_pane_command_names_the_connection_once() {
    let lab = lab();
    let connect = lab.driver().connect_argv(&handle()).unwrap();
    let steps = [BoxStep::Run("true".to_owned())];
    let deadline = Deadline(Duration::from_secs(5));
    let _ = lab.driver().batch(&handle(), &connect, &steps, deadline);
    let mut calls = lab.proc.calls().into_iter().filter(|c| c.tool == Tool::Ssh);
    let call = calls.next_back().unwrap();
    let paths = call.args.iter().filter(|a| a.starts_with("ControlPath="));
    assert_eq!(paths.count(), 2, "ours, then crabbox's ControlPath=none");
}
