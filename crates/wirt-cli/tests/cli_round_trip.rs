use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wirt"))
}

fn run(args: &[&str]) -> Output {
    command().args(args).output().unwrap()
}

fn run_in(project: &Path, args: &[&str]) -> Output {
    command()
        .current_dir(project)
        .env("CARGO_TARGET_DIR", project.join(".target"))
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn assert_failure(output: Output) -> Output {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn relative_files(root: &Path) -> Vec<PathBuf> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), files);
            } else {
                files.push(entry.path().strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }

    let mut files = Vec::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn new_copies_the_maintained_starter_and_vendored_sdk_verbatim() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("starter");
    assert_success(run(&["new", project.to_str().unwrap()]));

    let expected = [
        "Cargo.toml",
        "plugin.toml",
        "src/lib.rs",
        "wirt-sdk/Cargo.lock",
        "wirt-sdk/Cargo.toml",
        "wirt-sdk/README.md",
        "wirt-sdk/src/lib.rs",
        "wirt-sdk/wit/plugin.wit",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect::<Vec<_>>();
    assert_eq!(relative_files(&project), expected);

    let repository = repo_root();
    for path in ["Cargo.toml", "plugin.toml", "src/lib.rs"] {
        assert_eq!(
            fs::read(project.join(path)).unwrap(),
            fs::read(repository.join("wirt-sdk/template").join(path)).unwrap(),
            "starter file changed while copying: {path}"
        );
    }
    for path in [
        "Cargo.lock",
        "Cargo.toml",
        "README.md",
        "src/lib.rs",
        "wit/plugin.wit",
    ] {
        assert_eq!(
            fs::read(project.join("wirt-sdk").join(path)).unwrap(),
            fs::read(repository.join("wirt-sdk").join(path)).unwrap(),
            "vendored SDK file changed while copying: {path}"
        );
    }
}

#[test]
fn new_refuses_nonempty_destinations_without_touching_them() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("existing");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("sentinel.txt"), b"keep me").unwrap();

    let error = assert_failure(run(&["new", project.to_str().unwrap()]));
    assert!(String::from_utf8_lossy(&error.stderr).contains("not empty"));
    assert_eq!(fs::read(project.join("sentinel.txt")).unwrap(), b"keep me");
    assert_eq!(relative_files(&project), [PathBuf::from("sentinel.txt")]);
}

#[test]
fn new_accepts_the_current_empty_directory() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("current-directory");
    fs::create_dir(&project).unwrap();

    assert_success(
        command()
            .current_dir(&project)
            .args(["new", "."])
            .output()
            .unwrap(),
    );
    assert!(project.join("Cargo.toml").is_file());
    assert!(project.join("wirt-sdk/wit/plugin.wit").is_file());
}

#[test]
fn help_and_argument_errors_pin_the_small_command_surface() {
    let help = assert_success(run(&["--help"]));
    let help = String::from_utf8(help.stdout).unwrap();
    for line in [
        "wirt new <directory>",
        "wirt build [directory]",
        "wirt validate <package-or-project>",
        "wirt package [directory] [--output <path>]",
    ] {
        assert!(help.contains(line), "missing help line: {line}");
    }

    for args in [
        vec![],
        vec!["unknown"],
        vec!["new"],
        vec!["new", "one", "two"],
        vec!["build", "one", "two"],
        vec!["validate"],
        vec!["validate", "one", "two"],
        vec!["package", "one", "--output"],
        vec!["package", "one", "--output", "out", "extra"],
        vec!["package", "--output", "one", "--output", "two"],
    ] {
        let error = assert_failure(run(&args));
        assert!(!error.stderr.is_empty(), "no error for: {args:?}");
    }
}

#[test]
fn starter_round_trip_is_deterministic_and_failures_leave_no_output() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("round-trip");
    assert_success(run(&["new", project.to_str().unwrap()]));
    assert_success(run_in(&project, &["build"]));

    let reported_abi = format!("ABI: {}", wirt::WIRT_ABI_VERSION);
    let project_validation = assert_success(run_in(&project, &["validate", "."]));
    assert!(String::from_utf8_lossy(&project_validation.stdout).contains(&reported_abi));

    assert_success(run_in(&project, &["package"]));
    let default_package = project.join("wirt-starter-0.1.0.wirt");
    assert!(default_package.is_file());
    let package_validation = assert_success(run(&["validate", default_package.to_str().unwrap()]));
    assert!(String::from_utf8_lossy(&package_validation.stdout).contains(&reported_abi));

    let custom_package = project.join("custom-output.wirt");
    assert_success(run_in(
        &project,
        &["package", "--output", "custom-output.wirt"],
    ));
    assert_eq!(
        fs::read(&default_package).unwrap(),
        fs::read(&custom_package).unwrap()
    );

    let manifest_path = project.join("plugin.toml");
    let valid_manifest = fs::read_to_string(&manifest_path).unwrap();
    let stale_manifest = valid_manifest.replace(
        &format!("abi = \"{}\"", wirt::WIRT_ABI_VERSION),
        "abi = \"0.1.0\"",
    );
    assert_ne!(
        stale_manifest, valid_manifest,
        "the starter manifest no longer declares the host ABI"
    );
    fs::write(&manifest_path, &stale_manifest).unwrap();
    let mismatch = assert_failure(run_in(&project, &["validate", "."]));
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("unsupported Wirt ABI"));

    let absent = project.join("must-not-exist.wirt");
    let package_error = assert_failure(run_in(
        &project,
        &["package", "--output", absent.to_str().unwrap()],
    ));
    assert!(String::from_utf8_lossy(&package_error.stderr).contains("unsupported Wirt ABI"));
    assert!(!absent.exists());
    fs::write(&manifest_path, valid_manifest).unwrap();

    fs::create_dir(project.join("wirt-starter-x")).unwrap();
    let escaped = temp.path().join("escaped.wirt");
    let valid_manifest = fs::read_to_string(&manifest_path).unwrap();
    fs::write(
        &manifest_path,
        valid_manifest.replace("version = \"0.1.0\"", "version = \"x/../../escaped\""),
    )
    .unwrap();
    let unsafe_default = assert_failure(run_in(&project, &["package"]));
    assert!(String::from_utf8_lossy(&unsafe_default.stderr).contains("--output"));
    assert!(
        !escaped.exists(),
        "default package path escaped the project"
    );
    fs::write(&manifest_path, valid_manifest).unwrap();

    let occupied = project.join("occupied.wirt");
    fs::write(&occupied, b"keep me").unwrap();
    assert_failure(run_in(
        &project,
        &["package", "--output", occupied.to_str().unwrap()],
    ));
    assert_eq!(fs::read(occupied).unwrap(), b"keep me");

    let invalid_package = project.join("invalid.wirt");
    fs::write(&invalid_package, b"not a package").unwrap();
    let invalid = assert_failure(run(&["validate", invalid_package.to_str().unwrap()]));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("Invalid Wirt package"));
}

#[test]
fn missing_target_reports_manual_install_without_invoking_rustup() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    assert_success(run(&["new", project.to_str().unwrap()]));
    let fake_bin = temp.path().join("bin");
    fs::create_dir(&fake_bin).unwrap();
    let source = temp.path().join("fake-tool.rs");
    fs::write(
        &source,
        r#"use std::{env, fs, process};
fn main() {
    let tool = env::args().next().unwrap();
    if tool.contains("rustup") {
        fs::write(env::var_os("FAKE_RUSTUP_LOG").unwrap(), b"invoked").unwrap();
        process::exit(99);
    }
    let args = env::args().skip(1).collect::<Vec<_>>().join(" ");
    fs::write(env::var_os("FAKE_CARGO_LOG").unwrap(), args).unwrap();
    eprintln!("error[E0463]: can't find crate for `std`");
    eprintln!("the wasm32-wasip2 target may not be installed");
    process::exit(1);
}
"#,
    )
    .unwrap();
    let suffix = std::env::consts::EXE_SUFFIX;
    let cargo = fake_bin.join(format!("cargo{suffix}"));
    assert_success(
        Command::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(&cargo)
            .output()
            .unwrap(),
    );
    fs::copy(&cargo, fake_bin.join(format!("rustup{suffix}"))).unwrap();

    let cargo_log = temp.path().join("cargo.log");
    let rustup_log = temp.path().join("rustup.log");
    let path = std::env::join_paths(std::iter::once(fake_bin.clone()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let output = command()
        .current_dir(&project)
        .env("PATH", path)
        .env("FAKE_CARGO_LOG", &cargo_log)
        .env("FAKE_RUSTUP_LOG", &rustup_log)
        .args(["build"])
        .output()
        .unwrap();

    let error = assert_failure(output);
    assert!(String::from_utf8_lossy(&error.stderr).contains("rustup target add wasm32-wasip2"));
    assert_eq!(
        fs::read_to_string(cargo_log).unwrap(),
        "build --target wasm32-wasip2 --release"
    );
    assert!(!rustup_log.exists(), "wirt invoked rustup");
}

#[test]
fn new_rejects_a_reparse_destination_without_touching_its_target() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let link = temp.path().join("link");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("sentinel.txt"), b"keep me").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&target, &link).unwrap();

    let error = assert_failure(run(&["new", link.to_str().unwrap()]));
    assert!(String::from_utf8_lossy(&error.stderr).contains("link"));
    assert_eq!(fs::read(target.join("sentinel.txt")).unwrap(), b"keep me");
}
