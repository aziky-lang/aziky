//! Semantic orchestration and interpreter entry points.
//!
//! Runtime IR, specialized kernels, value sorting, and runtime-native lowering
//! live in focused child modules. This file retains program indexing,
//! monomorphization, semantic execution, and the public `lower_program` entry.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::thread;

use crate::frontend::ast::{
    BinaryOp, DictEntry, EnumDef, EnumTupleFieldDef, EnumVariantDef, EnumVariantPayloadDef, Expr,
    Function, InherentImplDef, Item, MatchArm, MatchPattern, ParForReduction, Program, ReductionOp,
    Span, Stmt, StructDef, StructInitField, TraitDef, TraitImplDef, TypeName, UnaryOp,
};
use crate::frontend::diagnostics::Diagnostic;

const LOOP_LIMIT: usize = 1_000_000;
const PARFOR_MIN_ITERATIONS: usize = 64;
const MAX_CALL_DEPTH: usize = 256;

fn source_callable_name(name: &str) -> String {
    name.split_once("__")
        .map(|(owner, method)| format!("{owner}::{method}"))
        .unwrap_or_else(|| name.to_string())
}

fn unknown_function_diagnostic(name: &str, span: Span) -> Diagnostic {
    Diagnostic::at_span(
        format!("unknown function '{}'", source_callable_name(name)),
        span,
    )
}

fn builtin_generic_enums() -> Vec<EnumDef> {
    let span = Span::new(0, 0);
    vec![
        EnumDef {
            name: "Option".to_string(),
            public: true,
            type_params: vec!["T".to_string()],
            variants: vec![
                EnumVariantDef {
                    name: "None".to_string(),
                    payload: EnumVariantPayloadDef::Unit,
                    span,
                },
                EnumVariantDef {
                    name: "Some".to_string(),
                    payload: EnumVariantPayloadDef::Tuple(vec![EnumTupleFieldDef {
                        ty: TypeName::Struct("T".to_string()),
                        span,
                    }]),
                    span,
                },
            ],
            span,
        },
        EnumDef {
            name: "Result".to_string(),
            public: true,
            type_params: vec!["T".to_string(), "E".to_string()],
            variants: vec![
                EnumVariantDef {
                    name: "Ok".to_string(),
                    payload: EnumVariantPayloadDef::Tuple(vec![EnumTupleFieldDef {
                        ty: TypeName::Struct("T".to_string()),
                        span,
                    }]),
                    span,
                },
                EnumVariantDef {
                    name: "Err".to_string(),
                    payload: EnumVariantPayloadDef::Tuple(vec![EnumTupleFieldDef {
                        ty: TypeName::Struct("E".to_string()),
                        span,
                    }]),
                    span,
                },
            ],
            span,
        },
    ]
}

fn argument_noun(count: usize) -> &'static str {
    if count == 1 { "argument" } else { "arguments" }
}

fn is_file_runtime_intrinsic(name: &str) -> bool {
    matches!(
        name,
        "file_open_read"
            | "file_create"
            | "file_write_all"
            | "file_read"
            | "file_close"
            | "File__open_read"
            | "File__create"
            | "Path__new"
            | "Path__join"
            | "Thread__spawn"
            | "Thread::spawn"
            | "Channel::bounded"
            | "Channel__bounded"
            | "Channel::unbounded"
            | "Channel__unbounded"
            | "runtime_arg_count"
            | "runtime_monotonic_nanos"
            | "runtime_wall_time_nanos"
            | "runtime_process_id"
            | "runtime_argument"
            | "runtime_environment_count"
            | "runtime_environment_entry"
            | "runtime_stdlib_abi_version"
    )
}

mod ir;
pub use ir::*;

#[derive(Debug, Clone)]
enum Value {
    Bool(bool),
    Str(String),
    Char(char),
    Int {
        bits: u16,
        value: i128,
    },
    UInt {
        bits: u16,
        value: u128,
    },
    Float {
        bits: u16,
        value: f64,
    },
    Ref {
        target: String,
        mutable: bool,
        inner: TypeName,
    },
    Struct {
        name: String,
        fields: HashMap<String, Value>,
    },
    Enum {
        name: String,
        variant: String,
        type_args: Vec<Option<TypeName>>,
        payload: EnumPayloadValue,
    },
    Array {
        elem_type: TypeName,
        elems: Vec<Value>,
    },
    List {
        elem_type: TypeName,
        elems: Vec<Value>,
    },
    Dict {
        key_type: TypeName,
        value_type: TypeName,
        entries: BTreeMap<String, Value>,
    },
    Map {
        key_type: TypeName,
        value_type: TypeName,
        entries: BTreeMap<String, Value>,
        keys: BTreeMap<String, Value>,
    },
}

#[derive(Debug, Clone)]
enum EnumPayloadValue {
    Unit,
    Tuple(Vec<Value>),
    Named(HashMap<String, Value>),
}

#[derive(Debug, Clone)]
struct Binding {
    value: Value,
    ty: TypeName,
    mutable: bool,
    shared_borrows: usize,
    mut_borrowed: bool,
}

#[derive(Clone)]
struct Env {
    scopes: Vec<HashMap<String, Binding>>,
}

impl Env {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) -> Vec<(String, Binding)> {
        self.scopes.pop().unwrap_or_default().into_iter().collect()
    }

    fn current_scope_contains(&self, name: &str) -> bool {
        self.scopes
            .last()
            .map(|scope| scope.contains_key(name))
            .unwrap_or(false)
    }

    fn insert(&mut self, name: String, binding: Binding) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, binding);
        }
    }

    fn get(&self, name: &str) -> Option<&Binding> {
        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.get(name) {
                return Some(binding);
            }
        }
        None
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut Binding> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(binding) = scope.get_mut(name) {
                return Some(binding);
            }
        }
        None
    }
}

#[derive(Default)]
struct ProgramIndex {
    functions: HashMap<String, Function>,
    structs: HashMap<String, StructDef>,
    enums: HashMap<String, EnumDef>,
    traits: HashMap<String, TraitDef>,
    struct_layouts: HashMap<String, Vec<LayoutField>>,
    pure_functions: HashMap<String, bool>,
}

#[derive(Debug, Clone)]
struct LayoutField {
    name: String,
    ty: TypeName,
}

fn analyze_pure_functions(functions: &HashMap<String, Function>) -> HashMap<String, bool> {
    let mut memo = HashMap::new();
    let mut visiting = HashSet::new();
    for name in functions.keys() {
        let pure = analyze_function_purity(name, functions, &mut memo, &mut visiting);
        memo.insert(name.clone(), pure);
    }
    memo
}

fn analyze_function_purity(
    name: &str,
    functions: &HashMap<String, Function>,
    memo: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    if let Some(pure) = memo.get(name) {
        return *pure;
    }
    if !visiting.insert(name.to_string()) {
        // Recursive cycles are conservatively considered impure.
        return false;
    }
    let pure = functions
        .get(name)
        .map(|func| {
            func.body
                .iter()
                .all(|stmt| analyze_stmt_purity(stmt, functions, memo, visiting))
        })
        .unwrap_or(false);
    visiting.remove(name);
    memo.insert(name.to_string(), pure);
    pure
}

fn analyze_stmt_purity(
    stmt: &Stmt,
    functions: &HashMap<String, Function>,
    memo: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    match stmt {
        Stmt::Let { expr, .. } => analyze_expr_purity(expr, functions, memo, visiting),
        Stmt::Assign { expr, .. } => analyze_expr_purity(expr, functions, memo, visiting),
        Stmt::AssignField { expr, .. } => analyze_expr_purity(expr, functions, memo, visiting),
        Stmt::AssignIndex { index, expr, .. } => {
            analyze_expr_purity(index, functions, memo, visiting)
                && analyze_expr_purity(expr, functions, memo, visiting)
        }
        Stmt::AssignStructListIndex { index, expr, .. } => {
            analyze_expr_purity(index, functions, memo, visiting)
                && analyze_expr_purity(expr, functions, memo, visiting)
        }
        Stmt::Call { name, args, .. } => {
            args.iter()
                .all(|arg| analyze_expr_purity(arg, functions, memo, visiting))
                && analyze_function_purity(name, functions, memo, visiting)
        }
        Stmt::MethodCall { .. } | Stmt::StructListMethodCall { .. } => false,
        Stmt::Return { expr, .. } => expr
            .as_ref()
            .map(|expr| analyze_expr_purity(expr, functions, memo, visiting))
            .unwrap_or(true),
        Stmt::Print { .. }
        | Stmt::Exit { .. }
        | Stmt::BenchLoop { .. }
        | Stmt::Assert { .. }
        | Stmt::Panic { .. } => false,
        Stmt::Block { stmts, .. } => stmts
            .iter()
            .all(|stmt| analyze_stmt_purity(stmt, functions, memo, visiting)),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            analyze_expr_purity(cond, functions, memo, visiting)
                && then_branch
                    .iter()
                    .all(|stmt| analyze_stmt_purity(stmt, functions, memo, visiting))
                && else_branch
                    .as_ref()
                    .map(|branch| {
                        branch
                            .iter()
                            .all(|stmt| analyze_stmt_purity(stmt, functions, memo, visiting))
                    })
                    .unwrap_or(true)
        }
        Stmt::While { cond, body, .. } => {
            analyze_expr_purity(cond, functions, memo, visiting)
                && body
                    .iter()
                    .all(|stmt| analyze_stmt_purity(stmt, functions, memo, visiting))
        }
        Stmt::Loop { body, .. } => body
            .iter()
            .all(|stmt| analyze_stmt_purity(stmt, functions, memo, visiting)),
        Stmt::For {
            start, end, body, ..
        }
        | Stmt::ParFor {
            start, end, body, ..
        } => {
            analyze_expr_purity(start, functions, memo, visiting)
                && analyze_expr_purity(end, functions, memo, visiting)
                && body
                    .iter()
                    .all(|stmt| analyze_stmt_purity(stmt, functions, memo, visiting))
        }
        Stmt::ForEach { iterable, body, .. } => {
            analyze_expr_purity(iterable, functions, memo, visiting)
                && body
                    .iter()
                    .all(|stmt| analyze_stmt_purity(stmt, functions, memo, visiting))
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => true,
    }
}

fn analyze_expr_purity(
    expr: &Expr,
    functions: &HashMap<String, Function>,
    memo: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    match expr {
        Expr::Call { name, args, .. } => {
            if name == "runtime_seed"
                || name == "heap_alloc"
                || name == "heap_free"
                || is_file_runtime_intrinsic(name)
            {
                return false;
            }
            args.iter()
                .all(|arg| analyze_expr_purity(arg, functions, memo, visiting))
                && analyze_function_purity(name, functions, memo, visiting)
        }
        Expr::QualifiedCall { args, .. } | Expr::EnumTupleVariant { args, .. } => args
            .iter()
            .all(|arg| analyze_expr_purity(arg, functions, memo, visiting)),
        Expr::Unary { expr, .. } => analyze_expr_purity(expr, functions, memo, visiting),
        Expr::Binary { left, right, .. } => {
            analyze_expr_purity(left, functions, memo, visiting)
                && analyze_expr_purity(right, functions, memo, visiting)
        }
        Expr::FieldAccess { base, .. } => analyze_expr_purity(base, functions, memo, visiting),
        Expr::Index { base, index, .. } => {
            analyze_expr_purity(base, functions, memo, visiting)
                && analyze_expr_purity(index, functions, memo, visiting)
        }
        Expr::ArrayLit { elems, .. } => elems
            .iter()
            .all(|elem| analyze_expr_purity(elem, functions, memo, visiting)),
        Expr::StructInit { fields, .. } | Expr::EnumStructVariant { fields, .. } => fields
            .iter()
            .all(|field| analyze_expr_purity(&field.expr, functions, memo, visiting)),
        Expr::DictLit { entries, .. } => entries
            .iter()
            .all(|entry| analyze_expr_purity(&entry.value, functions, memo, visiting)),
        Expr::MethodCall { receiver, .. } => {
            analyze_expr_purity(receiver, functions, memo, visiting)
        }
        Expr::Match { value, arms, .. } => {
            analyze_expr_purity(value, functions, memo, visiting)
                && arms
                    .iter()
                    .all(|arm| analyze_expr_purity(&arm.expr, functions, memo, visiting))
        }
        Expr::Bool { .. }
        | Expr::String { .. }
        | Expr::Char { .. }
        | Expr::Number { .. }
        | Expr::Ident { .. }
        | Expr::EnumVariant { .. } => true,
    }
}

fn resolve_struct_layouts(
    structs: &HashMap<String, StructDef>,
) -> Result<HashMap<String, Vec<LayoutField>>, Diagnostic> {
    fn validate_inline_acyclic(
        name: &str,
        structs: &HashMap<String, StructDef>,
        visiting: &mut Vec<String>,
        validated: &mut HashSet<String>,
    ) -> Result<(), Diagnostic> {
        if validated.contains(name) {
            return Ok(());
        }
        if let Some(start) = visiting.iter().position(|item| item == name) {
            let mut cycle = visiting[start..].to_vec();
            cycle.push(name.to_string());
            let def = structs.get(name).expect("visited struct must exist");
            return Err(Diagnostic::at_span(
                format!(
                    "recursive resource layout is not allowed: {}",
                    cycle.join(" -> ")
                ),
                def.span,
            ));
        }
        let Some(def) = structs.get(name) else {
            return Ok(());
        };
        visiting.push(name.to_string());
        for field in &def.fields {
            if let TypeName::Struct(child) = &field.ty {
                validate_inline_acyclic(child, structs, visiting, validated)?;
            }
        }
        visiting.pop();
        validated.insert(name.to_string());
        Ok(())
    }

    let mut names = structs.keys().cloned().collect::<Vec<_>>();
    names.sort();
    let mut validated = HashSet::new();
    for name in &names {
        validate_inline_acyclic(name, structs, &mut Vec::new(), &mut validated)?;
    }
    let mut cache = HashMap::new();
    let mut visiting = HashSet::new();
    for name in &names {
        let _ = resolve_struct_layout(name, structs, &mut cache, &mut visiting)?;
    }
    Ok(cache)
}

fn resolve_struct_layout(
    name: &str,
    structs: &HashMap<String, StructDef>,
    cache: &mut HashMap<String, Vec<LayoutField>>,
    visiting: &mut HashSet<String>,
) -> Result<Vec<LayoutField>, Diagnostic> {
    if let Some(layout) = cache.get(name) {
        return Ok(layout.clone());
    }
    let def = structs.get(name).ok_or_else(|| {
        Diagnostic::new(format!("unknown struct '{name}' in embedded layout"), 0, 0)
    })?;
    if !visiting.insert(name.to_string()) {
        return Err(Diagnostic::at_span(
            format!("cyclic struct embedding detected for '{name}'"),
            def.span,
        ));
    }

    let mut out = Vec::new();
    let mut seen_names = HashSet::new();
    for field in &def.fields {
        if field.embedded {
            let TypeName::Struct(embed_name) = &field.ty else {
                visiting.remove(name);
                return Err(Diagnostic::at_span(
                    "embedded field must reference a struct type",
                    field.span,
                ));
            };
            let embedded_layout = resolve_struct_layout(embed_name, structs, cache, visiting)?;
            for embedded in embedded_layout {
                if !seen_names.insert(embedded.name.clone()) {
                    visiting.remove(name);
                    return Err(Diagnostic::at_span(
                        format!(
                            "embedded field '{}' collides in struct '{}'",
                            embedded.name, name
                        ),
                        field.span,
                    ));
                }
                out.push(embedded);
            }
        } else {
            if !seen_names.insert(field.name.clone()) {
                visiting.remove(name);
                return Err(Diagnostic::at_span(
                    format!("duplicate field '{}' in struct '{}'", field.name, name),
                    field.span,
                ));
            }
            out.push(LayoutField {
                name: field.name.clone(),
                ty: field.ty.clone(),
            });
        }
    }
    visiting.remove(name);
    cache.insert(name.to_string(), out.clone());
    Ok(out)
}

fn substitute_self_type(ty: &TypeName, for_type: &str) -> TypeName {
    match ty {
        TypeName::Struct(name) if name == "Self" => TypeName::Struct(for_type.to_string()),
        TypeName::Applied { name, args } => TypeName::Applied {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| substitute_self_type(arg, for_type))
                .collect(),
        },
        TypeName::Dict { key, value } => TypeName::Dict {
            key: Box::new(substitute_self_type(key, for_type)),
            value: Box::new(substitute_self_type(value, for_type)),
        },
        TypeName::List { elem } => TypeName::List {
            elem: Box::new(substitute_self_type(elem, for_type)),
        },
        TypeName::Map { key, value } => TypeName::Map {
            key: Box::new(substitute_self_type(key, for_type)),
            value: Box::new(substitute_self_type(value, for_type)),
        },
        TypeName::Array { elem, len } => TypeName::Array {
            elem: Box::new(substitute_self_type(elem, for_type)),
            len: *len,
        },
        TypeName::Ref { mutable, inner } => TypeName::Ref {
            mutable: *mutable,
            inner: Box::new(substitute_self_type(inner, for_type)),
        },
        other => other.clone(),
    }
}

fn substitute_self_expr(expr: &mut Expr, for_type: &str) {
    match expr {
        Expr::Bool { .. }
        | Expr::String { .. }
        | Expr::Char { .. }
        | Expr::Number { .. }
        | Expr::Ident { .. } => {}
        Expr::Call { name, args, .. } => {
            if let Some(method) = name.strip_prefix("Self__") {
                *name = format!("{for_type}__{method}");
            }
            for arg in args {
                substitute_self_expr(arg, for_type);
            }
        }
        Expr::QualifiedCall { owner, args, .. } => {
            if owner == "Self" {
                *owner = for_type.to_string();
            }
            for arg in args {
                substitute_self_expr(arg, for_type);
            }
        }
        Expr::Unary { expr, .. } => substitute_self_expr(expr, for_type),
        Expr::Binary { left, right, .. } => {
            substitute_self_expr(left, for_type);
            substitute_self_expr(right, for_type);
        }
        Expr::FieldAccess { base, .. } => substitute_self_expr(base, for_type),
        Expr::Index { base, index, .. } => {
            substitute_self_expr(base, for_type);
            substitute_self_expr(index, for_type);
        }
        Expr::ArrayLit { elems, .. } => {
            for elem in elems {
                substitute_self_expr(elem, for_type);
            }
        }
        Expr::StructInit { name, fields, .. } => {
            if name == "Self" {
                *name = for_type.to_string();
            }
            for field in fields {
                substitute_self_expr(&mut field.expr, for_type);
            }
        }
        Expr::EnumVariant { enum_name, .. } => {
            if enum_name == "Self" {
                *enum_name = for_type.to_string();
            }
        }
        Expr::EnumTupleVariant {
            enum_name, args, ..
        } => {
            if enum_name == "Self" {
                *enum_name = for_type.to_string();
            }
            for arg in args {
                substitute_self_expr(arg, for_type);
            }
        }
        Expr::EnumStructVariant {
            enum_name, fields, ..
        } => {
            if enum_name == "Self" {
                *enum_name = for_type.to_string();
            }
            for field in fields {
                substitute_self_expr(&mut field.expr, for_type);
            }
        }
        Expr::DictLit { entries, .. } => {
            for entry in entries {
                substitute_self_expr(&mut entry.value, for_type);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            substitute_self_expr(receiver, for_type);
            for arg in args {
                substitute_self_expr(arg, for_type);
            }
        }
        Expr::Match { value, arms, .. } => {
            substitute_self_expr(value, for_type);
            for arm in arms {
                match &mut arm.pattern {
                    MatchPattern::EnumUnit { enum_name, .. }
                    | MatchPattern::EnumTuple { enum_name, .. }
                    | MatchPattern::EnumNamed { enum_name, .. }
                        if enum_name == "Self" =>
                    {
                        *enum_name = for_type.to_string();
                    }
                    _ => {}
                }
                substitute_self_expr(&mut arm.expr, for_type);
            }
        }
    }
}

fn substitute_self_stmts(stmts: &mut [Stmt], for_type: &str) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { ty, expr, .. } => {
                if let Some(ty) = ty {
                    *ty = substitute_self_type(ty, for_type);
                }
                substitute_self_expr(expr, for_type);
            }
            Stmt::Assign { expr, .. }
            | Stmt::AssignField { expr, .. }
            | Stmt::Print { expr, .. }
            | Stmt::Exit { expr, .. } => substitute_self_expr(expr, for_type),
            Stmt::AssignIndex { index, expr, .. }
            | Stmt::AssignStructListIndex { index, expr, .. } => {
                substitute_self_expr(index, for_type);
                substitute_self_expr(expr, for_type);
            }
            Stmt::Call { name, args, .. } => {
                if let Some(method) = name.strip_prefix("Self__") {
                    *name = format!("{for_type}__{method}");
                }
                for arg in args {
                    substitute_self_expr(arg, for_type);
                }
            }
            Stmt::MethodCall { args, .. } | Stmt::StructListMethodCall { args, .. } => {
                for arg in args {
                    substitute_self_expr(arg, for_type);
                }
            }
            Stmt::Return { expr, .. } => {
                if let Some(expr) = expr {
                    substitute_self_expr(expr, for_type);
                }
            }
            Stmt::BenchLoop { iterations, .. } => substitute_self_expr(iterations, for_type),
            Stmt::Block { stmts, .. } | Stmt::Loop { body: stmts, .. } => {
                substitute_self_stmts(stmts, for_type);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                substitute_self_expr(cond, for_type);
                substitute_self_stmts(then_branch, for_type);
                if let Some(else_branch) = else_branch {
                    substitute_self_stmts(else_branch, for_type);
                }
            }
            Stmt::While { cond, body, .. } => {
                substitute_self_expr(cond, for_type);
                substitute_self_stmts(body, for_type);
            }
            Stmt::For {
                start, end, body, ..
            } => {
                substitute_self_expr(start, for_type);
                substitute_self_expr(end, for_type);
                substitute_self_stmts(body, for_type);
            }
            Stmt::ParFor {
                start,
                end,
                body,
                reduction,
                ..
            } => {
                substitute_self_expr(start, for_type);
                substitute_self_expr(end, for_type);
                substitute_self_stmts(body, for_type);
                if let Some(reduction) = reduction {
                    substitute_self_expr(&mut reduction.expr, for_type);
                }
            }
            Stmt::ForEach { iterable, body, .. } => {
                substitute_self_expr(iterable, for_type);
                substitute_self_stmts(body, for_type);
            }
            Stmt::Assert { cond, message, .. } => {
                substitute_self_expr(cond, for_type);
                if let Some(message) = message {
                    substitute_self_expr(message, for_type);
                }
            }
            Stmt::Panic { message, .. } => substitute_self_expr(message, for_type),
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
}

fn resolve_qualified_expr(
    expr: &mut Expr,
    enums: &HashMap<String, EnumDef>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Bool { .. }
        | Expr::String { .. }
        | Expr::Char { .. }
        | Expr::Number { .. }
        | Expr::Ident { .. }
        | Expr::EnumVariant { .. } => Ok(()),
        Expr::Call { args, .. } => {
            for arg in args {
                resolve_qualified_expr(arg, enums)?;
            }
            Ok(())
        }
        Expr::QualifiedCall {
            owner,
            member,
            args,
            span,
        } => {
            for arg in args.iter_mut() {
                resolve_qualified_expr(arg, enums)?;
            }
            let owner_name = owner.clone();
            let member_name = member.clone();
            let call_span = *span;
            if let Some(enum_def) = enums.get(&owner_name) {
                let variant = enum_def
                    .variants
                    .iter()
                    .find(|variant| variant.name == member_name)
                    .ok_or_else(|| {
                        Diagnostic::at_span(
                            format!(
                                "unknown variant '{}' for enum '{}'",
                                member_name, owner_name
                            ),
                            call_span,
                        )
                    })?;
                match &variant.payload {
                    EnumVariantPayloadDef::Unit => {
                        if !args.is_empty() {
                            return Err(type_error(
                                format!(
                                    "unit variant '{}::{}' expects no payload, got {} arguments",
                                    owner_name,
                                    member_name,
                                    args.len()
                                )
                                .as_str(),
                                call_span,
                            ));
                        }
                        *expr = Expr::EnumVariant {
                            enum_name: owner_name,
                            variant: member_name,
                            span: call_span,
                        };
                    }
                    EnumVariantPayloadDef::Tuple(_) => {
                        let resolved_args = std::mem::take(args);
                        *expr = Expr::EnumTupleVariant {
                            enum_name: owner_name,
                            variant: member_name,
                            args: resolved_args,
                            span: call_span,
                        };
                    }
                    EnumVariantPayloadDef::Named(_) => {
                        return Err(type_error(
                            format!(
                                "named variant '{}::{}' must be constructed with '{{ ... }}'",
                                owner_name, member_name
                            )
                            .as_str(),
                            call_span,
                        ));
                    }
                }
            } else {
                let resolved_args = std::mem::take(args);
                *expr = Expr::Call {
                    name: format!("{owner_name}__{member_name}"),
                    args: resolved_args,
                    span: call_span,
                };
            }
            Ok(())
        }
        Expr::Unary { expr, .. } => resolve_qualified_expr(expr, enums),
        Expr::Binary { left, right, .. } => {
            resolve_qualified_expr(left, enums)?;
            resolve_qualified_expr(right, enums)
        }
        Expr::FieldAccess { base, .. } => resolve_qualified_expr(base, enums),
        Expr::Index { base, index, .. } => {
            resolve_qualified_expr(base, enums)?;
            resolve_qualified_expr(index, enums)
        }
        Expr::ArrayLit { elems, .. } | Expr::EnumTupleVariant { args: elems, .. } => {
            for elem in elems {
                resolve_qualified_expr(elem, enums)?;
            }
            Ok(())
        }
        Expr::StructInit { fields, .. } | Expr::EnumStructVariant { fields, .. } => {
            for field in fields {
                resolve_qualified_expr(&mut field.expr, enums)?;
            }
            Ok(())
        }
        Expr::DictLit { entries, .. } => {
            for entry in entries {
                resolve_qualified_expr(&mut entry.value, enums)?;
            }
            Ok(())
        }
        Expr::MethodCall { receiver, args, .. } => {
            resolve_qualified_expr(receiver, enums)?;
            for arg in args {
                resolve_qualified_expr(arg, enums)?;
            }
            Ok(())
        }
        Expr::Match { value, arms, .. } => {
            resolve_qualified_expr(value, enums)?;
            for arm in arms {
                resolve_qualified_expr(&mut arm.expr, enums)?;
            }
            Ok(())
        }
    }
}

fn resolve_qualified_stmts(
    stmts: &mut [Stmt],
    enums: &HashMap<String, EnumDef>,
) -> Result<(), Diagnostic> {
    for stmt in stmts {
        match stmt {
            Stmt::Let { expr, .. }
            | Stmt::Assign { expr, .. }
            | Stmt::AssignField { expr, .. }
            | Stmt::Print { expr, .. }
            | Stmt::Exit { expr, .. } => resolve_qualified_expr(expr, enums)?,
            Stmt::AssignIndex { index, expr, .. }
            | Stmt::AssignStructListIndex { index, expr, .. } => {
                resolve_qualified_expr(index, enums)?;
                resolve_qualified_expr(expr, enums)?;
            }
            Stmt::Call { args, .. }
            | Stmt::MethodCall { args, .. }
            | Stmt::StructListMethodCall { args, .. } => {
                for arg in args {
                    resolve_qualified_expr(arg, enums)?;
                }
            }
            Stmt::Return { expr, .. } => {
                if let Some(expr) = expr {
                    resolve_qualified_expr(expr, enums)?;
                }
            }
            Stmt::BenchLoop { iterations, .. } => resolve_qualified_expr(iterations, enums)?,
            Stmt::Block { stmts, .. } | Stmt::Loop { body: stmts, .. } => {
                resolve_qualified_stmts(stmts, enums)?;
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                resolve_qualified_expr(cond, enums)?;
                resolve_qualified_stmts(then_branch, enums)?;
                if let Some(else_branch) = else_branch {
                    resolve_qualified_stmts(else_branch, enums)?;
                }
            }
            Stmt::While { cond, body, .. } => {
                resolve_qualified_expr(cond, enums)?;
                resolve_qualified_stmts(body, enums)?;
            }
            Stmt::For {
                start, end, body, ..
            } => {
                resolve_qualified_expr(start, enums)?;
                resolve_qualified_expr(end, enums)?;
                resolve_qualified_stmts(body, enums)?;
            }
            Stmt::ParFor {
                start,
                end,
                body,
                reduction,
                ..
            } => {
                resolve_qualified_expr(start, enums)?;
                resolve_qualified_expr(end, enums)?;
                resolve_qualified_stmts(body, enums)?;
                if let Some(reduction) = reduction {
                    resolve_qualified_expr(&mut reduction.expr, enums)?;
                }
            }
            Stmt::ForEach { iterable, body, .. } => {
                resolve_qualified_expr(iterable, enums)?;
                resolve_qualified_stmts(body, enums)?;
            }
            Stmt::Assert { cond, message, .. } => {
                resolve_qualified_expr(cond, enums)?;
                if let Some(message) = message {
                    resolve_qualified_expr(message, enums)?;
                }
            }
            Stmt::Panic { message, .. } => resolve_qualified_expr(message, enums)?,
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
    Ok(())
}

fn monomorphize_inherent_impls(
    index: &mut ProgramIndex,
    impls: &[InherentImplDef],
) -> Result<(), Diagnostic> {
    for impl_def in impls {
        if !index.structs.contains_key(&impl_def.for_type) {
            return Err(Diagnostic::at_span(
                format!(
                    "inherent impl target '{}' must be a known struct",
                    impl_def.for_type
                ),
                impl_def.span,
            ));
        }

        for method in &impl_def.methods {
            let mangled = format!("{}__{}", impl_def.for_type, method.name);
            if index.functions.contains_key(&mangled) {
                return Err(Diagnostic::at_span(
                    format!(
                        "method '{}' is already defined for '{}'",
                        method.name, impl_def.for_type
                    ),
                    method.span,
                ));
            }

            let mut lowered = method.clone();
            lowered.name = mangled.clone();
            for param in &mut lowered.params {
                param.ty = substitute_self_type(&param.ty, &impl_def.for_type);
            }
            if let Some(return_type) = &mut lowered.return_type {
                *return_type = substitute_self_type(return_type, &impl_def.for_type);
            }
            substitute_self_stmts(&mut lowered.body, &impl_def.for_type);
            index.functions.insert(mangled, lowered);
        }
    }
    Ok(())
}

fn monomorphize_trait_impls(
    index: &mut ProgramIndex,
    impls: &[TraitImplDef],
) -> Result<(), Diagnostic> {
    for impl_def in impls {
        let trait_def = index.traits.get(&impl_def.trait_name).ok_or_else(|| {
            Diagnostic::at_span(
                format!("unknown trait '{}'", impl_def.trait_name),
                impl_def.span,
            )
        })?;
        if !index.structs.contains_key(&impl_def.for_type) {
            return Err(Diagnostic::at_span(
                format!(
                    "trait impl target '{}' must be a known struct",
                    impl_def.for_type
                ),
                impl_def.span,
            ));
        }

        for method in &impl_def.methods {
            let sig = trait_def
                .methods
                .iter()
                .find(|sig| sig.name == method.name)
                .ok_or_else(|| {
                    Diagnostic::at_span(
                        format!(
                            "method '{}' is not declared in trait '{}'",
                            method.name, trait_def.name
                        ),
                        method.span,
                    )
                })?;
            if method.params.len() != sig.params.len() {
                return Err(Diagnostic::at_span(
                    format!(
                        "trait method '{}' parameter count mismatch: expected {}, got {}",
                        method.name,
                        sig.params.len(),
                        method.params.len()
                    ),
                    method.span,
                ));
            }
            for (impl_param, trait_param) in method.params.iter().zip(sig.params.iter()) {
                let expected = substitute_self_type(&trait_param.ty, &impl_def.for_type);
                if impl_param.ty != expected {
                    return Err(Diagnostic::at_span(
                        format!(
                            "trait method '{}' parameter '{}' type mismatch: expected {}, got {}",
                            method.name,
                            impl_param.name,
                            expected.display(),
                            impl_param.ty.display()
                        ),
                        impl_param.span,
                    ));
                }
            }
            let expected_ret = sig
                .return_type
                .as_ref()
                .map(|ty| substitute_self_type(ty, &impl_def.for_type));
            if method.return_type != expected_ret {
                let expected = expected_ret
                    .as_ref()
                    .map(TypeName::display)
                    .unwrap_or_else(|| "void".to_string());
                let got = method
                    .return_type
                    .as_ref()
                    .map(TypeName::display)
                    .unwrap_or_else(|| "void".to_string());
                return Err(Diagnostic::at_span(
                    format!(
                        "trait method '{}' return type mismatch: expected {}, got {}",
                        method.name, expected, got
                    ),
                    method.span,
                ));
            }

            let mangled = format!("{}__{}", impl_def.for_type, method.name);
            if index.functions.contains_key(&mangled) {
                return Err(Diagnostic::at_span(
                    format!("monomorphized function '{}' already exists", mangled),
                    method.span,
                ));
            }
            let mut lowered = method.clone();
            lowered.name = mangled.clone();
            substitute_self_stmts(&mut lowered.body, &impl_def.for_type);
            index.functions.insert(mangled, lowered);
        }
    }
    Ok(())
}

pub fn lower_program(program: &Program) -> Result<Vec<LoweredStmt>, Diagnostic> {
    let mut index = ProgramIndex::default();
    for def in builtin_generic_enums() {
        index.enums.insert(def.name.clone(), def);
    }
    let mut trait_impls = Vec::new();
    let mut inherent_impls = Vec::new();
    for item in &program.items {
        match item {
            Item::Function(func) => {
                if index.functions.contains_key(&func.name) {
                    return Err(Diagnostic::at_span(
                        format!("redefinition of function '{}'", func.name),
                        func.span,
                    ));
                }
                index.functions.insert(func.name.clone(), func.clone());
            }
            Item::Struct(def) => {
                if index.structs.contains_key(&def.name) {
                    return Err(Diagnostic::at_span(
                        format!("redefinition of struct '{}'", def.name),
                        def.span,
                    ));
                }
                index.structs.insert(def.name.clone(), def.clone());
            }
            Item::Enum(def) => {
                if index.enums.contains_key(&def.name) {
                    return Err(Diagnostic::at_span(
                        format!("redefinition of enum '{}'", def.name),
                        def.span,
                    ));
                }
                if def.variants.is_empty() {
                    return Err(Diagnostic::at_span(
                        format!("enum '{}' must have at least one variant", def.name),
                        def.span,
                    ));
                }
                let mut type_params = HashSet::new();
                for param in &def.type_params {
                    if param == "_" {
                        return Err(Diagnostic::at_span(
                            "'_' cannot be used as a generic type parameter",
                            def.span,
                        ));
                    }
                    if !type_params.insert(param.clone()) {
                        return Err(Diagnostic::at_span(
                            format!(
                                "duplicate generic parameter '{}' in enum '{}'",
                                param, def.name
                            ),
                            def.span,
                        ));
                    }
                }
                let mut seen = HashMap::new();
                for variant in &def.variants {
                    if seen.contains_key(&variant.name) {
                        return Err(Diagnostic::at_span(
                            format!(
                                "duplicate variant '{}' in enum '{}'",
                                variant.name, def.name
                            ),
                            variant.span,
                        ));
                    }
                    seen.insert(variant.name.clone(), ());
                    match &variant.payload {
                        EnumVariantPayloadDef::Unit => {}
                        EnumVariantPayloadDef::Tuple(fields) => {
                            if fields.is_empty() {
                                return Err(Diagnostic::at_span(
                                    format!(
                                        "tuple variant '{}::{}' must contain at least one field; remove '()' for a unit variant",
                                        def.name, variant.name
                                    ),
                                    variant.span,
                                ));
                            }
                        }
                        EnumVariantPayloadDef::Named(fields) => {
                            if fields.is_empty() {
                                return Err(Diagnostic::at_span(
                                    format!(
                                        "named variant '{}::{}' must contain at least one field",
                                        def.name, variant.name
                                    ),
                                    variant.span,
                                ));
                            }
                            let mut field_names = HashSet::new();
                            for field in fields {
                                if !field_names.insert(field.name.clone()) {
                                    return Err(Diagnostic::at_span(
                                        format!(
                                            "duplicate payload field '{}' in variant '{}::{}'",
                                            field.name, def.name, variant.name
                                        ),
                                        field.span,
                                    ));
                                }
                            }
                        }
                    }
                }
                index.enums.insert(def.name.clone(), def.clone());
            }
            Item::Trait(def) => {
                if index.traits.contains_key(&def.name) {
                    return Err(Diagnostic::at_span(
                        format!("redefinition of trait '{}'", def.name),
                        def.span,
                    ));
                }
                let mut seen = HashSet::new();
                for method in &def.methods {
                    if !seen.insert(method.name.clone()) {
                        return Err(Diagnostic::at_span(
                            format!(
                                "duplicate trait method '{}' in trait '{}'",
                                method.name, def.name
                            ),
                            method.span,
                        ));
                    }
                }
                index.traits.insert(def.name.clone(), def.clone());
            }
            Item::Impl(def) => {
                trait_impls.push(def.clone());
            }
            Item::InherentImpl(def) => {
                inherent_impls.push(def.clone());
            }
            Item::Module(_) | Item::Use(_) => {}
        }
    }
    for enum_def in index.enums.values() {
        let type_params: HashSet<&str> = enum_def.type_params.iter().map(String::as_str).collect();
        for variant in &enum_def.variants {
            match &variant.payload {
                EnumVariantPayloadDef::Unit => {}
                EnumVariantPayloadDef::Tuple(fields) => {
                    for field in fields {
                        ensure_enum_payload_type_known(
                            &field.ty,
                            &type_params,
                            &index,
                            field.span,
                        )?;
                    }
                }
                EnumVariantPayloadDef::Named(fields) => {
                    for field in fields {
                        ensure_enum_payload_type_known(
                            &field.ty,
                            &type_params,
                            &index,
                            field.span,
                        )?;
                    }
                }
            }
        }
    }
    index.struct_layouts = resolve_struct_layouts(&index.structs)?;
    monomorphize_inherent_impls(&mut index, &inherent_impls)?;
    monomorphize_trait_impls(&mut index, &trait_impls)?;
    for function in index.functions.values() {
        for param in &function.params {
            ensure_type_known(&param.ty, &index, param.span)?;
        }
        if let Some(return_type) = &function.return_type {
            ensure_type_known(return_type, &index, function.span)?;
        }
    }
    for struct_def in index.structs.values() {
        for field in &struct_def.fields {
            ensure_type_known(&field.ty, &index, field.span)?;
        }
    }
    for function in index.functions.values_mut() {
        resolve_qualified_stmts(&mut function.body, &index.enums)?;
    }
    index.pure_functions = analyze_pure_functions(&index.functions);

    let main = index
        .functions
        .get("main")
        .ok_or_else(|| Diagnostic::new("missing fn main()", 0, 0))?
        .clone();

    if let Some(runtime_lowered) = try_lower_runtime_seeded_for_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_seeded_alloc_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_ring_write_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_prefix_scan_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_bloom_filter_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_sort_window_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_seeded_branch_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_branch_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_seeded_dual_state_branch_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_seeded_affine_index_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_affine_index_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_seeded_struct_latency_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_seeded_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_for_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    if let Some(runtime_lowered) = try_lower_runtime_while_kernel(&main)? {
        return Ok(runtime_lowered);
    }

    match try_lower_runtime_generic(&main, &index.functions, &index.struct_layouts, &index.enums)? {
        RuntimeGenericAttempt::Lowered(runtime_lowered) => return Ok(runtime_lowered),
        RuntimeGenericAttempt::Fallback { reason } => {
            maybe_report_runtime_generic_fallback(&main, &reason);
        }
    }

    let mut env = Env::new();
    let mut lowered = Vec::new();
    let mut exec = ExecState {
        lowered: &mut lowered,
    };
    let mut ctx = FunctionContext {
        env: &mut env,
        index: &index,
        loop_depth: 0,
        call_depth: 0,
        call_stack: Vec::new(),
    };
    exec_function(&main, &[], &mut ctx, &mut exec)?;
    Ok(lowered)
}

struct ExecState<'a> {
    lowered: &'a mut Vec<LoweredStmt>,
}

struct FunctionContext<'a> {
    env: &'a mut Env,
    index: &'a ProgramIndex,
    loop_depth: usize,
    call_depth: usize,
    call_stack: Vec<String>,
}

#[derive(Debug, Clone)]
enum ForRange {
    Signed { bits: u16, start: i128, end: i128 },
    Unsigned { bits: u16, start: u128, end: u128 },
}

#[derive(Debug, Clone)]
enum Flow {
    Next,
    BreakLoop,
    ContinueLoop,
    Exit,
    Return(Option<Value>),
}

#[derive(Debug, Clone)]
enum CallResult {
    Continue(Option<Value>),
    Exit,
}

fn exec_function(
    func: &Function,
    args: &[Value],
    ctx: &mut FunctionContext<'_>,
    exec: &mut ExecState<'_>,
) -> Result<CallResult, Diagnostic> {
    let callable_name = source_callable_name(&func.name);
    if ctx.call_depth >= MAX_CALL_DEPTH {
        return Err(type_error(
            format!(
                "call depth exceeded maximum of {} (possible recursion cycle)",
                MAX_CALL_DEPTH
            )
            .as_str(),
            func.span,
        ));
    }

    ctx.call_depth += 1;
    ctx.call_stack.push(func.name.clone());

    let result = (|| -> Result<CallResult, Diagnostic> {
        ctx.env.push();
        if args.len() != func.params.len() {
            return Err(type_error(
                format!(
                    "function '{}' expects {} {}, got {}",
                    callable_name,
                    func.params.len(),
                    argument_noun(func.params.len()),
                    args.len()
                )
                .as_str(),
                func.span,
            ));
        }
        for (param, arg) in func.params.iter().zip(args.iter()) {
            ensure_type_known(&param.ty, ctx.index, param.span)?;
            let coerced = coerce_value(arg.clone(), &param.ty, param.span)?;
            ctx.env.insert(
                param.name.clone(),
                Binding {
                    value: coerced,
                    ty: param.ty.clone(),
                    mutable: false,
                    shared_borrows: 0,
                    mut_borrowed: false,
                },
            );
        }
        let flow_result = exec_stmts(&func.body, ctx, exec);
        let popped = ctx.env.pop();
        release_borrows(&popped, ctx.env)?;
        let flow = flow_result?;
        match flow {
            Flow::Next => {
                if func.return_type.is_some() {
                    Err(type_error(
                        format!("function '{callable_name}' is missing return value").as_str(),
                        func.span,
                    ))
                } else {
                    Ok(CallResult::Continue(None))
                }
            }
            Flow::Return(value) => match (&func.return_type, value) {
                (Some(ret_ty), Some(v)) => {
                    let coerced = coerce_value(v, ret_ty, func.span)?;
                    Ok(CallResult::Continue(Some(coerced)))
                }
                (Some(_), None) => Err(type_error(
                    format!("function '{callable_name}' must return a value").as_str(),
                    func.span,
                )),
                (None, Some(_)) => Err(type_error(
                    format!("function '{callable_name}' cannot return a value").as_str(),
                    func.span,
                )),
                (None, None) => Ok(CallResult::Continue(None)),
            },
            Flow::Exit => Ok(CallResult::Exit),
            Flow::BreakLoop | Flow::ContinueLoop => Err(Diagnostic::at_span(
                "internal error: illegal loop flow escaped function",
                func.span,
            )),
        }
    })();

    ctx.call_stack.pop();
    ctx.call_depth = ctx.call_depth.saturating_sub(1);
    result.map_err(|diag| diag.with_context(format!("in function '{callable_name}'")))
}

fn exec_stmts(
    stmts: &[Stmt],
    ctx: &mut FunctionContext<'_>,
    exec: &mut ExecState<'_>,
) -> Result<Flow, Diagnostic> {
    for stmt in stmts {
        let flow = exec_stmt(stmt, ctx, exec).map_err(|diag| {
            let span = stmt_span(stmt);
            diag.with_context(format!(
                "statement::{} at {}:{}",
                stmt_kind_name(stmt),
                span.line,
                span.column
            ))
        })?;
        if !matches!(flow, Flow::Next) {
            return Ok(flow);
        }
    }
    Ok(Flow::Next)
}

fn exec_stmt(
    stmt: &Stmt,
    ctx: &mut FunctionContext<'_>,
    exec: &mut ExecState<'_>,
) -> Result<Flow, Diagnostic> {
    match stmt {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if ctx.env.current_scope_contains(name) {
                return Err(Diagnostic::at_span(
                    format!("redefinition of '{name}'"),
                    span,
                ));
            }
            let value = eval_expr(expr, ctx.env, ctx.index)?;
            let (value, inferred_type) = match ty {
                Some(target) => {
                    ensure_type_known(target, ctx.index, *span)?;
                    let coerced = coerce_value(value, target, *span)?;
                    (coerced, target.clone())
                }
                None => {
                    if value_has_unresolved_generic_args(&value) {
                        return Err(type_error(
                            "cannot infer every generic type argument; add an explicit type annotation",
                            *span,
                        ));
                    }
                    (value.clone(), value_type(&value))
                }
            };

            ctx.env.insert(
                name.clone(),
                Binding {
                    value,
                    ty: inferred_type,
                    mutable: *mutable,
                    shared_borrows: 0,
                    mut_borrowed: false,
                },
            );
        }
        Stmt::Assign { name, expr, span } => {
            let (target_ty, target_mutable) = {
                let binding = ctx.env.get(name).ok_or_else(|| {
                    Diagnostic::at_span(format!("unknown identifier '{name}'"), span)
                })?;
                (binding.ty.clone(), binding.mutable)
            };
            if !target_mutable {
                return Err(type_error(
                    format!("cannot assign to immutable '{name}'").as_str(),
                    *span,
                ));
            }
            let value = eval_expr(expr, ctx.env, ctx.index)?;
            let coerced = coerce_value(value, &target_ty, *span)?;
            let binding = ctx
                .env
                .get_mut(name)
                .ok_or_else(|| Diagnostic::at_span(format!("unknown identifier '{name}'"), span))?;
            binding.value = coerced;
        }
        Stmt::AssignField {
            receiver,
            field,
            expr,
            span,
        } => {
            let (struct_name, target_name) = {
                let binding = ctx.env.get(receiver).ok_or_else(|| {
                    Diagnostic::at_span(format!("unknown identifier '{receiver}'"), span)
                })?;
                match (&binding.ty, &binding.value) {
                    (TypeName::Struct(name), Value::Struct { .. }) => {
                        if !binding.mutable {
                            return Err(type_error(
                                format!("cannot assign to immutable '{receiver}'").as_str(),
                                *span,
                            ));
                        }
                        (name.clone(), receiver.clone())
                    }
                    (
                        TypeName::Ref {
                            mutable: true,
                            inner,
                        },
                        Value::Ref {
                            target,
                            mutable: true,
                            ..
                        },
                    ) => match inner.as_ref() {
                        TypeName::Struct(name) => (name.clone(), target.clone()),
                        _ => return Err(type_error("field assignment on non-struct value", *span)),
                    },
                    (TypeName::Ref { mutable: false, .. }, Value::Ref { .. }) => {
                        return Err(type_error(
                            format!("cannot assign through shared reference '{receiver}'").as_str(),
                            *span,
                        ));
                    }
                    _ => return Err(type_error("field assignment on non-struct value", *span)),
                }
            };

            let field_ty = {
                let layout = ctx.index.struct_layouts.get(&struct_name).ok_or_else(|| {
                    Diagnostic::at_span(format!("unknown struct '{struct_name}'"), span)
                })?;
                let entry = layout
                    .iter()
                    .find(|entry| entry.name == *field)
                    .ok_or_else(|| Diagnostic::at_span(format!("unknown field '{field}'"), span))?;
                entry.ty.clone()
            };

            let rhs_value = eval_expr(expr, ctx.env, ctx.index)?;
            let coerced = coerce_value(rhs_value, &field_ty, *span)?;
            let binding = ctx.env.get_mut(&target_name).ok_or_else(|| {
                Diagnostic::at_span(format!("unknown identifier '{target_name}'"), span)
            })?;
            match &mut binding.value {
                Value::Struct { fields, .. } => {
                    if !fields.contains_key(field) {
                        return Err(type_error(
                            format!("unknown field '{field}'").as_str(),
                            *span,
                        ));
                    }
                    fields.insert(field.clone(), coerced);
                }
                _ => return Err(type_error("field assignment on non-struct value", *span)),
            }
        }
        Stmt::AssignIndex {
            name,
            index,
            expr,
            span,
        } => {
            let target_mutable = {
                let binding = ctx.env.get(name).ok_or_else(|| {
                    Diagnostic::at_span(format!("unknown identifier '{name}'"), span)
                })?;
                binding.mutable
            };
            if !target_mutable {
                return Err(type_error(
                    format!("cannot assign to immutable '{name}'").as_str(),
                    *span,
                ));
            }

            let idx_value = eval_expr(index, ctx.env, ctx.index)?;
            let rhs_value = eval_expr(expr, ctx.env, ctx.index)?;
            let binding = ctx
                .env
                .get_mut(name)
                .ok_or_else(|| Diagnostic::at_span(format!("unknown identifier '{name}'"), span))?;
            match (&mut binding.value, &binding.ty) {
                (Value::Array { elem_type, elems }, TypeName::Array { elem, .. })
                | (Value::List { elem_type, elems }, TypeName::List { elem }) => {
                    let idx = match idx_value {
                        Value::UInt { value, .. } => usize::try_from(value)
                            .map_err(|_| type_error("index out of bounds", *span))?,
                        Value::Int { value, .. } => {
                            if value < 0 {
                                return Err(type_error("index must be non-negative", *span));
                            }
                            usize::try_from(value as u128)
                                .map_err(|_| type_error("index out of bounds", *span))?
                        }
                        _ => return Err(type_error("index must be integer", *span)),
                    };
                    let slot = elems
                        .get_mut(idx)
                        .ok_or_else(|| type_error("index out of bounds", *span))?;
                    let coerced = coerce_value(rhs_value, elem.as_ref(), *span)?;
                    *slot = coerced;
                    *elem_type = elem.as_ref().clone();
                }
                (
                    Value::Dict {
                        key_type,
                        value_type,
                        entries,
                    },
                    TypeName::Dict { key, value },
                ) => {
                    let key_string = dict_key_from_typed_value(idx_value, key.as_ref(), *span)?;
                    let coerced = coerce_value(rhs_value, value.as_ref(), *span)?;
                    entries.insert(key_string, coerced);
                    *key_type = key.as_ref().clone();
                    *value_type = value.as_ref().clone();
                }
                (
                    Value::Map {
                        key_type,
                        value_type,
                        entries,
                        keys,
                    },
                    TypeName::Map { key, value },
                ) => {
                    let coerced_key = coerce_value(idx_value, key.as_ref(), *span)?;
                    let key_string = dict_key_repr(&coerced_key, *span)?;
                    let coerced = coerce_value(rhs_value, value.as_ref(), *span)?;
                    entries.insert(key_string.clone(), coerced);
                    keys.insert(key_string, coerced_key);
                    *key_type = key.as_ref().clone();
                    *value_type = value.as_ref().clone();
                }
                _ => {
                    return Err(type_error(
                        "index assignment requires array, list, dictionary, or map",
                        *span,
                    ));
                }
            }
        }
        Stmt::AssignStructListIndex {
            receiver,
            field,
            index,
            expr,
            span,
        } => {
            let (mutable, idx_value, rhs_value) = {
                let binding = ctx.env.get(receiver).ok_or_else(|| {
                    Diagnostic::at_span(format!("unknown identifier '{receiver}'"), span)
                })?;
                (
                    binding.mutable,
                    eval_expr(index, ctx.env, ctx.index)?,
                    eval_expr(expr, ctx.env, ctx.index)?,
                )
            };
            if !mutable {
                return Err(type_error(
                    format!("cannot assign through immutable '{receiver}'").as_str(),
                    *span,
                ));
            }
            let idx = match idx_value {
                Value::UInt { value, .. } => {
                    usize::try_from(value).map_err(|_| type_error("index out of bounds", *span))?
                }
                Value::Int { value, .. } if value >= 0 => usize::try_from(value as u128)
                    .map_err(|_| type_error("index out of bounds", *span))?,
                Value::Int { .. } => return Err(type_error("index must be non-negative", *span)),
                _ => return Err(type_error("index must be integer", *span)),
            };
            let binding = ctx.env.get_mut(receiver).ok_or_else(|| {
                Diagnostic::at_span(format!("unknown identifier '{receiver}'"), span)
            })?;
            let Value::Struct { fields, .. } = &mut binding.value else {
                return Err(type_error("list-field mutation requires a struct", *span));
            };
            let Some(Value::List { elem_type, elems }) = fields.get_mut(field) else {
                return Err(type_error("indexed struct field must be a list", *span));
            };
            let slot = elems
                .get_mut(idx)
                .ok_or_else(|| type_error("index out of bounds", *span))?;
            *slot = coerce_value(rhs_value, elem_type, *span)?;
        }
        Stmt::Call { name, args, span } => {
            let func = ctx
                .index
                .functions
                .get(name)
                .ok_or_else(|| unknown_function_diagnostic(name, *span))?;
            let arg_values: Vec<Value> = args
                .iter()
                .map(|arg| eval_expr(arg, ctx.env, ctx.index))
                .collect::<Result<Vec<_>, _>>()?;
            match exec_function(func, &arg_values, ctx, exec)? {
                CallResult::Exit => return Ok(Flow::Exit),
                CallResult::Continue(_) => {}
            }
        }
        Stmt::MethodCall {
            receiver,
            name,
            args,
            span,
        } => {
            let arg_values: Vec<Value> = args
                .iter()
                .map(|arg| {
                    if name == "sort_by" {
                        if let Expr::Ident {
                            name: comparator_name,
                            ..
                        } = arg
                        {
                            if ctx.index.functions.contains_key(comparator_name) {
                                return Ok(Value::Str(comparator_name.clone()));
                            }
                        }
                    }
                    eval_expr(arg, ctx.env, ctx.index)
                })
                .collect::<Result<Vec<_>, _>>()?;

            if let Some(flow) =
                try_exec_user_method_stmt(receiver, name, &arg_values, *span, ctx, exec)?
            {
                return Ok(flow);
            }

            let target_mutable = {
                let binding = ctx.env.get(receiver).ok_or_else(|| {
                    Diagnostic::at_span(format!("unknown identifier '{receiver}'"), span)
                })?;
                binding.mutable
            };
            if !target_mutable {
                return Err(type_error(
                    format!("cannot mutate immutable '{receiver}'").as_str(),
                    *span,
                ));
            }
            let sort_env_snapshot = if matches!(
                name.as_str(),
                "sort"
                    | "sort_unstable"
                    | "sort_stable"
                    | "sort_radix_unstable"
                    | "sort_radix_stable"
                    | "sort_by"
            ) {
                Some(ctx.env.clone())
            } else {
                None
            };
            let binding = ctx.env.get_mut(receiver).ok_or_else(|| {
                Diagnostic::at_span(format!("unknown identifier '{receiver}'"), span)
            })?;
            match (&mut binding.value, &binding.ty) {
                (Value::Array { elem_type, elems }, TypeName::Array { elem, .. }) => match name
                    .as_str()
                {
                    "push" => {
                        if arg_values.len() != 1 {
                            return Err(type_error("push() expects exactly one argument", *span));
                        }
                        let coerced = coerce_value(arg_values[0].clone(), elem.as_ref(), *span)?;
                        elems.push(coerced);
                        *elem_type = elem.as_ref().clone();
                    }
                    "pop" => {
                        if !arg_values.is_empty() {
                            return Err(type_error("pop() expects no arguments", *span));
                        }
                        if elems.pop().is_none() {
                            return Err(type_error("cannot pop() from empty array", *span));
                        }
                    }
                    "sort" | "sort_unstable" => {
                        if !arg_values.is_empty() {
                            return Err(type_error(
                                format!("{name}() expects no arguments").as_str(),
                                *span,
                            ));
                        }
                        sort_array_values(
                            elems,
                            None,
                            *span,
                            ctx.index,
                            sort_env_snapshot
                                .as_ref()
                                .expect("sort snapshot should exist for sort methods"),
                            SortPlan::auto_unstable(),
                        )?;
                    }
                    "sort_stable" => {
                        if !arg_values.is_empty() {
                            return Err(type_error("sort_stable() expects no arguments", *span));
                        }
                        sort_array_values(
                            elems,
                            None,
                            *span,
                            ctx.index,
                            sort_env_snapshot
                                .as_ref()
                                .expect("sort snapshot should exist for sort methods"),
                            SortPlan::auto_stable(),
                        )?;
                    }
                    "sort_radix_unstable" => {
                        if !arg_values.is_empty() {
                            return Err(type_error(
                                "sort_radix_unstable() expects no arguments",
                                *span,
                            ));
                        }
                        sort_array_values(
                            elems,
                            None,
                            *span,
                            ctx.index,
                            sort_env_snapshot
                                .as_ref()
                                .expect("sort snapshot should exist for sort methods"),
                            SortPlan::radix_unstable(),
                        )?;
                    }
                    "sort_radix_stable" => {
                        if !arg_values.is_empty() {
                            return Err(type_error(
                                "sort_radix_stable() expects no arguments",
                                *span,
                            ));
                        }
                        sort_array_values(
                            elems,
                            None,
                            *span,
                            ctx.index,
                            sort_env_snapshot
                                .as_ref()
                                .expect("sort snapshot should exist for sort methods"),
                            SortPlan::radix_stable(),
                        )?;
                    }
                    "sort_by" => {
                        if arg_values.len() != 1 {
                            return Err(type_error(
                                "sort_by() expects one comparator function argument",
                                *span,
                            ));
                        }
                        let compare_fn = match &arg_values[0] {
                            Value::Str(name) => name.as_str(),
                            _ => {
                                return Err(type_error(
                                    "sort_by() expects comparator function name (identifier or string)",
                                    *span,
                                ));
                            }
                        };
                        sort_array_values(
                            elems,
                            Some(compare_fn),
                            *span,
                            ctx.index,
                            sort_env_snapshot
                                .as_ref()
                                .expect("sort snapshot should exist for sort_by()"),
                            SortPlan::auto_unstable(),
                        )?;
                    }
                    _ => {
                        return Err(type_error(
                            format!("unknown mutable array method '{}()'", name).as_str(),
                            *span,
                        ));
                    }
                },
                (Value::List { elem_type, elems }, TypeName::List { elem }) => {
                    match name.as_str() {
                        "push" => {
                            if arg_values.len() != 1 {
                                return Err(type_error(
                                    "push() expects exactly one argument",
                                    *span,
                                ));
                            }
                            elems.push(coerce_value(arg_values[0].clone(), elem.as_ref(), *span)?);
                            *elem_type = elem.as_ref().clone();
                        }
                        "pop" => {
                            if !arg_values.is_empty() {
                                return Err(type_error("pop() expects no arguments", *span));
                            }
                            if elems.pop().is_none() {
                                return Err(type_error("cannot pop() from empty list", *span));
                            }
                        }
                        "clear" => {
                            if !arg_values.is_empty() {
                                return Err(type_error("clear() expects no arguments", *span));
                            }
                            elems.clear();
                        }
                        "reserve" => {
                            if arg_values.len() != 1 {
                                return Err(type_error(
                                    "reserve() expects exactly one additional-capacity argument",
                                    *span,
                                ));
                            }
                            let additional = value_to_index(arg_values[0].clone(), *span)?
                                .ok_or_else(|| {
                                    type_error("reserve() capacity is out of range", *span)
                                })?;
                            elems.try_reserve(additional).map_err(|_| {
                                type_error("reserve() capacity is too large", *span)
                            })?;
                        }
                        "shrink_to_fit" => {
                            if !arg_values.is_empty() {
                                return Err(type_error(
                                    "shrink_to_fit() expects no arguments",
                                    *span,
                                ));
                            }
                            elems.shrink_to_fit();
                        }
                        "shrink_to" => {
                            if arg_values.len() != 1 {
                                return Err(type_error(
                                    "shrink_to() expects exactly one minimum-capacity argument",
                                    *span,
                                ));
                            }
                            let minimum = value_to_index(arg_values[0].clone(), *span)?
                                .ok_or_else(|| {
                                    type_error("shrink_to() capacity is out of range", *span)
                                })?;
                            elems.shrink_to(minimum);
                        }
                        _ => {
                            return Err(type_error(
                                format!("unknown mutable list method '{}()'", name).as_str(),
                                *span,
                            ));
                        }
                    }
                }
                (
                    Value::Dict {
                        key_type,
                        value_type,
                        entries,
                    },
                    TypeName::Dict { key, value },
                ) => match name.as_str() {
                    "set" => {
                        if arg_values.len() != 2 {
                            return Err(type_error("set() expects exactly two arguments", *span));
                        }
                        let key_string =
                            dict_key_from_typed_value(arg_values[0].clone(), key.as_ref(), *span)?;
                        let coerced = coerce_value(arg_values[1].clone(), value.as_ref(), *span)?;
                        entries.insert(key_string, coerced);
                        *key_type = key.as_ref().clone();
                        *value_type = value.as_ref().clone();
                    }
                    "remove" => {
                        if arg_values.len() != 1 {
                            return Err(type_error("remove() expects exactly one argument", *span));
                        }
                        let key_string =
                            dict_key_from_typed_value(arg_values[0].clone(), key.as_ref(), *span)?;
                        entries.remove(&key_string);
                    }
                    _ => {
                        return Err(type_error(
                            format!("unknown mutable dictionary method '{}()'", name).as_str(),
                            *span,
                        ));
                    }
                },
                (
                    Value::Map {
                        key_type,
                        value_type,
                        entries,
                        keys,
                    },
                    TypeName::Map { key, value },
                ) => match name.as_str() {
                    "set" => {
                        if arg_values.len() != 2 {
                            return Err(type_error("set() expects exactly two arguments", *span));
                        }
                        let coerced_key = coerce_value(arg_values[0].clone(), key.as_ref(), *span)?;
                        let key_string = dict_key_repr(&coerced_key, *span)?;
                        let coerced = coerce_value(arg_values[1].clone(), value.as_ref(), *span)?;
                        entries.insert(key_string.clone(), coerced);
                        keys.insert(key_string, coerced_key);
                        *key_type = key.as_ref().clone();
                        *value_type = value.as_ref().clone();
                    }
                    "remove" => {
                        if arg_values.len() != 1 {
                            return Err(type_error("remove() expects exactly one argument", *span));
                        }
                        let key_string =
                            dict_key_from_typed_value(arg_values[0].clone(), key.as_ref(), *span)?;
                        entries.remove(&key_string);
                        keys.remove(&key_string);
                    }
                    "clear" => {
                        if !arg_values.is_empty() {
                            return Err(type_error("clear() expects no arguments", *span));
                        }
                        entries.clear();
                        keys.clear();
                    }
                    _ => {
                        return Err(type_error(
                            format!("unknown mutable map method '{}()'", name).as_str(),
                            *span,
                        ));
                    }
                },
                _ => {
                    return Err(type_error(
                        "method call requires mutable array, list, dictionary, or map",
                        *span,
                    ));
                }
            }
        }
        Stmt::StructListMethodCall {
            receiver,
            field,
            name,
            args,
            span,
        } => {
            let arg_values = args
                .iter()
                .map(|arg| eval_expr(arg, ctx.env, ctx.index))
                .collect::<Result<Vec<_>, _>>()?;
            let binding = ctx.env.get_mut(receiver).ok_or_else(|| {
                Diagnostic::at_span(format!("unknown identifier '{receiver}'"), span)
            })?;
            if !binding.mutable {
                return Err(type_error(
                    format!("cannot mutate immutable '{receiver}'").as_str(),
                    *span,
                ));
            }
            let Value::Struct { fields, .. } = &mut binding.value else {
                return Err(type_error("list-field mutation requires a struct", *span));
            };
            let Some(Value::List { elem_type, elems }) = fields.get_mut(field) else {
                return Err(type_error("mutated struct field must be a list", *span));
            };
            match name.as_str() {
                "push" if arg_values.len() == 1 => {
                    elems.push(coerce_value(arg_values[0].clone(), elem_type, *span)?)
                }
                "pop" if args.is_empty() => {
                    if elems.pop().is_none() {
                        return Err(type_error("cannot pop() from empty list", *span));
                    }
                }
                "clear" if args.is_empty() => elems.clear(),
                "push" => return Err(type_error("push() expects exactly one argument", *span)),
                "pop" | "clear" => {
                    return Err(type_error(
                        format!("{name}() expects no arguments").as_str(),
                        *span,
                    ));
                }
                _ => return Err(type_error("unknown mutable list-field method", *span)),
            }
        }
        Stmt::Print { expr, span } => {
            let value = eval_expr(expr, ctx.env, ctx.index)?;
            let output = match value {
                Value::Str(text) => text,
                Value::Int { value, .. } => value.to_string(),
                Value::UInt { value, .. } => value.to_string(),
                Value::Float { value, .. } => value.to_string(),
                Value::Bool(value) => value.to_string(),
                Value::Char(value) => value.to_string(),
                Value::Ref { .. } => {
                    return Err(type_error("print does not accept references", *span));
                }
                Value::Struct { .. } => {
                    return Err(type_error("print does not accept structs", *span));
                }
                Value::Enum { .. } => {
                    return Err(type_error("print does not accept enums", *span));
                }
                Value::Array { .. } => {
                    return Err(type_error("print does not accept arrays", *span));
                }
                Value::List { .. } => {
                    return Err(type_error("print does not accept lists", *span));
                }
                Value::Dict { .. } => {
                    return Err(type_error("print does not accept dictionaries", *span));
                }
                Value::Map { .. } => {
                    return Err(type_error("print does not accept maps", *span));
                }
            };
            exec.lowered.push(LoweredStmt::Print(output));
        }
        Stmt::Exit { expr, span } => {
            let value = eval_expr(expr, ctx.env, ctx.index)?;
            let code = match value {
                Value::Int { value, .. } => {
                    if value < 0 {
                        return Err(type_error("exit expects a non-negative integer", *span));
                    }
                    u64::try_from(value)
                        .map_err(|_| type_error("exit code out of range for u64", *span))?
                }
                Value::UInt { value, .. } => u64::try_from(value)
                    .map_err(|_| type_error("exit code out of range for u64", *span))?,
                _ => return Err(type_error("exit expects an integer", *span)),
            };
            exec.lowered.push(LoweredStmt::Exit(code));
            return Ok(Flow::Exit);
        }
        Stmt::BenchLoop { iterations, span } => {
            let value = eval_expr(iterations, ctx.env, ctx.index)?;
            let iterations = match value {
                Value::UInt { value, .. } => u64::try_from(value)
                    .map_err(|_| type_error("benchloop iterations out of range", *span))?,
                Value::Int { value, .. } => {
                    if value < 0 {
                        return Err(type_error(
                            "benchloop expects a non-negative integer",
                            *span,
                        ));
                    }
                    u64::try_from(value)
                        .map_err(|_| type_error("benchloop iterations out of range", *span))?
                }
                _ => return Err(type_error("benchloop expects an integer", *span)),
            };
            exec.lowered
                .push(LoweredStmt::RuntimeBenchLoop { iterations });
            return Ok(Flow::Exit);
        }
        Stmt::Block { stmts, .. } => {
            ctx.env.push();
            let flow = exec_stmts(stmts, ctx, exec)?;
            let popped = ctx.env.pop();
            release_borrows(&popped, ctx.env)?;
            if !matches!(flow, Flow::Next) {
                return Ok(flow);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let value = eval_expr(cond, ctx.env, ctx.index)?;
            let take_then = match value {
                Value::Bool(v) => v,
                _ => return Err(type_error("if condition must be bool", cond.span())),
            };
            if take_then {
                ctx.env.push();
                let flow = exec_stmts(then_branch, ctx, exec)?;
                let popped = ctx.env.pop();
                release_borrows(&popped, ctx.env)?;
                if !matches!(flow, Flow::Next) {
                    return Ok(flow);
                }
            } else if let Some(branch) = else_branch {
                ctx.env.push();
                let flow = exec_stmts(branch, ctx, exec)?;
                let popped = ctx.env.pop();
                release_borrows(&popped, ctx.env)?;
                if !matches!(flow, Flow::Next) {
                    return Ok(flow);
                }
            }
        }
        Stmt::While { cond, body, .. } => {
            let mut iterations = 0usize;
            ctx.loop_depth += 1;
            loop {
                let value = eval_expr(cond, ctx.env, ctx.index)?;
                let proceed = match value {
                    Value::Bool(v) => v,
                    _ => return Err(type_error("while condition must be bool", cond.span())),
                };
                if !proceed {
                    break;
                }
                iterations += 1;
                if iterations > LOOP_LIMIT {
                    return Err(type_error("loop limit exceeded", cond.span()));
                }
                ctx.env.push();
                let flow = exec_stmts(body, ctx, exec)?;
                let popped = ctx.env.pop();
                release_borrows(&popped, ctx.env)?;
                match flow {
                    Flow::Next => {}
                    Flow::ContinueLoop => continue,
                    Flow::BreakLoop => break,
                    Flow::Exit => {
                        ctx.loop_depth -= 1;
                        return Ok(Flow::Exit);
                    }
                    Flow::Return(value) => {
                        ctx.loop_depth -= 1;
                        return Ok(Flow::Return(value));
                    }
                }
            }
            ctx.loop_depth -= 1;
        }
        Stmt::Loop { body, span } => {
            let mut iterations = 0usize;
            ctx.loop_depth += 1;
            loop {
                iterations += 1;
                if iterations > LOOP_LIMIT {
                    ctx.loop_depth -= 1;
                    return Err(type_error("loop limit exceeded", *span));
                }
                ctx.env.push();
                let flow = exec_stmts(body, ctx, exec)?;
                let popped = ctx.env.pop();
                release_borrows(&popped, ctx.env)?;
                match flow {
                    Flow::Next => {}
                    Flow::ContinueLoop => continue,
                    Flow::BreakLoop => break,
                    Flow::Exit => {
                        ctx.loop_depth -= 1;
                        return Ok(Flow::Exit);
                    }
                    Flow::Return(value) => {
                        ctx.loop_depth -= 1;
                        return Ok(Flow::Return(value));
                    }
                }
            }
            ctx.loop_depth -= 1;
        }
        Stmt::For {
            name,
            start,
            end,
            body,
            span,
        } => {
            let start_val = eval_expr(start, ctx.env, ctx.index)?;
            let end_val = eval_expr(end, ctx.env, ctx.index)?;
            let range = coerce_for_range(start_val, end_val, *span)?;
            let mut iterations = 0usize;
            ctx.loop_depth += 1;
            match range {
                ForRange::Signed { bits, start, end } => {
                    let mut cursor = start;
                    while cursor < end {
                        iterations += 1;
                        if iterations > LOOP_LIMIT {
                            ctx.loop_depth -= 1;
                            return Err(type_error("loop limit exceeded", *span));
                        }
                        ctx.env.push();
                        ctx.env.insert(
                            name.clone(),
                            Binding {
                                value: Value::Int {
                                    bits,
                                    value: cursor,
                                },
                                ty: TypeName::Int { signed: true, bits },
                                mutable: false,
                                shared_borrows: 0,
                                mut_borrowed: false,
                            },
                        );
                        let flow = exec_stmts(body, ctx, exec)?;
                        let popped = ctx.env.pop();
                        release_borrows(&popped, ctx.env)?;
                        match flow {
                            Flow::Next => {}
                            Flow::ContinueLoop => {
                                cursor = cursor
                                    .checked_add(1)
                                    .ok_or_else(|| type_error("integer overflow", *span))?;
                                continue;
                            }
                            Flow::BreakLoop => break,
                            Flow::Exit => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Exit);
                            }
                            Flow::Return(value) => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Return(value));
                            }
                        }
                        cursor = cursor
                            .checked_add(1)
                            .ok_or_else(|| type_error("integer overflow", *span))?;
                    }
                }
                ForRange::Unsigned { bits, start, end } => {
                    let mut cursor = start;
                    while cursor < end {
                        iterations += 1;
                        if iterations > LOOP_LIMIT {
                            ctx.loop_depth -= 1;
                            return Err(type_error("loop limit exceeded", *span));
                        }
                        ctx.env.push();
                        ctx.env.insert(
                            name.clone(),
                            Binding {
                                value: Value::UInt {
                                    bits,
                                    value: cursor,
                                },
                                ty: TypeName::Int {
                                    signed: false,
                                    bits,
                                },
                                mutable: false,
                                shared_borrows: 0,
                                mut_borrowed: false,
                            },
                        );
                        let flow = exec_stmts(body, ctx, exec)?;
                        let popped = ctx.env.pop();
                        release_borrows(&popped, ctx.env)?;
                        match flow {
                            Flow::Next => {}
                            Flow::ContinueLoop => {
                                cursor = cursor
                                    .checked_add(1)
                                    .ok_or_else(|| type_error("integer overflow", *span))?;
                                continue;
                            }
                            Flow::BreakLoop => break,
                            Flow::Exit => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Exit);
                            }
                            Flow::Return(value) => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Return(value));
                            }
                        }
                        cursor = cursor
                            .checked_add(1)
                            .ok_or_else(|| type_error("integer overflow", *span))?;
                    }
                }
            }
            ctx.loop_depth -= 1;
        }
        Stmt::ParFor {
            name,
            start,
            end,
            body,
            reduction,
            span,
        } => {
            let start_val = eval_expr(start, ctx.env, ctx.index)?;
            let end_val = eval_expr(end, ctx.env, ctx.index)?;
            let range = coerce_for_range(start_val, end_val, *span)?;
            let iterations = for_range_values(range);
            if iterations.len() > LOOP_LIMIT {
                return Err(type_error("loop limit exceeded", *span));
            }
            if iterations.is_empty() {
                return Ok(Flow::Next);
            }

            if let Some(reduction) = reduction {
                execute_parfor_reduction(name, reduction, &iterations, ctx, *span)?;
            } else {
                let lowered = execute_parfor_body(name, body, &iterations, ctx, *span)?;
                exec.lowered.extend(lowered);
            }
        }
        Stmt::ForEach {
            name,
            iterable,
            body,
            span,
        } => {
            let iterable_value = eval_expr(iterable, ctx.env, ctx.index)?;
            let mut iterations = 0usize;
            ctx.loop_depth += 1;
            match iterable_value {
                Value::Array { elem_type, elems } | Value::List { elem_type, elems } => {
                    for value in elems {
                        iterations += 1;
                        if iterations > LOOP_LIMIT {
                            ctx.loop_depth -= 1;
                            return Err(type_error("loop limit exceeded", *span));
                        }
                        ctx.env.push();
                        ctx.env.insert(
                            name.clone(),
                            Binding {
                                value,
                                ty: elem_type.clone(),
                                mutable: false,
                                shared_borrows: 0,
                                mut_borrowed: false,
                            },
                        );
                        let flow = exec_stmts(body, ctx, exec)?;
                        let popped = ctx.env.pop();
                        release_borrows(&popped, ctx.env)?;
                        match flow {
                            Flow::Next => {}
                            Flow::ContinueLoop => continue,
                            Flow::BreakLoop => break,
                            Flow::Exit => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Exit);
                            }
                            Flow::Return(value) => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Return(value));
                            }
                        }
                    }
                }
                Value::Dict { entries, .. } => {
                    for key in entries.keys() {
                        iterations += 1;
                        if iterations > LOOP_LIMIT {
                            ctx.loop_depth -= 1;
                            return Err(type_error("loop limit exceeded", *span));
                        }
                        ctx.env.push();
                        ctx.env.insert(
                            name.clone(),
                            Binding {
                                value: Value::Str(key.clone()),
                                ty: TypeName::String,
                                mutable: false,
                                shared_borrows: 0,
                                mut_borrowed: false,
                            },
                        );
                        let flow = exec_stmts(body, ctx, exec)?;
                        let popped = ctx.env.pop();
                        release_borrows(&popped, ctx.env)?;
                        match flow {
                            Flow::Next => {}
                            Flow::ContinueLoop => continue,
                            Flow::BreakLoop => break,
                            Flow::Exit => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Exit);
                            }
                            Flow::Return(value) => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Return(value));
                            }
                        }
                    }
                }
                Value::Map { key_type, keys, .. } => {
                    for key in keys.values() {
                        iterations += 1;
                        if iterations > LOOP_LIMIT {
                            ctx.loop_depth -= 1;
                            return Err(type_error("loop limit exceeded", *span));
                        }
                        ctx.env.push();
                        ctx.env.insert(
                            name.clone(),
                            Binding {
                                value: key.clone(),
                                ty: key_type.clone(),
                                mutable: false,
                                shared_borrows: 0,
                                mut_borrowed: false,
                            },
                        );
                        let flow = exec_stmts(body, ctx, exec)?;
                        let popped = ctx.env.pop();
                        release_borrows(&popped, ctx.env)?;
                        match flow {
                            Flow::Next => {}
                            Flow::ContinueLoop => continue,
                            Flow::BreakLoop => break,
                            Flow::Exit => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Exit);
                            }
                            Flow::Return(value) => {
                                ctx.loop_depth -= 1;
                                return Ok(Flow::Return(value));
                            }
                        }
                    }
                }
                _ => {
                    ctx.loop_depth -= 1;
                    return Err(type_error(
                        "foreach requires array, list, dictionary, or map",
                        iterable.span(),
                    ));
                }
            }
            ctx.loop_depth -= 1;
        }
        Stmt::Assert {
            cond,
            message,
            span,
        } => {
            let value = eval_expr(cond, ctx.env, ctx.index)?;
            let passed = match value {
                Value::Bool(value) => value,
                _ => return Err(type_error("assert condition must be bool", cond.span())),
            };
            if !passed {
                let mut diagnostic = if let Some(message_expr) = message {
                    let message_value = eval_expr(message_expr, ctx.env, ctx.index)?;
                    let msg = stringify_value_for_error(message_value, *span)?;
                    Diagnostic::at_span(format!("assertion failed: {msg}"), span)
                } else {
                    Diagnostic::at_span("assertion failed", span)
                };
                diagnostic = diagnostic.with_context("statement::assert");
                return Err(diagnostic);
            }
        }
        Stmt::Panic { message, span } => {
            let message_value = eval_expr(message, ctx.env, ctx.index)?;
            let msg = stringify_value_for_error(message_value, *span)?;
            return Err(
                Diagnostic::at_span(format!("panic: {msg}"), span).with_context("statement::panic")
            );
        }
        Stmt::Return { expr, .. } => {
            let value = expr
                .as_ref()
                .map(|expr| eval_expr(expr, ctx.env, ctx.index))
                .transpose()?;
            return Ok(Flow::Return(value));
        }
        Stmt::Break { span } => {
            if ctx.loop_depth == 0 {
                return Err(type_error("break used outside loop", *span));
            }
            return Ok(Flow::BreakLoop);
        }
        Stmt::Continue { span } => {
            if ctx.loop_depth == 0 {
                return Err(type_error("continue used outside loop", *span));
            }
            return Ok(Flow::ContinueLoop);
        }
    }

    Ok(Flow::Next)
}

fn value_has_unresolved_generic_args(value: &Value) -> bool {
    match value {
        Value::Enum {
            type_args, payload, ..
        } => {
            type_args.iter().any(Option::is_none)
                || match payload {
                    EnumPayloadValue::Unit => false,
                    EnumPayloadValue::Tuple(values) => {
                        values.iter().any(value_has_unresolved_generic_args)
                    }
                    EnumPayloadValue::Named(fields) => {
                        fields.values().any(value_has_unresolved_generic_args)
                    }
                }
        }
        Value::Struct { fields, .. } => fields.values().any(value_has_unresolved_generic_args),
        Value::Array { elem_type, elems } => {
            matches!(elem_type, TypeName::Struct(name) if name == "_")
                || elems.iter().any(value_has_unresolved_generic_args)
        }
        Value::List { elem_type, elems } => {
            matches!(elem_type, TypeName::Struct(name) if name == "_")
                || elems.iter().any(value_has_unresolved_generic_args)
        }
        Value::Dict { entries, .. } => entries.values().any(value_has_unresolved_generic_args),
        Value::Map { entries, .. } => entries.values().any(value_has_unresolved_generic_args),
        Value::Ref { .. }
        | Value::Bool(_)
        | Value::Str(_)
        | Value::Char(_)
        | Value::Int { .. }
        | Value::UInt { .. }
        | Value::Float { .. } => false,
    }
}

mod kernels;
use kernels::*;
mod runtime_generic;
use runtime_generic::*;
fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Let { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::AssignField { span, .. }
        | Stmt::AssignIndex { span, .. }
        | Stmt::AssignStructListIndex { span, .. }
        | Stmt::MethodCall { span, .. }
        | Stmt::StructListMethodCall { span, .. }
        | Stmt::Return { span, .. }
        | Stmt::Call { span, .. }
        | Stmt::Print { span, .. }
        | Stmt::Exit { span, .. }
        | Stmt::BenchLoop { span, .. }
        | Stmt::Block { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. }
        | Stmt::Loop { span, .. }
        | Stmt::For { span, .. }
        | Stmt::ParFor { span, .. }
        | Stmt::ForEach { span, .. }
        | Stmt::Assert { span, .. }
        | Stmt::Panic { span, .. }
        | Stmt::Break { span, .. }
        | Stmt::Continue { span, .. } => *span,
    }
}

fn stmt_kind_name(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::Let { .. } => "let",
        Stmt::Assign { .. } => "assign",
        Stmt::AssignField { .. } => "assign_field",
        Stmt::AssignIndex { .. } => "assign_index",
        Stmt::AssignStructListIndex { .. } => "assign_struct_list_index",
        Stmt::MethodCall { .. } => "method_call",
        Stmt::StructListMethodCall { .. } => "struct_list_method_call",
        Stmt::Return { .. } => "return",
        Stmt::Call { .. } => "call",
        Stmt::Print { .. } => "print",
        Stmt::Exit { .. } => "exit",
        Stmt::BenchLoop { .. } => "benchloop",
        Stmt::Block { .. } => "block",
        Stmt::If { .. } => "if",
        Stmt::While { .. } => "while",
        Stmt::Loop { .. } => "loop",
        Stmt::For { .. } => "for",
        Stmt::ParFor { .. } => "parfor",
        Stmt::ForEach { .. } => "foreach",
        Stmt::Assert { .. } => "assert",
        Stmt::Panic { .. } => "panic",
        Stmt::Break { .. } => "break",
        Stmt::Continue { .. } => "continue",
    }
}

fn ensure_type_known(ty: &TypeName, index: &ProgramIndex, span: Span) -> Result<(), Diagnostic> {
    match ty {
        TypeName::Struct(name) => {
            if !index.structs.contains_key(name) && !index.enums.contains_key(name) {
                return Err(Diagnostic::at_span(
                    format!("unknown nominal type '{name}'"),
                    span,
                ));
            }
            if let Some(enum_def) = index.enums.get(name) {
                if !enum_def.type_params.is_empty() {
                    return Err(Diagnostic::at_span(
                        format!(
                            "generic enum '{}' requires {} type {}",
                            name,
                            enum_def.type_params.len(),
                            argument_noun(enum_def.type_params.len())
                        ),
                        span,
                    ));
                }
            }
        }
        TypeName::Applied { name, args } => {
            if matches!(name.as_str(), "Channel" | "Sender" | "Receiver") {
                if !matches!(
                    args.as_slice(),
                    [TypeName::Int {
                        signed: false,
                        bits: 64
                    }]
                ) {
                    return Err(Diagnostic::at_span(
                        format!("{name} currently requires the u64 element type"),
                        span,
                    ));
                }
                return Ok(());
            }
            let enum_def = index.enums.get(name).ok_or_else(|| {
                Diagnostic::at_span(format!("unknown generic enum '{name}'"), span)
            })?;
            if args.len() != enum_def.type_params.len() {
                return Err(Diagnostic::at_span(
                    format!(
                        "generic enum '{}' expects {} type {}, got {}",
                        name,
                        enum_def.type_params.len(),
                        argument_noun(enum_def.type_params.len()),
                        args.len()
                    ),
                    span,
                ));
            }
            for arg in args {
                ensure_type_known(arg, index, span)?;
            }
        }
        TypeName::Dict { key, value } | TypeName::Map { key, value } => {
            ensure_type_known(key, index, span)?;
            ensure_type_known(value, index, span)?;
        }
        TypeName::Array { elem, .. } | TypeName::List { elem } => {
            ensure_type_known(elem, index, span)?;
        }
        TypeName::Ref { inner, .. } => ensure_type_known(inner, index, span)?,
        _ => {}
    }
    Ok(())
}

fn ensure_enum_payload_type_known(
    ty: &TypeName,
    type_params: &HashSet<&str>,
    index: &ProgramIndex,
    span: Span,
) -> Result<(), Diagnostic> {
    match ty {
        TypeName::Struct(name) if type_params.contains(name.as_str()) => Ok(()),
        TypeName::Applied { name, args } => {
            let enum_def = index.enums.get(name).ok_or_else(|| {
                Diagnostic::at_span(format!("unknown generic enum '{name}'"), span)
            })?;
            if args.len() != enum_def.type_params.len() {
                return Err(Diagnostic::at_span(
                    format!(
                        "generic enum '{}' expects {} type {}, got {}",
                        name,
                        enum_def.type_params.len(),
                        argument_noun(enum_def.type_params.len()),
                        args.len()
                    ),
                    span,
                ));
            }
            for arg in args {
                ensure_enum_payload_type_known(arg, type_params, index, span)?;
            }
            Ok(())
        }
        TypeName::Dict { key, value } | TypeName::Map { key, value } => {
            ensure_enum_payload_type_known(key, type_params, index, span)?;
            ensure_enum_payload_type_known(value, type_params, index, span)
        }
        TypeName::Array { elem, .. }
        | TypeName::List { elem }
        | TypeName::Ref { inner: elem, .. } => {
            ensure_enum_payload_type_known(elem, type_params, index, span)
        }
        _ => ensure_type_known(ty, index, span),
    }
}

fn coerce_for_range(start: Value, end: Value, span: Span) -> Result<ForRange, Diagnostic> {
    match start {
        Value::Int { bits, value } => {
            let end = coerce_value(end, &TypeName::Int { signed: true, bits }, span)?;
            match end {
                Value::Int { value: end, .. } => Ok(ForRange::Signed {
                    bits,
                    start: value,
                    end,
                }),
                _ => Err(type_error("for range end must be integer", span)),
            }
        }
        Value::UInt { bits, value } => {
            let end = coerce_value(
                end,
                &TypeName::Int {
                    signed: false,
                    bits,
                },
                span,
            )?;
            match end {
                Value::UInt { value: end, .. } => Ok(ForRange::Unsigned {
                    bits,
                    start: value,
                    end,
                }),
                _ => Err(type_error("for range end must be integer", span)),
            }
        }
        _ => Err(type_error("for range start must be integer", span)),
    }
}

fn for_range_values(range: ForRange) -> Vec<Value> {
    match range {
        ForRange::Signed { bits, start, end } => {
            if end <= start {
                return Vec::new();
            }
            let mut values = Vec::new();
            let mut cursor = start;
            while cursor < end {
                if values.len() > LOOP_LIMIT {
                    break;
                }
                values.push(Value::Int {
                    bits,
                    value: cursor,
                });
                cursor += 1;
            }
            values
        }
        ForRange::Unsigned { bits, start, end } => {
            if end <= start {
                return Vec::new();
            }
            let mut values = Vec::new();
            let mut cursor = start;
            while cursor < end {
                if values.len() > LOOP_LIMIT {
                    break;
                }
                values.push(Value::UInt {
                    bits,
                    value: cursor,
                });
                cursor += 1;
            }
            values
        }
    }
}

fn execute_parfor_body(
    loop_name: &str,
    body: &[Stmt],
    iterations: &[Value],
    ctx: &FunctionContext<'_>,
    span: Span,
) -> Result<Vec<LoweredStmt>, Diagnostic> {
    let available_threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    if available_threads <= 1 || iterations.len() < PARFOR_MIN_ITERATIONS {
        let mut lowered_out = Vec::new();
        for value in iterations {
            let mut local_env = ctx.env.clone();
            local_env.push();
            local_env.insert(
                loop_name.to_string(),
                Binding {
                    value: value.clone(),
                    ty: value_type(value),
                    mutable: false,
                    shared_borrows: 0,
                    mut_borrowed: false,
                },
            );

            let mut lowered = Vec::new();
            let mut local_exec = ExecState {
                lowered: &mut lowered,
            };
            let mut local_ctx = FunctionContext {
                env: &mut local_env,
                index: ctx.index,
                loop_depth: ctx.loop_depth + 1,
                call_depth: ctx.call_depth,
                call_stack: ctx.call_stack.clone(),
            };
            let flow = exec_stmts(body, &mut local_ctx, &mut local_exec)?;
            match flow {
                Flow::Next | Flow::ContinueLoop => {}
                Flow::BreakLoop => {
                    return Err(type_error("break is not allowed inside parfor body", span));
                }
                Flow::Exit => {
                    return Err(type_error("exit is not allowed inside parfor body", span));
                }
                Flow::Return(_) => {
                    return Err(type_error("return is not allowed inside parfor body", span));
                }
            }
            lowered_out.extend(lowered);
        }
        return Ok(lowered_out);
    }

    let total = iterations.len();
    let worker_count = available_threads.min(total);
    let chunk_size = total.div_ceil(worker_count);
    let mut ordered = vec![Vec::<LoweredStmt>::new(); total];
    let base_env = ctx.env.clone();

    let scoped_result: Result<(), Diagnostic> = thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk_start in (0..total).step_by(chunk_size) {
            let chunk_end = (chunk_start + chunk_size).min(total);
            let chunk_values: Vec<(usize, Value)> = iterations[chunk_start..chunk_end]
                .iter()
                .cloned()
                .enumerate()
                .map(|(local, value)| (chunk_start + local, value))
                .collect();
            let base_env = base_env.clone();
            let loop_name = loop_name.to_string();
            let body = body.to_vec();
            let index = ctx.index;
            let loop_depth = ctx.loop_depth + 1;
            handles.push(scope.spawn(
                move || -> Result<Vec<(usize, Vec<LoweredStmt>)>, Diagnostic> {
                    let mut chunk_out = Vec::with_capacity(chunk_values.len());
                    for (global_index, iter_value) in chunk_values {
                        let mut local_env = base_env.clone();
                        local_env.push();
                        local_env.insert(
                            loop_name.clone(),
                            Binding {
                                value: iter_value.clone(),
                                ty: value_type(&iter_value),
                                mutable: false,
                                shared_borrows: 0,
                                mut_borrowed: false,
                            },
                        );

                        let mut lowered = Vec::new();
                        let mut local_exec = ExecState {
                            lowered: &mut lowered,
                        };
                        let mut local_ctx = FunctionContext {
                            env: &mut local_env,
                            index,
                            loop_depth,
                            call_depth: ctx.call_depth,
                            call_stack: ctx.call_stack.clone(),
                        };
                        let flow = exec_stmts(&body, &mut local_ctx, &mut local_exec)?;
                        match flow {
                            Flow::Next | Flow::ContinueLoop => {}
                            Flow::BreakLoop => {
                                return Err(type_error(
                                    "break is not allowed inside parfor body",
                                    span,
                                ));
                            }
                            Flow::Exit => {
                                return Err(type_error(
                                    "exit is not allowed inside parfor body",
                                    span,
                                ));
                            }
                            Flow::Return(_) => {
                                return Err(type_error(
                                    "return is not allowed inside parfor body",
                                    span,
                                ));
                            }
                        }
                        chunk_out.push((global_index, lowered));
                    }
                    Ok(chunk_out)
                },
            ));
        }

        for handle in handles {
            let chunk = handle
                .join()
                .map_err(|_| type_error("parallel loop worker panicked", span))??;
            for (index, lowered) in chunk {
                ordered[index] = lowered;
            }
        }
        Ok(())
    });
    scoped_result?;

    let mut lowered_out = Vec::new();
    for lowered in ordered {
        lowered_out.extend(lowered);
    }
    Ok(lowered_out)
}

fn execute_parfor_reduction(
    loop_name: &str,
    reduction: &ParForReduction,
    iterations: &[Value],
    ctx: &mut FunctionContext<'_>,
    span: Span,
) -> Result<(), Diagnostic> {
    let (target_ty, target_mutable) = {
        let binding = ctx.env.get(&reduction.target).ok_or_else(|| {
            Diagnostic::at_span(
                format!("unknown identifier '{}'", reduction.target),
                reduction.span,
            )
        })?;
        (binding.ty.clone(), binding.mutable)
    };

    if !target_mutable {
        return Err(type_error(
            format!("reduction target '{}' must be mutable", reduction.target).as_str(),
            reduction.span,
        ));
    }
    ensure_reduction_type(reduction.op, &target_ty, reduction.span)?;

    if let Some(fast_value) =
        try_eval_reduction_fast_path(loop_name, reduction, &target_ty, iterations, span)?
    {
        let binding = ctx.env.get_mut(&reduction.target).ok_or_else(|| {
            Diagnostic::at_span(
                format!("unknown identifier '{}'", reduction.target),
                reduction.span,
            )
        })?;
        binding.value = fast_value;
        return Ok(());
    }

    let available_threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    let final_value = if available_threads <= 1 || iterations.len() < PARFOR_MIN_ITERATIONS {
        reduce_iterations_sequential(
            loop_name, reduction, &target_ty, iterations, ctx.index, ctx.env, span,
        )?
    } else {
        reduce_iterations_parallel(
            loop_name, reduction, &target_ty, iterations, ctx.index, ctx.env, span,
        )?
    };

    let binding = ctx.env.get_mut(&reduction.target).ok_or_else(|| {
        Diagnostic::at_span(
            format!("unknown identifier '{}'", reduction.target),
            reduction.span,
        )
    })?;
    binding.value = final_value;
    Ok(())
}

fn try_eval_reduction_fast_path(
    loop_name: &str,
    reduction: &ParForReduction,
    target_ty: &TypeName,
    iterations: &[Value],
    span: Span,
) -> Result<Option<Value>, Diagnostic> {
    let Expr::Ident { name, .. } = &reduction.expr else {
        return Ok(None);
    };
    if name != loop_name {
        return Ok(None);
    }

    let mut accumulator: Option<Value> = None;
    for value in iterations {
        let reduced = coerce_value(value.clone(), target_ty, span)?;
        accumulator = Some(match accumulator {
            Some(current) => combine_reduction(reduction.op, current, reduced, span)?,
            None => reduced,
        });
    }
    let reduced =
        accumulator.ok_or_else(|| type_error("reduction requires at least one iteration", span))?;
    Ok(Some(reduced))
}

fn reduce_iterations_sequential(
    loop_name: &str,
    reduction: &ParForReduction,
    target_ty: &TypeName,
    iterations: &[Value],
    index: &ProgramIndex,
    env: &Env,
    span: Span,
) -> Result<Value, Diagnostic> {
    let mut accumulator: Option<Value> = None;
    for value in iterations {
        let reduced_value =
            eval_reduction_value(loop_name, reduction, target_ty, value, index, env, span)?;
        accumulator = Some(match accumulator {
            Some(current) => combine_reduction(reduction.op, current, reduced_value, span)?,
            None => reduced_value,
        });
    }
    accumulator.ok_or_else(|| type_error("reduction requires at least one iteration", span))
}

fn reduce_iterations_parallel(
    loop_name: &str,
    reduction: &ParForReduction,
    target_ty: &TypeName,
    iterations: &[Value],
    index: &ProgramIndex,
    env: &Env,
    span: Span,
) -> Result<Value, Diagnostic> {
    let total = iterations.len();
    let worker_count = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(total);
    let chunk_size = total.div_ceil(worker_count);
    let base_env = env.clone();
    let mut partials: Vec<Option<Value>> = vec![None; worker_count];

    let scoped_result: Result<(), Diagnostic> = thread::scope(|scope| {
        let mut handles = Vec::new();
        for (worker_index, chunk_start) in (0..total).step_by(chunk_size).enumerate() {
            let chunk_end = (chunk_start + chunk_size).min(total);
            let chunk_values = iterations[chunk_start..chunk_end].to_vec();
            let base_env = base_env.clone();
            let loop_name = loop_name.to_string();
            let reduction = reduction.clone();
            let target_ty = target_ty.clone();
            handles.push(
                scope.spawn(move || -> Result<(usize, Option<Value>), Diagnostic> {
                    let mut accumulator: Option<Value> = None;
                    for value in &chunk_values {
                        let reduced_value = eval_reduction_value(
                            &loop_name, &reduction, &target_ty, value, index, &base_env, span,
                        )?;
                        accumulator = Some(match accumulator {
                            Some(current) => {
                                combine_reduction(reduction.op, current, reduced_value, span)?
                            }
                            None => reduced_value,
                        });
                    }
                    Ok((worker_index, accumulator))
                }),
            );
        }

        for handle in handles {
            let (worker_index, partial) = handle
                .join()
                .map_err(|_| type_error("parallel reduction worker panicked", span))??;
            partials[worker_index] = partial;
        }
        Ok(())
    });
    scoped_result?;

    let mut accumulator: Option<Value> = None;
    for partial in partials.into_iter().flatten() {
        accumulator = Some(match accumulator {
            Some(current) => combine_reduction(reduction.op, current, partial, span)?,
            None => partial,
        });
    }
    let reduced =
        accumulator.ok_or_else(|| type_error("reduction requires at least one iteration", span))?;

    Ok(reduced)
}

fn eval_reduction_value(
    loop_name: &str,
    reduction: &ParForReduction,
    target_ty: &TypeName,
    iter_value: &Value,
    index: &ProgramIndex,
    base_env: &Env,
    span: Span,
) -> Result<Value, Diagnostic> {
    let mut local_env = base_env.clone();
    local_env.push();
    local_env.insert(
        loop_name.to_string(),
        Binding {
            value: iter_value.clone(),
            ty: value_type(iter_value),
            mutable: false,
            shared_borrows: 0,
            mut_borrowed: false,
        },
    );
    let value = eval_expr(&reduction.expr, &mut local_env, index)?;
    coerce_value(value, target_ty, span)
}

fn ensure_reduction_type(op: ReductionOp, ty: &TypeName, span: Span) -> Result<(), Diagnostic> {
    let is_integer = matches!(ty, TypeName::Int { .. } | TypeName::Byte);
    if !is_integer {
        return Err(type_error(
            "parfor reductions require integer targets for deterministic associativity",
            span,
        ));
    }
    match op {
        ReductionOp::Sum | ReductionOp::Min | ReductionOp::Max => Ok(()),
    }
}

fn combine_reduction(
    op: ReductionOp,
    left: Value,
    right: Value,
    span: Span,
) -> Result<Value, Diagnostic> {
    match op {
        ReductionOp::Sum => add_values(left, right, span),
        ReductionOp::Min => {
            let cmp = cmp_values(BinaryOp::Le, left.clone(), right.clone(), span)?;
            match cmp {
                Value::Bool(true) => Ok(left),
                Value::Bool(false) => Ok(right),
                _ => Err(type_error("invalid min reduction comparison", span)),
            }
        }
        ReductionOp::Max => {
            let cmp = cmp_values(BinaryOp::Ge, left.clone(), right.clone(), span)?;
            match cmp {
                Value::Bool(true) => Ok(left),
                Value::Bool(false) => Ok(right),
                _ => Err(type_error("invalid max reduction comparison", span)),
            }
        }
    }
}

fn type_method_mangled_prefix(ty: &TypeName) -> String {
    match ty {
        TypeName::Struct(name) => name.clone(),
        other => other
            .display()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect(),
    }
}

fn method_candidate_function_names(receiver_ty: &TypeName, method: &str) -> [String; 2] {
    [
        method.to_string(),
        format!("{}__{}", type_method_mangled_prefix(receiver_ty), method),
    ]
}

fn try_exec_user_method_stmt(
    receiver_name: &str,
    method_name: &str,
    args: &[Value],
    span: Span,
    ctx: &mut FunctionContext<'_>,
    exec: &mut ExecState<'_>,
) -> Result<Option<Flow>, Diagnostic> {
    let (receiver_ty, receiver_mutable) = {
        let binding = ctx.env.get(receiver_name).ok_or_else(|| {
            Diagnostic::at_span(format!("unknown identifier '{receiver_name}'"), span)
        })?;
        (binding.ty.clone(), binding.mutable)
    };
    let candidates = method_candidate_function_names(&receiver_ty, method_name);
    for candidate in &candidates {
        let Some(func) = ctx.index.functions.get(candidate) else {
            continue;
        };
        if func.params.len() != args.len() + 1 {
            continue;
        }
        let TypeName::Ref { mutable, inner } = &func.params[0].ty else {
            continue;
        };
        if inner.as_ref() != &receiver_ty {
            continue;
        }
        if *mutable && !receiver_mutable {
            return Err(type_error(
                format!(
                    "cannot call mut method '{}' on immutable '{}'",
                    method_name, receiver_name
                )
                .as_str(),
                span,
            ));
        }
        let mut call_args = Vec::with_capacity(args.len() + 1);
        let receiver_expr = Expr::Ident {
            name: receiver_name.to_string(),
            span,
        };
        call_args.push(borrow_value(&receiver_expr, ctx.env, *mutable, span)?);
        call_args.extend_from_slice(args);
        match exec_function(func, &call_args, ctx, exec)? {
            CallResult::Exit => return Ok(Some(Flow::Exit)),
            CallResult::Continue(_) => return Ok(Some(Flow::Next)),
        }
    }
    Ok(None)
}

fn try_eval_user_method_expr(
    receiver_name: &str,
    method_name: &str,
    args: &[Expr],
    span: Span,
    env: &mut Env,
    index: &ProgramIndex,
) -> Result<Option<Value>, Diagnostic> {
    let receiver_binding = env.get(receiver_name).ok_or_else(|| {
        Diagnostic::at_span(format!("unknown identifier '{receiver_name}'"), span)
    })?;
    let receiver_ty = receiver_binding.ty.clone();
    let candidates = method_candidate_function_names(&receiver_ty, method_name);
    for candidate in &candidates {
        let Some(func) = index.functions.get(candidate) else {
            continue;
        };
        if !index
            .pure_functions
            .get(&func.name)
            .copied()
            .unwrap_or(false)
        {
            return Err(type_error(
                format!(
                    "expression call requires pure function; '{}' is impure",
                    source_callable_name(&func.name)
                )
                .as_str(),
                span,
            ));
        }
        if func.params.len() != args.len() + 1 {
            continue;
        }
        let TypeName::Ref {
            mutable: false,
            inner,
        } = &func.params[0].ty
        else {
            continue;
        };
        if inner.as_ref() != &receiver_ty {
            continue;
        }
        let mut call_env = env.clone();
        let mut lowered = Vec::new();
        let mut local_exec = ExecState {
            lowered: &mut lowered,
        };
        let mut local_ctx = FunctionContext {
            env: &mut call_env,
            index,
            loop_depth: 0,
            call_depth: 0,
            call_stack: Vec::new(),
        };
        let receiver_expr = Expr::Ident {
            name: receiver_name.to_string(),
            span,
        };
        let mut call_args = Vec::with_capacity(args.len() + 1);
        call_args.push(borrow_value(&receiver_expr, local_ctx.env, false, span)?);
        for arg in args {
            call_args.push(eval_expr(arg, env, index)?);
        }
        return match exec_function(func, &call_args, &mut local_ctx, &mut local_exec)? {
            CallResult::Exit => Err(type_error(
                "cannot use function call with exit() as expression",
                span,
            )),
            CallResult::Continue(None) => Err(type_error(
                "expression call requires function with a return value",
                span,
            )),
            CallResult::Continue(Some(value)) => Ok(Some(value)),
        };
    }
    Ok(None)
}

fn try_eval_user_method_value(
    receiver: &Value,
    method_name: &str,
    args: &[Expr],
    span: Span,
    env: &mut Env,
    index: &ProgramIndex,
) -> Result<Option<Value>, Diagnostic> {
    let receiver_ty = value_type(receiver);
    let candidates = method_candidate_function_names(&receiver_ty, method_name);
    for candidate in &candidates {
        let Some(func) = index.functions.get(candidate) else {
            continue;
        };
        if !index
            .pure_functions
            .get(&func.name)
            .copied()
            .unwrap_or(false)
        {
            return Err(type_error(
                format!(
                    "expression call requires pure function; '{}' is impure",
                    source_callable_name(&func.name)
                )
                .as_str(),
                span,
            ));
        }
        if func.params.len() != args.len() + 1 {
            continue;
        }
        let TypeName::Ref {
            mutable: false,
            inner,
        } = &func.params[0].ty
        else {
            continue;
        };
        if inner.as_ref() != &receiver_ty {
            continue;
        }

        let mut evaluated_args = Vec::with_capacity(args.len());
        for arg in args {
            evaluated_args.push(eval_expr(arg, env, index)?);
        }
        let mut call_env = env.clone();
        call_env.push();
        let receiver_name = "__aziky_temporary_receiver";
        call_env.insert(
            receiver_name.to_string(),
            Binding {
                value: receiver.clone(),
                ty: receiver_ty,
                mutable: false,
                shared_borrows: 0,
                mut_borrowed: false,
            },
        );
        let mut lowered = Vec::new();
        let mut local_exec = ExecState {
            lowered: &mut lowered,
        };
        let mut local_ctx = FunctionContext {
            env: &mut call_env,
            index,
            loop_depth: 0,
            call_depth: 0,
            call_stack: Vec::new(),
        };
        let receiver_expr = Expr::Ident {
            name: receiver_name.to_string(),
            span,
        };
        let mut call_args = Vec::with_capacity(args.len() + 1);
        call_args.push(borrow_value(&receiver_expr, local_ctx.env, false, span)?);
        call_args.extend(evaluated_args);
        return match exec_function(func, &call_args, &mut local_ctx, &mut local_exec)? {
            CallResult::Exit => Err(type_error(
                "cannot use function call with exit() as expression",
                span,
            )),
            CallResult::Continue(None) => Err(type_error(
                "expression call requires function with a return value",
                span,
            )),
            CallResult::Continue(Some(value)) => Ok(Some(value)),
        };
    }
    Ok(None)
}

fn call_sort_compare_function(
    compare_name: &str,
    left: &Value,
    right: &Value,
    index: &ProgramIndex,
    env_snapshot: &Env,
    span: Span,
) -> Result<bool, Diagnostic> {
    let is_pure = index
        .pure_functions
        .get(compare_name)
        .copied()
        .unwrap_or(false);
    if !is_pure {
        return Err(type_error(
            format!(
                "sort comparator '{}' must be a pure function returning bool",
                compare_name
            )
            .as_str(),
            span,
        ));
    }
    let func = index.functions.get(compare_name).ok_or_else(|| {
        Diagnostic::at_span(
            format!("unknown comparator function '{compare_name}'"),
            span,
        )
    })?;
    let args = vec![left.clone(), right.clone()];
    let mut call_env = env_snapshot.clone();
    let mut lowered = Vec::new();
    let mut local_exec = ExecState {
        lowered: &mut lowered,
    };
    let mut local_ctx = FunctionContext {
        env: &mut call_env,
        index,
        loop_depth: 0,
        call_depth: 0,
        call_stack: Vec::new(),
    };
    match exec_function(func, &args, &mut local_ctx, &mut local_exec)? {
        CallResult::Exit => Err(type_error(
            "sort comparator function cannot call exit()",
            span,
        )),
        CallResult::Continue(None) => Err(type_error(
            "sort comparator function must return bool",
            span,
        )),
        CallResult::Continue(Some(Value::Bool(v))) => Ok(v),
        CallResult::Continue(Some(_)) => Err(type_error(
            "sort comparator function must return bool",
            span,
        )),
    }
}

const SORT_NETWORK_8: [(usize, usize); 19] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (1, 2),
    (5, 6),
    (0, 4),
    (3, 7),
    (1, 5),
    (2, 6),
    (1, 4),
    (3, 6),
    (2, 4),
    (3, 5),
    (3, 4),
];

const SORT_NETWORK_9: [(usize, usize); 27] = [
    (0, 1),
    (2, 3),
    (0, 2),
    (1, 3),
    (1, 2),
    (4, 5),
    (7, 8),
    (6, 8),
    (6, 7),
    (4, 7),
    (4, 6),
    (5, 8),
    (5, 7),
    (5, 6),
    (0, 5),
    (0, 4),
    (1, 6),
    (1, 5),
    (1, 4),
    (2, 7),
    (3, 8),
    (3, 7),
    (2, 5),
    (2, 4),
    (3, 6),
    (3, 5),
    (3, 4),
];

const SORT_NETWORK_10: [(usize, usize); 32] = [
    (0, 1),
    (3, 4),
    (2, 4),
    (2, 3),
    (0, 3),
    (0, 2),
    (1, 4),
    (1, 3),
    (1, 2),
    (5, 6),
    (8, 9),
    (7, 9),
    (7, 8),
    (5, 8),
    (5, 7),
    (6, 9),
    (6, 8),
    (6, 7),
    (0, 5),
    (1, 6),
    (1, 5),
    (2, 7),
    (2, 6),
    (2, 5),
    (3, 8),
    (4, 9),
    (4, 8),
    (3, 6),
    (3, 5),
    (4, 7),
    (4, 6),
    (4, 5),
];

const SORT_NETWORK_11: [(usize, usize); 37] = [
    (0, 1),
    (3, 4),
    (2, 4),
    (2, 3),
    (0, 3),
    (0, 2),
    (1, 4),
    (1, 3),
    (1, 2),
    (6, 7),
    (5, 7),
    (5, 6),
    (9, 10),
    (8, 10),
    (8, 9),
    (5, 8),
    (6, 9),
    (6, 8),
    (7, 10),
    (7, 9),
    (7, 8),
    (0, 5),
    (1, 6),
    (1, 5),
    (2, 7),
    (2, 6),
    (2, 5),
    (3, 9),
    (3, 8),
    (4, 10),
    (4, 9),
    (4, 8),
    (3, 6),
    (3, 5),
    (4, 7),
    (4, 6),
    (4, 5),
];

const SORT_NETWORK_12: [(usize, usize); 42] = [
    (1, 2),
    (0, 2),
    (0, 1),
    (4, 5),
    (3, 5),
    (3, 4),
    (0, 3),
    (1, 4),
    (1, 3),
    (2, 5),
    (2, 4),
    (2, 3),
    (7, 8),
    (6, 8),
    (6, 7),
    (10, 11),
    (9, 11),
    (9, 10),
    (6, 9),
    (7, 10),
    (7, 9),
    (8, 11),
    (8, 10),
    (8, 9),
    (0, 6),
    (1, 7),
    (1, 6),
    (2, 8),
    (2, 7),
    (2, 6),
    (3, 9),
    (4, 10),
    (4, 9),
    (5, 11),
    (5, 10),
    (5, 9),
    (3, 6),
    (4, 7),
    (4, 6),
    (5, 8),
    (5, 7),
    (5, 6),
];

const SORT_NETWORK_13: [(usize, usize); 47] = [
    (1, 2),
    (0, 2),
    (0, 1),
    (4, 5),
    (3, 5),
    (3, 4),
    (0, 3),
    (1, 4),
    (1, 3),
    (2, 5),
    (2, 4),
    (2, 3),
    (7, 8),
    (6, 8),
    (6, 7),
    (9, 10),
    (11, 12),
    (9, 11),
    (10, 12),
    (10, 11),
    (6, 9),
    (7, 10),
    (7, 9),
    (8, 12),
    (8, 11),
    (8, 10),
    (8, 9),
    (0, 6),
    (1, 7),
    (1, 6),
    (2, 9),
    (2, 8),
    (2, 7),
    (2, 6),
    (3, 10),
    (4, 11),
    (4, 10),
    (5, 12),
    (5, 11),
    (5, 10),
    (3, 6),
    (4, 7),
    (4, 6),
    (5, 9),
    (5, 8),
    (5, 7),
    (5, 6),
];

const SORT_NETWORK_14: [(usize, usize); 52] = [
    (1, 2),
    (0, 2),
    (0, 1),
    (3, 4),
    (5, 6),
    (3, 5),
    (4, 6),
    (4, 5),
    (0, 3),
    (1, 4),
    (1, 3),
    (2, 6),
    (2, 5),
    (2, 4),
    (2, 3),
    (8, 9),
    (7, 9),
    (7, 8),
    (10, 11),
    (12, 13),
    (10, 12),
    (11, 13),
    (11, 12),
    (7, 10),
    (8, 11),
    (8, 10),
    (9, 13),
    (9, 12),
    (9, 11),
    (9, 10),
    (0, 7),
    (1, 8),
    (1, 7),
    (2, 9),
    (3, 10),
    (3, 9),
    (2, 7),
    (3, 8),
    (3, 7),
    (4, 11),
    (5, 12),
    (5, 11),
    (6, 13),
    (6, 12),
    (6, 11),
    (4, 7),
    (5, 8),
    (5, 7),
    (6, 10),
    (6, 9),
    (6, 8),
    (6, 7),
];

const SORT_NETWORK_15: [(usize, usize); 57] = [
    (1, 2),
    (0, 2),
    (0, 1),
    (3, 4),
    (5, 6),
    (3, 5),
    (4, 6),
    (4, 5),
    (0, 3),
    (1, 4),
    (1, 3),
    (2, 6),
    (2, 5),
    (2, 4),
    (2, 3),
    (7, 8),
    (9, 10),
    (7, 9),
    (8, 10),
    (8, 9),
    (11, 12),
    (13, 14),
    (11, 13),
    (12, 14),
    (12, 13),
    (7, 11),
    (8, 12),
    (8, 11),
    (9, 13),
    (10, 14),
    (10, 13),
    (9, 11),
    (10, 12),
    (10, 11),
    (0, 7),
    (1, 8),
    (1, 7),
    (2, 9),
    (3, 10),
    (3, 9),
    (2, 7),
    (3, 8),
    (3, 7),
    (4, 11),
    (5, 12),
    (5, 11),
    (6, 14),
    (6, 13),
    (6, 12),
    (6, 11),
    (4, 7),
    (5, 8),
    (5, 7),
    (6, 10),
    (6, 9),
    (6, 8),
    (6, 7),
];

const SORT_NETWORK_16: [(usize, usize); 65] = [
    (0, 1),
    (2, 3),
    (0, 2),
    (1, 3),
    (1, 2),
    (4, 5),
    (6, 7),
    (4, 6),
    (5, 7),
    (5, 6),
    (0, 4),
    (1, 5),
    (1, 4),
    (2, 6),
    (3, 7),
    (3, 6),
    (2, 4),
    (3, 5),
    (3, 4),
    (8, 9),
    (10, 11),
    (8, 10),
    (9, 11),
    (9, 10),
    (12, 13),
    (14, 15),
    (12, 14),
    (13, 15),
    (13, 14),
    (8, 12),
    (9, 13),
    (9, 12),
    (10, 14),
    (11, 15),
    (11, 14),
    (10, 12),
    (11, 13),
    (11, 12),
    (0, 8),
    (1, 9),
    (1, 8),
    (2, 10),
    (3, 11),
    (3, 10),
    (2, 8),
    (3, 9),
    (3, 8),
    (4, 12),
    (5, 13),
    (5, 12),
    (6, 14),
    (7, 15),
    (7, 14),
    (6, 12),
    (7, 13),
    (7, 12),
    (4, 8),
    (5, 9),
    (5, 8),
    (6, 10),
    (7, 11),
    (7, 10),
    (6, 8),
    (7, 9),
    (7, 8),
];

mod value_sort;
use value_sort::*;

fn eval_expr(expr: &Expr, env: &mut Env, index: &ProgramIndex) -> Result<Value, Diagnostic> {
    match expr {
        Expr::Bool { value, .. } => Ok(Value::Bool(*value)),
        Expr::String { value, .. } => Ok(Value::Str(value.clone())),
        Expr::Char { value, .. } => Ok(Value::Char(*value)),
        Expr::Number { literal, span } => parse_number_literal(literal, *span),
        Expr::Ident { name, span } => {
            let binding = env
                .get(name)
                .ok_or_else(|| Diagnostic::at_span(format!("unknown identifier '{name}'"), span))?;
            if binding.mut_borrowed {
                return Err(type_error(
                    format!("cannot read '{name}' while mutably borrowed").as_str(),
                    *span,
                ));
            }
            Ok(binding.value.clone())
        }
        Expr::Call { name, span, args } => {
            if name == "runtime_seed" {
                let _ = args;
                return Err(type_error(
                    "runtime_seed() is runtime-only and must be lowered through runtime generic path",
                    *span,
                ));
            }
            if name == "heap_alloc" || name == "heap_free" || is_file_runtime_intrinsic(name) {
                let _ = args;
                return Err(type_error(
                    "resource intrinsics are runtime-only and must be lowered through runtime generic path",
                    *span,
                ));
            }
            if name == "runtime_bloom_sbbf_insert"
                || name == "runtime_bloom_sbbf_maybe"
                || name == "runtime_hash_probe_grouped16"
                || name == "runtime_join_select_adaptive"
            {
                let _ = args;
                return Err(type_error(
                    "runtime intrinsic is runtime-only and must be lowered through runtime generic path",
                    *span,
                ));
            }
            let func = index
                .functions
                .get(name)
                .ok_or_else(|| unknown_function_diagnostic(name, *span))?;
            let is_pure = index.pure_functions.get(name).copied().unwrap_or(false);
            if !is_pure {
                return Err(type_error(
                    format!(
                        "expression call requires pure function; '{}' is impure",
                        source_callable_name(name)
                    )
                    .as_str(),
                    *span,
                ));
            }
            let arg_values: Vec<Value> = args
                .iter()
                .map(|arg| eval_expr(arg, env, index))
                .collect::<Result<Vec<_>, _>>()?;

            let mut call_env = env.clone();
            let mut lowered = Vec::new();
            let mut local_exec = ExecState {
                lowered: &mut lowered,
            };
            let mut local_ctx = FunctionContext {
                env: &mut call_env,
                index,
                loop_depth: 0,
                call_depth: 0,
                call_stack: Vec::new(),
            };
            match exec_function(func, &arg_values, &mut local_ctx, &mut local_exec)? {
                CallResult::Exit => Err(type_error(
                    "cannot use function call with exit() as expression",
                    *span,
                )),
                CallResult::Continue(None) => Err(type_error(
                    "expression call requires function with a return value",
                    *span,
                )),
                CallResult::Continue(Some(value)) => Ok(value),
            }
        }
        Expr::QualifiedCall { span, .. } => Err(type_error(
            "internal error: unresolved qualified call",
            *span,
        )),
        Expr::Unary { op, expr, span } => {
            let value = eval_expr(expr, env, index)?;
            match op {
                UnaryOp::Plus => Ok(value),
                UnaryOp::Neg => neg_value(value, *span),
                UnaryOp::Not => not_value(value, *span),
                UnaryOp::Ref => borrow_value(expr, env, false, *span),
                UnaryOp::RefMut => borrow_value(expr, env, true, *span),
            }
        }
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => {
            if *op == BinaryOp::And {
                let left_value = eval_expr(left, env, index)?;
                let left_bool = match left_value {
                    Value::Bool(v) => v,
                    _ => return Err(type_error("logical and expects bool operands", *span)),
                };
                if !left_bool {
                    return Ok(Value::Bool(false));
                }
                let right_value = eval_expr(right, env, index)?;
                return match right_value {
                    Value::Bool(v) => Ok(Value::Bool(v)),
                    _ => Err(type_error("logical and expects bool operands", *span)),
                };
            }

            if *op == BinaryOp::Or {
                let left_value = eval_expr(left, env, index)?;
                let left_bool = match left_value {
                    Value::Bool(v) => v,
                    _ => return Err(type_error("logical or expects bool operands", *span)),
                };
                if left_bool {
                    return Ok(Value::Bool(true));
                }
                let right_value = eval_expr(right, env, index)?;
                return match right_value {
                    Value::Bool(v) => Ok(Value::Bool(v)),
                    _ => Err(type_error("logical or expects bool operands", *span)),
                };
            }

            let left = eval_expr(left, env, index)?;
            let right = eval_expr(right, env, index)?;
            match op {
                BinaryOp::Add => add_values(left, right, *span),
                BinaryOp::Sub => sub_values(left, right, *span),
                BinaryOp::Mul => mul_values(left, right, *span),
                BinaryOp::Div => div_values(left, right, *span),
                BinaryOp::Mod => mod_values(left, right, *span),
                BinaryOp::BitAnd => bitand_values(left, right, *span),
                BinaryOp::BitOr => bitor_values(left, right, *span),
                BinaryOp::BitXor => bitxor_values(left, right, *span),
                BinaryOp::Shl => shl_values(left, right, *span),
                BinaryOp::Shr => shr_values(left, right, *span),
                BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge => cmp_values(*op, left, right, *span),
                BinaryOp::And | BinaryOp::Or => unreachable!(),
            }
        }
        Expr::FieldAccess { base, field, span } => {
            let base_val = eval_expr(base, env, index)?;
            let base_val = resolve_receiver_value(base_val, env, *span)?;
            match base_val {
                Value::Struct { fields, .. } => fields
                    .get(field)
                    .cloned()
                    .ok_or_else(|| Diagnostic::at_span(format!("unknown field '{field}'"), span)),
                _ => Err(type_error("field access on non-struct", *span)),
            }
        }
        Expr::Index {
            base,
            index: idx,
            span,
        } => {
            let base_val = eval_expr(base, env, index)?;
            let idx_val = eval_expr(idx, env, index)?;
            match base_val {
                Value::Array { elems, .. } | Value::List { elems, .. } => {
                    let index = match idx_val {
                        Value::UInt { value, .. } => value as usize,
                        Value::Int { value, .. } => {
                            if value < 0 {
                                return Err(type_error("index must be non-negative", *span));
                            }
                            value as usize
                        }
                        _ => return Err(type_error("index must be integer", *span)),
                    };
                    elems
                        .get(index)
                        .cloned()
                        .ok_or_else(|| type_error("index out of bounds", *span))
                }
                Value::Dict {
                    key_type, entries, ..
                }
                | Value::Map {
                    key_type, entries, ..
                } => {
                    let key = dict_key_from_typed_value(idx_val, &key_type, *span)?;
                    entries
                        .get(&key)
                        .cloned()
                        .ok_or_else(|| type_error("unknown dictionary key", *span))
                }
                _ => Err(type_error(
                    "indexing requires array, list, dictionary, or map",
                    *span,
                )),
            }
        }
        Expr::ArrayLit { elems, span } => {
            if elems.is_empty() {
                return Ok(Value::Array {
                    elem_type: TypeName::Struct("_".to_string()),
                    elems: Vec::new(),
                });
            }
            let mut values = Vec::new();
            for elem in elems {
                values.push(eval_expr(elem, env, index)?);
            }
            let elem_type = value_type(&values[0]);
            for value in &values {
                let target = value_type(value);
                if target != elem_type {
                    return Err(type_error("array elements must have same type", *span));
                }
            }
            Ok(Value::Array {
                elem_type,
                elems: values,
            })
        }
        Expr::StructInit { name, fields, span } => {
            let _def = index
                .structs
                .get(name)
                .ok_or_else(|| Diagnostic::at_span(format!("unknown struct '{name}'"), span))?;
            let layout = index.struct_layouts.get(name).ok_or_else(|| {
                Diagnostic::at_span(format!("missing layout for struct '{name}'"), span)
            })?;
            let mut seen: HashMap<String, Value> = HashMap::new();
            for StructInitField { name, expr, span } in fields {
                if seen.contains_key(name) {
                    return Err(Diagnostic::at_span(
                        format!("duplicate field '{name}'"),
                        span,
                    ));
                }
                let value = eval_expr(expr, env, index)?;
                let field_ty = layout
                    .iter()
                    .find(|f| f.name == *name)
                    .map(|f| f.ty.clone())
                    .ok_or_else(|| Diagnostic::at_span(format!("unknown field '{name}'"), span))?;
                let coerced = coerce_value(value, &field_ty, *span)?;
                seen.insert(name.clone(), coerced);
            }
            for field in layout {
                if !seen.contains_key(&field.name) {
                    return Err(Diagnostic::at_span(
                        format!("missing field '{}'", field.name),
                        span,
                    ));
                }
            }
            Ok(Value::Struct {
                name: name.clone(),
                fields: seen,
            })
        }
        Expr::EnumVariant {
            enum_name,
            variant,
            span,
        } => {
            let def = index
                .enums
                .get(enum_name)
                .ok_or_else(|| Diagnostic::at_span(format!("unknown enum '{enum_name}'"), span))?;
            let variant_def = def
                .variants
                .iter()
                .find(|item| item.name == *variant)
                .ok_or_else(|| {
                    Diagnostic::at_span(
                        format!("unknown variant '{variant}' for enum '{enum_name}'"),
                        span,
                    )
                })?;
            if !matches!(variant_def.payload, EnumVariantPayloadDef::Unit) {
                return Err(type_error(
                    format!(
                        "variant '{enum_name}::{variant}' requires a payload; use its declared constructor form"
                    )
                    .as_str(),
                    *span,
                ));
            }
            Ok(Value::Enum {
                name: enum_name.clone(),
                variant: variant.clone(),
                type_args: vec![None; def.type_params.len()],
                payload: EnumPayloadValue::Unit,
            })
        }
        Expr::EnumTupleVariant {
            enum_name,
            variant,
            args,
            span,
        } => {
            let def = index
                .enums
                .get(enum_name)
                .ok_or_else(|| Diagnostic::at_span(format!("unknown enum '{enum_name}'"), span))?;
            let variant_def = def
                .variants
                .iter()
                .find(|item| item.name == *variant)
                .ok_or_else(|| {
                    Diagnostic::at_span(
                        format!("unknown variant '{variant}' for enum '{enum_name}'"),
                        span,
                    )
                })?;
            let EnumVariantPayloadDef::Tuple(field_defs) = &variant_def.payload else {
                return Err(type_error(
                    format!("variant '{enum_name}::{variant}' is not a tuple variant").as_str(),
                    *span,
                ));
            };
            if args.len() != field_defs.len() {
                return Err(type_error(
                    format!(
                        "variant '{enum_name}::{variant}' expects {} {}, got {}",
                        field_defs.len(),
                        argument_noun(field_defs.len()),
                        args.len()
                    )
                    .as_str(),
                    *span,
                ));
            }
            let mut raw_values = Vec::with_capacity(args.len());
            for arg in args {
                let value = eval_expr(arg, env, index)?;
                raw_values.push(value);
            }
            let mut inferred = HashMap::new();
            for (value, field_def) in raw_values.iter().zip(field_defs.iter()) {
                infer_generic_type_bindings(
                    &field_def.ty,
                    &value_type(value),
                    &def.type_params,
                    &mut inferred,
                    *span,
                )?;
            }
            let type_args = enum_type_args(def, &inferred);
            let mut values = Vec::with_capacity(raw_values.len());
            for ((value, field_def), arg) in raw_values
                .into_iter()
                .zip(field_defs.iter())
                .zip(args.iter())
            {
                let target = instantiate_generic_type(&field_def.ty, &inferred);
                values.push(coerce_value(value, &target, arg.span())?);
            }
            Ok(Value::Enum {
                name: enum_name.clone(),
                variant: variant.clone(),
                type_args,
                payload: EnumPayloadValue::Tuple(values),
            })
        }
        Expr::EnumStructVariant {
            enum_name,
            variant,
            fields,
            span,
        } => {
            let def = index
                .enums
                .get(enum_name)
                .ok_or_else(|| Diagnostic::at_span(format!("unknown enum '{enum_name}'"), span))?;
            let variant_def = def
                .variants
                .iter()
                .find(|item| item.name == *variant)
                .ok_or_else(|| {
                    Diagnostic::at_span(
                        format!("unknown variant '{variant}' for enum '{enum_name}'"),
                        span,
                    )
                })?;
            let EnumVariantPayloadDef::Named(field_defs) = &variant_def.payload else {
                return Err(type_error(
                    format!("variant '{enum_name}::{variant}' is not a named variant").as_str(),
                    *span,
                ));
            };
            let mut raw_values = HashMap::new();
            for field in fields {
                if raw_values.contains_key(&field.name) {
                    return Err(Diagnostic::at_span(
                        format!(
                            "duplicate payload field '{}' for variant '{}::{}'",
                            field.name, enum_name, variant
                        ),
                        field.span,
                    ));
                }
                let _field_def = field_defs
                    .iter()
                    .find(|candidate| candidate.name == field.name)
                    .ok_or_else(|| {
                        Diagnostic::at_span(
                            format!(
                                "unknown payload field '{}' for variant '{}::{}'",
                                field.name, enum_name, variant
                            ),
                            field.span,
                        )
                    })?;
                let value = eval_expr(&field.expr, env, index)?;
                raw_values.insert(field.name.clone(), value);
            }
            for field_def in field_defs {
                if !raw_values.contains_key(&field_def.name) {
                    return Err(Diagnostic::at_span(
                        format!(
                            "missing payload field '{}' for variant '{}::{}'",
                            field_def.name, enum_name, variant
                        ),
                        span,
                    ));
                }
            }
            let mut inferred = HashMap::new();
            for field_def in field_defs {
                let value = raw_values
                    .get(&field_def.name)
                    .expect("validated enum payload field should exist");
                infer_generic_type_bindings(
                    &field_def.ty,
                    &value_type(value),
                    &def.type_params,
                    &mut inferred,
                    *span,
                )?;
            }
            let type_args = enum_type_args(def, &inferred);
            let mut values = HashMap::new();
            for field_def in field_defs {
                let value = raw_values
                    .remove(&field_def.name)
                    .expect("validated enum payload field should exist");
                let target = instantiate_generic_type(&field_def.ty, &inferred);
                values.insert(field_def.name.clone(), coerce_value(value, &target, *span)?);
            }
            Ok(Value::Enum {
                name: enum_name.clone(),
                variant: variant.clone(),
                type_args,
                payload: EnumPayloadValue::Named(values),
            })
        }
        Expr::Match { value, arms, span } => eval_match_expr(value, arms, env, index, *span),
        Expr::DictLit { entries, .. } => {
            if entries.is_empty() {
                return Ok(Value::Dict {
                    key_type: TypeName::String,
                    value_type: TypeName::String,
                    entries: BTreeMap::new(),
                });
            }
            let mut out = BTreeMap::new();
            let mut inferred_value_type = None;
            for DictEntry { key, value, span } in entries {
                let norm_key =
                    dict_key_from_typed_value(Value::Str(key.clone()), &TypeName::String, *span)?;
                if out.contains_key(&norm_key) {
                    return Err(Diagnostic::at_span(
                        format!("duplicate dictionary key '{key}'"),
                        span,
                    ));
                }
                let evaluated = eval_expr(value, env, index)?;
                if let Some(target) = &inferred_value_type {
                    if value_type(&evaluated) != *target {
                        return Err(type_error("dictionary values must share one type", *span));
                    }
                } else {
                    inferred_value_type = Some(value_type(&evaluated));
                }
                out.insert(norm_key, evaluated);
            }
            Ok(Value::Dict {
                key_type: TypeName::String,
                value_type: inferred_value_type.expect("dict literal type inferred"),
                entries: out,
            })
        }
        Expr::MethodCall {
            receiver,
            name,
            args,
            span,
        } => {
            if let Expr::Ident {
                name: receiver_name,
                ..
            } = receiver.as_ref()
            {
                if let Some(value) =
                    try_eval_user_method_expr(receiver_name, name, args, *span, env, index)?
                {
                    return Ok(value);
                }
            }
            let receiver = eval_expr(receiver, env, index)?;
            if let Some(value) =
                try_eval_user_method_value(&receiver, name, args, *span, env, index)?
            {
                return Ok(value);
            }
            let arg_values = args
                .iter()
                .map(|arg| eval_expr(arg, env, index))
                .collect::<Result<Vec<_>, _>>()?;
            apply_method_call(receiver, name, &arg_values, *span, env)
        }
    }
}

fn infer_generic_type_bindings(
    template: &TypeName,
    actual: &TypeName,
    type_params: &[String],
    inferred: &mut HashMap<String, TypeName>,
    span: Span,
) -> Result<(), Diagnostic> {
    if let TypeName::Struct(name) = template {
        if type_params.iter().any(|param| param == name) {
            if matches!(actual, TypeName::Struct(actual_name) if actual_name == "_") {
                return Ok(());
            }
            if let Some(previous) = inferred.get(name) {
                if previous != actual {
                    return Err(type_error(
                        format!(
                            "conflicting inferred types for generic parameter '{}': {} and {}",
                            name,
                            previous.display(),
                            actual.display()
                        )
                        .as_str(),
                        span,
                    ));
                }
            } else {
                inferred.insert(name.clone(), actual.clone());
            }
            return Ok(());
        }
    }
    match (template, actual) {
        (
            TypeName::Applied {
                name: template_name,
                args: template_args,
            },
            TypeName::Applied {
                name: actual_name,
                args: actual_args,
            },
        ) if template_name == actual_name && template_args.len() == actual_args.len() => {
            for (template_arg, actual_arg) in template_args.iter().zip(actual_args.iter()) {
                infer_generic_type_bindings(template_arg, actual_arg, type_params, inferred, span)?;
            }
        }
        (
            TypeName::Dict {
                key: template_key,
                value: template_value,
            },
            TypeName::Dict {
                key: actual_key,
                value: actual_value,
            },
        ) => {
            infer_generic_type_bindings(template_key, actual_key, type_params, inferred, span)?;
            infer_generic_type_bindings(template_value, actual_value, type_params, inferred, span)?;
        }
        (
            TypeName::Map {
                key: template_key,
                value: template_value,
            },
            TypeName::Map {
                key: actual_key,
                value: actual_value,
            },
        ) => {
            infer_generic_type_bindings(template_key, actual_key, type_params, inferred, span)?;
            infer_generic_type_bindings(template_value, actual_value, type_params, inferred, span)?;
        }
        (TypeName::List { elem: template }, TypeName::List { elem: actual }) => {
            infer_generic_type_bindings(template, actual, type_params, inferred, span)?;
        }
        (
            TypeName::Array {
                elem: template_elem,
                len: template_len,
            },
            TypeName::Array {
                elem: actual_elem,
                len: actual_len,
            },
        ) if template_len == actual_len => {
            infer_generic_type_bindings(template_elem, actual_elem, type_params, inferred, span)?
        }
        (
            TypeName::Ref {
                mutable: template_mutable,
                inner: template_inner,
            },
            TypeName::Ref {
                mutable: actual_mutable,
                inner: actual_inner,
            },
        ) if template_mutable == actual_mutable => {
            infer_generic_type_bindings(template_inner, actual_inner, type_params, inferred, span)?
        }
        _ => {}
    }
    Ok(())
}

fn instantiate_generic_type(template: &TypeName, inferred: &HashMap<String, TypeName>) -> TypeName {
    match template {
        TypeName::Struct(name) => inferred
            .get(name)
            .cloned()
            .unwrap_or_else(|| template.clone()),
        TypeName::Applied { name, args } => TypeName::Applied {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| instantiate_generic_type(arg, inferred))
                .collect(),
        },
        TypeName::Dict { key, value } => TypeName::Dict {
            key: Box::new(instantiate_generic_type(key, inferred)),
            value: Box::new(instantiate_generic_type(value, inferred)),
        },
        TypeName::List { elem } => TypeName::List {
            elem: Box::new(instantiate_generic_type(elem, inferred)),
        },
        TypeName::Map { key, value } => TypeName::Map {
            key: Box::new(instantiate_generic_type(key, inferred)),
            value: Box::new(instantiate_generic_type(value, inferred)),
        },
        TypeName::Array { elem, len } => TypeName::Array {
            elem: Box::new(instantiate_generic_type(elem, inferred)),
            len: *len,
        },
        TypeName::Ref { mutable, inner } => TypeName::Ref {
            mutable: *mutable,
            inner: Box::new(instantiate_generic_type(inner, inferred)),
        },
        _ => template.clone(),
    }
}

fn enum_type_args(def: &EnumDef, inferred: &HashMap<String, TypeName>) -> Vec<Option<TypeName>> {
    def.type_params
        .iter()
        .map(|param| inferred.get(param).cloned())
        .collect()
}

fn eval_match_expr(
    value_expr: &Expr,
    arms: &[MatchArm],
    env: &mut Env,
    index: &ProgramIndex,
    span: Span,
) -> Result<Value, Diagnostic> {
    let value = eval_expr(value_expr, env, index)?;
    let Value::Enum {
        name: enum_name,
        variant: active_variant,
        type_args,
        payload,
    } = value
    else {
        return Err(type_error("match value must be an enum", value_expr.span()));
    };
    let enum_def = index.enums.get(&enum_name).ok_or_else(|| {
        Diagnostic::at_span(format!("unknown enum '{enum_name}'"), value_expr.span())
    })?;

    let mut covered = HashSet::new();
    let mut wildcard_seen = false;
    let mut selected = None;
    for (arm_index, arm) in arms.iter().enumerate() {
        match &arm.pattern {
            MatchPattern::Wildcard { .. } => {
                if wildcard_seen || arm_index + 1 != arms.len() {
                    return Err(type_error(
                        "wildcard match arm must appear exactly once and be last",
                        arm.pattern.span(),
                    ));
                }
                if covered.len() == enum_def.variants.len() {
                    return Err(type_error(
                        "unreachable wildcard match arm: every enum variant is already covered",
                        arm.pattern.span(),
                    ));
                }
                wildcard_seen = true;
                if selected.is_none() {
                    selected = Some(arm_index);
                }
            }
            pattern => {
                let variant = validate_enum_match_pattern(pattern, enum_def)?;
                if !covered.insert(variant.to_string()) {
                    return Err(type_error(
                        format!(
                            "unreachable duplicate match arm for '{}::{}'",
                            enum_name, variant
                        )
                        .as_str(),
                        arm.pattern.span(),
                    ));
                }
                if variant == active_variant {
                    selected = Some(arm_index);
                }
            }
        }
    }

    if !wildcard_seen {
        let missing: Vec<String> = enum_def
            .variants
            .iter()
            .filter(|variant| !covered.contains(&variant.name))
            .map(|variant| format!("{}::{}", enum_name, variant.name))
            .collect();
        if !missing.is_empty() {
            return Err(type_error(
                format!(
                    "non-exhaustive match for enum '{}'; missing {}",
                    enum_name,
                    missing.join(", ")
                )
                .as_str(),
                span,
            ));
        }
    }

    let selected = selected.ok_or_else(|| {
        type_error(
            "internal error: exhaustive enum match selected no arm",
            span,
        )
    })?;
    let arm = &arms[selected];
    env.push();
    let result = (|| -> Result<Value, Diagnostic> {
        bind_enum_match_pattern(
            &arm.pattern,
            enum_def,
            &active_variant,
            &type_args,
            &payload,
            env,
        )?;
        eval_expr(&arm.expr, env, index)
    })();
    let popped = env.pop();
    release_borrows(&popped, env)?;
    result
}

fn validate_enum_match_pattern<'a>(
    pattern: &'a MatchPattern,
    enum_def: &EnumDef,
) -> Result<&'a str, Diagnostic> {
    let (pattern_enum, variant_name) = match pattern {
        MatchPattern::EnumUnit {
            enum_name, variant, ..
        }
        | MatchPattern::EnumTuple {
            enum_name, variant, ..
        }
        | MatchPattern::EnumNamed {
            enum_name, variant, ..
        } => (enum_name, variant),
        MatchPattern::Wildcard { .. } => {
            return Err(type_error(
                "internal error: wildcard passed to enum-pattern validator",
                pattern.span(),
            ));
        }
    };
    if pattern_enum != &enum_def.name {
        return Err(type_error(
            format!(
                "pattern enum '{}' does not match scrutinee enum '{}'",
                pattern_enum, enum_def.name
            )
            .as_str(),
            pattern.span(),
        ));
    }
    let variant_def = enum_def
        .variants
        .iter()
        .find(|variant| variant.name == *variant_name)
        .ok_or_else(|| {
            type_error(
                format!(
                    "unknown variant '{}' for enum '{}' in match pattern",
                    variant_name, enum_def.name
                )
                .as_str(),
                pattern.span(),
            )
        })?;

    let mut bindings = HashSet::new();
    match (pattern, &variant_def.payload) {
        (MatchPattern::EnumUnit { .. }, EnumVariantPayloadDef::Unit) => {}
        (
            MatchPattern::EnumTuple {
                bindings: fields, ..
            },
            EnumVariantPayloadDef::Tuple(field_defs),
        ) => {
            if fields.len() != field_defs.len() {
                return Err(type_error(
                    format!(
                        "pattern '{}::{}' expects {} fields, got {}",
                        enum_def.name,
                        variant_name,
                        field_defs.len(),
                        fields.len()
                    )
                    .as_str(),
                    pattern.span(),
                ));
            }
            for name in fields.iter().flatten() {
                if !bindings.insert(name) {
                    return Err(type_error(
                        format!("duplicate binding '{name}' in match pattern").as_str(),
                        pattern.span(),
                    ));
                }
            }
        }
        (MatchPattern::EnumNamed { fields, .. }, EnumVariantPayloadDef::Named(field_defs)) => {
            let mut seen_fields = HashSet::new();
            for field in fields {
                if !seen_fields.insert(&field.name) {
                    return Err(type_error(
                        format!("duplicate field '{}' in match pattern", field.name).as_str(),
                        field.span,
                    ));
                }
                if !field_defs
                    .iter()
                    .any(|candidate| candidate.name == field.name)
                {
                    return Err(type_error(
                        format!(
                            "unknown payload field '{}' for pattern '{}::{}'",
                            field.name, enum_def.name, variant_name
                        )
                        .as_str(),
                        field.span,
                    ));
                }
                if let Some(binding) = &field.binding {
                    if !bindings.insert(binding) {
                        return Err(type_error(
                            format!("duplicate binding '{binding}' in match pattern").as_str(),
                            field.span,
                        ));
                    }
                }
            }
        }
        (MatchPattern::EnumUnit { .. }, _) => {
            return Err(type_error(
                format!(
                    "pattern '{}::{}' must destructure its payload",
                    enum_def.name, variant_name
                )
                .as_str(),
                pattern.span(),
            ));
        }
        (MatchPattern::EnumTuple { .. }, _) => {
            return Err(type_error(
                format!(
                    "pattern '{}::{}' uses tuple syntax for a non-tuple variant",
                    enum_def.name, variant_name
                )
                .as_str(),
                pattern.span(),
            ));
        }
        (MatchPattern::EnumNamed { .. }, _) => {
            return Err(type_error(
                format!(
                    "pattern '{}::{}' uses named syntax for a non-named variant",
                    enum_def.name, variant_name
                )
                .as_str(),
                pattern.span(),
            ));
        }
        (MatchPattern::Wildcard { .. }, _) => unreachable!(),
    }
    Ok(variant_name)
}

fn bind_enum_match_pattern(
    pattern: &MatchPattern,
    enum_def: &EnumDef,
    active_variant: &str,
    type_args: &[Option<TypeName>],
    payload: &EnumPayloadValue,
    env: &mut Env,
) -> Result<(), Diagnostic> {
    let inferred: HashMap<String, TypeName> = enum_def
        .type_params
        .iter()
        .zip(type_args.iter())
        .filter_map(|(param, arg)| arg.clone().map(|arg| (param.clone(), arg)))
        .collect();
    let variant_def = enum_def
        .variants
        .iter()
        .find(|variant| variant.name == active_variant)
        .ok_or_else(|| {
            type_error(
                "internal error: active enum variant is absent from its definition",
                pattern.span(),
            )
        })?;
    match (pattern, &variant_def.payload, payload) {
        (MatchPattern::Wildcard { .. }, _, _) | (MatchPattern::EnumUnit { .. }, _, _) => Ok(()),
        (
            MatchPattern::EnumTuple { bindings, .. },
            EnumVariantPayloadDef::Tuple(field_defs),
            EnumPayloadValue::Tuple(values),
        ) => {
            for ((binding, field_def), value) in
                bindings.iter().zip(field_defs.iter()).zip(values.iter())
            {
                if let Some(name) = binding {
                    env.insert(
                        name.clone(),
                        Binding {
                            value: value.clone(),
                            ty: instantiate_generic_type(&field_def.ty, &inferred),
                            mutable: false,
                            shared_borrows: 0,
                            mut_borrowed: false,
                        },
                    );
                }
            }
            Ok(())
        }
        (
            MatchPattern::EnumNamed { fields, .. },
            EnumVariantPayloadDef::Named(field_defs),
            EnumPayloadValue::Named(values),
        ) => {
            for field in fields {
                let Some(binding) = &field.binding else {
                    continue;
                };
                let field_def = field_defs
                    .iter()
                    .find(|candidate| candidate.name == field.name)
                    .expect("validated enum match field should exist");
                let value = values
                    .get(&field.name)
                    .expect("constructed enum payload field should exist");
                env.insert(
                    binding.clone(),
                    Binding {
                        value: value.clone(),
                        ty: instantiate_generic_type(&field_def.ty, &inferred),
                        mutable: false,
                        shared_borrows: 0,
                        mut_borrowed: false,
                    },
                );
            }
            Ok(())
        }
        _ => Err(type_error(
            "internal error: enum payload does not match its definition",
            pattern.span(),
        )),
    }
}

include!("semantics/value_ops.rs");

#[cfg(test)]
#[path = "semantics/tests.rs"]
mod tests;
