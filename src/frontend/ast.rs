#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub column: usize,
    pub source_id: usize,
}

impl Span {
    pub fn new(line: usize, column: usize) -> Self {
        Self {
            line,
            column,
            source_id: 0,
        }
    }

    pub fn in_source(line: usize, column: usize, source_id: usize) -> Self {
        Self {
            line,
            column,
            source_id,
        }
    }
}

impl From<&Span> for Span {
    fn from(span: &Span) -> Self {
        *span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeName {
    Int {
        signed: bool,
        bits: u16,
    },
    Float {
        bits: u16,
    },
    Bool,
    Byte,
    Char,
    String,
    Path,
    File,
    Thread,
    Struct(String),
    Applied {
        name: String,
        args: Vec<TypeName>,
    },
    Dict {
        key: Box<TypeName>,
        value: Box<TypeName>,
    },
    List {
        elem: Box<TypeName>,
    },
    Map {
        key: Box<TypeName>,
        value: Box<TypeName>,
    },
    Array {
        elem: Box<TypeName>,
        len: u64,
    },
    Ref {
        mutable: bool,
        inner: Box<TypeName>,
    },
}

impl TypeName {
    pub fn display(&self) -> String {
        match self {
            TypeName::Int { signed, bits } => {
                if *signed {
                    format!("i{bits}")
                } else {
                    format!("u{bits}")
                }
            }
            TypeName::Float { bits } => format!("f{bits}"),
            TypeName::Bool => "bool".to_string(),
            TypeName::Byte => "byte".to_string(),
            TypeName::Char => "char".to_string(),
            TypeName::String => "string".to_string(),
            TypeName::Path => "Path".to_string(),
            TypeName::File => "File".to_string(),
            TypeName::Thread => "Thread".to_string(),
            TypeName::Struct(name) => name.clone(),
            TypeName::Applied { name, args } => format!(
                "{}<{}>",
                name,
                args.iter()
                    .map(TypeName::display)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeName::Dict { key, value } => {
                format!("dict<{}, {}>", key.display(), value.display())
            }
            TypeName::List { elem } => format!("list<{}>", elem.display()),
            TypeName::Map { key, value } => {
                format!("map<{}, {}>", key.display(), value.display())
            }
            TypeName::Array { elem, len } => format!("[{}; {len}]", elem.display()),
            TypeName::Ref { mutable, inner } => {
                if *mutable {
                    format!("&mut {}", inner.display())
                } else {
                    format!("&{}", inner.display())
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Function(Function),
    Struct(StructDef),
    Enum(EnumDef),
    Trait(TraitDef),
    Impl(TraitImplDef),
    InherentImpl(InherentImplDef),
    Module(ModuleDecl),
    Use(UseDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDecl {
    pub name: String,
    pub public: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UseDecl {
    pub module: String,
    pub name: String,
    pub alias: Option<String>,
    pub public: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub public: bool,
    pub params: Vec<FunctionParam>,
    pub return_type: Option<TypeName>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionParam {
    pub name: String,
    pub ty: TypeName,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub public: bool,
    pub fields: Vec<StructField>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: String,
    pub ty: TypeName,
    pub embedded: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitDef {
    pub name: String,
    pub public: bool,
    pub methods: Vec<TraitMethodSig>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitMethodSig {
    pub name: String,
    pub params: Vec<FunctionParam>,
    pub return_type: Option<TypeName>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplDef {
    pub trait_name: String,
    pub for_type: String,
    pub methods: Vec<Function>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InherentImplDef {
    pub for_type: String,
    pub methods: Vec<Function>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReductionOp {
    Sum,
    Min,
    Max,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParForReduction {
    pub op: ReductionOp,
    pub target: String,
    pub expr: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    pub name: String,
    pub public: bool,
    pub type_params: Vec<String>,
    pub variants: Vec<EnumVariantDef>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantDef {
    pub name: String,
    pub payload: EnumVariantPayloadDef,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnumVariantPayloadDef {
    Unit,
    Tuple(Vec<EnumTupleFieldDef>),
    Named(Vec<StructField>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumTupleFieldDef {
    pub ty: TypeName,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        mutable: bool,
        ty: Option<TypeName>,
        expr: Expr,
        span: Span,
    },
    Assign {
        name: String,
        expr: Expr,
        span: Span,
    },
    AssignIndex {
        name: String,
        index: Expr,
        expr: Expr,
        span: Span,
    },
    /// Mutation of a list stored directly in a struct field. This is kept
    /// distinct from `AssignIndex` so lowering can retain the aggregate's
    /// ownership descriptor instead of manufacturing a temporary list owner.
    AssignStructListIndex {
        receiver: String,
        field: String,
        index: Expr,
        expr: Expr,
        span: Span,
    },
    AssignField {
        receiver: String,
        field: String,
        expr: Expr,
        span: Span,
    },
    Call {
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    MethodCall {
        receiver: String,
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// A mutating list method on a direct struct field, for example
    /// `bag.values.push(value)`.
    StructListMethodCall {
        receiver: String,
        field: String,
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    Return {
        expr: Option<Expr>,
        span: Span,
    },
    Print {
        expr: Expr,
        span: Span,
    },
    Exit {
        expr: Expr,
        span: Span,
    },
    BenchLoop {
        iterations: Expr,
        span: Span,
    },
    Block {
        stmts: Vec<Stmt>,
        span: Span,
    },
    If {
        cond: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    Loop {
        body: Vec<Stmt>,
        span: Span,
    },
    For {
        name: String,
        start: Expr,
        end: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    ParFor {
        name: String,
        start: Expr,
        end: Expr,
        body: Vec<Stmt>,
        reduction: Option<ParForReduction>,
        span: Span,
    },
    ForEach {
        name: String,
        iterable: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    Assert {
        cond: Expr,
        message: Option<Expr>,
        span: Span,
    },
    Panic {
        message: Expr,
        span: Span,
    },
    Break {
        span: Span,
    },
    Continue {
        span: Span,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Plus,
    Neg,
    Not,
    Ref,
    RefMut,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructInitField {
    pub name: String,
    pub expr: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DictEntry {
    pub key: String,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub expr: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchNamedFieldPattern {
    pub name: String,
    pub binding: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchPattern {
    Wildcard {
        span: Span,
    },
    EnumUnit {
        enum_name: String,
        variant: String,
        span: Span,
    },
    EnumTuple {
        enum_name: String,
        variant: String,
        bindings: Vec<Option<String>>,
        span: Span,
    },
    EnumNamed {
        enum_name: String,
        variant: String,
        fields: Vec<MatchNamedFieldPattern>,
        span: Span,
    },
}

impl MatchPattern {
    pub fn span(&self) -> Span {
        match self {
            MatchPattern::Wildcard { span }
            | MatchPattern::EnumUnit { span, .. }
            | MatchPattern::EnumTuple { span, .. }
            | MatchPattern::EnumNamed { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Bool {
        value: bool,
        span: Span,
    },
    String {
        value: String,
        span: Span,
    },
    Char {
        value: char,
        span: Span,
    },
    Number {
        literal: String,
        span: Span,
    },
    Ident {
        name: String,
        span: Span,
    },
    Call {
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    QualifiedCall {
        owner: String,
        member: String,
        args: Vec<Expr>,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    FieldAccess {
        base: Box<Expr>,
        field: String,
        span: Span,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    ArrayLit {
        elems: Vec<Expr>,
        span: Span,
    },
    StructInit {
        name: String,
        fields: Vec<StructInitField>,
        span: Span,
    },
    EnumVariant {
        enum_name: String,
        variant: String,
        span: Span,
    },
    EnumTupleVariant {
        enum_name: String,
        variant: String,
        args: Vec<Expr>,
        span: Span,
    },
    EnumStructVariant {
        enum_name: String,
        variant: String,
        fields: Vec<StructInitField>,
        span: Span,
    },
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    DictLit {
        entries: Vec<DictEntry>,
        span: Span,
    },
    MethodCall {
        receiver: Box<Expr>,
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Bool { span, .. }
            | Expr::String { span, .. }
            | Expr::Char { span, .. }
            | Expr::Number { span, .. }
            | Expr::Ident { span, .. }
            | Expr::Call { span, .. }
            | Expr::QualifiedCall { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::FieldAccess { span, .. }
            | Expr::Index { span, .. }
            | Expr::ArrayLit { span, .. }
            | Expr::StructInit { span, .. }
            | Expr::EnumVariant { span, .. }
            | Expr::EnumTupleVariant { span, .. }
            | Expr::EnumStructVariant { span, .. }
            | Expr::Match { span, .. }
            | Expr::DictLit { span, .. }
            | Expr::MethodCall { span, .. } => *span,
        }
    }
}
