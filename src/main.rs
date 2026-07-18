mod backend;
mod frontend;
mod object;
mod package;
mod target;
mod tooling;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use backend::profile::CompileProfile;
use backend::x86_64::{
    ProgramKind, TargetFeatureSet, X86BackendOptions, X86Program, emit_linux_program,
};
use frontend::ast::{
    EnumVariantPayloadDef, Expr, Function, Item, MatchPattern, Program, Stmt, TypeName,
};
use frontend::semantics::{RuntimeInstr, RuntimeLoadKind};
use frontend::{LoweredStmt, lower_program, optimize_semantics_ir};
use object::artifact::{ArtifactMetadata, DebugDeclaration, DebugSource};
use object::elf64::emit_elf64_executable;
use object::macho64::emit_macho64_executable;
use object::pe64::{emit_coff_relocatable, emit_pe64_executable};
use package::{PackageGraph, PackageId, PackageOptions};
use target::{ObjectFormat, TargetSpec};

const COMPILER_STDLIB_ABI_VERSION: u32 = 1;
const EMBEDDED_STDLIB_ABI_VERSION: &str = include_str!("../stdlib/ABI_VERSION");
const MACHINE_DIAGNOSTIC_EMITTED: &str = "aziky machine diagnostic emitted";

fn main() {
    if let Err(err) = run() {
        if err != MACHINE_DIAGNOSTIC_EMITTED {
            eprintln!("error: {err}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        return Err(usage());
    }

    if args[1] == "--help" || args[1] == "-h" {
        println!("{}", usage());
        return Ok(());
    }

    match args[1].as_str() {
        "compile" => compile_mode(&args[2..]),
        "target" => target_mode(&args[2..]),
        "package" => package_mode(&args[2..]),
        "test" => test_mode(&args[2..]),
        "fmt" => format_mode(&args[2..]),
        "lint" => lint_mode(&args[2..]),
        "profile-merge" => profile_merge_mode(&args[2..]),
        "check" => check_mode(&args[2..]),
        "emit" => emit_mode(&args[2..]),
        _ => legacy_emit_mode(&args[1..]),
    }
}

fn target_mode(args: &[String]) -> Result<(), String> {
    match args {
        [command] if command == "list" => {
            let mut targets = TargetSpec::known().to_vec();
            targets.sort_unstable_by_key(|target| target.triple());
            for target in targets {
                println!("{}", target.describe());
            }
            Ok(())
        }
        [command, triple] if command == "show" => {
            println!("{}", TargetSpec::parse(triple)?.describe());
            Ok(())
        }
        _ => Err(String::from("target expects `list` or `show <triple>`")),
    }
}

fn compile_mode(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err(usage());
    }
    let input = PathBuf::from(&args[0]);
    let options = parse_compile_args(&args[1..])?;
    options.target.require_codegen()?;
    let mut prepared = prepare_lower_optimize_package_mode(&input, &options.package_options, true)?;
    validate_target_runtime(&prepared.lowered, options.target)?;
    prepared.metadata.target = options.target.triple().to_string();
    let compiled = compile_program(&prepared.lowered, &options.backend_options);
    prepared.metadata.block_symbols = parse_block_symbols(compiled.block_map.as_deref());
    let artifact = match (options.artifact_kind, options.format) {
        (ArtifactKind::Executable, OutputFormat::Elf64) => emit_elf64_executable(&compiled.code),
        (ArtifactKind::Executable, OutputFormat::Macho64) => {
            emit_macho64_executable(&compiled.code)
        }
        (ArtifactKind::Object, OutputFormat::Elf64) => {
            object::artifact::emit_elf64_relocatable(&compiled.code, &prepared.metadata)
        }
        (ArtifactKind::Object, OutputFormat::Macho64) => {
            object::artifact::emit_macho64_relocatable(&compiled.code, &prepared.metadata)
        }
        (ArtifactKind::StaticLibrary, OutputFormat::Elf64) => {
            let object =
                object::artifact::emit_elf64_relocatable(&compiled.code, &prepared.metadata);
            object::artifact::emit_static_archive(&object, &["_start", "aziky_program_entry"])
        }
        (ArtifactKind::StaticLibrary, OutputFormat::Macho64) => {
            let object =
                object::artifact::emit_macho64_relocatable(&compiled.code, &prepared.metadata);
            object::artifact::emit_static_archive(&object, &["_start", "_aziky_program_entry"])
        }
        (ArtifactKind::SharedLibrary, OutputFormat::Elf64) => {
            object::artifact::emit_elf64_shared(&compiled.code, &prepared.metadata)
        }
        (ArtifactKind::SharedLibrary, OutputFormat::Macho64) => {
            return Err(String::from(
                "shared-library output is not supported for macho64 until the Darwin runtime/loader target is accepted; use elf64 on the current linux-x86_64 target",
            ));
        }
        (ArtifactKind::Executable, OutputFormat::Coff) => emit_pe64_executable(&compiled.code),
        (ArtifactKind::Object, OutputFormat::Coff) => emit_coff_relocatable(&compiled.code),
        (ArtifactKind::StaticLibrary, OutputFormat::Coff) => {
            let object = emit_coff_relocatable(&compiled.code);
            object::artifact::emit_static_archive(&object, &["_start", "aziky_program_entry"])
        }
        (ArtifactKind::SharedLibrary, OutputFormat::Coff) => {
            return Err(String::from(
                "shared-library output is not supported for coff until the Windows DLL export contract is accepted; use executable, object, or static-library output",
            ));
        }
    };

    write_output(
        &options.output,
        &artifact,
        matches!(
            options.artifact_kind,
            ArtifactKind::Executable | ArtifactKind::SharedLibrary
        ),
    )
    .map_err(|e| format!("failed to write output: {e}"))?;
    if options.dump_lir {
        if let Some(lir_dump) = compiled.lir_dump {
            println!("{lir_dump}");
        } else {
            println!("MachineLIR unavailable for this compilation unit");
        }
    }
    if let Some(path) = &options.profile_generate {
        let mut payload = String::new();
        if let Some(profile_template) = compiled.profile_template {
            payload.push_str(&profile_template);
        }
        if let Some(block_map) = compiled.block_map {
            if !payload.is_empty() {
                payload.push('\n');
            }
            payload.push_str("# block map\n");
            for line in block_map.lines() {
                payload.push_str("# ");
                payload.push_str(line);
                payload.push('\n');
            }
        }
        fs::write(path, payload)
            .map_err(|e| format!("failed to write profile template '{}': {e}", path.display()))?;
    }
    println!(
        "emitted kind={} target={} format={} bytes={} to {}",
        options.artifact_kind.as_str(),
        options.target.triple(),
        options.format.as_str(),
        artifact.len(),
        options.output.to_string_lossy()
    );
    Ok(())
}

fn validate_target_runtime(stmts: &[LoweredStmt], target: TargetSpec) -> Result<(), String> {
    let capabilities = target.runtime;
    let mut required = std::collections::BTreeSet::new();
    for stmt in stmts {
        match stmt {
            LoweredStmt::RuntimeSeededLcgAllocLoop { .. } => {
                required.insert("allocation");
            }
            LoweredStmt::RuntimeGeneric { program } => {
                for instr in &program.instrs {
                    match instr {
                        RuntimeInstr::Alloc { .. }
                        | RuntimeInstr::Free { .. }
                        | RuntimeInstr::ChannelCreate { .. }
                        | RuntimeInstr::ChannelDestroy { .. } => {
                            required.insert("allocation");
                        }
                        RuntimeInstr::FileOpen { .. }
                        | RuntimeInstr::FileWrite { .. }
                        | RuntimeInstr::FileRead { .. }
                        | RuntimeInstr::FileClose { .. } => {
                            required.insert("files");
                        }
                        RuntimeInstr::ThreadSpawn { .. } | RuntimeInstr::ThreadJoin { .. } => {
                            required.insert("threads");
                        }
                        RuntimeInstr::ChannelSend { .. }
                        | RuntimeInstr::ChannelRecv { .. }
                        | RuntimeInstr::ChannelClose { .. } => {
                            required.insert("synchronization");
                        }
                        RuntimeInstr::LoadSeed { kind, .. } => match kind {
                            RuntimeLoadKind::MonotonicNanos | RuntimeLoadKind::WallTimeNanos => {
                                required.insert("clocks");
                            }
                            RuntimeLoadKind::ProcessId => {
                                required.insert("process");
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let unavailable: Vec<&str> = required
        .into_iter()
        .filter(|name| match *name {
            "allocation" => !capabilities.allocation,
            "files" => !capabilities.files,
            "clocks" => !capabilities.clocks,
            "process" => !capabilities.process,
            "threads" => !capabilities.threads,
            "synchronization" => !capabilities.synchronization,
            _ => false,
        })
        .collect();
    if unavailable.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "target '{}' does not provide required runtime capabilities: {}",
            target.triple(),
            unavailable.join(", ")
        ))
    }
}

fn profile_merge_mode(args: &[String]) -> Result<(), String> {
    if args.len() != 4 || args[2] != "-o" {
        return Err(format!(
            "profile-merge expects <template> <raw-counts> -o <profile>\n{}",
            usage()
        ));
    }
    let template_path = PathBuf::from(&args[0]);
    let raw_path = PathBuf::from(&args[1]);
    let output_path = PathBuf::from(&args[3]);
    let template = fs::read_to_string(&template_path).map_err(|error| {
        format!(
            "failed to read profile template '{}': {error}",
            template_path.display()
        )
    })?;
    let raw = fs::read(&raw_path).map_err(|error| {
        format!(
            "failed to read raw profile '{}': {error}",
            raw_path.display()
        )
    })?;
    let mut profile = CompileProfile::parse(&template)?;
    profile.merge_instrumentation_raw("runtime_generic", &raw)?;
    fs::write(&output_path, profile.render()).map_err(|error| {
        format!(
            "failed to write merged profile '{}': {error}",
            output_path.display()
        )
    })?;
    println!(
        "merged {} bytes of counters into {}",
        raw.len(),
        output_path.display()
    );
    Ok(())
}

fn check_mode(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err(usage());
    }
    let input = PathBuf::from(&args[0]);
    let mut diagnostic_format_json = false;
    let mut package_args = Vec::new();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--diagnostic-format=json" => {
                diagnostic_format_json = true;
                index += 1;
            }
            "--diagnostic-format" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| String::from("--diagnostic-format requires 'json'"))?;
                if value != "json" {
                    return Err(format!("unsupported diagnostic format '{value}'"));
                }
                diagnostic_format_json = true;
                index += 2;
            }
            _ => {
                package_args.push(args[index].clone());
                index += 1;
            }
        }
    }
    let package_options = parse_package_options(&package_args)?;
    let lowered = match parse_lower_optimize_with_packages(&input, &package_options) {
        Ok(lowered) => lowered,
        Err(error) if diagnostic_format_json => {
            let diagnostic = tooling::diagnostics::MachineDiagnostic::from_rendered(&error, &input);
            println!(
                "{}",
                tooling::diagnostics::MachineDiagnostic::collection_json("error", &[diagnostic])
            );
            return Err(MACHINE_DIAGNOSTIC_EMITTED.to_string());
        }
        Err(error) => return Err(error),
    };
    if diagnostic_format_json {
        println!(
            "{}",
            tooling::diagnostics::MachineDiagnostic::collection_json("ok", &[])
        );
        return Ok(());
    }
    println!(
        "check=PASS input={} lowered_stmts={}",
        input.to_string_lossy(),
        lowered.len()
    );
    Ok(())
}

fn package_mode(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(format!(
            "package expects `lock`, `verify`, or `checksum`\n{}",
            usage()
        ));
    };
    match command {
        "checksum" => {
            if args.len() != 2 {
                return Err(String::from(
                    "package checksum expects exactly one package directory",
                ));
            }
            let checksum = package::checksum_package(Path::new(&args[1]))?;
            println!("{checksum}");
            Ok(())
        }
        "lock" | "verify" => {
            let mut path = PathBuf::from(".");
            let option_start = if args.get(1).is_some_and(|arg| !arg.starts_with('-')) {
                path = PathBuf::from(&args[1]);
                2
            } else {
                1
            };
            let options = parse_package_options(&args[option_start..])?;
            if command == "lock" {
                let graph = package::write_lock(&path, &options)?;
                println!(
                    "lock=UPDATED package={} features={} dependencies={} path={}",
                    graph.root_id.display(),
                    graph.root_features.len(),
                    graph.packages.len(),
                    graph.root_dir.join(package::LOCK_FILE).display()
                );
            } else {
                let manifest = package::discover_manifest(&path)?.ok_or_else(|| {
                    format!(
                        "no {} found for '{}'",
                        package::MANIFEST_FILE,
                        path.display()
                    )
                })?;
                let graph = package::resolve_for_input(&manifest, &options)?
                    .expect("manifest discovery guarantees package graph");
                println!(
                    "lock=PASS package={} features={} dependencies={} entry={}",
                    graph.root_id.display(),
                    graph.root_features.len(),
                    graph.packages.len(),
                    graph.root_entry.display()
                );
            }
            Ok(())
        }
        other => Err(format!(
            "unknown package command '{other}' (expected lock, verify, or checksum)"
        )),
    }
}

#[derive(Debug)]
struct TestArgs {
    path: PathBuf,
    filter: Option<String>,
    list: bool,
    timeout: Duration,
    package_options: PackageOptions,
}

fn test_mode(args: &[String]) -> Result<(), String> {
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = args;
        return Err(String::from(
            "native test execution is currently supported only for target linux-x86_64",
        ));
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let options = parse_test_args(args)?;
        run_native_tests(&options)
    }
}

fn parse_test_args(args: &[String]) -> Result<TestArgs, String> {
    let mut path = None;
    let mut filter = None;
    let mut list = false;
    let mut timeout = Duration::from_millis(10_000);
    let mut package_args = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--filter" => {
                filter = Some(
                    args.get(index + 1)
                        .ok_or_else(|| String::from("--filter requires a substring"))?
                        .clone(),
                );
                index += 2;
            }
            "--list" => {
                list = true;
                index += 1;
            }
            "--timeout-ms" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| String::from("--timeout-ms requires a positive integer"))?;
                let millis = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --timeout-ms value '{value}'"))?;
                if millis == 0 {
                    return Err(String::from("--timeout-ms must be greater than zero"));
                }
                timeout = Duration::from_millis(millis);
                index += 2;
            }
            "--features" | "--package-cache" => {
                package_args.push(args[index].clone());
                package_args.push(
                    args.get(index + 1)
                        .ok_or_else(|| format!("{} requires a value", args[index]))?
                        .clone(),
                );
                index += 2;
            }
            "--no-default-features" => {
                package_args.push(args[index].clone());
                index += 1;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown test option '{value}'"));
            }
            value => {
                if path.replace(PathBuf::from(value)).is_some() {
                    return Err(String::from("test accepts at most one path"));
                }
                index += 1;
            }
        }
    }
    Ok(TestArgs {
        path: path.unwrap_or_else(|| PathBuf::from(".")),
        filter,
        list,
        timeout,
        package_options: parse_package_options(&package_args)?,
    })
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn run_native_tests(options: &TestArgs) -> Result<(), String> {
    let (search_root, _project_root) = test_search_root(&options.path)?;
    let files = tooling::discover_sources(&search_root)?;
    let selected: Vec<_> = files
        .into_iter()
        .filter_map(|path| {
            let name = if search_root.is_file() {
                path.file_name().map_or_else(
                    || tooling::portable_path(&path),
                    |name| name.to_string_lossy().into_owned(),
                )
            } else {
                tooling::display_relative(&path, &search_root)
            };
            if options
                .filter
                .as_ref()
                .is_some_and(|filter| !name.contains(filter))
            {
                None
            } else {
                Some((name, path))
            }
        })
        .collect();
    if selected.is_empty() {
        return Err(format!(
            "no Aziky tests discovered under '{}'{}",
            search_root.display(),
            options
                .filter
                .as_ref()
                .map_or(String::new(), |filter| format!(" matching '{filter}'"))
        ));
    }
    if options.list {
        for (name, _) in &selected {
            println!("{name}");
        }
        println!("tests={}", selected.len());
        return Ok(());
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock error: {error}"))?
        .as_nanos();
    let temp = env::temp_dir().join(format!("aziky-tests-{}-{nonce}", std::process::id()));
    fs::create_dir(&temp).map_err(|error| {
        format!(
            "failed to create test workspace '{}': {error}",
            temp.display()
        )
    })?;
    let result = run_native_tests_in(&selected, options, &temp);
    let cleanup = fs::remove_dir_all(&temp).map_err(|error| {
        format!(
            "failed to clean test workspace '{}': {error}",
            temp.display()
        )
    });
    match (result, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn run_native_tests_in(
    selected: &[(String, PathBuf)],
    options: &TestArgs,
    temp: &Path,
) -> Result<(), String> {
    let mut passed = 0usize;
    let mut failures = Vec::new();
    println!(
        "running {} test{}",
        selected.len(),
        if selected.len() == 1 { "" } else { "s" }
    );
    for (index, (name, source)) in selected.iter().enumerate() {
        let binary = temp.join(format!("test-{index}.bin"));
        let stdout_path = temp.join(format!("test-{index}.stdout"));
        let stderr_path = temp.join(format!("test-{index}.stderr"));
        let lowered = match parse_lower_optimize_package_member(source, &options.package_options) {
            Ok(lowered) => lowered,
            Err(error) => {
                println!("test {name} ... FAILED (compile)");
                failures.push((name.clone(), format!("compile error:\n{error}")));
                continue;
            }
        };
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        if let Err(error) = write_executable(&binary, &emit_elf64_executable(&compiled.code)) {
            println!("test {name} ... FAILED (emit)");
            failures.push((name.clone(), format!("emit error: {error}")));
            continue;
        }
        let stdout_file = fs::File::create(&stdout_path)
            .map_err(|error| format!("failed to capture test stdout: {error}"))?;
        let stderr_file = fs::File::create(&stderr_path)
            .map_err(|error| format!("failed to capture test stderr: {error}"))?;
        let mut child = Command::new(&binary)
            .current_dir(source.parent().unwrap_or_else(|| Path::new(".")))
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file))
            .spawn()
            .map_err(|error| format!("failed to execute test '{name}': {error}"))?;
        let start = Instant::now();
        let (status, timed_out) = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("failed to wait for test '{name}': {error}"))?
            {
                break (status, false);
            }
            if start.elapsed() >= options.timeout {
                child.kill().map_err(|error| {
                    format!("failed to terminate timed-out test '{name}': {error}")
                })?;
                let status = child
                    .wait()
                    .map_err(|error| format!("failed to reap timed-out test '{name}': {error}"))?;
                break (status, true);
            }
            thread::sleep(Duration::from_millis(2));
        };
        if status.success() && !timed_out {
            println!("test {name} ... ok");
            passed += 1;
        } else {
            let reason = if timed_out {
                format!("timed out after {} ms", options.timeout.as_millis())
            } else {
                status.code().map_or_else(
                    || "terminated by signal".to_string(),
                    |code| format!("exit code {code}"),
                )
            };
            println!("test {name} ... FAILED ({reason})");
            let stdout = fs::read_to_string(&stdout_path)
                .unwrap_or_else(|_| String::from("<non-UTF-8 stdout>"));
            let stderr = fs::read_to_string(&stderr_path)
                .unwrap_or_else(|_| String::from("<non-UTF-8 stderr>"));
            failures.push((
                name.clone(),
                format!("reason: {reason}\nstdout:\n{stdout}stderr:\n{stderr}"),
            ));
        }
    }
    if failures.is_empty() {
        println!("test result: ok. {passed} passed; 0 failed");
        return Ok(());
    }
    println!("\nfailures:");
    for (name, detail) in &failures {
        println!("\n---- {name} ----\n{detail}");
    }
    Err(format!(
        "test result: FAILED. {passed} passed; {} failed",
        failures.len()
    ))
}

fn test_search_root(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    if path.is_file() {
        let file = fs::canonicalize(path)
            .map_err(|error| format!("failed to resolve '{}': {error}", path.display()))?;
        let root = file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        return Ok((file, root));
    }
    let project_root = fs::canonicalize(path)
        .map_err(|error| format!("failed to resolve '{}': {error}", path.display()))?;
    let tests = project_root.join("tests");
    if tests.is_dir() {
        Ok((tests, project_root))
    } else {
        Ok((project_root.clone(), project_root))
    }
}

fn format_mode(args: &[String]) -> Result<(), String> {
    let mut check = false;
    let mut stdout = false;
    let mut stdin = false;
    let mut paths = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--stdout" => stdout = true,
            "--stdin" => stdin = true,
            value if value.starts_with('-') => return Err(format!("unknown fmt option '{value}'")),
            value => paths.push(PathBuf::from(value)),
        }
    }
    if stdin {
        if check || stdout || !paths.is_empty() {
            return Err(String::from(
                "fmt --stdin cannot be combined with paths, --check, or --stdout",
            ));
        }
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| format!("failed to read UTF-8 source from stdin: {error}"))?;
        print!("{}", tooling::format::format_source(&source));
        return Ok(());
    }
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    let files = discover_tool_files(&paths)?;
    if stdout && files.len() != 1 {
        return Err(String::from(
            "fmt --stdout requires exactly one .azk source file",
        ));
    }
    let cwd =
        env::current_dir().map_err(|error| format!("failed to read working directory: {error}"))?;
    let mut changed = Vec::new();
    for file in &files {
        let source = read_utf8_source(file)?;
        let formatted = tooling::format::format_source(&source);
        if stdout {
            print!("{formatted}");
            continue;
        }
        if formatted != source {
            changed.push(tooling::display_relative(file, &cwd));
            if !check {
                fs::write(file, formatted)
                    .map_err(|error| format!("failed to format '{}': {error}", file.display()))?;
            }
        }
    }
    if stdout {
        return Ok(());
    }
    if check && !changed.is_empty() {
        for path in &changed {
            println!("would reformat: {path}");
        }
        return Err(format!(
            "{} source file(s) require formatting",
            changed.len()
        ));
    }
    println!(
        "format=PASS files={} changed={} mode={}",
        files.len(),
        changed.len(),
        if check { "check" } else { "write" }
    );
    Ok(())
}

fn lint_mode(args: &[String]) -> Result<(), String> {
    let mut deny_warnings = false;
    let mut diagnostic_format_json = false;
    let mut paths = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--deny-warnings" | "--check" => deny_warnings = true,
            "--diagnostic-format=json" => diagnostic_format_json = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown lint option '{value}'"));
            }
            value => paths.push(PathBuf::from(value)),
        }
    }
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    let files = discover_tool_files(&paths)?;
    let cwd =
        env::current_dir().map_err(|error| format!("failed to read working directory: {error}"))?;
    let mut diagnostics = Vec::new();
    for file in &files {
        let source = read_utf8_source(file)?;
        match tooling::lint::lint_source(file, &source) {
            Ok(file_diagnostics) => diagnostics.extend(file_diagnostics),
            Err(error) if diagnostic_format_json => {
                let diagnostic =
                    tooling::diagnostics::MachineDiagnostic::from_rendered(&error, file);
                println!(
                    "{}",
                    tooling::diagnostics::MachineDiagnostic::collection_json(
                        "error",
                        &[diagnostic]
                    )
                );
                return Err(MACHINE_DIAGNOSTIC_EMITTED.to_string());
            }
            Err(error) => return Err(error),
        }
    }
    diagnostics.sort();
    if diagnostic_format_json {
        let machine_diagnostics = diagnostics
            .iter()
            .map(|diagnostic| tooling::diagnostics::MachineDiagnostic {
                severity: "warning",
                code: Some(diagnostic.code.to_string()),
                message: diagnostic.message.clone(),
                path: diagnostic.path.to_string_lossy().into_owned(),
                line: diagnostic.line,
                column: diagnostic.column,
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            tooling::diagnostics::MachineDiagnostic::collection_json(
                if machine_diagnostics.is_empty() {
                    "ok"
                } else {
                    "warning"
                },
                &machine_diagnostics
            )
        );
        if deny_warnings && !machine_diagnostics.is_empty() {
            return Err(MACHINE_DIAGNOSTIC_EMITTED.to_string());
        }
        return Ok(());
    }
    for diagnostic in &diagnostics {
        let display = tooling::display_relative(&diagnostic.path, &cwd);
        println!("{}", diagnostic.render(&display));
    }
    println!(
        "lint={} files={} warnings={}",
        if diagnostics.is_empty() {
            "PASS"
        } else {
            "WARN"
        },
        files.len(),
        diagnostics.len()
    );
    if deny_warnings && !diagnostics.is_empty() {
        Err(format!("lint failed with {} warning(s)", diagnostics.len()))
    } else {
        Ok(())
    }
}

fn discover_tool_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut files = BTreeMap::new();
    for path in paths {
        for file in tooling::discover_sources(path)? {
            files.insert(tooling::portable_path(&file), file);
        }
    }
    if files.is_empty() {
        return Err(String::from("no .azk source files discovered"));
    }
    Ok(files.into_values().collect())
}

fn emit_mode(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err(usage());
    }

    let out_path = PathBuf::from(&args[0]);
    let (exit_only, message) = parse_emit_args(&args[1..])?;

    let code = if exit_only {
        emit_linux_program(ProgramKind::ExitOnly)
    } else {
        emit_linux_program(ProgramKind::WriteAndExit {
            message: message.as_bytes(),
        })
    };

    let elf = emit_elf64_executable(&code);
    write_executable(&out_path, &elf).map_err(|e| format!("failed to write output: {e}"))?;
    println!(
        "emitted {} bytes to {}",
        elf.len(),
        out_path.to_string_lossy()
    );
    Ok(())
}

fn legacy_emit_mode(args: &[String]) -> Result<(), String> {
    emit_mode(args)
}

#[cfg_attr(not(test), allow(dead_code))]
fn parse_lower_optimize(input: &Path) -> Result<Vec<LoweredStmt>, String> {
    parse_lower_optimize_with_packages(input, &PackageOptions::defaults_enabled())
}

fn parse_lower_optimize_with_packages(
    input: &Path,
    package_options: &PackageOptions,
) -> Result<Vec<LoweredStmt>, String> {
    parse_lower_optimize_package_mode(input, package_options, true)
}

fn parse_lower_optimize_package_member(
    input: &Path,
    package_options: &PackageOptions,
) -> Result<Vec<LoweredStmt>, String> {
    parse_lower_optimize_package_mode(input, package_options, false)
}

fn parse_lower_optimize_package_mode(
    input: &Path,
    package_options: &PackageOptions,
    require_manifest_entry: bool,
) -> Result<Vec<LoweredStmt>, String> {
    Ok(
        prepare_lower_optimize_package_mode(input, package_options, require_manifest_entry)?
            .lowered,
    )
}

struct PreparedProgram {
    lowered: Vec<LoweredStmt>,
    metadata: ArtifactMetadata,
}

fn prepare_lower_optimize_package_mode(
    input: &Path,
    package_options: &PackageOptions,
    require_manifest_entry: bool,
) -> Result<PreparedProgram, String> {
    let graph = package::resolve_for_input(input, package_options)?;
    if require_manifest_entry && let Some(graph) = &graph {
        let canonical_input = fs::canonicalize(input)
            .map_err(|error| format!("failed to resolve input '{}': {error}", input.display()))?;
        if canonical_input != graph.root_entry {
            return Err(format!(
                "package input '{}' is not the manifest entry '{}'; package builds must start from [package].entry in '{}'",
                canonical_input.display(),
                graph.root_entry.display(),
                graph.manifest_path.display()
            ));
        }
    }
    let (program, sources) = load_project_program_with_packages(input, graph.as_ref())?;
    let lowered =
        lower_program(&program).map_err(|e| sources.render(&e.with_context("semantics::lower")))?;
    let metadata = build_artifact_metadata(input, graph.as_ref(), &sources)?;
    Ok(PreparedProgram {
        lowered: optimize_semantics_ir(lowered),
        metadata,
    })
}

fn build_artifact_metadata(
    input: &Path,
    graph: Option<&PackageGraph>,
    sources: &ProjectSources,
) -> Result<ArtifactMetadata, String> {
    let canonical_input = fs::canonicalize(input)
        .map_err(|error| format!("failed to resolve input '{}': {error}", input.display()))?;
    let legacy_root = canonical_input.parent().unwrap_or_else(|| Path::new("."));
    let mut stable_sources = Vec::with_capacity(sources.files.len());
    for source in &sources.files {
        let path_text = source.path.to_string_lossy();
        let stable = if path_text.starts_with("<aziky:") {
            path_text.into_owned()
        } else if let Some(graph) = graph {
            if let Some((id, relative)) = graph.packages.iter().find_map(|(id, package)| {
                source
                    .path
                    .strip_prefix(&package.root)
                    .ok()
                    .map(|relative| (id, relative))
            }) {
                format!("{}/{}", id.display(), tooling::portable_path(relative))
            } else if let Ok(relative) = source.path.strip_prefix(&graph.root_dir) {
                format!(
                    "{}/{}",
                    graph.root_id.display(),
                    tooling::portable_path(relative)
                )
            } else {
                source.path.file_name().map_or_else(
                    || String::from("<source>"),
                    |name| name.to_string_lossy().into_owned(),
                )
            }
        } else {
            source.path.strip_prefix(legacy_root).map_or_else(
                |_| {
                    source.path.file_name().map_or_else(
                        || String::from("<source>"),
                        |name| name.to_string_lossy().into_owned(),
                    )
                },
                tooling::portable_path,
            )
        };
        stable_sources.push(DebugSource { path: stable });
    }
    Ok(ArtifactMetadata {
        target: String::from("x86_64-unknown-linux"),
        sources: stable_sources,
        declarations: sources.declarations.clone(),
        block_symbols: Vec::new(),
    })
}

fn parse_block_symbols(block_map: Option<&str>) -> Vec<(usize, usize, usize)> {
    let mut symbols = Vec::new();
    for line in block_map.unwrap_or_default().lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() != 4 || parts[0] != "block" {
            continue;
        }
        let Some(start) = parts[2].strip_prefix("code_start=") else {
            continue;
        };
        let Some(end) = parts[3].strip_prefix("code_end=") else {
            continue;
        };
        if let (Ok(block), Ok(start), Ok(end)) = (parts[1].parse(), start.parse(), end.parse()) {
            symbols.push((block, start, end));
        }
    }
    symbols.sort_unstable();
    symbols
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModuleSymbolKind {
    Function,
    Type,
    Trait,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModuleSymbol {
    qualified: String,
    kind: ModuleSymbolKind,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum BuiltinModule {
    Std,
    Core,
    Cmp,
    Convert,
    Alloc,
    Fs,
    Args,
    Time,
    Process,
    Env,
    PathApi,
}

impl BuiltinModule {
    fn display(self) -> &'static str {
        match self {
            BuiltinModule::Std => "<aziky:std>",
            BuiltinModule::Core => "<aziky:std::core>",
            BuiltinModule::Cmp => "<aziky:std::cmp>",
            BuiltinModule::Convert => "<aziky:std::convert>",
            BuiltinModule::Alloc => "<aziky:std::alloc>",
            BuiltinModule::Fs => "<aziky:std::fs>",
            BuiltinModule::Args => "<aziky:std::args>",
            BuiltinModule::Time => "<aziky:std::time>",
            BuiltinModule::Process => "<aziky:std::process>",
            BuiltinModule::Env => "<aziky:std::env>",
            BuiltinModule::PathApi => "<aziky:std::path>",
        }
    }

    fn source(self) -> &'static str {
        match self {
            BuiltinModule::Std => include_str!("../stdlib/std.azk"),
            BuiltinModule::Core => include_str!("../stdlib/core.azk"),
            BuiltinModule::Cmp => include_str!("../stdlib/cmp.azk"),
            BuiltinModule::Convert => include_str!("../stdlib/convert.azk"),
            BuiltinModule::Alloc => include_str!("../stdlib/alloc.azk"),
            BuiltinModule::Fs => include_str!("../stdlib/fs.azk"),
            BuiltinModule::Args => include_str!("../stdlib/args.azk"),
            BuiltinModule::Time => include_str!("../stdlib/time.azk"),
            BuiltinModule::Process => include_str!("../stdlib/process.azk"),
            BuiltinModule::Env => include_str!("../stdlib/env.azk"),
            BuiltinModule::PathApi => include_str!("../stdlib/path.azk"),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum ModuleLocation {
    File(PathBuf),
    Builtin(BuiltinModule),
}

impl ModuleLocation {
    fn canonicalized(&self) -> Result<Self, String> {
        match self {
            ModuleLocation::File(path) => fs::canonicalize(path)
                .map(ModuleLocation::File)
                .map_err(|error| format!("failed to resolve module '{}': {error}", path.display())),
            ModuleLocation::Builtin(module) => Ok(ModuleLocation::Builtin(*module)),
        }
    }

    fn display(&self) -> String {
        match self {
            ModuleLocation::File(path) => path.display().to_string(),
            ModuleLocation::Builtin(module) => module.display().to_string(),
        }
    }

    fn source(&self) -> Result<String, String> {
        match self {
            ModuleLocation::File(path) => read_utf8_source(path),
            ModuleLocation::Builtin(module) => Ok(module.source().to_string()),
        }
    }

    fn diagnostic_path(&self) -> PathBuf {
        match self {
            ModuleLocation::File(path) => path.clone(),
            ModuleLocation::Builtin(module) => PathBuf::from(module.display()),
        }
    }
}

#[derive(Clone)]
struct LoadedModule {
    namespace: String,
    exports: BTreeMap<String, ModuleSymbol>,
    declared: BTreeMap<String, bool>,
}

#[derive(Clone, Debug)]
struct ProjectSource {
    path: PathBuf,
    source: String,
}

#[derive(Clone, Debug, Default)]
struct ProjectSources {
    files: Vec<ProjectSource>,
    declarations: Vec<DebugDeclaration>,
}

impl ProjectSources {
    fn record_declaration(
        &mut self,
        name: String,
        kind: &str,
        span: frontend::ast::Span,
        public: bool,
    ) {
        self.declarations.push(DebugDeclaration {
            name,
            kind: kind.to_string(),
            source_index: span.source_id,
            line: span.line,
            column: span.column,
            public,
        });
    }
}

impl ProjectSources {
    fn insert(&mut self, path: PathBuf, source: String) -> usize {
        let source_id = self.files.len();
        self.files.push(ProjectSource { path, source });
        source_id
    }

    fn render(&self, diagnostic: &frontend::Diagnostic) -> String {
        let source = self
            .files
            .get(diagnostic.source_id)
            .or_else(|| self.files.first());
        match source {
            Some(source) => diagnostic.render(&source.source, &source.path),
            None => diagnostic.to_string(),
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn load_project_program(input: &Path) -> Result<(Program, ProjectSources), String> {
    load_project_program_with_packages(input, None)
}

fn load_project_program_with_packages(
    input: &Path,
    packages: Option<&PackageGraph>,
) -> Result<(Program, ProjectSources), String> {
    validate_stdlib_abi(EMBEDDED_STDLIB_ABI_VERSION)?;
    let input = fs::canonicalize(input)
        .map_err(|error| format!("failed to resolve input '{}': {error}", input.display()))?;
    let mut loaded = HashMap::new();
    let mut stack = Vec::new();
    let mut sources = ProjectSources::default();
    let root = ModuleLocation::File(input);
    let items = load_module_unit(
        &root,
        true,
        "",
        None,
        packages,
        &mut loaded,
        &mut stack,
        &mut sources,
    )?
    .0;
    Ok((Program { items }, sources))
}

fn validate_stdlib_abi(source: &str) -> Result<(), String> {
    let version = source.trim().parse::<u32>().map_err(|_| {
        "embedded Aziky standard-library ABI version must be one unsigned integer".to_string()
    })?;
    if version != COMPILER_STDLIB_ABI_VERSION {
        return Err(format!(
            "Aziky standard-library ABI mismatch: compiler requires {}, embedded library provides {}",
            COMPILER_STDLIB_ABI_VERSION, version
        ));
    }
    Ok(())
}

fn parse_module_source(
    location: &ModuleLocation,
    source: String,
    sources: &mut ProjectSources,
) -> Result<Program, String> {
    let diagnostic_path = location.diagnostic_path();
    let source_id = sources.insert(diagnostic_path.clone(), source.clone());
    frontend::parse_program_in_source(&source, source_id).map_err(|error| {
        error
            .with_context("frontend::parse module")
            .render(&source, &diagnostic_path)
    })
}

fn load_module_unit(
    location: &ModuleLocation,
    is_root: bool,
    namespace: &str,
    package_owner: Option<&PackageId>,
    packages: Option<&PackageGraph>,
    loaded: &mut HashMap<ModuleLocation, LoadedModule>,
    stack: &mut Vec<ModuleLocation>,
    sources: &mut ProjectSources,
) -> Result<(Vec<Item>, LoadedModule), String> {
    let location = location.canonicalized()?;
    let display = location.display();
    if let Some(position) = stack.iter().position(|entry| entry == &location) {
        let mut cycle: Vec<String> = stack[position..]
            .iter()
            .map(ModuleLocation::display)
            .collect();
        cycle.push(display.clone());
        return Err(format!("module cycle detected: {}", cycle.join(" -> ")));
    }
    if let Some(previous) = loaded.get(&location) {
        if previous.namespace != namespace {
            return Err(format!(
                "module file '{}' is already loaded as '{}' and cannot also be loaded as '{}'",
                display, previous.namespace, namespace
            ));
        }
        return Ok((Vec::new(), previous.clone()));
    }

    let source = location.source()?;
    let program = parse_module_source(&location, source, sources)?;
    stack.push(location.clone());

    let mut declarations = Vec::new();
    let mut declared_names = HashSet::new();
    for item in &program.items {
        if let Item::Module(decl) = item {
            if decl.public {
                stack.pop();
                return Err(format!(
                    "public module declaration '{}': '{}' is not supported yet; re-export explicit items with 'pub use {}::item;'",
                    decl.name, display, decl.name
                ));
            }
            if !declared_names.insert(decl.name.clone()) {
                stack.pop();
                return Err(format!(
                    "duplicate module declaration '{}' in {} at {}:{}",
                    decl.name, display, decl.span.line, decl.span.column
                ));
            }
            declarations.push(decl.name.clone());
        }
    }

    for item in &program.items {
        record_item_debug_declarations(item, namespace, sources);
    }

    let mut local_symbols = BTreeMap::new();
    let mut local_public = BTreeMap::new();
    for item in &program.items {
        let Some((name, kind, public)) = item_declared_symbol(item) else {
            continue;
        };
        if local_symbols.contains_key(name) {
            stack.pop();
            return Err(format!(
                "duplicate top-level name '{}' in {}",
                name, display
            ));
        }
        if !is_root && name == "main" && kind == ModuleSymbolKind::Function {
            stack.pop();
            return Err(format!("module '{}' must not define fn main()", display));
        }
        local_symbols.insert(
            name.to_string(),
            ModuleSymbol {
                qualified: qualify_module_name(namespace, name),
                kind,
            },
        );
        local_public.insert(name.to_string(), public);
    }

    let mut merged = Vec::new();
    let mut direct_modules = HashMap::new();
    for module_name in &declarations {
        let child = resolve_child_module(&location, module_name, package_owner, packages)?;
        let child_namespace = if let Some(namespace) = child.package_namespace {
            namespace
        } else if matches!(&child.location, ModuleLocation::Builtin(BuiltinModule::Std)) {
            "std".to_string()
        } else {
            qualify_module_name(namespace, module_name)
        };
        let (child_items, child) = load_module_unit(
            &child.location,
            false,
            &child_namespace,
            child.package_owner.as_ref().or(package_owner),
            packages,
            loaded,
            stack,
            sources,
        )?;
        merged.extend(child_items);
        direct_modules.insert(module_name.clone(), child);
    }

    let mut imports = BTreeMap::new();
    for item in &program.items {
        if let Item::Use(import) = item {
            let local_name = import.alias.as_deref().unwrap_or(&import.name);
            let module = direct_modules.get(&import.module).ok_or_else(|| {
                format!(
                    "use refers to undeclared module '{}' in {} at {}:{}; add 'mod {};'",
                    import.module, display, import.span.line, import.span.column, import.module
                )
            })?;
            let symbol = if let Some(symbol) = module.exports.get(&import.name) {
                symbol.clone()
            } else if module.declared.contains_key(&import.name) {
                stack.pop();
                return Err(format!(
                    "item '{}::{}' is private (imported from {} at {}:{})",
                    import.module, import.name, display, import.span.line, import.span.column
                ));
            } else {
                stack.pop();
                return Err(format!(
                    "module '{}' has no exported item '{}' (imported from {} at {}:{})",
                    import.module, import.name, display, import.span.line, import.span.column
                ));
            };
            if local_symbols.contains_key(local_name) || imports.contains_key(local_name) {
                stack.pop();
                return Err(format!(
                    "imported name '{}' conflicts with another name in {} at {}:{}",
                    local_name, display, import.span.line, import.span.column
                ));
            }
            imports.insert(local_name.to_string(), symbol);
        }
    }

    let mut visible = local_symbols.clone();
    visible.extend(imports.clone());
    let mut exports = BTreeMap::new();
    let mut declared = local_public.clone();
    for (name, symbol) in &local_symbols {
        if local_public.get(name).copied().unwrap_or(false) {
            exports.insert(name.clone(), symbol.clone());
        }
    }
    for item in &program.items {
        if let Item::Use(import) = item {
            let local_name = import.alias.as_deref().unwrap_or(&import.name);
            declared.insert(local_name.to_string(), import.public);
            if import.public {
                exports.insert(
                    local_name.to_string(),
                    imports
                        .get(local_name)
                        .expect("validated import should exist")
                        .clone(),
                );
            }
        }
    }

    for mut item in program.items {
        match item {
            Item::Module(_) | Item::Use(_) => {}
            _ => {
                rewrite_module_item(&mut item, &local_symbols, &visible);
                merged.push(item);
            }
        }
    }

    stack.pop();
    let module = LoadedModule {
        namespace: namespace.to_string(),
        exports,
        declared,
    };
    loaded.insert(location, module.clone());
    Ok((merged, module))
}

fn record_item_debug_declarations(item: &Item, namespace: &str, sources: &mut ProjectSources) {
    match item {
        Item::Function(function) => sources.record_declaration(
            qualify_module_name(namespace, &function.name),
            "function",
            function.span,
            function.public,
        ),
        Item::Struct(definition) => sources.record_declaration(
            qualify_module_name(namespace, &definition.name),
            "struct",
            definition.span,
            definition.public,
        ),
        Item::Enum(definition) => sources.record_declaration(
            qualify_module_name(namespace, &definition.name),
            "enum",
            definition.span,
            definition.public,
        ),
        Item::Trait(definition) => sources.record_declaration(
            qualify_module_name(namespace, &definition.name),
            "trait",
            definition.span,
            definition.public,
        ),
        Item::Module(declaration) => sources.record_declaration(
            qualify_module_name(namespace, &declaration.name),
            "module",
            declaration.span,
            declaration.public,
        ),
        Item::Use(declaration) => sources.record_declaration(
            qualify_module_name(
                namespace,
                declaration.alias.as_deref().unwrap_or(&declaration.name),
            ),
            "import",
            declaration.span,
            declaration.public,
        ),
        Item::Impl(definition) => {
            for method in &definition.methods {
                sources.record_declaration(
                    format!(
                        "{}::{}",
                        qualify_module_name(namespace, &definition.for_type),
                        method.name
                    ),
                    "method",
                    method.span,
                    method.public,
                );
            }
        }
        Item::InherentImpl(definition) => {
            for method in &definition.methods {
                sources.record_declaration(
                    format!(
                        "{}::{}",
                        qualify_module_name(namespace, &definition.for_type),
                        method.name
                    ),
                    "method",
                    method.span,
                    method.public,
                );
            }
        }
    }
}

fn read_utf8_source(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("input '{}' is not valid UTF-8", path.display()))
}

struct ResolvedChild {
    location: ModuleLocation,
    package_owner: Option<PackageId>,
    package_namespace: Option<String>,
}

fn resolve_child_module(
    parent: &ModuleLocation,
    module: &str,
    package_owner: Option<&PackageId>,
    packages: Option<&PackageGraph>,
) -> Result<ResolvedChild, String> {
    match parent {
        ModuleLocation::File(parent_file) => {
            let directory = parent_file.parent().unwrap_or_else(|| Path::new("."));
            let flat = directory.join(format!("{module}.azk"));
            let nested = directory.join(module).join("mod.azk");
            let dependency = packages.and_then(|graph| graph.dependency(package_owner, module));
            match (flat.is_file(), nested.is_file(), dependency) {
                (true, false, None) => Ok(ResolvedChild {
                    location: ModuleLocation::File(flat),
                    package_owner: None,
                    package_namespace: None,
                }),
                (false, true, None) => Ok(ResolvedChild {
                    location: ModuleLocation::File(nested),
                    package_owner: None,
                    package_namespace: None,
                }),
                (true, false, Some(_)) | (false, true, Some(_)) => Err(format!(
                    "module '{module}' declared by '{}' is ambiguous between a local module and a package dependency",
                    parent_file.display()
                )),
                (true, true, _) => Err(format!(
                    "ambiguous module '{module}' declared by '{}': both '{}' and '{}' exist",
                    parent_file.display(),
                    flat.display(),
                    nested.display()
                )),
                (false, false, Some(package)) => Ok(ResolvedChild {
                    location: ModuleLocation::File(package.entry.clone()),
                    package_owner: Some(package.id.clone()),
                    package_namespace: Some(package.id.namespace()),
                }),
                (false, false, None) if module == "std" => Ok(ResolvedChild {
                    location: ModuleLocation::Builtin(BuiltinModule::Std),
                    package_owner: None,
                    package_namespace: None,
                }),
                (false, false, None) => Err(format!(
                    "module '{module}' declared by '{}' was not found; expected '{}' or '{}'",
                    parent_file.display(),
                    flat.display(),
                    nested.display()
                )),
            }
        }
        ModuleLocation::Builtin(BuiltinModule::Std) => match module {
            "core" => Ok(builtin_child(BuiltinModule::Core)),
            "cmp" => Ok(builtin_child(BuiltinModule::Cmp)),
            "convert" => Ok(builtin_child(BuiltinModule::Convert)),
            "alloc" => Ok(builtin_child(BuiltinModule::Alloc)),
            "fs" => Ok(builtin_child(BuiltinModule::Fs)),
            "args" => Ok(builtin_child(BuiltinModule::Args)),
            "time" => Ok(builtin_child(BuiltinModule::Time)),
            "process" => Ok(builtin_child(BuiltinModule::Process)),
            "env" => Ok(builtin_child(BuiltinModule::Env)),
            "path" => Ok(builtin_child(BuiltinModule::PathApi)),
            _ => Err(format!(
                "built-in module '<aziky:std>' has no child module '{module}'"
            )),
        },
        ModuleLocation::Builtin(builtin) => Err(format!(
            "built-in module '{}' has no child modules (requested '{}')",
            builtin.display(),
            module
        )),
    }
}

fn builtin_child(module: BuiltinModule) -> ResolvedChild {
    ResolvedChild {
        location: ModuleLocation::Builtin(module),
        package_owner: None,
        package_namespace: None,
    }
}

fn item_declared_symbol(item: &Item) -> Option<(&str, ModuleSymbolKind, bool)> {
    match item {
        Item::Function(def) => Some((&def.name, ModuleSymbolKind::Function, def.public)),
        Item::Struct(def) => Some((&def.name, ModuleSymbolKind::Type, def.public)),
        Item::Enum(def) => Some((&def.name, ModuleSymbolKind::Type, def.public)),
        Item::Trait(def) => Some((&def.name, ModuleSymbolKind::Trait, def.public)),
        Item::Impl(_) | Item::InherentImpl(_) | Item::Module(_) | Item::Use(_) => None,
    }
}

fn qualify_module_name(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{namespace}::{name}")
    }
}

fn rewrite_module_item(
    item: &mut Item,
    local: &BTreeMap<String, ModuleSymbol>,
    visible: &BTreeMap<String, ModuleSymbol>,
) {
    match item {
        Item::Function(function) => {
            qualify_declared_name(&mut function.name, ModuleSymbolKind::Function, local);
            rewrite_module_function(function, visible);
        }
        Item::Struct(def) => {
            qualify_declared_name(&mut def.name, ModuleSymbolKind::Type, local);
            let protected = HashSet::new();
            for field in &mut def.fields {
                rewrite_module_type(&mut field.ty, visible, &protected);
            }
        }
        Item::Enum(def) => {
            qualify_declared_name(&mut def.name, ModuleSymbolKind::Type, local);
            let protected: HashSet<String> = def.type_params.iter().cloned().collect();
            for variant in &mut def.variants {
                match &mut variant.payload {
                    EnumVariantPayloadDef::Unit => {}
                    EnumVariantPayloadDef::Tuple(fields) => {
                        for field in fields {
                            rewrite_module_type(&mut field.ty, visible, &protected);
                        }
                    }
                    EnumVariantPayloadDef::Named(fields) => {
                        for field in fields {
                            rewrite_module_type(&mut field.ty, visible, &protected);
                        }
                    }
                }
            }
        }
        Item::Trait(def) => {
            qualify_declared_name(&mut def.name, ModuleSymbolKind::Trait, local);
            let protected = HashSet::new();
            for method in &mut def.methods {
                for param in &mut method.params {
                    rewrite_module_type(&mut param.ty, visible, &protected);
                }
                if let Some(return_type) = &mut method.return_type {
                    rewrite_module_type(return_type, visible, &protected);
                }
            }
        }
        Item::Impl(def) => {
            rewrite_symbol_name(&mut def.trait_name, ModuleSymbolKind::Trait, visible);
            rewrite_symbol_name(&mut def.for_type, ModuleSymbolKind::Type, visible);
            for method in &mut def.methods {
                rewrite_module_function(method, visible);
            }
        }
        Item::InherentImpl(def) => {
            rewrite_symbol_name(&mut def.for_type, ModuleSymbolKind::Type, visible);
            for method in &mut def.methods {
                rewrite_module_function(method, visible);
            }
        }
        Item::Module(_) | Item::Use(_) => {}
    }
}

fn qualify_declared_name(
    name: &mut String,
    kind: ModuleSymbolKind,
    local: &BTreeMap<String, ModuleSymbol>,
) {
    if let Some(symbol) = local.get(name) {
        if symbol.kind == kind {
            *name = symbol.qualified.clone();
        }
    }
}

fn rewrite_symbol_name(
    name: &mut String,
    kind: ModuleSymbolKind,
    visible: &BTreeMap<String, ModuleSymbol>,
) {
    if let Some(symbol) = visible.get(name) {
        if symbol.kind == kind {
            *name = symbol.qualified.clone();
        }
    }
}

fn rewrite_module_type(
    ty: &mut TypeName,
    visible: &BTreeMap<String, ModuleSymbol>,
    protected: &HashSet<String>,
) {
    match ty {
        TypeName::Struct(name) => {
            if name != "Self" && !protected.contains(name) {
                rewrite_symbol_name(name, ModuleSymbolKind::Type, visible);
            }
        }
        TypeName::Applied { name, args } => {
            if !protected.contains(name) {
                rewrite_symbol_name(name, ModuleSymbolKind::Type, visible);
            }
            for arg in args {
                rewrite_module_type(arg, visible, protected);
            }
        }
        TypeName::Dict { key, value } | TypeName::Map { key, value } => {
            rewrite_module_type(key, visible, protected);
            rewrite_module_type(value, visible, protected);
        }
        TypeName::List { elem } | TypeName::Array { elem, .. } => {
            rewrite_module_type(elem, visible, protected);
        }
        TypeName::Ref { inner, .. } => rewrite_module_type(inner, visible, protected),
        TypeName::Bool
        | TypeName::Byte
        | TypeName::Char
        | TypeName::Int { .. }
        | TypeName::Float { .. }
        | TypeName::String
        | TypeName::Path
        | TypeName::File
        | TypeName::Thread => {}
    }
}

fn rewrite_module_function(function: &mut Function, visible: &BTreeMap<String, ModuleSymbol>) {
    let protected = HashSet::new();
    for param in &mut function.params {
        rewrite_module_type(&mut param.ty, visible, &protected);
    }
    if let Some(return_type) = &mut function.return_type {
        rewrite_module_type(return_type, visible, &protected);
    }
    rewrite_module_stmts(&mut function.body, visible);
}

fn rewrite_module_stmts(stmts: &mut [Stmt], visible: &BTreeMap<String, ModuleSymbol>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { ty, expr, .. } => {
                if let Some(ty) = ty {
                    rewrite_module_type(ty, visible, &HashSet::new());
                }
                rewrite_module_expr(expr, visible);
            }
            Stmt::Assign { expr, .. } | Stmt::AssignField { expr, .. } => {
                rewrite_module_expr(expr, visible);
            }
            Stmt::AssignIndex { index, expr, .. }
            | Stmt::AssignStructListIndex { index, expr, .. } => {
                rewrite_module_expr(index, visible);
                rewrite_module_expr(expr, visible);
            }
            Stmt::Call { name, args, .. } => {
                rewrite_symbol_name(name, ModuleSymbolKind::Function, visible);
                for arg in args {
                    rewrite_module_expr(arg, visible);
                }
            }
            Stmt::MethodCall { name, args, .. } | Stmt::StructListMethodCall { name, args, .. } => {
                rewrite_sort_function_argument(name, args, visible);
                for arg in args {
                    rewrite_module_expr(arg, visible);
                }
            }
            Stmt::Return { expr, .. } => {
                if let Some(expr) = expr {
                    rewrite_module_expr(expr, visible);
                }
            }
            Stmt::Print { expr, .. }
            | Stmt::Exit { expr, .. }
            | Stmt::BenchLoop {
                iterations: expr, ..
            }
            | Stmt::Panic { message: expr, .. } => rewrite_module_expr(expr, visible),
            Stmt::Block { stmts, .. } => rewrite_module_stmts(stmts, visible),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_module_expr(cond, visible);
                rewrite_module_stmts(then_branch, visible);
                if let Some(else_branch) = else_branch {
                    rewrite_module_stmts(else_branch, visible);
                }
            }
            Stmt::While { cond, body, .. } => {
                rewrite_module_expr(cond, visible);
                rewrite_module_stmts(body, visible);
            }
            Stmt::Loop { body, .. } => rewrite_module_stmts(body, visible),
            Stmt::For {
                start, end, body, ..
            } => {
                rewrite_module_expr(start, visible);
                rewrite_module_expr(end, visible);
                rewrite_module_stmts(body, visible);
            }
            Stmt::ParFor {
                start,
                end,
                body,
                reduction,
                ..
            } => {
                rewrite_module_expr(start, visible);
                rewrite_module_expr(end, visible);
                rewrite_module_stmts(body, visible);
                if let Some(reduction) = reduction {
                    rewrite_module_expr(&mut reduction.expr, visible);
                }
            }
            Stmt::ForEach { iterable, body, .. } => {
                rewrite_module_expr(iterable, visible);
                rewrite_module_stmts(body, visible);
            }
            Stmt::Assert { cond, message, .. } => {
                rewrite_module_expr(cond, visible);
                if let Some(message) = message {
                    rewrite_module_expr(message, visible);
                }
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
}

fn rewrite_sort_function_argument(
    method: &str,
    args: &mut [Expr],
    visible: &BTreeMap<String, ModuleSymbol>,
) {
    if method != "sort_by" {
        return;
    }
    for arg in args {
        if let Expr::Ident { name, .. } = arg {
            rewrite_symbol_name(name, ModuleSymbolKind::Function, visible);
        }
    }
}

fn rewrite_module_expr(expr: &mut Expr, visible: &BTreeMap<String, ModuleSymbol>) {
    match expr {
        Expr::Bool { .. }
        | Expr::String { .. }
        | Expr::Char { .. }
        | Expr::Number { .. }
        | Expr::Ident { .. } => {}
        Expr::Call { name, args, .. } => {
            rewrite_symbol_name(name, ModuleSymbolKind::Function, visible);
            for arg in args {
                rewrite_module_expr(arg, visible);
            }
        }
        Expr::QualifiedCall { owner, args, .. } => {
            rewrite_symbol_name(owner, ModuleSymbolKind::Type, visible);
            for arg in args {
                rewrite_module_expr(arg, visible);
            }
        }
        Expr::Unary { expr, .. } => rewrite_module_expr(expr, visible),
        Expr::Binary { left, right, .. } => {
            rewrite_module_expr(left, visible);
            rewrite_module_expr(right, visible);
        }
        Expr::FieldAccess { base, .. } => rewrite_module_expr(base, visible),
        Expr::Index { base, index, .. } => {
            rewrite_module_expr(base, visible);
            rewrite_module_expr(index, visible);
        }
        Expr::ArrayLit { elems, .. } => {
            for elem in elems {
                rewrite_module_expr(elem, visible);
            }
        }
        Expr::StructInit { name, fields, .. } => {
            rewrite_symbol_name(name, ModuleSymbolKind::Type, visible);
            for field in fields {
                rewrite_module_expr(&mut field.expr, visible);
            }
        }
        Expr::EnumVariant { enum_name, .. } => {
            rewrite_symbol_name(enum_name, ModuleSymbolKind::Type, visible);
        }
        Expr::EnumTupleVariant {
            enum_name, args, ..
        } => {
            rewrite_symbol_name(enum_name, ModuleSymbolKind::Type, visible);
            for arg in args {
                rewrite_module_expr(arg, visible);
            }
        }
        Expr::EnumStructVariant {
            enum_name, fields, ..
        } => {
            rewrite_symbol_name(enum_name, ModuleSymbolKind::Type, visible);
            for field in fields {
                rewrite_module_expr(&mut field.expr, visible);
            }
        }
        Expr::Match { value, arms, .. } => {
            rewrite_module_expr(value, visible);
            for arm in arms {
                match &mut arm.pattern {
                    MatchPattern::Wildcard { .. } => {}
                    MatchPattern::EnumUnit { enum_name, .. }
                    | MatchPattern::EnumTuple { enum_name, .. }
                    | MatchPattern::EnumNamed { enum_name, .. } => {
                        rewrite_symbol_name(enum_name, ModuleSymbolKind::Type, visible);
                    }
                }
                rewrite_module_expr(&mut arm.expr, visible);
            }
        }
        Expr::DictLit { entries, .. } => {
            for entry in entries {
                rewrite_module_expr(&mut entry.value, visible);
            }
        }
        Expr::MethodCall {
            receiver,
            name,
            args,
            ..
        } => {
            rewrite_module_expr(receiver, visible);
            rewrite_sort_function_argument(name, args, visible);
            for arg in args {
                rewrite_module_expr(arg, visible);
            }
        }
    }
}

fn compile_program(stmts: &[LoweredStmt], options: &X86BackendOptions) -> CompiledProgram {
    let mut builder = X86Program::with_options(options.clone());
    let mut has_exit = false;
    let mut pending_print = Vec::new();

    let flush_print = |builder: &mut X86Program, pending_print: &mut Vec<u8>| {
        if !pending_print.is_empty() {
            builder.emit_write(pending_print);
            pending_print.clear();
        }
    };

    for stmt in stmts {
        match stmt {
            LoweredStmt::Print(value) => pending_print.extend_from_slice(value.as_bytes()),
            LoweredStmt::Exit(code) => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_exit(*code);
                has_exit = true;
            }
            LoweredStmt::RuntimeBenchLoop { iterations } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_bench_loop(*iterations);
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeLcgLoop {
                iterations,
                state_init,
                mul,
                add,
                exit_with_state,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_lcg_loop(
                    *iterations,
                    *state_init,
                    *mul,
                    *add,
                    *exit_with_state,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeSeededLcgLoop {
                iterations,
                mul,
                add,
                exit_with_state,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_seeded_lcg_loop(
                    *iterations,
                    *mul,
                    *add,
                    *exit_with_state,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeRingWriteLoop {
                iterations,
                state_init,
                index_init,
                mul,
                add,
                state_mask,
                ring_mask,
                value_shift,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                // Runtime ring-write with real x86-64 loop
                builder.emit_runtime_ring_write_loop(
                    *iterations,
                    *state_init,
                    *index_init,
                    *mul,
                    *add,
                    *state_mask,
                    *ring_mask,
                    *value_shift,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimePrefixScanLoop {
                batches,
                state_init,
                mul,
                add,
                state_mask,
                value_mask,
                width,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_prefix_scan_loop(
                    *batches,
                    *state_init,
                    *mul,
                    *add,
                    *state_mask,
                    *value_mask,
                    *width,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeBloomFilterLoop {
                state_init,
                build_iterations,
                query_iterations,
                hits_init,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_bloom_filter_loop(
                    *state_init,
                    *build_iterations,
                    *query_iterations,
                    *hits_init,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeBranchLcgLoop {
                iterations,
                state_init,
                state_mask,
                threshold,
                then_mul,
                then_add,
                else_mul,
                else_add,
                exit_with_state,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_branch_lcg_loop(
                    *iterations,
                    *state_init,
                    *state_mask,
                    *threshold,
                    *then_mul,
                    *then_add,
                    *else_mul,
                    *else_add,
                    *exit_with_state,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeSeededLcgAllocLoop {
                iterations,
                mul,
                add,
                alloc_bytes,
                exit_with_state,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_seeded_lcg_alloc_loop(
                    *iterations,
                    *mul,
                    *add,
                    *alloc_bytes,
                    *exit_with_state,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeSeededPredictableBranchLcgLoop {
                iterations,
                then_iterations,
                then_mul,
                then_add,
                else_mul,
                else_add,
                exit_with_state,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_seeded_predictable_branch_lcg_loop(
                    *iterations,
                    *then_iterations,
                    *then_mul,
                    *then_add,
                    *else_mul,
                    *else_add,
                    *exit_with_state,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeSeededUnpredictableBranchLcgLoop {
                iterations,
                threshold,
                then_mul,
                then_add,
                else_mul,
                else_add,
                exit_with_state,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_seeded_unpredictable_branch_lcg_loop(
                    *iterations,
                    *threshold,
                    *then_mul,
                    *then_add,
                    *else_mul,
                    *else_add,
                    *exit_with_state,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeSeededDualStateBranchLoop {
                iterations,
                index_init,
                adaptive,
                branchless,
                exit_with_sum,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_seeded_dual_state_branch_loop(
                    *iterations,
                    *index_init,
                    *adaptive,
                    *branchless,
                    *exit_with_sum,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeAffineIndexLoop {
                iterations,
                state_init,
                index_init,
                state_mul,
                index_mul,
                add,
                state_mask,
                exit_with_state,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_affine_index_loop(
                    *iterations,
                    *state_init,
                    *index_init,
                    *state_mul,
                    *index_mul,
                    *add,
                    *state_mask,
                    *exit_with_state,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeSeededAffineIndexLoop {
                iterations,
                index_init,
                state_mul,
                index_mul,
                add,
                state_mask,
                exit_with_state,
                exit_mask,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_seeded_affine_index_loop(
                    *iterations,
                    *index_init,
                    *state_mul,
                    *index_mul,
                    *add,
                    *state_mask,
                    *exit_with_state,
                    *exit_mask,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeSeededAffineClosedForm {
                state_mul,
                add,
                exit_with_state,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_seeded_affine_closed_form(*state_mul, *add, *exit_with_state);
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeSeededStructLatencyLoop {
                iterations,
                mul,
                add,
                exit_with_sum,
            } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_seeded_struct_latency_loop(
                    *iterations,
                    *mul,
                    *add,
                    *exit_with_sum,
                );
                has_exit = true;
                break;
            }
            LoweredStmt::RuntimeGeneric { program } => {
                flush_print(&mut builder, &mut pending_print);
                builder.emit_runtime_generic_program(program);
                has_exit = true;
                break;
            }
        }
    }

    flush_print(&mut builder, &mut pending_print);
    if !has_exit {
        builder.emit_exit(0);
    }

    let lir_dump = builder.runtime_generic_lir_dump();
    let profile_template = builder.runtime_generic_profile_template();
    let block_map = builder.runtime_generic_block_map();
    let code = builder.finalize();
    CompiledProgram {
        code,
        lir_dump,
        profile_template,
        block_map,
    }
}

type OutputFormat = ObjectFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactKind {
    Executable,
    Object,
    StaticLibrary,
    SharedLibrary,
}

impl ArtifactKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Executable => "executable",
            Self::Object => "object",
            Self::StaticLibrary => "static-library",
            Self::SharedLibrary => "shared-library",
        }
    }
}

#[derive(Debug, Clone)]
struct CompileArgs {
    output: PathBuf,
    target: TargetSpec,
    format: OutputFormat,
    artifact_kind: ArtifactKind,
    profile_generate: Option<PathBuf>,
    dump_lir: bool,
    backend_options: X86BackendOptions,
    package_options: PackageOptions,
}

struct CompiledProgram {
    code: Vec<u8>,
    lir_dump: Option<String>,
    profile_template: Option<String>,
    block_map: Option<String>,
}

fn parse_compile_args(args: &[String]) -> Result<CompileArgs, String> {
    let mut i = 0;
    let mut output = None;
    let mut format = None;
    let mut target = None;
    let mut artifact_kind = ArtifactKind::Executable;
    let mut profile_generate = None;
    let mut profile_use = None;
    let mut target_cpu = String::from("native");
    let mut target_features = TargetFeatureSet::default();
    let mut dump_lir = false;
    let mut emit_full_checksum = false;
    let mut preserve_full_checksum = false;
    let mut profile_instrument = false;
    let mut package_options = PackageOptions::defaults_enabled();
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| String::from("-o/--output requires a path"))?;
                output = Some(PathBuf::from(value));
                i += 2;
            }
            "--format" | "--target-format" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    String::from("--format requires one of: elf64, macho64, coff")
                })?;
                format = Some(OutputFormat::parse(value)?);
                i += 2;
            }
            "--target" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| String::from("--target requires a target triple"))?;
                target = Some(TargetSpec::parse(value)?);
                i += 2;
            }
            "--emit" | "--artifact-kind" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    String::from("--emit requires one of: executable, object, static-library, shared-library")
                })?;
                artifact_kind = match value.as_str() {
                    "executable" | "exe" => ArtifactKind::Executable,
                    "object" | "obj" => ArtifactKind::Object,
                    "static-library" | "static" => ArtifactKind::StaticLibrary,
                    "shared-library" | "shared" => ArtifactKind::SharedLibrary,
                    other => {
                        return Err(format!(
                            "unsupported artifact kind '{other}' (expected executable, object, static-library, or shared-library)"
                        ));
                    }
                };
                i += 2;
            }
            "--profile-generate" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| String::from("--profile-generate requires a path"))?;
                profile_generate = Some(PathBuf::from(value));
                i += 2;
            }
            "--profile-use" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| String::from("--profile-use requires a path"))?;
                profile_use = Some(PathBuf::from(value));
                i += 2;
            }
            "--target-cpu" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| String::from("--target-cpu requires a value"))?;
                target_cpu = value.clone();
                i += 2;
            }
            "--target-features" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| String::from("--target-features requires a value"))?;
                target_features = parse_target_features(value)?;
                i += 2;
            }
            "--dump-lir" => {
                dump_lir = true;
                i += 1;
            }
            "--emit-full-checksum" => {
                emit_full_checksum = true;
                preserve_full_checksum = true;
                i += 1;
            }
            "--preserve-full-checksum" => {
                preserve_full_checksum = true;
                i += 1;
            }
            "--profile-instrument" => {
                profile_instrument = true;
                i += 1;
            }
            "--features" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| String::from("--features requires a comma-separated list"))?;
                add_package_features(&mut package_options, value)?;
                i += 2;
            }
            "--no-default-features" => {
                package_options.default_features = false;
                i += 1;
            }
            "--package-cache" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| String::from("--package-cache requires a path"))?;
                package_options.cache_dir = Some(PathBuf::from(value));
                i += 2;
            }
            other => {
                return Err(format!("unknown argument: {other}\n{}", usage()));
            }
        }
    }

    let output = output.ok_or_else(|| String::from("missing -o/--output"))?;
    let target_was_explicit = target.is_some();
    let target = target.unwrap_or_default();
    let format = format.unwrap_or(target.object_format);
    if target_was_explicit {
        target.validate_explicit_format(format)?;
    }
    let runtime_generic_profile = if let Some(path) = profile_use {
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read profile '{}': {e}", path.display()))?;
        let profile = CompileProfile::parse(&text)?;
        profile.functions.get("runtime_generic").cloned()
    } else {
        None
    };

    Ok(CompileArgs {
        output,
        target,
        format,
        artifact_kind,
        profile_generate,
        dump_lir,
        backend_options: X86BackendOptions {
            target,
            target_cpu,
            target_features,
            runtime_generic_profile,
            emit_full_checksum,
            preserve_full_checksum,
            profile_instrument,
        },
        package_options,
    })
}

fn parse_package_options(args: &[String]) -> Result<PackageOptions, String> {
    let mut options = PackageOptions::defaults_enabled();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--features" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| String::from("--features requires a comma-separated list"))?;
                add_package_features(&mut options, value)?;
                index += 2;
            }
            "--no-default-features" => {
                options.default_features = false;
                index += 1;
            }
            "--package-cache" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| String::from("--package-cache requires a path"))?;
                options.cache_dir = Some(PathBuf::from(value));
                index += 2;
            }
            other => return Err(format!("unknown package option '{other}'")),
        }
    }
    Ok(options)
}

fn add_package_features(options: &mut PackageOptions, value: &str) -> Result<(), String> {
    for feature in value.split(',') {
        let feature = feature.trim();
        if feature.is_empty() {
            return Err(String::from("--features contains an empty feature name"));
        }
        options.features.insert(feature.to_string());
    }
    Ok(())
}

fn parse_emit_args(args: &[String]) -> Result<(bool, String), String> {
    let mut exit_only = false;
    let mut message = String::from("Hello from aziky\n");

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--exit-only" => {
                exit_only = true;
                i += 1;
            }
            "--message" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    String::from("--message requires a value (example: --message 'hi\\n')")
                })?;
                message = decode_escaped_newlines(value);
                i += 2;
            }
            other => {
                return Err(format!("unknown argument: {other}\n{}", usage()));
            }
        }
    }

    Ok((exit_only, message))
}

fn write_executable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_output(path, bytes, true)
}

fn write_output(path: &Path, bytes: &[u8], executable: bool) -> io::Result<()> {
    fs::write(path, bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(if executable { 0o755 } else { 0o644 });
        fs::set_permissions(path, perms)?;
    }

    Ok(())
}

fn decode_escaped_newlines(input: &str) -> String {
    input.replace("\\\\n", "\n").replace("\\n", "\n")
}

fn usage() -> String {
    String::from(
        "usage:\n\
         aziky compile <input.azk> -o <output> [--target <triple>] [--emit executable|object|static-library|shared-library] [--format elf64|macho64|coff] [--features <list>] [--no-default-features] [--package-cache <path>] [--profile-generate <path>] [--profile-instrument] [--profile-use <path>] [--target-cpu <cpu|native>] [--target-features <list>] [--dump-lir] [--preserve-full-checksum] [--emit-full-checksum]\n\
         aziky profile-merge <template> <raw-counts> -o <profile>\n\
         aziky check <input.azk> [--diagnostic-format json] [--features <list>] [--no-default-features] [--package-cache <path>]\n\
         aziky package lock [path] [--features <list>] [--no-default-features] [--package-cache <path>]\n\
         aziky package verify [path] [--features <list>] [--no-default-features] [--package-cache <path>]\n\
         aziky package checksum <cached-package-directory>\n\
         aziky test [path] [--filter <substring>] [--list] [--timeout-ms <n>] [--features <list>] [--no-default-features] [--package-cache <path>]\n\
         aziky fmt [paths...] [--check] [--stdout]\n\
         aziky fmt --stdin\n\
         aziky lint [paths...] [--deny-warnings|--check] [--diagnostic-format=json]\n\
         aziky target list\n\
         aziky target show <triple>\n\
         aziky emit <output-bin> [--exit-only] [--message <text>]\n\
         aziky <output-bin> [--exit-only] [--message <text>]\n\
         examples:\n\
         aziky compile ./examples/hello.azk -o ./hello.bin\n\
         aziky check ./examples/hello.azk\n\
         aziky compile ./examples/hello.azk -o ./hello.bin --dump-lir\n\
         aziky compile ./examples/hello.azk -o ./hello.macho --format macho64\n\
         aziky emit ./out.bin --exit-only\n\
         aziky ./hello.bin --message 'Hello aziky\\n'",
    )
}

fn parse_target_features(value: &str) -> Result<TargetFeatureSet, String> {
    if value.trim().is_empty() || value == "default" {
        return Ok(TargetFeatureSet::default());
    }

    let mut features = TargetFeatureSet {
        avx2: false,
        avx512f: false,
        bmi2: false,
        popcnt: false,
    };
    if value == "none" {
        return Ok(features);
    }

    for part in value.split(',') {
        let feature = part.trim().trim_start_matches('+');
        let enabled = !part.trim().starts_with('-');
        match feature {
            "avx2" => features.avx2 = enabled,
            "avx512f" => features.avx512f = enabled,
            "bmi2" => features.bmi2 = enabled,
            "popcnt" => features.popcnt = enabled,
            other => {
                return Err(format!(
                    "unsupported target feature '{other}' (expected avx2,avx512f,bmi2,popcnt)"
                ));
            }
        }
    }
    Ok(features)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parse_target_features_allows_none() {
        let features = parse_target_features("none").expect("features should parse");
        assert!(!features.avx2);
        assert!(!features.avx512f);
        assert!(!features.bmi2);
        assert!(!features.popcnt);
    }

    #[test]
    fn parse_test_args_tracks_filter_timeout_and_package_features() {
        let args = vec![
            "examples/package_app".to_string(),
            "--filter".to_string(),
            "math".to_string(),
            "--timeout-ms".to_string(),
            "250".to_string(),
            "--features".to_string(),
            "extra,other".to_string(),
            "--no-default-features".to_string(),
        ];
        let parsed = parse_test_args(&args).expect("test args should parse");
        assert_eq!(parsed.path, PathBuf::from("examples/package_app"));
        assert_eq!(parsed.filter.as_deref(), Some("math"));
        assert_eq!(parsed.timeout, Duration::from_millis(250));
        assert!(!parsed.package_options.default_features);
        assert!(parsed.package_options.features.contains("extra"));
        assert!(parsed.package_options.features.contains("other"));
    }

    #[test]
    fn parse_compile_args_reads_profile_and_flags() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough")
            .as_nanos();
        let profile_path = env::temp_dir().join(format!("aziky-profile-{unique}.txt"));
        fs::write(
            &profile_path,
            "function runtime_generic\nblock 0 7\nedge 0 1 7\nblock 1 7\nend\n",
        )
        .expect("should write profile");

        let args = vec![
            "-o".to_string(),
            "out.bin".to_string(),
            "--profile-use".to_string(),
            profile_path.to_string_lossy().to_string(),
            "--target-cpu".to_string(),
            "x86-64-v3".to_string(),
            "--target-features".to_string(),
            "avx2,bmi2,popcnt".to_string(),
            "--dump-lir".to_string(),
            "--profile-instrument".to_string(),
            "--preserve-full-checksum".to_string(),
        ];
        let parsed = parse_compile_args(&args).expect("compile args should parse");
        assert_eq!(parsed.output, PathBuf::from("out.bin"));
        assert!(parsed.dump_lir);
        assert!(parsed.backend_options.profile_instrument);
        assert!(parsed.backend_options.preserve_full_checksum);
        assert_eq!(parsed.backend_options.target_cpu, "x86-64-v3");
        assert!(parsed.backend_options.target_features.avx2);
        assert!(!parsed.backend_options.target_features.avx512f);
        assert_eq!(
            parsed
                .backend_options
                .runtime_generic_profile
                .as_ref()
                .and_then(|profile| profile.block_exec_count(0)),
            Some(7)
        );

        let _ = fs::remove_file(profile_path);
    }

    #[test]
    fn parse_compile_args_selects_library_artifacts() {
        let args = vec![
            "-o".to_string(),
            "libsample.a".to_string(),
            "--emit".to_string(),
            "static-library".to_string(),
            "--format".to_string(),
            "macho64".to_string(),
        ];
        let parsed = parse_compile_args(&args).expect("artifact options should parse");
        assert_eq!(parsed.artifact_kind, ArtifactKind::StaticLibrary);
        assert_eq!(parsed.format, OutputFormat::Macho64);
        assert_eq!(parsed.target, TargetSpec::LINUX_X86_64);
    }

    #[test]
    fn parse_compile_args_accepts_explicit_canonical_target() {
        let args = vec![
            "-o".to_string(),
            "app".to_string(),
            "--target".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
        ];
        let parsed = parse_compile_args(&args).expect("explicit Linux target should parse");
        assert_eq!(parsed.target, TargetSpec::LINUX_X86_64);
        assert_eq!(parsed.format, OutputFormat::Elf64);
        assert_eq!(parsed.backend_options.target, parsed.target);
    }

    #[test]
    fn parse_compile_args_rejects_explicit_target_format_mismatch() {
        let args = vec![
            "-o".to_string(),
            "app".to_string(),
            "--target".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
            "--format".to_string(),
            "macho64".to_string(),
        ];
        let error = parse_compile_args(&args).expect_err("target/format mismatch must fail");
        assert!(error.contains("explicit format 'macho64' is incompatible"));
    }

    fn temporary_project(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough")
            .as_nanos();
        let path = env::temp_dir().join(format!("aziky-{name}-{unique}"));
        fs::create_dir_all(&path).expect("create temporary project");
        path
    }

    #[test]
    fn project_loader_compiles_declared_sibling_module() {
        let project = temporary_project("modules");
        let main_path = project.join("main.azk");
        fs::write(
            &main_path,
            "mod math; use math::add; fn main() { print(add(2i32, 3i32).to_str()); exit(0u64); }",
        )
        .expect("write main module");
        fs::write(
            project.join("math.azk"),
            "pub fn add(left: i32, right: i32) -> i32 { return left + right; }",
        )
        .expect("write math module");

        let lowered = parse_lower_optimize(&main_path).expect("project should compile");
        assert!(
            lowered
                .iter()
                .any(|stmt| matches!(stmt, LoweredStmt::Print(value) if value == "5"))
        );
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn package_loader_resolves_locked_transitive_and_feature_dependencies() {
        let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/package_app");
        let main = project.join("src/main.azk");
        let graph = package::resolve_for_input(&main, &PackageOptions::defaults_enabled())
            .expect("resolve checked-in package graph")
            .expect("package manifest should be discovered");
        assert_eq!(graph.packages.len(), 3);
        assert!(graph.root_dependencies.contains_key("math"));
        assert!(graph.root_dependencies.contains_key("flavor"));
        let lowered = parse_lower_optimize(&main).expect("lower package application");
        assert!(!lowered.is_empty());
    }

    #[test]
    fn package_resolution_reports_stable_conflicts_and_cycles() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/package_app/invalid");
        let conflict =
            package::write_lock(&root.join("conflict"), &PackageOptions::defaults_enabled())
                .expect_err("version conflict must fail");
        assert!(conflict.contains("package version conflict for 'shared'"));

        let cycle = package::write_lock(&root.join("cycle"), &PackageOptions::defaults_enabled())
            .expect_err("dependency cycle must fail");
        assert!(cycle.contains("package dependency cycle detected"));
    }

    #[test]
    fn project_loader_isolates_private_names_and_requires_imports() {
        let project = temporary_project("module-isolation");
        let main_path = project.join("main.azk");
        fs::write(
            &main_path,
            "mod math; use math::answer; fn helper() -> u64 { return 2u64; } fn main() { print(helper().to_str()); print(answer().to_str()); exit(0u64); }",
        )
        .expect("write main module");
        fs::write(
            project.join("math.azk"),
            "fn helper() -> u64 { return 40u64; } pub fn answer() -> u64 { return helper() + 2u64; }",
        )
        .expect("write math module");

        let lowered = parse_lower_optimize(&main_path).expect("isolated project should compile");
        let prints: Vec<&str> = lowered
            .iter()
            .filter_map(|stmt| match stmt {
                LoweredStmt::Print(value) => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(prints, vec!["242"]);

        fs::write(
            &main_path,
            "mod math; fn main() { print(answer().to_str()); exit(0u64); }",
        )
        .expect("rewrite main without import");
        let error = parse_lower_optimize(&main_path).expect_err("unimported name must be hidden");
        assert!(error.contains("unknown function 'answer'"));
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn project_loader_enforces_private_exports_and_import_collisions() {
        let private_project = temporary_project("module-private");
        let private_main = private_project.join("main.azk");
        fs::write(
            &private_main,
            "mod math; use math::hidden; fn main() { exit(0u64); }",
        )
        .expect("write private import root");
        fs::write(
            private_project.join("math.azk"),
            "fn hidden() -> u64 { return 1u64; }",
        )
        .expect("write private module");
        let error = load_project_program(&private_main).expect_err("private import must fail");
        assert!(error.contains("item 'math::hidden' is private"));
        let _ = fs::remove_dir_all(private_project);

        let collision_project = temporary_project("module-import-collision");
        let collision_main = collision_project.join("main.azk");
        fs::write(
            &collision_main,
            "mod left; mod right; use left::value; use right::value; fn main() { exit(0u64); }",
        )
        .expect("write colliding imports");
        fs::write(
            collision_project.join("left.azk"),
            "pub fn value() -> u64 { return 1u64; }",
        )
        .expect("write left module");
        fs::write(
            collision_project.join("right.azk"),
            "pub fn value() -> u64 { return 2u64; }",
        )
        .expect("write right module");
        let error = load_project_program(&collision_main).expect_err("collision must fail");
        assert!(error.contains("imported name 'value' conflicts"));
        let _ = fs::remove_dir_all(collision_project);
    }

    #[test]
    fn project_loader_supports_public_reexports_and_imported_types() {
        let project = temporary_project("module-reexports");
        let main_path = project.join("main.azk");
        fs::write(
            &main_path,
            "mod facade; use facade::Counter as PublicCounter; use facade::exposed_answer as final_answer; fn main() { let value: PublicCounter = PublicCounter::new(40u64); print(value.read().to_str()); print(final_answer().to_str()); exit(0u64); }",
        )
        .expect("write reexport root");
        fs::write(
            project.join("facade.azk"),
            "mod inner; pub use inner::Counter; pub use inner::answer as exposed_answer;",
        )
        .expect("write facade module");
        fs::write(
            project.join("inner.azk"),
            "pub struct Counter { value: u64; } impl Counter { fn new(value: u64) -> Self { return Self { value: value }; } fn read(self: &Self) -> u64 { return self.value; } } pub fn answer() -> u64 { let value: Counter = Counter::new(42u64); return value.read(); }",
        )
        .expect("write inner module");

        let lowered = parse_lower_optimize(&main_path).expect("reexport project should compile");
        let prints: Vec<&str> = lowered
            .iter()
            .filter_map(|stmt| match stmt {
                LoweredStmt::Print(value) => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(prints, vec!["4042"]);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn project_loader_compiles_embedded_user_language_std() {
        let project = temporary_project("embedded-std");
        let main_path = project.join("main.azk");
        fs::write(
            &main_path,
            "mod feature; mod std; use feature::reported_version; use std::AzikyVersion; use std::ParseError; use std::version; use std::checked_char_at; use std::checked_i64; use std::new_i32_list; fn main() { let current: AzikyVersion = version(); let mut values: list<i32> = new_i32_list(); values.push(20i32); values.push(22i32); let parsed: Result<i64, ParseError> = checked_i64(\"42\"); let letter: Option<char> = checked_char_at(\"λx\", 0u64); print(current.to_string()); print(parsed.unwrap_or(0i64).to_str()); print(letter.unwrap_or('?')); print(reported_version()); exit(0u64); }",
        )
        .expect("write embedded std root");
        fs::write(
            project.join("feature.azk"),
            "mod std; use std::version; pub fn reported_version() -> string { return version().to_string(); }",
        )
        .expect("write module that also imports embedded std");

        let lowered =
            parse_lower_optimize(&main_path).expect("embedded std project should compile");
        let prints: Vec<&str> = lowered
            .iter()
            .filter_map(|stmt| match stmt {
                LoweredStmt::Print(value) => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(prints, vec!["0.1.042λ0.1.0"]);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn embedded_stdlib_exports_checked_conversion_and_ordering_contracts() {
        let project = temporary_project("embedded-core-contracts");
        let main_path = project.join("main.azk");
        fs::write(
            &main_path,
            "mod std; use std::Ordering; use std::ParseError; use std::compare_i64; use std::ordering_is_less; use std::checked_char; use std::checked_i8; use std::checked_u64; use std::checked_f32; use std::checked_bool; fn main() { let order: Ordering = compare_i64(-4i64, 7i64); let letter: Option<char> = checked_char(955u32); let small: Result<i8, ParseError> = checked_i8(\"-8\"); let wide: Result<u64, ParseError> = checked_u64(\"18446744073709551615\"); let float: Result<f32, ParseError> = checked_f32(\"1.5\"); let boolean: Result<bool, ParseError> = checked_bool(\"true\"); print(ordering_is_less(order).to_str()); print(letter.unwrap_or('?')); print(small.unwrap_or(0i8).to_str()); print(wide.is_ok().to_str()); print(float.is_ok().to_str()); print(boolean.unwrap_or(false).to_str()); exit(0u64); }",
        )
        .expect("write embedded core-contract source");

        let lowered =
            parse_lower_optimize(&main_path).expect("embedded core contracts should compile");
        let prints: Vec<&str> = lowered
            .iter()
            .filter_map(|stmt| match stmt {
                LoweredStmt::Print(value) => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(prints, vec!["trueλ-8truetruetrue"]);
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_u64_list_executes_growth_mutation_and_cleanup_in_native_elf() {
        let project = temporary_project("owned-u64-list-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn main() { let mut values: list<u64> = []; values.push(10u64); values.push(11u64); values.push(12u64); values.push(13u64); values.push(14u64); values.push(15u64); values.push(16u64); values.push(17u64); values.push(18u64); values.push(19u64); values[5u64] = 42u64; values.pop(); values.reserve(20u64); values.shrink_to(12u64); let mut total: u64 = 0u64; foreach value in values { total = total + value; } if values.contains(42u64) { total = total + 1u64; } values.clear(); values.shrink_to_fit(); exit(total + values.len()); }",
        )
        .expect("write owned list program");

        let lowered = parse_lower_optimize(&main_path).expect("lower owned list program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native owned list binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned list binary");
        assert_eq!(status.code(), Some(154));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_heap_pointer_move_executes_single_native_release() {
        let project = temporary_project("owned-heap-move-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn main() { let p: u64 = heap_alloc(4096u64); let q: u64 = p; heap_free(q, 4096u64); exit(7u64); }",
        )
        .expect("write owned heap move program");

        let lowered = parse_lower_optimize(&main_path).expect("lower owned heap move program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native owned heap move binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned heap move binary");
        assert_eq!(status.code(), Some(7));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_lifecycle_executes_in_native_elf() {
        let project = temporary_project("runtime-file-lifecycle");
        let main_path = project.join("main.azk");
        let data_path = project.join("payload.txt");
        let binary_path = project.join("main.bin");
        let source = format!(
            "fn read_some(file: &File) -> string {{ let content: string = file.read(64u64); return content; }} fn finish(file: File) {{ file.close(); }} fn main() {{ let raw: string = \"{}\"; let base: Path = Path::new(raw); let segment: string = \"payload.txt\"; let path: Path = base.join(segment); let text: string = \"Aziky λ🙂\"; let output: File = File::create(path); let written: u64 = output.write_all(text); finish(output); let input: File = File::open_read(path); let content: string = read_some(&input); finish(input); if written != 12u64 {{ exit(1u64); }} if content.len() != 12u64 {{ exit(2u64); }} if content.char_count() != 8u64 {{ exit(3u64); }} exit(0u64); }}",
            project.display()
        );
        fs::write(&main_path, source).expect("write phase two file program");

        let lowered = parse_lower_optimize(&main_path).expect("lower phase two file program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let compiled_again = compile_program(&lowered, &X86BackendOptions::default());
        assert_eq!(compiled.code, compiled_again.code);
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native phase two file binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native phase two file binary");
        assert_eq!(status.code(), Some(0));
        assert_eq!(
            fs::read_to_string(&data_path).expect("read generated payload"),
            "Aziky λ🙂"
        );

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_file_has_deterministic_exit_code() {
        let project = temporary_project("runtime-missing-file");
        let main_path = project.join("main.azk");
        let missing_path = project.join("does-not-exist.txt");
        let binary_path = project.join("main.bin");
        let source = format!(
            "fn main() {{ let path: string = \"{}\"; let input: File = File::open_read(path); input.close(); exit(0u64); }}",
            missing_path.display()
        );
        fs::write(&main_path, source).expect("write missing file program");

        let lowered = parse_lower_optimize(&main_path).expect("lower missing file program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native missing file binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native missing file binary");
        assert_eq!(status.code(), Some(102));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_embedded_nul_before_native_open() {
        let project = temporary_project("runtime-nul-path");
        let main_path = project.join("main.azk");
        let truncated_path = project.join("must-not-exist.txt");
        let binary_path = project.join("main.bin");
        let source = format!(
            "fn main() {{ let raw: string = \"{}\0hidden\"; let path: Path = Path::new(raw); let output: File = File::create(path); output.close(); exit(0u64); }}",
            truncated_path.display()
        );
        fs::write(&main_path, source).expect("write embedded-NUL path program");

        let lowered = parse_lower_optimize(&main_path).expect("lower embedded-NUL path program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native embedded-NUL path binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native embedded-NUL path binary");
        assert_eq!(status.code(), Some(105));
        assert!(!truncated_path.exists());

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn startup_clock_and_process_services_execute_natively() {
        let project = temporary_project("runtime-platform-loads");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        let source = "mod std; use std::argument_count; use std::argument; use std::environment_count; use std::environment_entry; use std::monotonic_nanos; use std::wall_time_nanos; use std::process_id; fn main() { let argc: u64 = argument_count(); let first_arg: string = argument(1u64); let envc: u64 = environment_count(); let first_env: string = environment_entry(0u64); let first: u64 = monotonic_nanos(); let second: u64 = monotonic_nanos(); let wall: u64 = wall_time_nanos(); let pid: u64 = process_id(); if argc != 3u64 { exit(1u64); } if first_arg.len() != 5u64 { exit(2u64); } if envc == 0u64 { exit(3u64); } if first_env.len() == 0u64 { exit(4u64); } if second < first { exit(5u64); } if wall == 0u64 { exit(6u64); } if pid == 0u64 { exit(7u64); } exit(0u64); }";
        fs::write(&main_path, source).expect("write platform load program");

        let lowered = parse_lower_optimize(&main_path).expect("lower platform load program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let compiled_again = compile_program(&lowered, &X86BackendOptions::default());
        assert_eq!(compiled.code, compiled_again.code);
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write platform load binary");
        let status = std::process::Command::new(&binary_path)
            .args(["alpha", "beta"])
            .status()
            .expect("execute platform load binary");
        assert_eq!(status.code(), Some(0));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn multifile_platform_release_gate_is_reproducible() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = root.join("examples/platform_app/main.azk");
        let project = temporary_project("runtime-platform-release");
        let binary_path = project.join("platform-app.bin");
        let output_path = project.join("platform-output.txt");

        let lowered = parse_lower_optimize(&source).expect("lower multi-file platform example");
        let first = compile_program(&lowered, &X86BackendOptions::default());
        let second = compile_program(&lowered, &X86BackendOptions::default());
        assert_eq!(first.code, second.code);
        write_executable(&binary_path, &emit_elf64_executable(&first.code))
            .expect("write multi-file platform binary");
        let status = std::process::Command::new(&binary_path)
            .current_dir(&project)
            .status()
            .expect("execute multi-file platform example");
        assert_eq!(status.code(), Some(28));
        assert_eq!(
            fs::read_to_string(output_path).expect("read platform example output"),
            "Aziky platform"
        );

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shared_allocator_aligns_payloads_and_handles_multiple_slabs() {
        let project = temporary_project("shared-allocator-alignment-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn main() { let seed: u64 = runtime_seed(); let small: u64 = heap_alloc(1u64); let large: u64 = heap_alloc(70000u64); heap_free(small, 1u64); heap_free(large, 70000u64); exit(seed & 0u64); }",
        )
        .expect("write shared allocator alignment program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower shared allocator alignment program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write shared allocator alignment binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run shared allocator alignment binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shared_allocator_reports_oversized_allocation_failure_deterministically() {
        let project = temporary_project("shared-allocator-failure-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn main() { let value: u64 = heap_alloc(18446744073709551615u64); heap_free(value, 18446744073709551615u64); exit(0u64); }",
        )
        .expect("write shared allocator failure program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower shared allocator failure program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write shared allocator failure binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run shared allocator failure binary")
                .code(),
            Some(101)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shared_allocator_grows_large_owned_list_across_slabs() {
        let project = temporary_project("shared-allocator-list-growth-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn main() { let seed: u64 = runtime_seed(); let mut values: list<u64> = []; for i in 0u64..20000u64 { values.push(i); } if values.len() != 20000u64 { exit(1u64); } if values[19999u64] != 19999u64 { exit(2u64); } exit(seed & 0u64); }",
        )
        .expect("write shared allocator list-growth program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower shared allocator list-growth program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write shared allocator list-growth binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run shared allocator list-growth binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_list_move_executes_without_copying_in_native_elf() {
        let project = temporary_project("owned-list-move-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn main() { let mut values: list<u16> = [1u16, 2u16, 3u16, 4u16]; let mut moved: list<u16> = values; moved.push(5u16); moved.reserve(8u64); let mut total: u16 = 0u16; foreach value in moved { total = total + value; } exit(total); }",
        )
        .expect("write owned list move program");

        let lowered = parse_lower_optimize(&main_path).expect("lower owned list move program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native owned list move binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned list move binary");
        assert_eq!(status.code(), Some(15));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_list_argument_move_executes_in_native_elf() {
        let project = temporary_project("owned-list-call-move-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn consume(values: list<u16>) -> u64 { return values.len(); } fn main() { let mut values: list<u16> = [1u16, 2u16, 3u16, 4u16]; exit(consume(values)); }",
        )
        .expect("write owned list call move program");

        let lowered = parse_lower_optimize(&main_path).expect("lower owned list call move program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native owned list call move binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned list call move binary");
        assert_eq!(status.code(), Some(4));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_list_return_move_executes_in_native_elf() {
        let project = temporary_project("owned-list-return-move-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn make() -> list<u16> { let mut values: list<u16> = [4u16, 5u16]; return values; } fn main() { let mut result: list<u16> = make(); result.push(6u16); let mut total: u16 = 0u16; foreach value in result { total = total + value; } exit(total); }",
        )
        .expect("write owned list return move program");
        let lowered =
            parse_lower_optimize(&main_path).expect("lower owned list return move program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native owned list return move binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned list return move binary");
        assert_eq!(status.code(), Some(15));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_list_return_move_executes_in_native_elf() {
        let project = temporary_project("owned-struct-list-return-move-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Pair { left: u16; right: u16; } fn make() -> list<Pair> { let mut values: list<Pair> = [Pair { left: 4u16, right: 5u16 }]; return values; } fn main() { let result: list<Pair> = make(); let first: Pair = result.first().unwrap_or(Pair { left: 0u16, right: 0u16 }); exit(first.left + first.right); }",
        )
        .expect("write owned struct-list return move program");
        let lowered =
            parse_lower_optimize(&main_path).expect("lower owned struct-list return move program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf)
            .expect("write native owned struct-list return move binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned struct-list return move binary");
        assert_eq!(status.code(), Some(9));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scalar_struct_call_and_return_execute_in_native_elf() {
        let project = temporary_project("scalar-struct-call-return-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Pair { left: u16; right: u16; } fn add(pair: Pair) -> Pair { let mut result: Pair = pair; result.left = result.left + 2u16; result.right = result.right + 3u16; return result; } fn main() { let mut input: Pair = Pair { left: 4u16, right: 5u16 }; let output: Pair = add(input); exit(output.left + output.right); }",
        )
        .expect("write native scalar struct call and return program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower native scalar struct call and return program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf)
            .expect("write native scalar struct call and return binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native scalar struct call and return binary");
        assert_eq!(status.code(), Some(14));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inherent_receiver_methods_execute_as_direct_native_calls() {
        let project = temporary_project("inherent-receiver-direct-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Counter { value: i32; } impl Counter { fn current(self: &Self) -> i32 { return self.value; } fn add(self: &mut Self, amount: i32) { self.value = self.value + amount; } } fn main() { let seed: u64 = runtime_seed(); let mut counter: Counter = Counter { value: 4i32 }; counter.add(3i32); let value: i32 = counter.current(); if value != 7i32 { exit(1u64); } exit(seed & 0u64); }",
        )
        .expect("write receiver-method program");
        let lowered = parse_lower_optimize(&main_path).expect("lower receiver-method program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write receiver-method binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run receiver-method binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tagged_enums_match_and_cross_native_call_abi() {
        let project = temporary_project("tagged-enum-call-match-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "enum Event<T> { Idle, Value(T), Pair { left: T; right: T; }, } fn pass(event: Event<u16>) -> Event<u16> { return event; } fn make() -> Event<u16> { return Event::Pair { left: 5u16, right: 8u16 }; } fn main() { let seed: u64 = runtime_seed(); let input: Event<u16> = make(); let output: Event<u16> = pass(input); let value: u16 = match output { Event::Idle => 0u16, Event::Value(value) => value, Event::Pair { left: a, right: b } => a + b, }; if value != 13u16 { exit(1u64); } exit(seed & 0u64); }",
        )
        .expect("write tagged-enum program");
        let lowered = parse_lower_optimize(&main_path).expect("lower tagged-enum program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write tagged-enum binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run tagged-enum binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn option_result_tagged_abi_executes_in_native_elf() {
        let project = temporary_project("option-result-tagged-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn maybe(value: u16) -> Option<u16> { return Option::Some(value); } fn decide(ok: bool) -> Result<u16, u8> { if ok { return Result::Ok(13u16); } return Result::Err(2u8); } fn main() { let seed: u64 = runtime_seed(); let option: Option<u16> = maybe(12u16); if option.is_none() { exit(2u64); } let first: u16 = option.unwrap_or(0u16); if first != 12u16 { exit(3u64); } let result: Result<u16, u8> = decide(true); let second: u16 = match result { Result::Ok(value) => value, Result::Err(_) => 0u16, }; if second != 13u16 { exit(4u64); } exit(seed & 0u64); }",
        )
        .expect("write Option/Result program");
        let lowered = parse_lower_optimize(&main_path).expect("lower Option/Result program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write Option/Result binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run Option/Result binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nested_typed_error_crosses_result_native_abi() {
        let project = temporary_project("nested-typed-error-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "enum ParseError { Invalid, Overflow, } fn parse(enabled: bool) -> Result<u16, ParseError> { if enabled { return Result::Ok(42u16); } let error: ParseError = ParseError::Invalid; return Result::Err(error); } fn main() { let ok: Result<u16, ParseError> = parse(true); let failed: Result<u16, ParseError> = parse(false); let value: u16 = match ok { Result::Ok(value) => value, Result::Err(_) => 0u16, }; let code: u16 = match failed { Result::Ok(_) => 0u16, Result::Err(error) => match error { ParseError::Invalid => 1u16, ParseError::Overflow => 2u16, }, }; if value != 42u16 { exit(1u64); } if code != 1u16 { exit(2u64); } exit(0u64); }",
        )
        .expect("write nested typed-error program");
        let lowered =
            parse_lower_optimize(&main_path).expect("lower nested typed-error program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write nested typed-error binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run nested typed-error binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn checked_integer_boolean_parsing_executes_natively() {
        let project = temporary_project("checked-native-parsing");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn main() { let seed: u64 = runtime_seed(); let i8_text: string = \"-128\"; let i16_text: string = \"+32767\"; let i32_text: string = \"2147483647\"; let i64_text: string = \"-9223372036854775808\"; let u8_text: string = \"+255\"; let u16_text: string = \"65535\"; let u32_text: string = \"4294967295\"; let u64_text: string = \"18446744073709551615\"; let overflow_text: string = \"18446744073709551616\"; let true_text: string = \"true\"; let false_text: string = \"false\"; let invalid_bool_text: string = \"False\"; let i8_value: Result<i8, string> = i8_text.parse_i8(); let i16_value: Result<i16, string> = i16_text.parse_i16(); let i32_value: Result<i32, string> = i32_text.parse_i32(); let i64_value: Result<i64, string> = i64_text.parse_i64(); let u8_value: Result<u8, string> = u8_text.parse_u8(); let u16_value: Result<u16, string> = u16_text.parse_u16(); let u32_value: Result<u32, string> = u32_text.parse_u32(); let u64_value: Result<u64, string> = u64_text.parse_u64(); let overflow: Result<u64, string> = overflow_text.parse_u64(); let truth: Result<bool, string> = true_text.parse_bool(); let falsity: Result<bool, string> = false_text.parse_bool(); let invalid_bool: Result<bool, string> = invalid_bool_text.parse_bool(); if i8_value.is_err() { exit(1u64); } if i16_value.unwrap_or(0i16) != 32767i16 { exit(2u64); } if i32_value.unwrap_or(0i32) != 2147483647i32 { exit(3u64); } if i64_value.is_err() { exit(4u64); } if u8_value.unwrap_or(0u8) != 255u8 { exit(5u64); } if u16_value.unwrap_or(0u16) != 65535u16 { exit(6u64); } if u32_value.unwrap_or(0u32) != 4294967295u32 { exit(7u64); } if u64_value.unwrap_or(0u64) != 18446744073709551615u64 { exit(8u64); } if overflow.is_ok() { exit(9u64); } if !truth.unwrap_or(false) { exit(10u64); } if falsity.unwrap_or(true) { exit(11u64); } if invalid_bool.is_ok() { exit(12u64); } let integer_error_len: u64 = match overflow { Result::Ok(_) => 0u64, Result::Err(message) => message.len(), }; let boolean_error_len: u64 = match invalid_bool { Result::Ok(_) => 0u64, Result::Err(message) => message.len(), }; if integer_error_len != 15u64 { exit(13u64); } if boolean_error_len != 15u64 { exit(14u64); } exit(seed & 0u64); }",
        )
        .expect("write checked native-parsing program");
        let lowered =
            parse_lower_optimize(&main_path).expect("lower checked parsing program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write checked native-parsing binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run checked native-parsing binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recursively_nested_owned_enum_cleans_through_result_native_abi() {
        let project = temporary_project("nested-owned-enum-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "enum CoreError { Missing, } fn nested(enabled: bool) -> Result<Option<list<u64> >, CoreError> { if enabled { let values: list<u64> = [3u64, 4u64]; let option: Option<list<u64> > = Option::Some(values); return Result::Ok(option); } let error: CoreError = CoreError::Missing; return Result::Err(error); } fn main() { let result: Result<Option<list<u64> >, CoreError> = nested(true); let total: u64 = match result { Result::Ok(option) => match option { Option::Some(values) => values[0u64] + values[1u64], Option::None => 0u64, }, Result::Err(_) => 0u64, }; if total != 7u64 { exit(1u64); } exit(0u64); }",
        )
        .expect("write recursively nested owned-enum program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower recursively nested owned-enum program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write recursively nested owned-enum binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run recursively nested owned-enum binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stdlib_abi_and_ordering_contract_execute_in_native_elf() {
        let project = temporary_project("stdlib-core-contract-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "mod std; use std::Ordering; use std::compare_i64; use std::compare_u64; use std::stdlib_abi_version; fn main() { let seed: u64 = runtime_seed(); if stdlib_abi_version() != 1u64 { exit(10u64); } let signed: Ordering = compare_i64(-2i64, 4i64); let unsigned: Ordering = compare_u64(9u64, 3u64); let equal: Ordering = compare_u64(5u64, 5u64); let signed_score: u64 = match signed { Ordering::Less => 1u64, Ordering::Equal => 20u64, Ordering::Greater => 30u64, }; let unsigned_score: u64 = match unsigned { Ordering::Less => 40u64, Ordering::Equal => 50u64, Ordering::Greater => 2u64, }; let equal_score: u64 = match equal { Ordering::Less => 60u64, Ordering::Equal => 3u64, Ordering::Greater => 70u64, }; if signed_score != 1u64 { exit(11u64); } if unsigned_score != 2u64 { exit(12u64); } if equal_score != 3u64 { exit(13u64); } exit(seed & 0u64); }",
        )
        .expect("write native stdlib core-contract source");

        let lowered = parse_lower_optimize(&main_path).expect("lower native stdlib core contracts");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write native stdlib core-contract binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run native stdlib core-contract binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_option_result_contract_app_executes_in_native_elf() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = root.join("examples/core_contract_app/main.azk");
        let project = temporary_project("owned-fallibility-contract");
        let binary = project.join("main.bin");
        let lowered = parse_lower_optimize(&source)
            .expect("owned Option/Result contract application must lower");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary, &emit_elf64_executable(&compiled.code))
            .expect("write owned Option/Result contract application");
        assert_eq!(
            std::process::Command::new(&binary)
                .status()
                .expect("run owned Option/Result contract application")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foundation_chunk_one_integrated_program_executes_natively() {
        let project = temporary_project("foundation-integrated");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Pair { left: u16; right: u16; } struct Bag { values: list<Pair>; } struct Counter { value: u16; } impl Counter { fn add(self: &mut Self, amount: u16) { self.value = self.value + amount; } fn read(self: &Self) -> u16 { return self.value; } } enum Outcome<T> { Empty, Value(T), } fn wrap(value: u16) -> Outcome<u16> { return Outcome::Value(value); } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { values: [Pair { left: 1u16, right: 2u16 }] }; bag.values[0u64] = Pair { left: 4u16, right: 5u16 }; if bag.values.len() != 1u64 { exit(2u64); } let mut counter: Counter = Counter { value: 4u16 }; counter.add(3u16); let outcome: Outcome<u16> = wrap(counter.read()); let value: u16 = match outcome { Outcome::Empty => 0u16, Outcome::Value(number) => number, }; if value != 7u16 { exit(1u64); } exit(seed & 0u64); }",
        )
        .expect("write integrated Chunk 1 program");
        let lowered =
            parse_lower_optimize(&main_path).expect("lower integrated Chunk 1 program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write integrated Chunk 1 binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run integrated Chunk 1 binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resource_owning_struct_receiver_borrow_abi_executes_natively() {
        let project = temporary_project("owned-struct-receiver-borrow-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Bag { count: u64; values: list<u16>; } impl Bag { fn add(self: &mut Self, value: u16) { self.values.push(value); self.count = self.count + 1u64; } fn len(self: &Self) -> u64 { return self.values.len(); } } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { count: 1u64, values: [3u16] }; bag.add(5u16); bag.add(8u16); bag.add(13u16); bag.add(21u16); if bag.count != 5u64 { exit(1u64); } if bag.len() != 5u64 { exit(2u64); } if bag.values[1u64] != 5u16 { exit(3u64); } if bag.values[4u64] != 21u16 { exit(4u64); } exit(seed & 0u64); }",
        )
        .expect("write resource-owning receiver program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower resource-owning receiver program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write resource-owning receiver binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run resource-owning receiver binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn borrowed_resource_receiver_cleans_on_terminal_path() {
        let project = temporary_project("borrowed-resource-terminal-cleanup-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Bag { values: list<u16>; } impl Bag { fn fail(self: &Self) { panic(\"borrowed failure\"); } } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { values: [3u16, 5u16] }; if seed == 0u64 { exit(1u64); } bag.fail(); exit(2u64); }",
        )
        .expect("write borrowed terminal cleanup program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower borrowed terminal cleanup program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write borrowed terminal cleanup binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run borrowed terminal cleanup binary")
                .code(),
            Some(101)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resource_enum_moves_list_through_native_abi_and_match() {
        let project = temporary_project("resource-enum-list-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "enum Payload { Empty, Values(list<u16>), } fn wrap(values: list<u16>) -> Payload { return Payload::Values(values); } fn consume(payload: Payload) -> u64 { return match payload { Payload::Empty => 0u64, Payload::Values(items) => items.len(), }; } fn main() { let seed: u64 = runtime_seed(); let values: list<u16> = [3u16, 5u16, 8u16]; let payload: Payload = wrap(values); let count: u64 = consume(payload); if count != 3u64 { exit(1u64); } exit(seed & 0u64); }",
        )
        .expect("write resource enum program");
        let lowered =
            parse_lower_optimize(&main_path).expect("lower resource enum program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write resource enum binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run resource enum binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_utf8_string_moves_through_native_abi() {
        let project = temporary_project("owned-utf8-string-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn pass(text: string) -> string { return text; } fn main() { let seed: u64 = runtime_seed(); let text: string = \"λ🙂x\"; let moved: string = pass(text); if moved.len() != 7u64 { exit(1u64); } if moved.char_count() != 3u64 { exit(2u64); } let first: char = moved.char_at(0u64).unwrap_or('?'); let second: char = moved.char_at(1u64).unwrap_or('?'); if first != 'λ' { exit(3u64); } if second != '🙂' { exit(4u64); } if moved.char_at(3u64).is_some() { exit(5u64); } exit(seed & 0u64); }",
        )
        .expect("write owned UTF-8 string program");
        let lowered =
            parse_lower_optimize(&main_path).expect("lower owned UTF-8 string program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write owned UTF-8 string binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run owned UTF-8 string binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_map_dynamic_keys_growth_and_remove_execute_natively() {
        let project = temporary_project("owned-map-dynamic-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn pass(scores: map<u64, u16>) -> map<u64, u16> { return scores; } fn main() { let seed: u64 = runtime_seed(); let mut scores: map<u64, u16> = {}; let key: u64 = seed & 7u64; scores.set(key, 11u16); scores.set(key + 1u64, 13u16); scores.set(key + 2u64, 17u16); scores.set(key + 3u64, 19u16); scores.set(key + 4u64, 23u16); let mut moved: map<u64, u16> = pass(scores); moved.set(key, 29u16); let value: u16 = moved.get(key).unwrap_or(0u16); if value != 29u16 { exit(1u64); } if moved.len() != 5u64 { exit(2u64); } moved.remove(key + 2u64); if moved.get(key + 2u64).is_some() { exit(3u64); } if moved.len() != 4u64 { exit(4u64); } exit(seed & 0u64); }",
        )
        .expect("write owned map program");
        let lowered = parse_lower_optimize(&main_path).expect("lower owned map program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write owned map binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run owned map binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nested_owned_struct_moves_resources_through_native_abi() {
        let project = temporary_project("nested-owned-struct-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Inner { values: list<u16>; } struct Outer { marker: u64; inner: Inner; } fn pass(value: Outer) -> Outer { return value; } fn main() { let seed: u64 = runtime_seed(); let outer: Outer = Outer { marker: 9u64, inner: Inner { values: [2u16, 4u16, 6u16] } }; let moved: Outer = pass(outer); if moved.marker != 9u64 { exit(1u64); } if moved.inner.values.len() != 3u64 { exit(2u64); } exit(seed & 0u64); }",
        )
        .expect("write nested owned struct program");
        let lowered =
            parse_lower_optimize(&main_path).expect("lower nested owned struct program natively");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write nested owned struct binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run nested owned struct binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_value_milestone_integrates_in_multifile_native_program() {
        let project = temporary_project("owned-value-multifile-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "mod transfer; use transfer::pass; struct Inner { values: list<u16>; } struct Outer { inner: Inner; } impl Outer { fn count(self: &Self) -> u64 { return self.inner.values.len(); } } enum Payload { Empty, Values(list<u16>), } fn consume(payload: Payload) -> u64 { return match payload { Payload::Empty => 0u64, Payload::Values(values) => values.len(), }; } fn main() { let seed: u64 = runtime_seed(); let text: string = \"λ🙂x\"; if text.char_count() != 3u64 { exit(1u64); } let mut scores: map<u64, u16> = {}; let key: u64 = seed & 7u64; scores.set(key, 11u16); if scores.get(key).unwrap_or(0u16) != 11u16 { exit(2u64); } let values: list<u16> = [2u16, 4u16, 6u16]; let moved: list<u16> = pass(values); let payload: Payload = Payload::Values(moved); if consume(payload) != 3u64 { exit(3u64); } let nested: Outer = Outer { inner: Inner { values: [8u16, 9u16] } }; if nested.count() != 2u64 { exit(4u64); } exit(seed & 0u64); }",
        )
        .expect("write owned-value integration root");
        fs::write(
            project.join("transfer.azk"),
            "pub fn pass(values: list<u16>) -> list<u16> { return values; }",
        )
        .expect("write owned-value integration module");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower multi-file owned-value program natively");
        assert!(matches!(
            lowered.last(),
            Some(LoweredStmt::RuntimeGeneric { .. })
        ));
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write owned-value integration binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run owned-value integration binary")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_literal_cleans_nested_list_field_in_native_elf() {
        let project = temporary_project("owned-struct-nested-list-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Bag { count: u64; values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { count: seed, values: [1u16, 2u16, 3u16] }; exit(bag.count & 0u64); }",
        )
        .expect("write native owned struct literal program");
        let lowered =
            parse_lower_optimize(&main_path).expect("lower native owned struct literal program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native owned struct literal binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned struct literal binary");
        assert_eq!(status.code(), Some(0));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_moves_through_call_and_return_in_native_elf() {
        let project = temporary_project("owned-struct-call-return-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Bag { count: u64; values: list<u16>; } fn pass(bag: Bag) -> Bag { return bag; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { count: seed, values: [1u16, 2u16] }; let result: Bag = pass(bag); exit(result.count & 0u64); }",
        )
        .expect("write native owned struct call and return program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower native owned struct call and return program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf)
            .expect("write native owned struct call and return binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned struct call and return binary");
        assert_eq!(status.code(), Some(0));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_list_field_queries_execute_in_native_elf() {
        let project = temporary_project("owned-struct-list-field-queries-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Bag { values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { values: [1u16, 2u16] }; let count: u64 = bag.values.len(); let second: u16 = bag.values[1u64]; let empty: bool = bag.values.is_empty(); if empty { exit(1u64); } if second == 0u16 { exit(1u64); } exit(count + (seed & 0u64)); }",
        )
        .expect("write native owned struct list field query program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower native owned struct list field query program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf)
            .expect("write native owned struct list field query binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned struct list field query binary");
        assert_eq!(status.code(), Some(2));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_list_field_mutation_executes_in_native_elf() {
        let project = temporary_project("owned-struct-list-field-mutation-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Bag { values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { values: [1u16] }; bag.values.reserve(8u64); bag.values.push(2u16); bag.values[0u64] = 4u16; bag.values.pop(); bag.values.push(3u16); bag.values.shrink_to(2u64); bag.values.shrink_to_fit(); let value: u16 = bag.values[1u64]; if value == 0u16 { exit(1u64); } exit(bag.values.len() + (seed & 0u64)); }",
        )
        .expect("write native owned struct list field mutation program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower native owned struct list field mutation program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf)
            .expect("write native owned struct list field mutation binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned struct list field mutation binary");
        assert_eq!(status.code(), Some(2));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_struct_list_field_mutation_executes_in_native_elf() {
        let project = temporary_project("owned-struct-struct-list-field-mutation-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Pair { left: u16; right: u16; } struct Bag { values: list<Pair>; } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { values: [Pair { left: 1u16, right: 2u16 }] }; bag.values.reserve(8u64); bag.values.push(Pair { left: 3u16, right: 4u16 }); bag.values.pop(); bag.values.push(Pair { left: 5u16, right: 6u16 }); bag.values.shrink_to(2u64); bag.values.shrink_to_fit(); exit(bag.values.len() + (seed & 0u64)); }",
        )
        .expect("write native owned struct struct-list field mutation program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower native owned struct struct-list field mutation program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf)
            .expect("write native owned struct struct-list field mutation binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned struct struct-list field mutation binary");
        assert_eq!(status.code(), Some(2));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_struct_list_field_index_assignment_executes_in_native_elf() {
        let project = temporary_project("owned-struct-struct-list-index-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(&main_path, "struct Pair { left: u16; right: u16; } struct Bag { values: list<Pair>; } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { values: [Pair { left: 1u16, right: 2u16 }] }; bag.values[0u64] = Pair { left: 4u16, right: 5u16 }; exit(bag.values.len() + (seed & 0u64)); }").expect("write program");
        let lowered = parse_lower_optimize(&main_path).expect("lower program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write binary");
        assert_eq!(
            std::process::Command::new(&binary_path)
                .status()
                .expect("run binary")
                .code(),
            Some(1)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_list_field_foreach_executes_in_native_elf() {
        let project = temporary_project("owned-struct-list-field-foreach-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Bag { values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { values: [2u16, 3u16, 4u16] }; let mut total: u16 = 0u16; foreach value in bag.values { total = total + value; } if total == 0u16 { exit(1u64); } exit(bag.values.len() + (seed & 0u64)); }",
        )
        .expect("write native owned struct list field foreach program");
        let lowered = parse_lower_optimize(&main_path)
            .expect("lower native owned struct list field foreach program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf)
            .expect("write native owned struct list field foreach binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native owned struct list field foreach binary");
        assert_eq!(status.code(), Some(3));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_list_field_contains_executes_in_native_elf() {
        let project = temporary_project("owned-struct-list-field-contains-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(&main_path, "struct Bag { values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { values: [2u16, 3u16] }; let found: bool = bag.values.contains(3u16); if !found { exit(1u64); } exit(seed & 0u64); }").expect("write contains program");
        let lowered = parse_lower_optimize(&main_path).expect("lower contains program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        write_executable(&binary_path, &emit_elf64_executable(&compiled.code))
            .expect("write binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute binary");
        assert_eq!(status.code(), Some(0));
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_u64_list_checked_options_execute_in_native_elf() {
        let project = temporary_project("owned-u64-list-options-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn main() { let mut values: list<u64> = [10u64, 20u64, 30u64]; let got = values.get(1u64); let missing: Option<u64> = values.get(9u64); let head: Option<u64> = values.first(); let tail: Option<u64> = values.last(); let peeked: Option<u64> = values.peek(); let popped: Option<u64> = values.pop(); let second: Option<u64> = values.pop(); let first: Option<u64> = values.pop(); let absent: Option<u64> = values.pop(); let mut manual: Option<u64> = Option::None; manual = Option::Some(7u64); let mut total: u64 = got.unwrap_or(0u64) + missing.unwrap_or(4u64) + head.unwrap_or(0u64) + tail.unwrap_or(0u64) + peeked.unwrap_or(0u64) + popped.unwrap_or(0u64) + second.unwrap_or(0u64) + first.unwrap_or(0u64) + absent.unwrap_or(5u64) + manual.unwrap_or(0u64); if got.is_some() { total = total + 1u64; } if missing.is_none() { total = total + 1u64; } exit(total + values.get(0u64).unwrap_or(9u64) + values.len()); }",
        )
        .expect("write native checked-list program");

        let lowered = parse_lower_optimize(&main_path).expect("lower checked-list program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native checked-list binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native checked-list binary");
        assert_eq!(status.code(), Some(177));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_integer_lists_preserve_types_in_native_elf() {
        let cases = [
            (
                "owned-i8-list-native",
                "fn main() { let mut values: list<i8> = [-5i8, 7i8, -2i8]; values.push(4i8); values.push(-8i8); values.reserve(6u64); values.shrink_to_fit(); let head = values.first(); let tail: Option<i8> = values.last(); let peeked = values.peek(); let popped: Option<i8> = values.pop(); let missing = values.get(99u8); let mut total: i8 = 0i8; foreach value in values { total = total + value; } total = total + popped.unwrap_or(0i8) + missing.unwrap_or(6i8) + head.unwrap_or(0i8) + tail.unwrap_or(0i8) + peeked.unwrap_or(0i8) + 32i8; if values.contains(-2i8) { total = total + 1i8; } exit(total); }",
                14,
            ),
            (
                "owned-u16-list-native",
                "fn main() { let mut values: list<u16> = [65535u16, 2u16]; values.push(4u16); values.push(5u16); values.push(6u16); values.reserve(5u64); values.shrink_to_fit(); values[1u8] = 3u16; let popped = values.pop(); let missing: Option<u16> = values.get(99u8); let mut total: u16 = 0u16; foreach value in values { total = total + value; } total = total + popped.unwrap_or(0u16) + missing.unwrap_or(5u16); if values.contains(65535u16) { total = total + 1u16; } exit(total); }",
                23,
            ),
            (
                "owned-u32-list-native",
                "fn main() { let mut values: list<u32> = [4294967295u32, 2u32, 3u32, 4u32]; values.push(5u32); values.reserve(5u64); values.shrink_to_fit(); values[1u8] = 6u32; let popped = values.pop(); let mut total: u32 = 0u32; foreach value in values { total = total + value; } total = total + popped.unwrap_or(0u32); if values.contains(4294967295u32) { total = total + 1u32; } exit(total); }",
                18,
            ),
            (
                "owned-bool-list-native",
                "fn main() { let mut values: list<bool> = [true, false]; values.push(true); values.push(false); values.push(true); values.shrink_to_fit(); let first = values.first(); let popped: Option<bool> = values.pop(); if first.unwrap_or(false) && popped.unwrap_or(false) && values.contains(false) { exit(1u8); } exit(0u8); }",
                1,
            ),
        ];

        for (project_name, source, expected_exit) in cases {
            let project = temporary_project(project_name);
            let main_path = project.join("main.azk");
            let binary_path = project.join("main.bin");
            fs::write(&main_path, source).expect("write native integer-list program");

            let lowered = parse_lower_optimize(&main_path).expect("lower integer-list program");
            let compiled = compile_program(&lowered, &X86BackendOptions::default());
            let elf = emit_elf64_executable(&compiled.code);
            write_executable(&binary_path, &elf).expect("write native integer-list binary");
            let status = std::process::Command::new(&binary_path)
                .status()
                .expect("execute native integer-list binary");
            assert_eq!(status.code(), Some(expected_exit), "case {project_name}");

            let _ = fs::remove_dir_all(project);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_float_lists_execute_packed_operations_in_native_elf() {
        let cases = [
            (
                "owned-f32-list-native",
                "fn main() { let mut values: list<f32> = [1.5f32, -0.0f32, 2.25f32, 3.5f32]; values.push(4.75f32); values.reserve(5u64); values.shrink_to_fit(); values[0u8] = 6.5f32; let head: Option<f32> = values.first(); let popped = values.pop(); let missing: Option<f32> = values.get(99u8); let mut total: f32 = 0.0f32; foreach value in values { total = total + value; } let combined: f32 = total + popped.unwrap_or(0.0f32) + missing.unwrap_or(8.0f32); if values.contains(head.unwrap_or(0.0f32)) && values.contains(0.0f32) && !values.contains(combined) && head.is_some() && missing.is_none() { exit(1u8); } exit(0u8); }",
            ),
            (
                "owned-f64-list-native",
                "fn main() { let mut values: list<f64> = [-0.0f64, 1.25f64, 2.5f64, 3.75f64]; values.push(5.0f64); values.shrink_to_fit(); let popped: Option<f64> = values.pop(); let tail = values.last(); let mut total: f64 = 0.0f64; foreach value in values { total = total + value; } let nan: f64 = 0.0f64 / 0.0f64; values.push(nan); if values.contains(0.0f64) && values.contains(tail.unwrap_or(0.0f64)) && !values.contains(total + popped.unwrap_or(0.0f64)) && !values.contains(nan) { exit(1u8); } exit(0u8); }",
            ),
        ];

        for (project_name, source) in cases {
            let project = temporary_project(project_name);
            let main_path = project.join("main.azk");
            let binary_path = project.join("main.bin");
            fs::write(&main_path, source).expect("write native float-list program");

            let lowered = parse_lower_optimize(&main_path).expect("lower float-list program");
            let compiled = compile_program(&lowered, &X86BackendOptions::default());
            let elf = emit_elf64_executable(&compiled.code);
            write_executable(&binary_path, &elf).expect("write native float-list binary");
            let status = std::process::Command::new(&binary_path)
                .status()
                .expect("execute native float-list binary");
            assert_eq!(status.code(), Some(1), "case {project_name}");

            let _ = fs::remove_dir_all(project);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_lists_execute_aligned_aos_operations_in_native_elf() {
        let project = temporary_project("owned-struct-list-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Pair { small: u8; wide: u64; mid: u16; } fn main() { let mut values: list<Pair> = [Pair { small: 1u8, wide: 10u64, mid: 2u16 }, Pair { small: 3u8, wide: 20u64, mid: 4u16 }, Pair { small: 5u8, wide: 30u64, mid: 6u16 }, Pair { small: 7u8, wide: 40u64, mid: 8u16 }]; values.push(Pair { small: 9u8, wide: 50u64, mid: 10u16 }); values.reserve(5u64); values.shrink_to_fit(); values[1u8] = Pair { small: 11u8, wide: 40u64, mid: 12u16 }; let picked: Pair = values[1u8]; let mut total: u64 = 0u64; foreach pair in values { total = total + pair.wide; } if picked.small == 11u8 { if picked.wide == 40u64 { if picked.mid == 12u16 { if values.len() == 5u64 { exit(total); } } } } exit(0u8); }",
        )
        .expect("write native struct-list program");

        let lowered = parse_lower_optimize(&main_path).expect("lower struct-list program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native struct-list binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native struct-list binary");
        assert_eq!(status.code(), Some(170));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_struct_list_options_execute_in_native_elf() {
        let project = temporary_project("owned-struct-list-options-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Pair { small: u16; wide: u64; } fn main() { let mut values: list<Pair> = [Pair { small: 1u16, wide: 10u64 }, Pair { small: 2u16, wide: 20u64 }, Pair { small: 3u16, wide: 30u64 }]; let got = values.get(1u8); let missing: Option<Pair> = values.get(99u8); let head: Option<Pair> = values.first(); let tail = values.last(); let peeked: Option<Pair> = values.peek(); let popped: Option<Pair> = values.pop(); let absent = values.get(99u8); let mut manual: Option<Pair> = Option::None; manual = Option::Some(Pair { small: 7u16, wide: 7u64 }); let got_value: Pair = got.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let missing_value = missing.unwrap_or(Pair { small: 0u16, wide: 4u64 }); let head_value = head.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let tail_value = tail.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let peek_value = peeked.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let popped_value = popped.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let absent_value = absent.unwrap_or(Pair { small: 0u16, wide: 5u64 }); let manual_value = manual.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let direct_value: Pair = values.get(0u8).unwrap_or(Pair { small: 0u16, wide: 1u64 }); let mut total: u64 = got_value.wide + missing_value.wide + head_value.wide + tail_value.wide + peek_value.wide + popped_value.wide + absent_value.wide + manual_value.wide + direct_value.wide; if got.is_some() { total = total + 1u64; } if missing.is_none() { total = total + 1u64; } exit(total + values.len()); }",
        )
        .expect("write native aggregate-list-option program");

        let lowered =
            parse_lower_optimize(&main_path).expect("lower aggregate-list-option program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native aggregate-list-option binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native aggregate-list-option binary");
        assert_eq!(status.code(), Some(150));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_float_struct_lists_execute_in_native_elf() {
        let project = temporary_project("owned-float-struct-list-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Sample { id: u16; score: f32; weight: f64; } fn main() { let mut values: list<Sample> = [Sample { id: 1u16, score: 1.5f32, weight: 1.0f64 }, Sample { id: 2u16, score: 2.5f32, weight: 2.0f64 }, Sample { id: 3u16, score: 3.5f32, weight: 3.0f64 }, Sample { id: 4u16, score: 4.5f32, weight: 4.0f64 }]; values.push(Sample { id: 5u16, score: 5.5f32, weight: 5.0f64 }); values.reserve(4u64); values.shrink_to_fit(); values[1u8] = Sample { id: 20u16, score: 20.5f32, weight: 20.0f64 }; let picked: Sample = values[1u8]; let got: Option<Sample> = values.get(1u8); let missing: Option<Sample> = values.get(99u8); let popped: Option<Sample> = values.pop(); let got_value = got.unwrap_or(Sample { id: 0u16, score: 0.0f32, weight: 1.0f64 }); let missing_value = missing.unwrap_or(Sample { id: 0u16, score: 0.0f32, weight: 7.0f64 }); let popped_value = popped.unwrap_or(Sample { id: 0u16, score: 0.0f32, weight: 1.0f64 }); let mut total: f64 = 0.0f64; foreach value in values { total = total + value.weight; } if total == 28.0f64 { exit(picked.id + got_value.id + missing_value.id + popped_value.id); } exit(0u8); }",
        )
        .expect("write native float-struct-list program");

        let lowered = parse_lower_optimize(&main_path).expect("lower float-struct-list program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native float-struct-list binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native float-struct-list binary");
        assert_eq!(status.code(), Some(45));

        let _ = fs::remove_dir_all(project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_nested_struct_lists_execute_in_native_elf() {
        let project = temporary_project("owned-nested-struct-list-native");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "struct Point { x: u16; score: f32; weight: f64; } struct Record { id: u8; point: Point; } fn main() { let mut values: list<Record> = [Record { id: 1u8, point: Point { x: 10u16, score: 1.5f32, weight: 1.0f64 } }, Record { id: 2u8, point: Point { x: 20u16, score: 2.5f32, weight: 2.0f64 } }]; values.push(Record { id: 3u8, point: Point { x: 30u16, score: 3.5f32, weight: 3.0f64 } }); values.reserve(4u64); values[1u8] = Record { id: 4u8, point: Point { x: 40u16, score: 4.5f32, weight: 4.0f64 } }; let picked: Record = values[1u8]; let got: Option<Record> = values.get(1u8); let missing: Option<Record> = values.get(99u8); let chosen = got.unwrap_or(Record { id: 0u8, point: Point { x: 1u16, score: 0.0f32, weight: 1.0f64 } }); let fallback = missing.unwrap_or(Record { id: 5u8, point: Point { x: 6u16, score: 0.0f32, weight: 7.0f64 } }); let mut total: f64 = 0.0f64; foreach row in values { total = total + row.point.weight; } if total == 8.0f64 { if picked.point.x == 40u16 { if chosen.point.x == 40u16 { if fallback.point.x == 6u16 { exit(picked.id + fallback.id); } } } } exit(0u8); }",
        )
        .expect("write native nested-struct-list program");

        let lowered = parse_lower_optimize(&main_path).expect("lower nested-struct-list program");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write native nested-struct-list binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute native nested-struct-list binary");
        assert_eq!(status.code(), Some(9));

        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn foundation_application_is_runtime_native_and_reproducible() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = root.join("examples/foundation_app/main.azk");
        let first = parse_lower_optimize(&input).expect("lower foundation application");
        let second = parse_lower_optimize(&input).expect("repeat foundation lowering");
        assert_eq!(first.len(), 1, "application must lower as one native unit");
        assert!(matches!(
            first.first(),
            Some(LoweredStmt::RuntimeGeneric { .. })
        ));
        let first_code = compile_program(&first, &X86BackendOptions::default()).code;
        let second_code = compile_program(&second, &X86BackendOptions::default()).code;
        assert_eq!(
            first_code, second_code,
            "machine code must be deterministic"
        );
        assert_eq!(
            emit_elf64_executable(&first_code),
            emit_elf64_executable(&second_code)
        );
        assert_eq!(
            emit_macho64_executable(&first_code),
            emit_macho64_executable(&second_code)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foundation_application_executes_as_native_elf() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = root.join("examples/foundation_app/main.azk");
        let project = temporary_project("foundation-release-native");
        let binary = project.join("foundation-app");
        let lowered = parse_lower_optimize(&input).expect("lower foundation application");
        let code = compile_program(&lowered, &X86BackendOptions::default()).code;
        write_executable(&binary, &emit_elf64_executable(&code)).expect("write foundation ELF");
        assert_eq!(
            std::process::Command::new(&binary)
                .status()
                .expect("execute foundation ELF")
                .code(),
            Some(0)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn foundation_negative_diagnostic_is_stable_and_source_aware() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = root.join("examples/foundation_app/invalid/main.azk");
        let child = root.join("examples/foundation_app/invalid/broken.azk");
        let first = parse_lower_optimize(&input).expect_err("invalid fixture must fail");
        let second = parse_lower_optimize(&input).expect_err("invalid fixture must still fail");
        assert_eq!(first, second);
        assert!(first.contains("type mismatch: expected u64, got bool at 2:5"));
        assert!(first.contains(&format!(" --> {}", child.display())));
        assert!(first.contains("    let value: u64 = true;\n    ^"));
    }

    #[test]
    fn project_semantic_diagnostics_render_the_imported_source() {
        let project = temporary_project("module-semantic-provenance");
        let main_path = project.join("main.azk");
        let broken_path = project.join("broken.azk");
        fs::write(
            &main_path,
            "mod broken; use broken::answer; fn main() { print(answer().to_str()); exit(0u64); }",
        )
        .expect("write provenance root");
        fs::write(
            &broken_path,
            "\npub fn answer() -> u64 { let value: u64 = true; return value; }",
        )
        .expect("write broken imported module");

        let first = parse_lower_optimize(&main_path).expect_err("child type error must fail");
        let second = parse_lower_optimize(&main_path).expect_err("diagnostic must be repeatable");
        assert_eq!(first, second, "diagnostic rendering must be deterministic");
        assert!(first.contains("type mismatch: expected u64, got bool at 2:26"));
        assert!(first.contains(&format!(" --> {}", broken_path.display())));
        assert!(first.contains("pub fn answer() -> u64 { let value: u64 = true;"));
        assert!(!first.contains(&format!(" --> {}", main_path.display())));
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn project_ownership_diagnostics_render_the_imported_source() {
        let project = temporary_project("module-ownership-provenance");
        let main_path = project.join("main.azk");
        let broken_path = project.join("broken.azk");
        fs::write(
            &main_path,
            "mod broken; use broken::answer; fn main() { let seed: u64 = runtime_seed(); exit(answer() + (seed & 0u64)); }",
        )
        .expect("write ownership provenance root");
        fs::write(
            &broken_path,
            "\npub fn answer() -> u64 { let values: list<u64> = [1u64]; let moved: list<u64> = values; return values.len(); }",
        )
        .expect("write broken owner module");

        let error = parse_lower_optimize(&main_path).expect_err("use after move must fail");
        assert!(
            error.contains("resource owner 'values' was moved or consumed"),
            "unexpected ownership diagnostic: {error}"
        );
        assert!(error.contains(&format!(" --> {}", broken_path.display())));
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn embedded_module_parse_diagnostics_keep_the_virtual_filename() {
        let mut sources = ProjectSources::default();
        let location = ModuleLocation::Builtin(BuiltinModule::Core);
        let error = parse_module_source(&location, "pub fn broken( {".to_string(), &mut sources)
            .expect_err("malformed embedded source must fail");
        assert!(error.contains(" --> <aziky:std::core>"));
        assert!(error.contains("pub fn broken( {"));
    }

    #[test]
    fn embedded_stdlib_abi_rejects_malformed_and_mismatched_versions() {
        validate_stdlib_abi(EMBEDDED_STDLIB_ABI_VERSION)
            .expect("checked-in stdlib ABI must match the compiler");

        let malformed =
            validate_stdlib_abi("version one").expect_err("malformed ABI version must be rejected");
        assert_eq!(
            malformed,
            "embedded Aziky standard-library ABI version must be one unsigned integer"
        );

        let mismatched =
            validate_stdlib_abi("2").expect_err("mismatched ABI version must be rejected");
        assert_eq!(
            mismatched,
            "Aziky standard-library ABI mismatch: compiler requires 1, embedded library provides 2"
        );
    }

    #[test]
    fn required_native_lowering_reports_a_stable_capability_code() {
        let project = temporary_project("required-native-capability");
        let main_path = project.join("main.azk");
        fs::write(
            &main_path,
            "fn main() { let seed: u64 = runtime_seed(); benchloop(10u64); exit(seed & 0u64); }",
        )
        .expect("write required-native source");

        let error =
            parse_lower_optimize(&main_path).expect_err("unsupported native code must fail");
        assert!(error.contains("[AZL002] native execution is required"));
        assert!(error.contains("unsupported construct reachable from function 'main'"));
        assert!(error.contains("at 1:1"));
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn multi_file_artifacts_are_byte_identical_across_fresh_compilations() {
        let project = temporary_project("multi-file-artifact-determinism");
        let main_path = project.join("main.azk");
        fs::write(
            &main_path,
            "mod math; use math::answer; fn main() { exit(answer()); }",
        )
        .expect("write deterministic root");
        fs::write(
            project.join("math.azk"),
            "pub fn answer() -> u64 { return 42u64; }",
        )
        .expect("write deterministic child");

        let first = parse_lower_optimize(&main_path).expect("first lowering");
        let second = parse_lower_optimize(&main_path).expect("second lowering");
        let first_code = compile_program(&first, &X86BackendOptions::default()).code;
        let second_code = compile_program(&second, &X86BackendOptions::default()).code;
        assert_eq!(first_code, second_code);
        assert_eq!(
            emit_elf64_executable(&first_code),
            emit_elf64_executable(&second_code)
        );
        assert_eq!(
            emit_macho64_executable(&first_code),
            emit_macho64_executable(&second_code)
        );
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn project_loader_is_recursive_and_deterministic() {
        let project = temporary_project("nested-modules");
        let main_path = project.join("main.azk");
        fs::write(
            &main_path,
            "mod outer; use outer::answer; fn main() { print(answer().to_str()); exit(0u64); }",
        )
        .expect("write main module");
        fs::write(
            project.join("outer.azk"),
            "mod inner; use inner::base; pub fn answer() -> u64 { return base() + 2u64; }",
        )
        .expect("write outer module");
        fs::create_dir_all(project.join("outer")).expect("create nested module directory");
        fs::write(
            project.join("inner.azk"),
            "pub fn base() -> u64 { return 40u64; }",
        )
        .expect("write inner module");

        let (first, _) = load_project_program(&main_path).expect("first load");
        let (second, _) = load_project_program(&main_path).expect("second load");
        assert_eq!(first, second);
        let lowered = parse_lower_optimize(&main_path).expect("nested project should compile");
        assert!(
            lowered
                .iter()
                .any(|stmt| matches!(stmt, LoweredStmt::Print(value) if value == "42"))
        );
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn project_loader_rejects_cycles_and_invalid_imports() {
        let cycle_project = temporary_project("module-cycle");
        let cycle_main = cycle_project.join("main.azk");
        fs::write(&cycle_main, "mod a; fn main() { exit(0u64); }").expect("write main");
        fs::write(cycle_project.join("a.azk"), "mod b; fn a() { }").expect("write a");
        fs::write(cycle_project.join("b.azk"), "mod a; fn b() { }").expect("write b");
        let error = load_project_program(&cycle_main).expect_err("cycle must fail");
        assert!(error.contains("module cycle detected"));
        let _ = fs::remove_dir_all(cycle_project);

        let import_project = temporary_project("module-import");
        let import_main = import_project.join("main.azk");
        fs::write(
            &import_main,
            "mod math; use math::missing; fn main() { exit(0u64); }",
        )
        .expect("write main");
        fs::write(import_project.join("math.azk"), "fn add() { }").expect("write math");
        let error = load_project_program(&import_main).expect_err("invalid import must fail");
        assert!(error.contains("has no exported item 'missing'"));
        let _ = fs::remove_dir_all(import_project);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_threads_and_channels_execute_fifo_stress_and_failure_join() {
        let project = temporary_project("native-thread-channel-stress");
        let main_path = project.join("main.azk");
        let binary_path = project.join("main.bin");
        fs::write(
            &main_path,
            "fn produce(sender: Sender<u64>, count: u64) -> u64 { let mut i: u64 = 0u64; while i < count { sender.send(i + 1u64); i = i + 1u64; } sender.close(); return count; } fn consume(receiver: Receiver<u64>, count: u64) -> u64 { let mut i: u64 = 0u64; let mut sum: u64 = 0u64; while i < count { let value: u64 = receiver.recv(); sum = sum + value; i = i + 1u64; } receiver.close(); return sum; } fn main() { let channel: Channel<u64> = Channel::bounded(3u64); let sender: Sender<u64> = channel.sender(); let receiver: Receiver<u64> = channel.receiver(); let producer: Thread = Thread::spawn(produce, sender, 100u64); let consumer: Thread = Thread::spawn(consume, receiver, 100u64); let produced: u64 = producer.join(); let sum: u64 = consumer.join(); exit(sum); }",
        )
        .expect("write thread/channel stress program");
        let lowered = parse_lower_optimize(&main_path).expect("lower thread/channel stress");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write thread/channel binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute thread/channel binary");
        assert_eq!(status.code(), Some(186));

        fs::write(
            &main_path,
            "fn fail(value: u64) -> u64 { if value == 1u64 { panic(\"worker failed\"); } return 0u64; } fn main() { let thread: Thread = Thread::spawn(fail, 1u64); let status: u64 = thread.join(); exit(status); }",
        )
        .expect("write worker failure program");
        let lowered = parse_lower_optimize(&main_path).expect("lower worker failure");
        let compiled = compile_program(&lowered, &X86BackendOptions::default());
        let elf = emit_elf64_executable(&compiled.code);
        write_executable(&binary_path, &elf).expect("write worker failure binary");
        let status = std::process::Command::new(&binary_path)
            .status()
            .expect("execute worker failure binary");
        assert_eq!(status.code(), Some(101));
        let _ = fs::remove_dir_all(project);
    }
}
