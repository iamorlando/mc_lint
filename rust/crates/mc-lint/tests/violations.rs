use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

struct TempSource {
    dir: PathBuf,
}

impl TempSource {
    fn new(name: &str, source: &str) -> Self {
        let id = NEXT_TEST_DIR.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mc-lint-{name}-{}-{id}", std::process::id()));

        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("lib.rs"), source).unwrap();

        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TempSource {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn strict_hot_path_reports_representative_violations() {
    let source = TempSource::new(
        "strict",
        r#"
#[hot_path]
fn strict_kernel(
    owned: Vec<f64>,
    borrowed: &Vec<f64>,
    name: &String,
    input: &[f64],
) -> Result<(), &'static str> {
    let _storage = Vec::new();
    let _copy = input.to_vec();
    let _message = format!("paths: {}", input.len());
    println!("running");

    if input.len() > 1 {
        let _ = 1;
    }

    match input.len() {
        0 => {}
        _ => {}
    }

    let _ = input.len().checked_add(1).ok_or("overflow")?;
    assert!(input.len() > 0);
    Ok(())
}
"#,
    );

    let output = run_linter(source.path());
    let stderr = assert_failed(output);

    assert_contains_all(
        &stderr,
        &[
            "hot path violation in `strict_kernel`",
            "parameter `owned` takes owned `Vec<f64>`",
            "parameter `borrowed` uses `&Vec<f64>`",
            "parameter `name` uses `&String`",
            "allocation-like call `Vec::new` is not allowed",
            "allocation-like method `to_vec` is not allowed",
            "formatting macro `format!` is not allowed",
            "logging/stdout macro `println!` is not allowed",
            "branch `if` found in hot path",
            "branch `match` found in hot path",
            "validation-like method `checked_add` is not allowed",
            "validation-like method `ok_or` is not allowed",
            "`?` found in hot path",
            "panic/assert macro `assert!` is not allowed",
        ],
    );
}

#[test]
fn hot_path_reports_violations_in_direct_callees() {
    let source = TempSource::new(
        "callee",
        r#"
#[hot_path]
fn root(input: &[f64]) {
    helper(input);
}

fn helper(input: &[f64]) {
    let _storage = Vec::with_capacity(input.len());
}
"#,
    );

    let output = run_linter(source.path());
    let stderr = assert_failed(output);

    assert_contains_all(
        &stderr,
        &[
            "hot path violation in `helper`",
            "allocation-like call `Vec::with_capacity` is not allowed",
        ],
    );
}

#[test]
fn hot_path_reports_violations_in_transitive_unmarked_callees() {
    let source = TempSource::new(
        "transitive-callee",
        r#"
#[hot_path]
fn root(input: &[f64]) {
    first(input);
}

fn first(input: &[f64]) {
    second(input);
}

fn second(input: &[f64]) {
    let _label = String::with_capacity(input.len());
}
"#,
    );

    let output = run_linter(source.path());
    let stderr = assert_failed(output);

    assert_contains_all(
        &stderr,
        &[
            "hot path violation in `second`",
            "allocation-like call `String::with_capacity` is not allowed",
        ],
    );
}

#[test]
fn hot_path_reports_violations_in_module_qualified_callees() {
    let source = TempSource::new(
        "module-qualified-callee",
        r#"
#[hot_path]
fn root(input: &[f64]) {
    kernels::helper(input);
}

mod kernels {
    pub fn helper(input: &[f64]) {
        let _storage = Vec::with_capacity(input.len());
    }
}
"#,
    );

    let output = run_linter(source.path());
    let stderr = assert_failed(output);

    assert_contains_all(
        &stderr,
        &[
            "hot path violation in `helper`",
            "allocation-like call `Vec::with_capacity` is not allowed",
        ],
    );
}

#[test]
fn hot_path_reports_violations_in_impl_method_roots_and_callees() {
    let source = TempSource::new(
        "impl-method-callee",
        r#"
struct Kernel;

impl Kernel {
    #[hot_path]
    fn run(&self, input: &[f64]) {
        self.step(input);
        Self::associated(input);
        Kernel::static_step(input);
    }

    fn step(&self, input: &[f64]) {
        let _copy = input.to_vec();
    }

    fn associated(input: &[f64]) {
        let _storage = Vec::with_capacity(input.len());
    }

    fn static_step(input: &[f64]) {
        let _label = input.len().to_string();
    }
}
"#,
    );

    let output = run_linter(source.path());
    let stderr = assert_failed(output);

    assert_contains_all(
        &stderr,
        &[
            "hot path violation in `step`",
            "allocation-like method `to_vec` is not allowed",
            "hot path violation in `associated`",
            "allocation-like call `Vec::with_capacity` is not allowed",
            "hot path violation in `static_step`",
            "allocation-like method `to_string` is not allowed",
        ],
    );
}

#[test]
fn hot_path_checks_documented_method_calls() {
    let source = TempSource::new(
        "documented-method-calls",
        r#"
#[hot_path]
fn root(input: &[u64]) {
    let _collected: Vec<_> = input.iter().collect();
    let _copy = input.to_vec();
    let _text = input.len().to_string();
    let _value = Some(1u64).unwrap();
    let _also = Some(2u64).expect("value");
    let _result = Some(3u64).ok_or("missing");
}
"#,
    );

    let output = run_linter(source.path());
    let stderr = assert_failed(output);

    assert_contains_all(
        &stderr,
        &[
            "allocation-like method `collect` is not allowed",
            "allocation-like method `to_vec` is not allowed",
            "allocation-like method `to_string` is not allowed",
            "panic-like method `unwrap` is not allowed",
            "panic-like method `expect` is not allowed",
            "validation-like method `ok_or` is not allowed",
        ],
    );
}

#[test]
fn hot_path_reports_violations_in_called_local_functions() {
    let source = TempSource::new(
        "local-callee",
        r#"
#[hot_path]
fn root(input: &[f64]) {
    fn local_helper(input: &[f64]) {
        let _storage = Vec::with_capacity(input.len());
    }

    local_helper(input);
}
"#,
    );

    let output = run_linter(source.path());
    let stderr = assert_failed(output);

    assert_contains_all(
        &stderr,
        &[
            "hot path violation in `local_helper`",
            "allocation-like call `Vec::with_capacity` is not allowed",
        ],
    );

    assert_not_contains(
        &stderr,
        "hot path violation in `root`: allocation-like call",
    );
}

#[test]
fn hot_path_reaches_callees_through_conditions_and_cfg_gates() {
    let source = TempSource::new(
        "conditional-cfg-callee",
        r#"
#[hot_path(allow_branching)]
fn root(input: &[f64]) {
    if input.len() > 1 {
        conditional_helper(input);
    }

    #[cfg(feature = "fast_kernel")]
    {
        gated_helper(input);
    }
}

fn conditional_helper(input: &[f64]) {
    let _storage = Vec::with_capacity(input.len());
}

fn gated_helper(input: &[f64]) {
    let _copy = input.to_vec();
}
"#,
    );

    let output = run_linter(source.path());
    let stderr = assert_failed(output);

    assert_contains_all(
        &stderr,
        &[
            "hot path violation in `conditional_helper`",
            "allocation-like call `Vec::with_capacity` is not allowed",
            "hot path violation in `gated_helper`",
            "allocation-like method `to_vec` is not allowed",
        ],
    );
}

#[test]
fn does_not_lint_functions_that_are_not_reached_from_hot_paths() {
    let source = TempSource::new(
        "unreached-cold-functions",
        r#"
#[hot_path]
fn root(input: &[f64]) {
    fn cold_local() {
        let _storage = Vec::new();
    }

    let _ = input.len();
}

fn cold_top_level() {
    let _storage = Vec::new();
}
"#,
    );

    let output = run_linter(source.path());
    let stdout = assert_succeeded(output);

    assert!(
        stdout.contains("mc-lint: ok"),
        "expected success output to contain `mc-lint: ok`\nstdout:\n{stdout}"
    );
}

#[test]
fn does_not_lint_unreached_impl_methods() {
    let source = TempSource::new(
        "unreached-impl-methods",
        r#"
struct Kernel;

impl Kernel {
    #[hot_path]
    fn run(&self, input: &[f64]) {
        let _ = input.len();
    }

    fn cold_method(&self) {
        let _storage = Vec::new();
    }
}
"#,
    );

    let output = run_linter(source.path());
    let stdout = assert_succeeded(output);

    assert!(
        stdout.contains("mc-lint: ok"),
        "expected success output to contain `mc-lint: ok`\nstdout:\n{stdout}"
    );
}

#[test]
fn does_not_resolve_qualified_external_calls_by_last_segment() {
    let source = TempSource::new(
        "qualified-call-negative",
        r#"
#[hot_path(allow_allocation)]
fn root() {
    let _storage = Vec::<usize>::new();
    std::mem::drop(1usize);
}

fn new() {
    panic!("cold");
}

fn drop(_: usize) {
    panic!("cold");
}
"#,
    );

    let output = run_linter(source.path());
    let stdout = assert_succeeded(output);

    assert!(
        stdout.contains("mc-lint: ok"),
        "expected success output to contain `mc-lint: ok`\nstdout:\n{stdout}"
    );
}

#[test]
fn missing_input_path_fails_instead_of_passing() {
    let path = missing_temp_path("missing-input");

    let output = run_linter(&path);
    let stderr = assert_failed(output);

    assert!(
        stderr.contains("failed to scan"),
        "expected stderr to report a scan failure\nstderr:\n{stderr}"
    );
}

fn run_linter(path: &Path) -> Output {
    Command::new(linter_binary())
        .arg(path)
        .output()
        .expect("failed to run mc-lint")
}

fn linter_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_mc-lint") {
        return PathBuf::from(path);
    }

    let mut path = std::env::current_exe().expect("test executable path");
    path.pop();

    if path.ends_with("deps") {
        path.pop();
    }

    path.push(format!("mc-lint{}", std::env::consts::EXE_SUFFIX));
    path
}

fn missing_temp_path(name: &str) -> PathBuf {
    let id = NEXT_TEST_DIR.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("mc-lint-{name}-{}-{id}", std::process::id()))
}

fn assert_failed(output: Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "mc-lint unexpectedly succeeded\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    stderr.into_owned()
}

fn assert_succeeded(output: Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "mc-lint unexpectedly failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    stdout.into_owned()
}

fn assert_contains_all(haystack: &str, needles: &[&str]) {
    for needle in needles {
        assert!(
            haystack.contains(needle),
            "expected stderr to contain `{needle}`\nstderr:\n{haystack}"
        );
    }
}

fn assert_not_contains(haystack: &str, needle: &str) {
    assert!(
        !haystack.contains(needle),
        "expected stderr not to contain `{needle}`\nstderr:\n{haystack}"
    );
}
