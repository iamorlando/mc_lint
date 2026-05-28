# Monte Carlo Linter

Monte Carlo Linter keeps simulation hot paths dense, allocation-free, and predictable. Mark the functions that run in the inner loop with `#[hot_path]`; the linter treats those functions and the functions they call as hot code, then rejects operations that usually do not belong in a Monte Carlo kernel.

Use it for code where small per-path costs multiply across many paths, timesteps, trials, or scenarios.

## Installation

Install the CLI from crates.io:

```sh
cargo install mc-lint
```

Add the marker attributes to any crate you want to annotate:

```sh
cargo add mc-lint
```

Or add `mc-lint` alongside your crate's existing dependencies:

```toml
[dependencies]
serde = "1"
mc-lint = "0.1"
```



## Annotating Hot Paths

Add `#[hot_path]` to a hot-path root:

```rust
use mc_lint::hot_path;

#[hot_path]
fn evolve_paths(states: &mut [PathState], shocks: &[f64], dt: f64) {
    for state in states {
        evolve_one_step(state, shocks, dt);
    }
}

fn evolve_one_step(state: &mut PathState, shocks: &[f64], dt: f64) {
    // This is also linted because it is called from a hot function.
}
```

By default, hot paths are strict. If a specific hot function intentionally needs a normally-forbidden operation, allow that category explicitly:

```rust
use mc_lint::hot_path;

#[hot_path(allow_branching, allow_validation)]
fn bounded_step(input: &[f64], output: &mut [f64]) -> Result<(), StepError> {
    if input.len() != output.len() {
        return Err(StepError::Shape);
    }

    for (x, y) in input.iter().zip(output.iter_mut()) {
        *y = x.max(0.0);
    }

    Ok(())
}
```

Supported options:

- `allow_validation`
- `allow_allocation`
- `allow_branching`
- `allow_logging`
- `allow_panics`
- `allow_formatting`

Options can be combined with commas.

## What It Checks

Monte Carlo Linter checks hot functions and all functions called from hot functions. Hot roots may be free functions or `impl` methods. The call graph follows free-function calls, module-qualified calls, and same-impl method calls written as `self.step(...)`, `Self::step(...)`, or `Kernel::step(...)`; it does not type-infer arbitrary receiver expressions like `kernel.step(...)`.

The default policy is intentionally narrow: hot code should take compact inputs, operate over existing storage, avoid avoidable branches, and keep diagnostics outside the inner loop.

### Dense Function Signatures

Hot functions should receive borrowed views over data instead of owned heap-backed containers.

Violations include:

- Owned heap or container parameters: `Vec`, `String`, `Box`, `HashMap`, `HashSet`, `BTreeMap`, `BTreeSet`, `Rc`, and `Arc`.
- Borrowed containers where a thinner view is preferred: `&Vec<T>` or `&mut Vec<T>` should be `&[T]` or `&mut [T]`.
- Borrowed strings where a string slice is preferred: `&String` should be `&str`.

### Allocation

Hot paths should reuse caller-provided storage instead of allocating while paths are being simulated.

Violations include:

- Allocation constructors: `Vec::new`, `Vec::with_capacity`, `Box::new`, `String::new`, `String::with_capacity`, `HashMap::new`, `HashSet::new`, `BTreeMap::new`, `BTreeSet::new`, `Rc::new`, and `Arc::new`.
- Allocation-like conversions and iterator collection: calls ending in `::collect`, `::to_vec`, or `::to_string`.
- The `vec!` macro.

Allow with `#[hot_path(allow_allocation)]`.

### Branching

Hot paths should avoid control flow that makes the kernel branchy or data-dependent unless the branch is an intentional part of the numerical method.

Violations include:

- `if`
- `match`

Allow with `#[hot_path(allow_branching)]`.

### Validation

Shape checks, conversions, and error adaptation should usually happen before entering the hot path.

Violations include:

- The `?` operator.
- Calls ending in `::try_from`, `::checked_add`, `::checked_sub`, `::checked_mul`, `::checked_div`, `::ok_or`, `::ok_or_else`, or `::map_err`.

Allow with `#[hot_path(allow_validation)]`.

### Logging

Logging and stdout/stderr writes should stay outside hot loops.

Violations include:

- Calls under `log::...`.
- Calls under `tracing::...`.
- `println!` and `eprintln!`.

Allow with `#[hot_path(allow_logging)]`.

### Formatting

String formatting is allocation-prone and usually diagnostic-only, so it is not allowed in strict hot paths.

Violations include:

- `format(...)` calls.
- `format!`, `write!`, and `writeln!` macros.

Allow with `#[hot_path(allow_formatting)]`.

### Panics And Assertions

Hot paths should not rely on panic paths or runtime assertions for expected control flow.

Violations include:

- `panic`, `todo`, and `unimplemented` calls.
- Calls ending in `::unwrap` or `::expect`.
- `panic!`, `todo!`, `unimplemented!`, and `assert!` macros.

Allow with `#[hot_path(allow_panics)]`.

## Running The Linter

Run the linter against the Rust source directory that contains your hot-path code:

```sh
mc-lint path/to/crate/src
```

When developing this repository from source, use Cargo to run the workspace
binary:

```sh
cargo run -p mc-lint -- path/to/crate/src
```

Successful output:

```text
mc-lint: ok
```

Violations are reported in compiler-style form so editors and CI can parse them:

```text
path/to/crate/src/path.rs:42:17: error: hot path violation in `evolve_paths`: allocation-like call `Vec::new` is not allowed
```

## VS Code Task Example

The repository includes a VS Code task that runs the current example and wires the output into the Problems panel:

```json
{
  "label": "mc-lint hot paths",
  "type": "shell",
  "command": "mc-lint rust/crates/my-crate/src",
  "problemMatcher": {
    "owner": "mc-lint",
    "fileLocation": ["relative", "${workspaceFolder}"],
    "pattern": {
      "regexp": "^(.*):(\\d+):(\\d+):\\s+(error|warning):\\s+(.*)$",
      "file": 1,
      "line": 2,
      "column": 3,
      "severity": 4,
      "message": 5
    }
  },
  "group": "test"
}
```

For another crate, change the final path in the command:

```json
"command": "mc-lint path/to/crate/src"
```
