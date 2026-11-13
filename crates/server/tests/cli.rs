use std::process::Command;

fn check(text: &str) -> std::process::Output {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("config.toml");
    std::fs::write(&p, text).unwrap();
    Command::new(env!("CARGO_BIN_EXE_dcontroller"))
        .arg("--check-config")
        .arg(&p)
        .output()
        .unwrap()
}

#[test]
fn check_config_prints_bind_of_a_valid_config() {
    let out = check("bind = '127.0.0.1:8790'   # comment\nweb_dir = \"/opt/x\"\n");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "127.0.0.1:8790\n");
}

#[test]
fn check_config_rejects_what_the_server_would_refuse() {
    for bad in [
        "bind = \"0.0.0.0:8790\"\nweb_dir = \"/opt/x\"",
        "bind = \"127.0.0.1:99999\"\nweb_dir = \"/opt/x\"",
        "web_dir = \"/opt/x\"\ntypo = 1",
    ] {
        let out = check(bad);
        assert!(!out.status.success(), "accepted: {bad}");
        assert!(out.stdout.is_empty());
    }
}
