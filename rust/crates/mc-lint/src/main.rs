use clap::Parser;
use proc_macro2::Span;
use quote::ToTokens;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Block, Expr, ExprCall, ExprMacro, ExprMethodCall, File, FnArg, ImplItemFn, ItemFn,
    Macro, Pat, Path as SynPath, Signature, Type,
};
use walkdir::WalkDir;

#[derive(Debug, Parser)]
struct Args {
    /// Source directory to scan.
    ///
    /// Example:
    ///
    /// rust/crates/my-crate/src
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HotPathOptions {
    allow_validation: bool,
    allow_allocation: bool,
    allow_branching: bool,
    allow_logging: bool,
    allow_panics: bool,
    allow_formatting: bool,
}

impl HotPathOptions {
    fn strict() -> Self {
        Self {
            allow_validation: false,
            allow_allocation: false,
            allow_branching: false,
            allow_logging: false,
            allow_panics: false,
            allow_formatting: false,
        }
    }
}

#[derive(Debug)]
struct Violation {
    file: PathBuf,
    function: String,
    message: String,
    line: usize,
    column: usize,
}

struct SourceFile {
    path: PathBuf,
    module_path: Vec<String>,
    syntax: File,
}

struct FunctionDef<'a> {
    path: &'a Path,
    kind: FunctionKind<'a>,
    impl_type: Option<String>,
    options: Option<HotPathOptions>,
}

#[derive(Clone, Copy)]
enum FunctionKind<'a> {
    Free(&'a ItemFn),
    Method(&'a ImplItemFn),
}

impl<'a> FunctionDef<'a> {
    fn name(&self) -> String {
        self.kind.sig().ident.to_string()
    }

    fn start(&self) -> proc_macro2::LineColumn {
        self.kind.sig().ident.span().start()
    }
}

impl<'a> FunctionKind<'a> {
    fn sig(&self) -> &'a Signature {
        match self {
            FunctionKind::Free(item_fn) => &item_fn.sig,
            FunctionKind::Method(method) => &method.sig,
        }
    }

    fn block(&self) -> &'a Block {
        match self {
            FunctionKind::Free(item_fn) => &item_fn.block,
            FunctionKind::Method(method) => &method.block,
        }
    }
}

#[derive(Debug)]
enum CallTarget {
    Bare(String),
    Qualified(String),
}

#[derive(Default)]
struct FunctionIndex<'a> {
    defs: Vec<FunctionDef<'a>>,
    by_name: HashMap<String, Vec<usize>>,
    by_path: HashMap<String, Vec<usize>>,
}

impl<'a> FunctionIndex<'a> {
    fn insert(&mut self, module_path: &[String], def: FunctionDef<'a>) {
        let function = def.name();
        let function_path = if let Some(impl_type) = &def.impl_type {
            format!("{impl_type}::{function}")
        } else {
            function.clone()
        };
        let qualified_path = qualified_function_path(module_path, &function_path);
        let def_index = self.defs.len();

        if def.impl_type.is_none() {
            self.by_name.entry(function).or_default().push(def_index);
        }

        self.by_path
            .entry(function_path)
            .or_default()
            .push(def_index);
        self.by_path
            .entry(qualified_path)
            .or_default()
            .push(def_index);

        self.defs.push(def);
    }

    fn resolve(&self, target: &CallTarget) -> Option<&Vec<usize>> {
        match target {
            CallTarget::Bare(function) => self.by_name.get(function),
            CallTarget::Qualified(function_path) => self.by_path.get(function_path),
        }
    }
}

fn main() {
    let args = Args::parse();

    let sources = match load_sources(&args.path) {
        Ok(sources) => sources,
        Err(()) => std::process::exit(2),
    };

    let mut violations = Vec::new();
    scan_sources(&sources, &mut violations);

    if violations.is_empty() {
        println!("mc-lint: ok");
        return;
    }

    for violation in &violations {
        eprintln!(
            "{}:{}:{}: error: hot path violation in `{}`: {}",
            violation.file.display(),
            violation.line,
            violation.column + 1,
            violation.function,
            violation.message
        );
    }

    std::process::exit(1);
}

fn load_sources(root: &Path) -> Result<Vec<SourceFile>, ()> {
    let mut sources = Vec::new();
    let mut had_error = false;

    for path in rust_files_under(root)? {
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("failed to read {}: {error}", path.display());
                had_error = true;
                continue;
            }
        };

        let file = match syn::parse_file(&source) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("failed to parse {}: {error}", path.display());
                had_error = true;
                continue;
            }
        };

        sources.push(SourceFile {
            module_path: module_path_for_file(root, &path),
            path,
            syntax: file,
        });
    }

    if had_error {
        Err(())
    } else {
        Ok(sources)
    }
}

fn rust_files_under(root: &Path) -> Result<Vec<PathBuf>, ()> {
    let mut files = Vec::new();
    let mut had_error = false;

    for entry in WalkDir::new(root) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                if let Some(path) = error.path() {
                    eprintln!("failed to scan {}: {error}", path.display());
                } else {
                    eprintln!("failed to scan {}: {error}", root.display());
                }

                had_error = true;
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.into_path();

        if path.extension().and_then(|x| x.to_str()) == Some("rs") {
            files.push(path);
        }
    }

    if had_error {
        Err(())
    } else {
        Ok(files)
    }
}

fn module_path_for_file(root: &Path, path: &Path) -> Vec<String> {
    let relative_path = path.strip_prefix(root).unwrap_or(path);
    let mut components = relative_path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(str::to_string)
        .collect::<Vec<_>>();

    let Some(file_name) = components.pop() else {
        return Vec::new();
    };

    let file_stem = Path::new(&file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&file_name);

    if !matches!(file_stem, "lib" | "main" | "mod") {
        components.push(file_stem.to_string());
    }

    components
}

fn scan_sources(sources: &[SourceFile], violations: &mut Vec<Violation>) {
    let functions = function_index(sources);
    let mut queue = VecDeque::new();
    let mut scanned = HashSet::new();

    for (def_index, def) in functions.defs.iter().enumerate() {
        if let Some(options) = def.options {
            queue.push_back((def_index, options));
        }
    }

    while let Some((def_index, inherited_options)) = queue.pop_front() {
        let def = &functions.defs[def_index];
        let options = def.options.unwrap_or(inherited_options);
        let function_start = def.start();
        let scan_key = (
            def.path.to_path_buf(),
            function_start.line,
            function_start.column,
            options,
        );

        if !scanned.insert(scan_key) {
            continue;
        }

        for called_function in scan_fn(def, options, violations) {
            if let Some(callee_indices) = functions.resolve(&called_function) {
                for callee_index in callee_indices {
                    queue.push_back((*callee_index, options));
                }
            }
        }
    }
}

fn function_index(sources: &[SourceFile]) -> FunctionIndex<'_> {
    let mut functions = FunctionIndex::default();

    for source in sources {
        let mut collector = FunctionCollector {
            path: &source.path,
            module_path: source.module_path.clone(),
            impl_type: None,
            functions: &mut functions,
        };

        collector.visit_file(&source.syntax);
    }

    functions
}

struct FunctionCollector<'a, 'index> {
    path: &'a Path,
    module_path: Vec<String>,
    impl_type: Option<String>,
    functions: &'index mut FunctionIndex<'a>,
}

impl<'ast, 'index> Visit<'ast> for FunctionCollector<'ast, 'index> {
    fn visit_item_fn(&mut self, item_fn: &'ast ItemFn) {
        self.functions.insert(
            &self.module_path,
            FunctionDef {
                path: self.path,
                kind: FunctionKind::Free(item_fn),
                impl_type: None,
                options: hot_path_options(&item_fn.attrs),
            },
        );

        visit::visit_item_fn(self, item_fn);
    }

    fn visit_impl_item_fn(&mut self, method: &'ast ImplItemFn) {
        self.functions.insert(
            &self.module_path,
            FunctionDef {
                path: self.path,
                kind: FunctionKind::Method(method),
                impl_type: self.impl_type.clone(),
                options: hot_path_options(&method.attrs),
            },
        );

        visit::visit_impl_item_fn(self, method);
    }

    fn visit_item_impl(&mut self, item_impl: &'ast syn::ItemImpl) {
        let previous_impl_type = self.impl_type.clone();
        self.impl_type = impl_type_path(item_impl.self_ty.as_ref());

        for item in &item_impl.items {
            self.visit_impl_item(item);
        }

        self.impl_type = previous_impl_type;
    }

    fn visit_item_mod(&mut self, item_mod: &'ast syn::ItemMod) {
        let Some((_, items)) = &item_mod.content else {
            return;
        };

        self.module_path.push(item_mod.ident.to_string());

        for item in items {
            self.visit_item(item);
        }

        self.module_path.pop();
    }
}

fn qualified_function_path(module_path: &[String], function: &str) -> String {
    if module_path.is_empty() {
        return function.to_string();
    }

    let mut path = module_path.join("::");
    path.push_str("::");
    path.push_str(function);
    path
}

fn impl_type_path(ty: &Type) -> Option<String> {
    let Type::Path(type_path) = ty else {
        return None;
    };

    let segments = type_path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();

    if segments.is_empty() {
        None
    } else {
        Some(segments.join("::"))
    }
}

fn check_hot_path_signature(
    path: &Path,
    function: &str,
    sig: &Signature,
    violations: &mut Vec<Violation>,
) {
    for input in &sig.inputs {
        let FnArg::Typed(pat_type) = input else {
            continue;
        };

        let param_name = pat_name(&pat_type.pat);

        if let Some(message) = param_type_violation(&pat_type.ty, &param_name) {
            let start = pat_type.ty.span().start();

            violations.push(Violation {
                file: path.to_path_buf(),
                function: function.to_string(),
                message,
                line: start.line,
                column: start.column,
            });
        }
    }
}

fn pat_name(pat: &Pat) -> String {
    match pat {
        Pat::Ident(ident) => ident.ident.to_string(),
        _ => "_".to_string(),
    }
}
fn param_type_violation(ty: &Type, param_name: &str) -> Option<String> {
    if let Type::Reference(type_ref) = ty {
        return reference_param_violation(type_ref.elem.as_ref(), param_name, ty);
    }

    if let Type::Path(type_path) = ty {
        let last = last_path_segment(&type_path.path)?;

        if is_owned_heap_or_container_type(last.as_str()) {
            return Some(format!(
                "hot path parameter `{param_name}` takes owned `{}`; pass a borrow/slice instead",
                type_to_string(ty)
            ));
        }
    }

    None
}

fn reference_param_violation(inner: &Type, param_name: &str, original_ty: &Type) -> Option<String> {
    let Type::Path(type_path) = inner else {
        return None;
    };

    let last = last_path_segment(&type_path.path)?;

    match last.as_str() {
        "Vec" => Some(format!(
            "hot path parameter `{param_name}` uses `{}`; prefer `&[T]` or `&mut [T]`",
            type_to_string(original_ty)
        )),
        "String" => Some(format!(
            "hot path parameter `{param_name}` uses `{}`; prefer `&str`",
            type_to_string(original_ty)
        )),
        _ => None,
    }
}

fn last_path_segment(path: &SynPath) -> Option<String> {
    path.segments
        .last()
        .map(|segment| segment.ident.to_string())
}
fn is_owned_heap_or_container_type(name: &str) -> bool {
    matches!(
        name,
        "Vec" | "String" | "Box" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "Rc" | "Arc"
    )
}

fn type_to_string(ty: &Type) -> String {
    ty.to_token_stream().to_string().replace(' ', "")
}

fn scan_fn(
    def: &FunctionDef<'_>,
    options: HotPathOptions,
    violations: &mut Vec<Violation>,
) -> Vec<CallTarget> {
    let function = def.name();

    check_hot_path_signature(def.path, &function, def.kind.sig(), violations);

    let mut visitor = HotPathVisitor {
        file: def.path.to_path_buf(),
        function,
        impl_type: def.impl_type.clone(),
        options,
        called_functions: Vec::new(),
        violations,
    };

    visitor.visit_block(def.kind.block());
    visitor.called_functions
}

fn hot_path_options(attrs: &[Attribute]) -> Option<HotPathOptions> {
    let mut options = HotPathOptions::strict();

    let attr = attrs.iter().find(|attr| attr.path().is_ident("hot_path"))?;

    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("allow_validation") {
            options.allow_validation = true;
            return Ok(());
        }

        if meta.path.is_ident("allow_allocation") {
            options.allow_allocation = true;
            return Ok(());
        }

        if meta.path.is_ident("allow_branching") {
            options.allow_branching = true;
            return Ok(());
        }

        if meta.path.is_ident("allow_logging") {
            options.allow_logging = true;
            return Ok(());
        }

        if meta.path.is_ident("allow_panics") {
            options.allow_panics = true;
            return Ok(());
        }

        if meta.path.is_ident("allow_formatting") {
            options.allow_formatting = true;
            return Ok(());
        }

        Ok(())
    });

    Some(options)
}

struct HotPathVisitor<'a> {
    file: PathBuf,
    function: String,
    impl_type: Option<String>,
    options: HotPathOptions,
    called_functions: Vec<CallTarget>,
    violations: &'a mut Vec<Violation>,
}

impl<'a> HotPathVisitor<'a> {
    fn push(&mut self, span: Span, message: impl Into<String>) {
        let start = span.start();

        self.violations.push(Violation {
            file: self.file.clone(),
            function: self.function.clone(),
            message: message.into(),
            line: start.line,
            column: start.column,
        });
    }

    fn check_call_path(&mut self, path: &SynPath) {
        let path_text = path_to_string(path);

        if !self.options.allow_allocation && is_allocation_call(&path_text) {
            self.push(
                path.segments
                    .last()
                    .map(|s| s.ident.span())
                    .unwrap_or_else(Span::call_site),
                format!("allocation-like call `{path_text}` is not allowed"),
            );
        }

        if !self.options.allow_validation && is_validation_call(&path_text) {
            self.push(
                path.segments
                    .last()
                    .map(|s| s.ident.span())
                    .unwrap_or_else(Span::call_site),
                format!("validation-like call `{path_text}` is not allowed"),
            );
        }

        if !self.options.allow_logging && is_logging_call(&path_text) {
            self.push(
                path.segments
                    .last()
                    .map(|s| s.ident.span())
                    .unwrap_or_else(Span::call_site),
                format!("logging call `{path_text}` is not allowed"),
            );
        }

        if !self.options.allow_formatting && is_formatting_call(&path_text) {
            self.push(
                path.segments
                    .last()
                    .map(|s| s.ident.span())
                    .unwrap_or_else(Span::call_site),
                format!("formatting call `{path_text}` is not allowed"),
            );
        }

        if !self.options.allow_panics && is_panic_call(&path_text) {
            self.push(
                path.segments
                    .last()
                    .map(|s| s.ident.span())
                    .unwrap_or_else(Span::call_site),
                format!("panic-like call `{path_text}` is not allowed"),
            );
        }
    }

    fn check_method_call(&mut self, node: &ExprMethodCall) {
        let method = node.method.to_string();
        let method_path = format!("::{method}");

        if !self.options.allow_allocation && is_allocation_call(&method_path) {
            self.push(
                node.method.span(),
                format!("allocation-like method `{method}` is not allowed"),
            );
        }

        if !self.options.allow_validation && is_validation_call(&method_path) {
            self.push(
                node.method.span(),
                format!("validation-like method `{method}` is not allowed"),
            );
        }

        if !self.options.allow_panics && is_panic_call(&method_path) {
            self.push(
                node.method.span(),
                format!("panic-like method `{method}` is not allowed"),
            );
        }
    }

    fn check_macro(&mut self, mac: &Macro) {
        let path_text = path_to_string(&mac.path);

        if !self.options.allow_allocation && path_text == "vec" {
            self.push(
                mac.path.segments[0].ident.span(),
                "allocation macro `vec!` is not allowed",
            );
        }

        if !self.options.allow_panics
            && matches!(
                path_text.as_str(),
                "panic" | "todo" | "unimplemented" | "assert"
            )
        {
            self.push(
                mac.path.segments[0].ident.span(),
                format!("panic/assert macro `{path_text}!` is not allowed"),
            );
        }

        if !self.options.allow_formatting
            && matches!(path_text.as_str(), "format" | "write" | "writeln")
        {
            self.push(
                mac.path.segments[0].ident.span(),
                format!("formatting macro `{path_text}!` is not allowed"),
            );
        }

        if !self.options.allow_logging && matches!(path_text.as_str(), "println" | "eprintln") {
            self.push(
                mac.path.segments[0].ident.span(),
                format!("logging/stdout macro `{path_text}!` is not allowed"),
            );
        }
    }
}

impl<'ast, 'a> Visit<'ast> for HotPathVisitor<'a> {
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(expr_path) = node.func.as_ref() {
            self.check_call_path(&expr_path.path);

            if let Some(function) = function_call_target(&expr_path.path, self.impl_type.as_deref())
            {
                self.called_functions.push(function);
            }
        }

        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        self.check_method_call(node);

        if let Some(function) = method_call_target(node, self.impl_type.as_deref()) {
            self.called_functions.push(function);
        }

        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_macro(&mut self, node: &'ast ExprMacro) {
        visit::visit_expr_macro(self, node);
    }

    fn visit_item_fn(&mut self, _node: &'ast ItemFn) {}

    fn visit_macro(&mut self, node: &'ast Macro) {
        self.check_macro(node);
        visit::visit_macro(self, node);
    }

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        if !self.options.allow_branching {
            self.push(node.if_token.span, "branch `if` found in hot path");
        }

        visit::visit_expr_if(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        if !self.options.allow_branching {
            self.push(node.match_token.span, "branch `match` found in hot path");
        }

        visit::visit_expr_match(self, node);
    }

    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        if !self.options.allow_validation {
            self.push(node.question_token.span, "`?` found in hot path");
        }

        visit::visit_expr_try(self, node);
    }
}

fn path_to_string(path: &SynPath) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn function_call_target(path: &SynPath, current_impl_type: Option<&str>) -> Option<CallTarget> {
    let mut segments = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();

    while matches!(
        segments.first().map(String::as_str),
        Some("crate" | "self" | "super")
    ) {
        segments.remove(0);
    }

    if segments.first().map(String::as_str) == Some("Self") {
        let impl_type = current_impl_type?;
        segments[0] = impl_type.to_string();
    }

    match segments.len() {
        0 => None,
        1 => Some(CallTarget::Bare(segments.remove(0))),
        _ => Some(CallTarget::Qualified(segments.join("::"))),
    }
}

fn method_call_target(
    node: &ExprMethodCall,
    current_impl_type: Option<&str>,
) -> Option<CallTarget> {
    let impl_type = current_impl_type?;

    if receiver_is_self(node.receiver.as_ref()) {
        Some(CallTarget::Qualified(format!(
            "{impl_type}::{}",
            node.method
        )))
    } else {
        None
    }
}

fn receiver_is_self(receiver: &Expr) -> bool {
    let Expr::Path(expr_path) = receiver else {
        return false;
    };

    expr_path.path.segments.len() == 1
        && expr_path
            .path
            .segments
            .first()
            .is_some_and(|segment| segment.ident == "self")
}

fn is_allocation_call(path: &str) -> bool {
    matches!(
        path,
        "Vec::new"
            | "Vec::with_capacity"
            | "Box::new"
            | "String::new"
            | "String::with_capacity"
            | "HashMap::new"
            | "HashSet::new"
            | "BTreeMap::new"
            | "BTreeSet::new"
            | "Rc::new"
            | "Arc::new"
    ) || path.ends_with("::collect")
        || path.ends_with("::to_vec")
        || path.ends_with("::to_string")
}

fn is_validation_call(path: &str) -> bool {
    path.ends_with("::try_from")
        || path.ends_with("::checked_add")
        || path.ends_with("::checked_sub")
        || path.ends_with("::checked_mul")
        || path.ends_with("::checked_div")
        || path.ends_with("::ok_or")
        || path.ends_with("::ok_or_else")
        || path.ends_with("::map_err")
}

fn is_logging_call(path: &str) -> bool {
    path.starts_with("log::") || path.starts_with("tracing::")
}

fn is_formatting_call(path: &str) -> bool {
    path == "format"
}

fn is_panic_call(path: &str) -> bool {
    matches!(path, "panic" | "todo" | "unimplemented")
        || path.ends_with("::unwrap")
        || path.ends_with("::expect")
}
