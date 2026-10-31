use clap::Args;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Args)]
#[command(
    after_help = "Examples:\n  telora fmt src\n  telora fmt --check src tests\n  telora fmt --stdout src/lib.telora\n  cat src/lib.telora | telora fmt -"
)]
pub(crate) struct FmtArgs {
    /// Files or directories to format recursively (default: current directory); '-' reads stdin.
    #[arg(value_name = "PATH", default_value = ".")]
    paths: Vec<PathBuf>,
    /// Report unformatted sources without writing; exit 1 if formatting is needed.
    #[arg(long, conflicts_with = "stdout")]
    check: bool,
    /// Print a single formatted file instead of writing it.
    #[arg(long)]
    stdout: bool,
    /// Target line width; literals and indivisible expressions may exceed it.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(20..=240))]
    width: u16,
}

pub(crate) fn run(context: PathBuf, args: FmtArgs) -> Result<i32, String> {
    if args.paths.iter().any(|path| path == Path::new("-")) {
        if args.paths.len() != 1 {
            return Err("stdin ('-') must be the only fmt path".into());
        }
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|e| format!("cannot read stdin: {e}"))?;
        let output = telora_core::source_format::format(&source, usize::from(args.width))
            .map_err(|e| format!("<stdin>: {e}"))?;
        return finish_stdout(&source, &output, args.check);
    }
    let mut files = BTreeSet::new();
    for path in &args.paths {
        collect(&context.join(path), &mut files, true)?;
    }
    if args.stdout && files.len() != 1 {
        return Err("--stdout requires exactly one .telora file".into());
    }
    // Complete parsing/layout validation for the batch before replacing any source.
    let mut changes = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let formatted = telora_core::source_format::format(&source, usize::from(args.width))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if args.stdout {
            return finish_stdout(&source, &formatted, false);
        }
        if source != formatted {
            changes.push((path, formatted));
        }
    }
    if args.check {
        for (path, _) in &changes {
            eprintln!("Would reformat {}", path.display());
        }
        return Ok(i32::from(!changes.is_empty()));
    }
    for (path, formatted) in changes {
        let permissions = fs::metadata(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .permissions();
        let parent = path.parent().ok_or("source has no parent directory")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        temporary
            .write_all(formatted.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        temporary
            .as_file()
            .set_permissions(permissions)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        temporary
            .persist(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(0)
}

fn finish_stdout(source: &str, formatted: &str, check: bool) -> Result<i32, String> {
    if check {
        if source != formatted {
            eprintln!("Would reformat <stdin>");
        }
        return Ok(i32::from(source != formatted));
    }
    io::stdout()
        .lock()
        .write_all(formatted.as_bytes())
        .map_err(|e| format!("cannot write formatted source: {e}"))?;
    Ok(0)
}

fn collect(path: &Path, files: &mut BTreeSet<PathBuf>, explicit: bool) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        if explicit {
            return Err(format!(
                "{}: fmt paths must not be symbolic links",
                path.display()
            ));
        }
        return Ok(());
    }
    if meta.is_file() {
        if path
            .extension()
            .is_some_and(|extension| extension == "telora")
        {
            files.insert(
                path.canonicalize()
                    .map_err(|e| format!("{}: {e}", path.display()))?,
            );
        } else if explicit {
            return Err(format!(
                "{}: expected a .telora source file",
                path.display()
            ));
        }
    } else if meta.is_dir() {
        let entries = fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut paths = entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        paths.sort();
        for path in paths {
            let name = path.file_name().unwrap().to_string_lossy();
            if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules") {
                continue;
            }
            collect(&path, files, false)?;
        }
    } else if explicit {
        return Err(format!(
            "{}: expected a regular file or directory",
            path.display()
        ));
    }
    Ok(())
}
