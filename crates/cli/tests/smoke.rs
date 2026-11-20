//! CLI smoke: the binary starts and prints help.

#[test]
fn help_lists_core_commands() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mydb"))
        .arg("--help")
        .output()
        .expect("spawn mydb");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for cmd in [
        "conn", "ls", "query", "dump", "restore", "transfer", "diff", "verify",
    ] {
        assert!(stdout.contains(cmd), "help missing {cmd}: {stdout}");
    }
}
