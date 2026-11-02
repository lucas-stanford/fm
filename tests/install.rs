//! The installers' PATH handling, run offline: only the PATH block of
//! install.sh is executed, against a scratch profile.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn expected_line(dir: &str) -> String {
    format!("export PATH=\"{dir}:$PATH\"")
}

fn run_path_block(profile: &Path, dir: &str, modify: bool) -> std::io::Result<String> {
    let script = std::fs::read_to_string(repo_root().join("install.sh"))?;
    let start = script
        .find("profile_for_shell()")
        .expect("install.sh has no PATH block");
    let end = script
        .find("# --- verification")
        .expect("install.sh has no verification section");
    let block = &script[start..end];

    let harness = format!(
        "set -eu\n\
         DIR='{dir}'\n\
         MODIFY_PATH={}\n\
         say() {{ printf '  %s\\n' \"$*\"; }}\n\
         {block}\n",
        i32::from(modify),
    );
    // Point the block at the scratch profile instead of a real dotfile.
    let harness = harness.replace(
        "profile=\"$(profile_for_shell)\"",
        &format!("profile='{}'", profile.display()),
    );

    let output = Command::new(bash()?).arg("-c").arg(harness).output()?;
    assert!(
        output.status.success(),
        "install.sh PATH block failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Git for Windows ships bash, so these tests also run on Windows.
fn bash() -> std::io::Result<PathBuf> {
    for candidate in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        "/bin/bash",
        "/usr/bin/bash",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no bash available",
    ))
}

#[test]
fn the_shell_installer_adds_path_exactly_once() {
    let Ok(_) = bash() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let temp = tempfile::tempdir().expect("temp");
    let profile = temp.path().join("profile");
    std::fs::write(&profile, "# an existing profile\nexport EDITOR=vim\n").expect("seed");

    let dir = "/opt/fm/bin";
    run_path_block(&profile, dir, true).expect("first run");
    let after_first = std::fs::read_to_string(&profile).expect("read");

    run_path_block(&profile, dir, true).expect("second run");
    let after_second = std::fs::read_to_string(&profile).expect("read");

    assert_eq!(
        after_first, after_second,
        "a second install changed the profile again"
    );
    assert_eq!(
        after_second.matches(&expected_line(dir)).count(),
        1,
        "the PATH line was added twice:\n{after_second}"
    );
}

#[test]
fn the_shell_installer_preserves_the_existing_profile() {
    let Ok(_) = bash() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let temp = tempfile::tempdir().expect("temp");
    let profile = temp.path().join("profile");
    let original = "# an existing profile\nexport EDITOR=vim\nexport PATH=\"$HOME/tools:$PATH\"\n";
    std::fs::write(&profile, original).expect("seed");

    run_path_block(&profile, "/opt/fm/bin", true).expect("run");
    let after = std::fs::read_to_string(&profile).expect("read");

    assert!(
        after.starts_with(original),
        "the installer rewrote what was already there:\n{after}"
    );
    assert!(
        after.contains("$HOME/tools"),
        "an existing PATH entry was lost"
    );
}

#[test]
fn the_shell_installer_honours_the_opt_out() {
    let Ok(_) = bash() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let temp = tempfile::tempdir().expect("temp");
    let profile = temp.path().join("profile");
    std::fs::write(&profile, "# untouched\n").expect("seed");

    let output = run_path_block(&profile, "/opt/fm/bin", false).expect("run");
    assert!(output.contains("not modified"), "{output}");
    assert_eq!(
        std::fs::read_to_string(&profile).expect("read"),
        "# untouched\n",
        "--no-modify-path still edited the profile"
    );
}

#[test]
fn the_powershell_installer_reads_and_writes_the_user_scope_path() {
    // Writing `$env:PATH` back to the user scope would fold the machine PATH
    // into it and duplicate every entry.
    let script = std::fs::read_to_string(repo_root().join("install.ps1")).expect("install.ps1");
    assert!(
        script.contains("GetEnvironmentVariable('PATH', 'User')"),
        "install.ps1 does not read the user-scope PATH"
    );
    assert!(
        script.contains("SetEnvironmentVariable('PATH', $updated, 'User')"),
        "install.ps1 does not write the user-scope PATH"
    );
    let writes_machine_path = script
        .lines()
        .any(|line| line.contains("SetEnvironmentVariable") && line.contains("'Machine'"));
    assert!(
        !writes_machine_path,
        "install.ps1 writes the machine PATH, which needs admin and is not ours to change"
    );
}
