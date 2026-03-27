#[cfg(target_os = "macos")]
#[test]
fn axisd_binary_embeds_executable_rpath_for_ghostty_runtime() {
    let axisd_bin = env!("CARGO_BIN_EXE_axisd");
    let output = std::process::Command::new("otool")
        .args(["-l", axisd_bin])
        .output()
        .expect("otool should inspect axisd");

    assert!(
        output.status.success(),
        "otool -l {} failed: {}",
        axisd_bin,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("LC_RPATH"),
        "expected axisd to embed an LC_RPATH load command, got:\n{stdout}"
    );
    assert!(
        stdout.contains("@executable_path"),
        "expected axisd to embed @executable_path in its rpath, got:\n{stdout}"
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn axisd_binary_embeds_executable_rpath_for_ghostty_runtime() {}
