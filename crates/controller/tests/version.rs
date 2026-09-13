#[test]
fn both_server_binaries_identify_the_exact_source_used_for_packaging() {
    for binary in [
        env!("CARGO_BIN_EXE_nodeharbor-controller"),
        env!("CARGO_BIN_EXE_nodeharbor-probe"),
    ] {
        let output = std::process::Command::new(binary)
            .arg("--version")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout)
            .contains(option_env!("NODEHARBOR_COMMIT").unwrap_or("development")));
    }
}
