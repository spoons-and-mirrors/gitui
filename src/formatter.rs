use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, bail};

use crate::{
    filesystem,
    git::CommandOutput,
    process::{self, Limits},
};

const FORMATTER_OUTPUT_LIMIT: usize = 1024 * 1024;
const FORMATTER_TIMEOUT: Duration = Duration::from_secs(2 * 60);

#[derive(Debug, Clone)]
pub(crate) struct FormatCommand {
    pub(crate) label: &'static str,
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
}

#[derive(Clone, Copy)]
struct FormatterSpec {
    label: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    project_local: bool,
}

const PRETTIER: FormatterSpec = FormatterSpec {
    label: "Prettier",
    program: "prettier",
    args: &["--write"],
    project_local: true,
};
const BIOME: FormatterSpec = FormatterSpec {
    label: "Biome",
    program: "biome",
    args: &["format", "--write"],
    project_local: true,
};
const DENO: FormatterSpec = FormatterSpec {
    label: "Deno",
    program: "deno",
    args: &["fmt"],
    project_local: false,
};
const RUSTFMT: FormatterSpec = FormatterSpec {
    label: "rustfmt",
    program: "rustfmt",
    args: &[],
    project_local: false,
};
const GOFMT: FormatterSpec = FormatterSpec {
    label: "gofmt",
    program: "gofmt",
    args: &["-w"],
    project_local: false,
};
const RUFF: FormatterSpec = FormatterSpec {
    label: "Ruff",
    program: "ruff",
    args: &["format"],
    project_local: false,
};
const BLACK: FormatterSpec = FormatterSpec {
    label: "Black",
    program: "black",
    args: &["--quiet"],
    project_local: false,
};
const CLANG_FORMAT: FormatterSpec = FormatterSpec {
    label: "clang-format",
    program: "clang-format",
    args: &["-i"],
    project_local: false,
};
const SHFMT: FormatterSpec = FormatterSpec {
    label: "shfmt",
    program: "shfmt",
    args: &["-w"],
    project_local: false,
};
const STYLUA: FormatterSpec = FormatterSpec {
    label: "StyLua",
    program: "stylua",
    args: &[],
    project_local: false,
};
const TERRAFORM: FormatterSpec = FormatterSpec {
    label: "Terraform",
    program: "terraform",
    args: &["fmt"],
    project_local: false,
};
const TAPLO: FormatterSpec = FormatterSpec {
    label: "Taplo",
    program: "taplo",
    args: &["format"],
    project_local: false,
};
const DPRINT: FormatterSpec = FormatterSpec {
    label: "dprint",
    program: "dprint",
    args: &["fmt"],
    project_local: false,
};
const ZIG: FormatterSpec = FormatterSpec {
    label: "zig fmt",
    program: "zig",
    args: &["fmt"],
    project_local: false,
};
const MIX: FormatterSpec = FormatterSpec {
    label: "mix format",
    program: "mix",
    args: &["format"],
    project_local: false,
};
const NIXFMT: FormatterSpec = FormatterSpec {
    label: "nixfmt",
    program: "nixfmt",
    args: &[],
    project_local: false,
};

pub(crate) fn detect(root: &Path, relative: &Path) -> Result<FormatCommand> {
    let specs = formatter_specs(relative);
    if specs.is_empty() {
        let kind = relative
            .extension()
            .and_then(|extension| extension.to_str())
            .map_or_else(
                || "this file type".to_owned(),
                |extension| format!(".{extension} files"),
            );
        bail!("No known formatter for {kind}");
    }

    for spec in specs.iter().filter(|spec| spec.project_local) {
        if let Some(program) = project_program(root, relative, spec.program) {
            return Ok(command(root, relative, *spec, program));
        }
    }
    for spec in &specs {
        if let Some(program) = path_program(spec.program) {
            return Ok(command(root, relative, *spec, program));
        }
    }

    let names = specs
        .iter()
        .map(|spec| spec.program)
        .collect::<Vec<_>>()
        .join(", ");
    bail!("Formatter unavailable; install one of: {names}")
}

pub(crate) fn run(root: &Path, relative: &str, command: &FormatCommand) -> Result<CommandOutput> {
    let file = filesystem::safe_regular_file(root, relative)?;
    let output = process::run(
        Command::new(&command.program)
            .args(&command.args)
            .arg(file)
            .current_dir(root),
        Limits::new(
            FORMATTER_OUTPUT_LIMIT,
            FORMATTER_OUTPUT_LIMIT,
            FORMATTER_TIMEOUT,
        ),
    )
    .with_context(|| format!("Could not run {}", command.label))?;

    let mut stderr = output_text(output.stderr, output.stderr_truncated, "stderr");
    if output.timed_out {
        stderr.push_str("\n[formatter timed out]");
    }
    Ok(CommandOutput {
        stdout: output_text(output.stdout, output.stdout_truncated, "stdout"),
        stderr,
        success: output.status.success() && !output.timed_out,
        exit_code: output.status.code(),
    })
}

fn output_text(bytes: Vec<u8>, truncated: bool, stream: &str) -> String {
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        text.push_str(&format!("\n[{stream} truncated]"));
    }
    text
}

fn formatter_specs(path: &Path) -> Vec<FormatterSpec> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if matches!(name.as_str(), ".prettierrc" | ".eslintrc" | ".babelrc") {
        return vec![PRETTIER, BIOME, DENO];
    }

    match extension.as_str() {
        "rs" => vec![RUSTFMT],
        "go" => vec![GOFMT],
        "py" | "pyi" => vec![RUFF, BLACK],
        "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "m" | "mm" => {
            vec![CLANG_FORMAT]
        }
        "sh" | "bash" | "zsh" => vec![SHFMT],
        "lua" => vec![STYLUA],
        "tf" | "tfvars" => vec![TERRAFORM],
        "toml" => vec![TAPLO, DPRINT],
        "zig" | "zon" => vec![ZIG],
        "ex" | "exs" => vec![MIX],
        "nix" => vec![NIXFMT],
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" | "json" | "jsonc" => {
            vec![PRETTIER, BIOME, DENO]
        }
        "md" | "markdown" => vec![PRETTIER, DENO],
        "css" => vec![PRETTIER, BIOME],
        "scss" | "less" | "html" | "htm" | "vue" | "svelte" | "astro" | "graphql" | "gql"
        | "yaml" | "yml" => vec![PRETTIER],
        _ => Vec::new(),
    }
}

fn command(root: &Path, relative: &Path, spec: FormatterSpec, program: PathBuf) -> FormatCommand {
    let mut args: Vec<String> = spec.args.iter().map(|arg| (*arg).to_owned()).collect();
    if spec.program == "rustfmt" {
        args.extend(["--edition".to_owned(), rust_edition(root, relative)]);
    }
    FormatCommand {
        label: spec.label,
        program,
        args,
    }
}

fn rust_edition(root: &Path, relative: &Path) -> String {
    let mut directory = root.join(relative).parent().map(Path::to_path_buf);
    while let Some(current) = directory {
        if !current.starts_with(root) {
            break;
        }
        if let Ok(manifest) = fs::read_to_string(current.join("Cargo.toml")) {
            if let Some(edition) = manifest_edition(&manifest) {
                return edition;
            }
            if manifest.lines().any(|line| line.trim() == "[package]")
                && !manifest
                    .lines()
                    .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim()))
                    .any(|key| key == "edition.workspace")
            {
                return "2015".to_owned();
            }
        }
        if current == root {
            break;
        }
        directory = current.parent().map(Path::to_path_buf);
    }
    "2021".to_owned()
}

fn manifest_edition(manifest: &str) -> Option<String> {
    let mut section = "";
    let mut workspace_edition = None;
    for line in manifest.lines() {
        let line = line.split('#').next()?.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line;
            continue;
        }
        if !matches!(section, "[package]" | "[workspace.package]") {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "edition" {
            continue;
        }
        let edition = value.trim().trim_matches(['\'', '"']);
        if !matches!(edition, "2015" | "2018" | "2021" | "2024") {
            continue;
        }
        if section == "[package]" {
            return Some(edition.to_owned());
        }
        workspace_edition = Some(edition.to_owned());
    }
    workspace_edition
}

fn project_program(root: &Path, relative: &Path, program: &str) -> Option<PathBuf> {
    let mut directory = root.join(relative).parent()?.to_owned();
    loop {
        if !directory.starts_with(root) {
            return None;
        }
        let candidate = executable_candidate(&directory.join("node_modules/.bin"), program);
        if candidate.is_some() {
            return candidate;
        }
        if directory == root || !directory.starts_with(root) || !directory.pop() {
            return None;
        }
    }
}

fn path_program(program: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path).find_map(|directory| executable_candidate(&directory, program))
}

fn executable_candidate(directory: &Path, program: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let names = [
        program.to_owned(),
        format!("{program}.exe"),
        format!("{program}.cmd"),
        format!("{program}.bat"),
    ];
    #[cfg(not(windows))]
    let names = [program.to_owned()];

    names
        .into_iter()
        .map(|name| directory.join(name))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_file_types_to_formatters() {
        assert_eq!(
            formatter_specs(Path::new("src/main.rs"))[0].program,
            "rustfmt"
        );
        assert_eq!(formatter_specs(Path::new("app.py"))[0].program, "ruff");
        assert_eq!(
            formatter_specs(Path::new("config.jsonc"))[2].program,
            "deno"
        );
        assert_eq!(formatter_specs(Path::new("Cargo.toml"))[0].program, "taplo");
        assert!(formatter_specs(Path::new("notes.txt")).is_empty());
    }

    #[test]
    fn prefers_a_project_local_formatter() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        let formatter = root.join("node_modules/.bin/prettier");
        fs::write(&formatter, "formatter").unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&formatter).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&formatter, permissions).unwrap();
        }

        let command = detect(root, Path::new("src/config.jsonc")).unwrap();

        assert_eq!(command.label, "Prettier");
        assert_eq!(command.program, root.join("node_modules/.bin/prettier"));
        assert_eq!(command.args, ["--write"]);
    }

    #[test]
    fn reads_rust_edition_from_the_nearest_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("crates/app/src")).unwrap();
        fs::write(
            root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nedition = \"2018\"\n",
        )
        .unwrap();

        assert_eq!(
            rust_edition(root, Path::new("crates/app/src/main.rs")),
            "2018"
        );
        assert_eq!(rust_edition(root, Path::new("standalone.rs")), "2021");
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"legacy\"\n").unwrap();
        assert_eq!(rust_edition(root, Path::new("standalone.rs")), "2015");
        assert_eq!(
            manifest_edition("[workspace.package]\nedition = \"2024\"\n"),
            Some("2024".to_owned())
        );
    }
}
