use std::{
    env,
    ffi::OsStr,
    fmt::Write as _,
    fs,
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};

use serde_json::json;
use sha2::{Digest, Sha256};
use walkdir::{DirEntry, WalkDir};

const UF2_MAGIC0: u32 = 0x0a32_4655;
const UF2_MAGIC1: u32 = 0x9e5d_5157;
const UF2_MAGIC_END: u32 = 0x0ab1_6f30;
const UF2_FLAG_FAMILY_ID: u32 = 0x0000_2000;
const UF2_PAYLOAD_SIZE: usize = 256;
const APPLICATION_START: u32 = 0x0002_6000;
const APPLICATION_END: u32 = 0x000d_c000;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() {
    if let Err(error) = run() {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let root = repo_root()?;
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("check") if args.next().is_none() => check(&root),
        Some("dist") if args.next().is_none() => dist(&root),
        Some("inspect-uf2") => {
            let path = args.next().ok_or("inspect-uf2 requires a file")?;
            if args.next().is_some() {
                return Err("inspect-uf2 accepts exactly one file".into());
            }
            let info = inspect_uf2(&root.join(path), None)?;
            println!(
                "{}: {} blocks, {}-{}, family {}",
                info.path.display(),
                info.blocks,
                hex(info.start),
                hex(info.end),
                info.family.map(hex).unwrap_or_else(|| "(none)".to_owned())
            );
            Ok(())
        }
        _ => Err("usage: cargo run -p xtask -- <check|dist|inspect-uf2 FILE>".into()),
    }
}

fn repo_root() -> Result<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("xtask must live at crates/xtask")?
        .canonicalize()?)
}

fn check(root: &Path) -> Result<()> {
    validate_submodule(root)?;
    validate_local_paths(root)?;

    run_command(
        root,
        "cargo",
        &["check", "--workspace", "--all-targets"],
        &[],
    )?;
    run_command(root, "cargo", &["test", "--workspace"], &[])?;
    Ok(())
}

fn validate_submodule(root: &Path) -> Result<String> {
    let line = git(root, &["submodule", "status", "--", "dependencies/rmk"])?;
    if !line.starts_with(' ') {
        return Err(format!(
            "dependencies/rmk is uninitialized, modified, or on the wrong commit: {line}"
        )
        .into());
    }

    let expected = git(root, &["rev-parse", "HEAD:dependencies/rmk"])?;
    let actual = git(root, &["-C", "dependencies/rmk", "rev-parse", "HEAD"])?;
    if actual != expected {
        return Err(format!("RMK checkout {actual} does not match gitlink {expected}").into());
    }
    let status = git(root, &["-C", "dependencies/rmk", "status", "--porcelain"])?;
    if !status.is_empty() {
        return Err("dependencies/rmk has local changes".into());
    }
    Ok(actual)
}

fn validate_local_paths(root: &Path) -> Result<()> {
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !ignored_tree(entry, root))
    {
        let entry = entry?;
        if entry.file_type().is_file() && entry.file_name() == OsStr::new("Cargo.toml") {
            validate_manifest_paths(root, entry.path())?;
        }
    }
    Ok(())
}

fn ignored_tree(entry: &DirEntry, root: &Path) -> bool {
    if entry.path() == root {
        return true;
    }
    if !entry.file_type().is_dir() {
        return true;
    }
    matches!(
        entry.file_name().to_str(),
        Some(".git" | ".worktrees" | "target" | "node_modules")
    ) || entry.path() == root.join("dependencies/rmk")
}

fn validate_manifest_paths(root: &Path, manifest: &Path) -> Result<()> {
    let contents = fs::read_to_string(manifest)?;
    let value: toml::Value = toml::from_str(&contents)?;
    let mut paths = Vec::new();
    collect_path_values(&value, &mut paths);
    let manifest_dir = manifest.parent().ok_or("manifest has no parent")?;
    for relative in paths {
        let resolved = normalize(&manifest_dir.join(relative));
        if !resolved.starts_with(root) {
            return Err(format!(
                "{} has a path outside the repository: {relative}",
                manifest.strip_prefix(root).unwrap_or(manifest).display()
            )
            .into());
        }
    }
    Ok(())
}

fn collect_path_values<'a>(value: &'a toml::Value, paths: &mut Vec<&'a str>) {
    match value {
        toml::Value::Table(table) => {
            if let Some(path) = table.get("path").and_then(toml::Value::as_str) {
                paths.push(path);
            }
            for child in table.values() {
                collect_path_values(child, paths);
            }
        }
        toml::Value::Array(array) => {
            for child in array {
                collect_path_values(child, paths);
            }
        }
        _ => {}
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn dist(root: &Path) -> Result<()> {
    let rmk_commit = validate_submodule(root)?;
    let dirty = !git(root, &["status", "--porcelain", "--untracked-files=normal"])?.is_empty();
    if dirty && env::var("GLOVE80_ALLOW_DIRTY").as_deref() != Ok("1") {
        return Err(
            "release bundles require a clean repository (set GLOVE80_ALLOW_DIRTY=1 only for local validation)"
                .into(),
        );
    }

    let version = toml_value(
        root.join("crates/glove80-rmk/Cargo.toml"),
        &["package", "version"],
    )?;
    let rust_toolchain = toml_value(root.join("rust-toolchain.toml"), &["toolchain", "channel"])?;
    let source_commit = git(root, &["rev-parse", "HEAD"])?;
    let rmk_version = git(
        root,
        &[
            "-C",
            "dependencies/rmk",
            "describe",
            "--tags",
            "--always",
            "--dirty",
        ],
    )?;
    let config_commit =
        env::var("GLOVE80_CONFIG_GIT_COMMIT").unwrap_or_else(|_| "standalone".to_owned());
    let config_dirty = env::var("GLOVE80_CONFIG_GIT_DIRTY").unwrap_or_else(|_| "false".to_owned());

    let firmware_dir = root.join("crates/glove80-rmk");
    for binary in ["glove80_lh", "glove80_rh"] {
        run_command(
            &firmware_dir,
            "cargo",
            &["build", "--release", "--bin", binary],
            &[("GLOVE80_RMK_GIT_VERSION", &rmk_version)],
        )?;
    }

    let target = firmware_dir.join("target/thumbv7em-none-eabihf/release");
    let dist = root.join("dist");
    fs::create_dir_all(&dist)?;
    let halves = [
        Half::new("left", "lh", "glove80_lh", 0x9807_b007),
        Half::new("right", "rh", "glove80_rh", 0x9808_b007),
    ];
    for half in &halves {
        let base = format!("glove80-rmk-{version}-{}", half.suffix);
        let elf = dist.join(format!("{base}.elf"));
        fs::copy(target.join(half.binary), &elf)?;
        set_readable_permissions(&elf)?;
        let elf_bytes = fs::read(&elf)?;
        let segments = load_elf_segments(&elf_bytes)?;
        let uf2 = encode_uf2(&segments, half.family)?;
        fs::write(dist.join(format!("{base}.uf2")), uf2)?;
    }

    package_release(
        &dist,
        &version,
        &source_commit,
        dirty,
        &config_commit,
        config_dirty == "true",
        &rmk_commit,
        &rmk_version,
        &rust_toolchain,
        &halves,
    )
}

fn toml_value(path: PathBuf, keys: &[&str]) -> Result<String> {
    let contents = fs::read_to_string(&path)?;
    let document = toml::from_str::<toml::Value>(&contents)?;
    let mut value = &document;
    for key in keys {
        value = value
            .get(*key)
            .ok_or_else(|| format!("{} has no {}", path.display(), keys.join(".")))?;
    }
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} {} is not a string", path.display(), keys.join(".")).into())
}

#[cfg(unix)]
fn set_readable_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_readable_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[derive(Clone, Copy)]
struct Half {
    name: &'static str,
    suffix: &'static str,
    binary: &'static str,
    family: u32,
}

impl Half {
    const fn new(
        name: &'static str,
        suffix: &'static str,
        binary: &'static str,
        family: u32,
    ) -> Self {
        Self {
            name,
            suffix,
            binary,
            family,
        }
    }
}

struct Segment<'a> {
    address: u32,
    data: &'a [u8],
}

fn load_elf_segments(elf: &[u8]) -> Result<Vec<Segment<'_>>> {
    if read_u32(elf, 0)? != 0x464c_457f {
        return Err("not an ELF file".into());
    }
    if elf.get(4) != Some(&1) || elf.get(5) != Some(&1) {
        return Err("expected ELF32 little-endian".into());
    }
    let header_offset = read_u32(elf, 28)? as usize;
    let entry_size = read_u16(elf, 42)? as usize;
    let entry_count = read_u16(elf, 44)? as usize;
    let mut segments = Vec::new();
    for index in 0..entry_count {
        let offset = header_offset
            .checked_add(index.checked_mul(entry_size).ok_or("ELF header overflow")?)
            .ok_or("ELF header overflow")?;
        let kind = read_u32(elf, offset)?;
        let file_offset = read_u32(elf, offset + 4)? as usize;
        let address = read_u32(elf, offset + 12)?;
        let file_size = read_u32(elf, offset + 16)? as usize;
        if kind == 1 && file_size > 0 {
            let end = file_offset
                .checked_add(file_size)
                .ok_or("ELF segment overflow")?;
            let data = elf
                .get(file_offset..end)
                .ok_or("ELF segment is out of bounds")?;
            segments.push(Segment { address, data });
        }
    }
    segments.sort_by_key(|segment| segment.address);
    if segments.is_empty() {
        return Err("no PT_LOAD segments with file data".into());
    }
    Ok(segments)
}

fn encode_uf2(segments: &[Segment<'_>], family: u32) -> Result<Vec<u8>> {
    let start = segments.first().ok_or("no ELF segments")?.address;
    let end = segments.iter().try_fold(start, |end, segment| {
        let segment_end = segment
            .address
            .checked_add(u32::try_from(segment.data.len())?)
            .ok_or("firmware address overflow")?;
        Ok::<u32, Box<dyn std::error::Error>>(end.max(segment_end))
    })?;
    let image_len = usize::try_from(end - start)?;
    let mut image = vec![0xff; image_len];
    for segment in segments {
        let offset = usize::try_from(segment.address - start)?;
        let end = offset + segment.data.len();
        image
            .get_mut(offset..end)
            .ok_or("ELF segment falls outside flattened image")?
            .copy_from_slice(segment.data);
    }

    let blocks = image.len().div_ceil(UF2_PAYLOAD_SIZE);
    let mut uf2 = vec![0; blocks * 512];
    for block in 0..blocks {
        let output = &mut uf2[block * 512..(block + 1) * 512];
        write_u32(output, 0, UF2_MAGIC0)?;
        write_u32(output, 4, UF2_MAGIC1)?;
        write_u32(output, 8, UF2_FLAG_FAMILY_ID)?;
        write_u32(output, 12, start + u32::try_from(block * UF2_PAYLOAD_SIZE)?)?;
        write_u32(output, 16, UF2_PAYLOAD_SIZE as u32)?;
        write_u32(output, 20, u32::try_from(block)?)?;
        write_u32(output, 24, u32::try_from(blocks)?)?;
        write_u32(output, 28, family)?;
        let chunk_start = block * UF2_PAYLOAD_SIZE;
        let chunk_end = (chunk_start + UF2_PAYLOAD_SIZE).min(image.len());
        output[32..32 + chunk_end - chunk_start].copy_from_slice(&image[chunk_start..chunk_end]);
        write_u32(output, 508, UF2_MAGIC_END)?;
    }
    Ok(uf2)
}

struct Uf2Info {
    path: PathBuf,
    blocks: usize,
    start: u32,
    end: u32,
    family: Option<u32>,
}

fn inspect_uf2(path: &Path, expected_family: Option<u32>) -> Result<Uf2Info> {
    let data = fs::read(path)?;
    if data.is_empty() || data.len() % 512 != 0 {
        return Err(format!("{} is not a non-empty UF2 block stream", path.display()).into());
    }
    let blocks = data.len() / 512;
    let mut start = u32::MAX;
    let mut end = 0;
    let mut family = None;
    for block in 0..blocks {
        let offset = block * 512;
        if read_u32(&data, offset)? != UF2_MAGIC0
            || read_u32(&data, offset + 4)? != UF2_MAGIC1
            || read_u32(&data, offset + 508)? != UF2_MAGIC_END
        {
            return Err(
                format!("{} has invalid UF2 magic in block {block}", path.display()).into(),
            );
        }
        let flags = read_u32(&data, offset + 8)?;
        let address = read_u32(&data, offset + 12)?;
        let payload_size = read_u32(&data, offset + 16)?;
        if payload_size > 476 {
            return Err(format!("{} has an oversized UF2 payload", path.display()).into());
        }
        if read_u32(&data, offset + 20)? != u32::try_from(block)?
            || read_u32(&data, offset + 24)? != u32::try_from(blocks)?
        {
            return Err(format!(
                "{} has inconsistent UF2 block numbering at block {block}",
                path.display()
            )
            .into());
        }
        if flags & UF2_FLAG_FAMILY_ID != 0 {
            let current = read_u32(&data, offset + 28)?;
            if family.is_some_and(|previous| previous != current) {
                return Err(format!("{} contains multiple family IDs", path.display()).into());
            }
            family = Some(current);
        }
        start = start.min(address);
        end = end.max(
            address
                .checked_add(payload_size)
                .ok_or("UF2 address overflow")?,
        );
    }
    if let Some(expected) = expected_family
        && family != Some(expected)
    {
        return Err(format!(
            "{} has family {}, expected {}",
            path.display(),
            family.map(hex).unwrap_or_else(|| "(none)".to_owned()),
            hex(expected)
        )
        .into());
    }
    if expected_family.is_some() && (start < APPLICATION_START || end > APPLICATION_END) {
        return Err(format!(
            "{} range {}-{} is outside {}-{}",
            path.display(),
            hex(start),
            hex(end),
            hex(APPLICATION_START),
            hex(APPLICATION_END)
        )
        .into());
    }
    Ok(Uf2Info {
        path: path.to_owned(),
        blocks,
        start,
        end,
        family,
    })
}

#[allow(clippy::too_many_arguments)]
fn package_release(
    dist: &Path,
    version: &str,
    source_commit: &str,
    dirty: bool,
    config_commit: &str,
    config_dirty: bool,
    rmk_commit: &str,
    rmk_version: &str,
    rust_toolchain: &str,
    halves: &[Half],
) -> Result<()> {
    let mut artifacts = Vec::new();
    let mut checksums = String::new();
    for half in halves {
        let base = format!("glove80-rmk-{version}-{}", half.suffix);
        let uf2_name = format!("{base}.uf2");
        let elf_name = format!("{base}.elf");
        let uf2_path = dist.join(&uf2_name);
        let elf_path = dist.join(&elf_name);
        let info = inspect_uf2(&uf2_path, Some(half.family))?;
        let uf2_hash = sha256(&uf2_path)?;
        let elf_hash = sha256(&elf_path)?;
        artifacts.push(json!({
            "half": half.name,
            "target": "thumbv7em-none-eabihf",
            "uf2": {
                "file": uf2_name,
                "sha256": uf2_hash,
                "blocks": info.blocks,
                "familyId": hex(half.family),
                "addressStart": hex(info.start),
                "addressEnd": hex(info.end),
            },
            "elf": { "file": elf_name, "sha256": elf_hash },
        }));
        writeln!(checksums, "{uf2_hash}  {uf2_name}")?;
        writeln!(checksums, "{elf_hash}  {elf_name}")?;
        println!(
            "{}: {}-{}, {}, {}",
            half.name,
            hex(info.start),
            hex(info.end),
            hex(half.family),
            uf2_hash
        );
    }

    let configuration = if config_commit == "standalone" {
        serde_json::Value::Null
    } else {
        json!({ "commit": config_commit, "dirty": config_dirty })
    };
    let manifest = json!({
        "schemaVersion": 1,
        "project": "glove80-rmk",
        "version": version,
        "source": { "commit": source_commit, "dirty": dirty },
        "configuration": configuration,
        "rmk": { "commit": rmk_commit, "version": rmk_version },
        "rustToolchain": rust_toolchain,
        "applicationRange": { "start": hex(APPLICATION_START), "end": hex(APPLICATION_END) },
        "artifacts": artifacts,
    });
    fs::write(
        dist.join("manifest.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    fs::write(dist.join("SHA256SUMS"), checksums)?;
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let digest = Sha256::digest(fs::read(path)?);
    Ok(format!("{digest:x}"))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let value: [u8; 2] = bytes
        .get(offset..offset + 2)
        .ok_or("unexpected end of binary data")?
        .try_into()?;
    Ok(u16::from_le_bytes(value))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or("unexpected end of binary data")?
        .try_into()?;
    Ok(u32::from_le_bytes(value))
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) -> Result<()> {
    bytes
        .get_mut(offset..offset + 4)
        .ok_or("unexpected end of binary data")?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn hex(value: u32) -> String {
    format!("0x{value:x}")
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = command_output(root, "git", args, &[])?;
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn command_output(
    directory: &Path,
    program: &str,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<Output> {
    let mut command = Command::new(program);
    command.current_dir(directory).args(args);
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = command.output()?;
    if !output.status.success() {
        io::stderr().write_all(&output.stdout)?;
        io::stderr().write_all(&output.stderr)?;
        return Err(format!("{program} {} failed with {}", args.join(" "), output.status).into());
    }
    Ok(output)
}

fn run_command(
    directory: &Path,
    program: &str,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<()> {
    let mut command = Command::new(program);
    command.current_dir(directory).args(args);
    for (name, value) in environment {
        command.env(name, value);
    }
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{program} {} failed with {status}", args.join(" ")).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uf2_round_trip_preserves_range_and_family() {
        let segment = Segment {
            address: APPLICATION_START,
            data: &[1, 2, 3, 4],
        };
        let bytes = encode_uf2(&[segment], 0x9807_b007).unwrap();
        let path = env::temp_dir().join(format!("glove80-xtask-{}.uf2", std::process::id()));
        fs::write(&path, bytes).unwrap();
        let info = inspect_uf2(&path, Some(0x9807_b007)).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(info.blocks, 1);
        assert_eq!(info.start, APPLICATION_START);
        assert_eq!(info.end, APPLICATION_START + UF2_PAYLOAD_SIZE as u32);
    }

    #[test]
    fn sha256_matches_reference_vector() {
        let path = env::temp_dir().join(format!("glove80-sha-{}", std::process::id()));
        fs::write(&path, b"abc").unwrap();
        let hash = sha256(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
