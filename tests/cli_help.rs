use assert_cmd::Command;

#[test]
fn help_lists_the_operator_commands_and_hides_the_plumbing() {
    let out = Command::cargo_bin("flow")
        .expect("flow binary is built")
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(out).unwrap();

    let commands: Vec<&str> = help
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        commands,
        [
            "init",
            "status",
            "task",
            "dispatch",
            "run",
            "inventory",
            "sessions",
            "follow",
            "unfollow",
            "steer",
            "up",
            "down",
            "help"
        ],
        "{help}"
    );
    for hidden in ["guard", "watch", "wake", "hook"] {
        assert!(!help.contains(hidden), "{hidden}: {help}");
    }
}

#[test]
fn hidden_commands_still_run() {
    let out = Command::cargo_bin("flow")
        .expect("flow binary is built")
        .args(["wake", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8(out).unwrap().contains("drain"));
}
