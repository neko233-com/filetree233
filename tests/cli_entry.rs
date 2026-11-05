use std::process::Command;

#[test]
fn no_arguments_prints_cli_help() {
    let binary = env!("CARGO_BIN_EXE_filetree233");
    let help = Command::new(binary).output().unwrap();
    assert!(help.status.success());
    let output = String::from_utf8(help.stdout).unwrap();
    for command in ["scan", "tree", "plan", "apply", "serve"] {
        assert!(output.contains(command), "missing {command} in {output}");
    }
}
#[test]
fn gui_command_needs_a_gui_build() {
    let binary = env!("CARGO_BIN_EXE_filetree233");
    // Parsing stops at --help, so this never opens a window even in a gui build.
    let help = Command::new(binary)
        .args(["gui", "--help"])
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "gui --help must be accepted by the parser"
    );
    if !cfg!(feature = "gui") {
        let refused = Command::new(binary).arg("gui").output().unwrap();
        assert!(
            !refused.status.success(),
            "a CLI-only build must not launch a GUI"
        );
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("graphical interface"),
            "the refusal should explain how to get a GUI"
        );
    }
}
