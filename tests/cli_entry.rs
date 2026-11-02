use std::process::Command;

#[test]
fn no_arguments_prints_cli_help_and_gui_is_not_a_command() {
    let binary = env!("CARGO_BIN_EXE_filetree233");
    let help = Command::new(binary).output().unwrap();
    assert!(help.status.success());
    let output = String::from_utf8(help.stdout).unwrap();
    for command in ["scan", "tree", "plan", "apply", "serve"] {
        assert!(output.contains(command), "missing {command} in {output}");
    }
    let gui = Command::new(binary).arg("gui").output().unwrap();
    assert!(!gui.status.success());
}
