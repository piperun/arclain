use anyhow::{anyhow, bail, Context, Result};
use cap_fs_ext::{ambient_authority, DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions as CapOpenOptions};
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use wirt::{
    package_bytes, read_package, read_package_bytes, ValidatedPackage, MAX_PLUGIN_MANIFEST_BYTES,
    MAX_PLUGIN_WASM_BYTES,
};

const HELP: &str = "Wirt plugin developer command

Usage:
  wirt new <directory>
  wirt build [directory]
  wirt validate <package-or-project>
  wirt package [directory] [--output <path>]
";
const TARGET: &str = "wasm32-wasip2";
const TARGET_INSTALL_COMMAND: &str = "rustup target add wasm32-wasip2";
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn run(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let args = args.into_iter().collect::<Vec<_>>();
    match args.as_slice() {
        [help] if help == "--help" || help == "-h" => {
            print!("{HELP}");
            Ok(())
        }
        [command, directory] if command == "new" => new_project(Path::new(directory)),
        [command] if command == "build" => build_project(Path::new(".")),
        [command, directory] if command == "build" => build_project(Path::new(directory)),
        [command, input] if command == "validate" => validate(Path::new(input)),
        [command, rest @ ..] if command == "package" => {
            let (directory, output) = parse_package_args(rest)?;
            package_project(&directory, output.as_deref())
        }
        [] => bail!("missing command; run `wirt --help`"),
        [command, ..] => bail!(
            "unknown or malformed command {:?}; run `wirt --help`",
            command
        ),
    }
}

fn parse_package_args(args: &[OsString]) -> Result<(PathBuf, Option<PathBuf>)> {
    match args {
        [] => Ok((PathBuf::from("."), None)),
        [directory] if directory != "--output" => Ok((PathBuf::from(directory), None)),
        [output_flag, output] if output_flag == "--output" => {
            Ok((PathBuf::from("."), Some(PathBuf::from(output))))
        }
        [directory, output_flag, output] if output_flag == "--output" => {
            Ok((PathBuf::from(directory), Some(PathBuf::from(output))))
        }
        _ => bail!(
            "malformed package arguments; expected `wirt package [directory] [--output <path>]`"
        ),
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("wirt-cli is under crates/")
        .to_path_buf()
}

fn new_project(destination: &Path) -> Result<()> {
    let destination_dir = prepare_empty_destination(destination)?;
    let repository = open_dir_nofollow(&repository_root(), "could not open repository root")?;
    let sdk_source = repository
        .open_dir_nofollow("wirt-sdk")
        .with_context(|| "wirt-sdk source is missing or a link/reparse point")?;
    let template_source = sdk_source
        .open_dir_nofollow("template")
        .with_context(|| "starter source is missing or a link/reparse point")?;
    copy_directory(&template_source, &destination_dir, true, CopyMode::Template)?;
    destination_dir
        .create_dir("wirt-sdk")
        .with_context(|| "could not create vendored SDK")?;
    let sdk_destination = destination_dir
        .open_dir_nofollow("wirt-sdk")
        .with_context(|| "vendored SDK destination became a link/reparse point")?;
    copy_directory(&sdk_source, &sdk_destination, true, CopyMode::Sdk)?;
    println!("Created Wirt plugin project at {}", destination.display());
    Ok(())
}

fn prepare_empty_destination(destination: &Path) -> Result<Dir> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            reject_link_or_reparse(&metadata, "destination is a link or reparse point")?;
            if !metadata.is_dir() {
                bail!("destination exists and is not a directory");
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(destination).with_context(|| "could not create destination")?;
        }
        Err(error) => return Err(error).with_context(|| "could not inspect destination"),
    }
    let destination = open_dir_nofollow(destination, "destination became a link or reparse point")?;
    if destination
        .entries()
        .with_context(|| "could not inspect destination")?
        .next()
        .transpose()?
        .is_some()
    {
        bail!("destination is not empty");
    }
    Ok(destination)
}

#[derive(Clone, Copy)]
enum CopyMode {
    Template,
    Sdk,
}

fn copy_directory(source: &Dir, destination: &Dir, at_root: bool, mode: CopyMode) -> Result<()> {
    let mut entries = source.entries()?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if should_skip(&name, at_root, mode) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!("copy source contains a link or reparse point");
        }
        if file_type.is_dir() {
            let source_child = source
                .open_dir_nofollow(&name)
                .with_context(|| "copy source directory became a link or reparse point")?;
            destination.create_dir(&name)?;
            let destination_child = destination
                .open_dir_nofollow(&name)
                .with_context(|| "copy destination directory became a link or reparse point")?;
            copy_directory(&source_child, &destination_child, false, mode)?;
        } else if file_type.is_file() {
            copy_file(source, destination, &name)?;
        } else {
            bail!("copy source contains a non-regular entry");
        }
    }
    Ok(())
}

fn should_skip(name: &OsStr, at_root: bool, mode: CopyMode) -> bool {
    match mode {
        CopyMode::Template => false,
        CopyMode::Sdk => {
            (at_root && name == "template")
                || name == "target"
                || name == ".rustc_info.json"
                || name.to_string_lossy().starts_with(".target-")
        }
    }
}

fn copy_file(source: &Dir, destination: &Dir, name: &OsStr) -> Result<()> {
    let mut read_options = CapOpenOptions::new();
    read_options.read(true).follow(FollowSymlinks::No);
    let mut input = source
        .open_with(name, &read_options)
        .with_context(|| "copy source file became a link or reparse point")?;
    let mut write_options = CapOpenOptions::new();
    write_options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut output = destination
        .open_with(name, &write_options)
        .with_context(|| "copy destination file already exists or is a link/reparse point")?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(())
}

fn open_dir_nofollow(path: &Path, context: &str) -> Result<Dir> {
    if path == Path::new(".") || path.file_name().is_none() {
        return Dir::open_ambient_dir(path, ambient_authority())
            .with_context(|| context.to_owned());
    }
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("directory path has no final component"))?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent =
        Dir::open_ambient_dir(parent, ambient_authority()).with_context(|| context.to_owned())?;
    parent
        .open_dir_nofollow(name)
        .with_context(|| context.to_owned())
}

fn reject_link_or_reparse(metadata: &fs::Metadata, message: &str) -> Result<()> {
    if is_link_or_reparse(metadata) {
        bail!("{message}");
    }
    Ok(())
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn build_project(project: &Path) -> Result<()> {
    let project = project_root(project)?;
    let output = cargo(&project)
        .args(["build", "--target", TARGET, "--release"])
        .output()
        .with_context(|| "could not start Cargo")?;
    if !output.status.success() {
        return Err(build_failure(&output));
    }
    println!("Built Wirt component for {TARGET}");
    Ok(())
}

fn cargo(project: &Path) -> Command {
    let mut command = Command::new("cargo");
    command.current_dir(project);
    command
}

fn build_failure(output: &Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}\n{stdout}").to_ascii_lowercase();
    if combined.contains("wasm32-wasip2")
        && (combined.contains("can't find crate for `std`")
            || combined.contains("target may not be installed")
            || combined.contains("target is not installed"))
    {
        anyhow!(
            "the {TARGET} Rust target is not installed; run `{TARGET_INSTALL_COMMAND}` manually"
        )
    } else {
        anyhow!("Cargo failed to build the Wirt component")
    }
}

fn project_root(project: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(project).with_context(|| "project directory is missing")?;
    reject_link_or_reparse(&metadata, "project directory is a link or reparse point")?;
    if !metadata.is_dir() {
        bail!("project path is not a directory");
    }
    let project = fs::canonicalize(project)?;
    for required in ["Cargo.toml", "plugin.toml"] {
        let path = project.join(required);
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("project is missing {required}"))?;
        reject_link_or_reparse(&metadata, "project file is a link or reparse point")?;
        if !metadata.is_file() {
            bail!("project {required} is not a regular file");
        }
    }
    Ok(project)
}

fn component_path(project: &Path) -> Result<PathBuf> {
    let output = cargo(project)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .with_context(|| "could not start Cargo metadata")?;
    if !output.status.success() {
        bail!("Cargo metadata failed for the Wirt project");
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .with_context(|| "Cargo metadata returned invalid JSON")?;
    let manifest_path = fs::canonicalize(project.join("Cargo.toml"))?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or_else(|| anyhow!("Cargo metadata omitted packages"))?;
    let package = packages
        .iter()
        .find(|package| {
            package["manifest_path"]
                .as_str()
                .and_then(|path| fs::canonicalize(path).ok())
                .is_some_and(|path| path == manifest_path)
        })
        .ok_or_else(|| anyhow!("Cargo metadata omitted the project package"))?;
    let targets = package["targets"]
        .as_array()
        .ok_or_else(|| anyhow!("Cargo metadata omitted targets"))?;
    let mut component_targets = targets.iter().filter(|target| {
        target["crate_types"]
            .as_array()
            .is_some_and(|types| types.iter().any(|kind| kind == "cdylib"))
    });
    let target = component_targets
        .next()
        .ok_or_else(|| anyhow!("project has no cdylib component target"))?;
    if component_targets.next().is_some() {
        bail!("project has more than one cdylib component target");
    }
    let name = target["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Cargo metadata target has no name"))?;
    let target_directory = metadata["target_directory"]
        .as_str()
        .ok_or_else(|| anyhow!("Cargo metadata omitted target_directory"))?;
    Ok(Path::new(target_directory)
        .join(TARGET)
        .join("release")
        .join(format!("{name}.wasm")))
}

fn validate(input: &Path) -> Result<()> {
    let package = if input.is_dir() {
        validate_project(input)?.1
    } else {
        if input
            .extension()
            .and_then(OsStr::to_str)
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("wirt"))
        {
            bail!("validation accepts only a Wirt project or .wirt package");
        }
        read_package(input)?
    };
    print_validation(&package);
    Ok(())
}

fn print_validation(package: &ValidatedPackage) {
    println!("Valid Wirt plugin");
    println!("ID: {}", package.manifest.plugin.id);
    println!("Version: {}", package.manifest.plugin.version);
    println!("ABI: {}", package.manifest.wirt.abi);
    println!("SHA-256: {}", package.fingerprint);
}

fn validate_project(project: &Path) -> Result<(Vec<u8>, ValidatedPackage)> {
    let project = project_root(project)?;
    let component = component_path(&project)?;
    let manifest_bytes = read_regular_bounded(
        &project.join("plugin.toml"),
        MAX_PLUGIN_MANIFEST_BYTES,
        "plugin manifest",
    )?;
    let component_bytes = read_regular_bounded(
        &component,
        MAX_PLUGIN_WASM_BYTES,
        "built plugin component; run `wirt build` first",
    )?;
    let bytes = package_bytes(&manifest_bytes, &component_bytes)?;
    let package = read_package_bytes(&bytes)?;
    Ok((bytes, package))
}

fn read_regular_bounded(path: &Path, limit: usize, description: &str) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("missing {description}"))?;
    reject_link_or_reparse(&metadata, "input is a link or reparse point")?;
    if !metadata.is_file() {
        bail!("{description} is not a regular file");
    }
    if metadata.len() > limit as u64 {
        bail!("{description} exceeds its byte limit");
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        bail!("{description} exceeds its byte limit");
    }
    Ok(bytes)
}

fn package_project(project: &Path, output: Option<&Path>) -> Result<()> {
    let project = project_root(project)?;
    let component = component_path(&project)?;
    if !component.is_file() {
        build_project(&project)?;
    }
    let (bytes, package) = validate_project(&project)?;
    let destination = match output {
        Some(output) => output.to_path_buf(),
        None => {
            let version = &package.manifest.plugin.version;
            if version.contains(['/', '\\']) {
                bail!("plugin version is unsafe for a default filename; pass `--output <path>`");
            }
            project.join(format!("{}-{version}.wirt", package.manifest.plugin.id))
        }
    };
    atomic_create(&destination, &bytes)?;
    println!("Created {}", destination.display());
    Ok(())
}

fn atomic_create(destination: &Path, bytes: &[u8]) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => bail!("output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| "could not inspect output path"),
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).with_context(|| "output directory does not exist")?;
    let file_name = destination
        .file_name()
        .ok_or_else(|| anyhow!("output path has no filename"))?;
    let published = parent.join(file_name);
    let mut temporary = None;
    for _ in 0..32 {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = file_name.to_os_string();
        name.push(format!(".tmp-{}-{sequence}", std::process::id()));
        let path = parent.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                    let _ = fs::remove_file(&path);
                    return Err(error.into());
                }
                temporary = Some(path);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let temporary = temporary.ok_or_else(|| anyhow!("could not reserve an output temp file"))?;
    if let Err(error) = fs::hard_link(&temporary, &published) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| "could not publish output atomically");
    }
    fs::remove_file(&temporary).with_context(|| "could not remove output temp link")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{copy_file, open_dir_nofollow, prepare_empty_destination};
    use std::ffi::OsStr;
    use std::fs;

    #[test]
    fn handle_relative_copy_rejects_a_source_replaced_with_a_link() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let outside = temp.path().join("outside.txt");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source.join("plugin.toml"), b"checked bytes").unwrap();
        fs::write(&outside, b"outside bytes").unwrap();

        let source = open_dir_nofollow(&source, "source").unwrap();
        let destination = open_dir_nofollow(&destination, "destination").unwrap();
        fs::remove_file(temp.path().join("source/plugin.toml")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, temp.path().join("source/plugin.toml")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, temp.path().join("source/plugin.toml"))
            .unwrap();

        let error = copy_file(&source, &destination, OsStr::new("plugin.toml")).unwrap_err();
        assert!(error.to_string().contains("link or reparse point"));
        assert!(!temp.path().join("destination/plugin.toml").exists());
    }

    #[test]
    fn prepared_destination_handle_cannot_be_redirected_to_a_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("selected");
        fs::create_dir(&selected).unwrap();
        let selected_handle = prepare_empty_destination(&selected).unwrap();
        let moved = temp.path().join("original");

        #[cfg(unix)]
        {
            fs::rename(&selected, &moved).unwrap();
            fs::create_dir(&selected).unwrap();
            fs::write(selected.join("sentinel.txt"), b"replacement").unwrap();
            selected_handle.create("marker.txt").unwrap();
            assert!(moved.join("marker.txt").is_file());
            assert!(!selected.join("marker.txt").exists());
        }
        #[cfg(windows)]
        {
            assert!(fs::rename(&selected, &moved).is_err());
            selected_handle.create("marker.txt").unwrap();
            assert!(selected.join("marker.txt").is_file());
        }
    }
}
