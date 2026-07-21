//! Runtime-native type, ownership, ABI, and lowering orchestration.

use super::*;

#[derive(Debug)]
enum RuntimeGenericLowerError {
    Unsupported,
    Diagnostic(Diagnostic),
}

type RuntimeGenericLowerResult<T> = Result<T, RuntimeGenericLowerError>;

/// Diagnostic for former benchmark-only runtime calls.
///
/// These names deliberately remain recognizable during runtime-lowering
/// selection so a program receives this actionable error instead of silently
/// taking the semantic-evaluation fallback. They are not runtime APIs.
fn removed_benchmark_kernel_diagnostic(name: &str, span: Span) -> Option<Diagnostic> {
    matches!(
        name,
        "runtime_bloom_sbbf_insert"
            | "runtime_bloom_sbbf_maybe"
            | "runtime_hash_probe_grouped16"
            | "runtime_join_select_adaptive"
    )
    .then(|| {
        type_error(
            format!(
                "{name}() is no longer available; express the algorithm with ordinary Aziky control flow so the generic optimizer can analyze it"
            )
            .as_str(),
            span,
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeIntType {
    signed: bool,
    bits: u16,
}

impl RuntimeIntType {
    fn new(signed: bool, bits: u16) -> RuntimeGenericLowerResult<Self> {
        if matches!(bits, 8 | 16 | 32 | 64) {
            Ok(Self { signed, bits })
        } else {
            Err(RuntimeGenericLowerError::Unsupported)
        }
    }

    fn from_type_name(ty: &TypeName) -> RuntimeGenericLowerResult<Self> {
        match ty {
            TypeName::Int { signed, bits } => Self::new(*signed, *bits),
            TypeName::Byte => Self::new(false, 8),
            TypeName::Bool => Self::new(false, 8),
            TypeName::Char => Self::new(false, 32),
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    fn from_value(value: &Value) -> RuntimeGenericLowerResult<Self> {
        match value {
            Value::Int { bits, .. } => Self::new(true, *bits),
            Value::UInt { bits, .. } => Self::new(false, *bits),
            Value::Bool(_) => Self::new(false, 8),
            Value::Char(_) => Self::new(false, 32),
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    fn display(self) -> String {
        if self.signed {
            format!("i{}", self.bits)
        } else {
            format!("u{}", self.bits)
        }
    }

    fn storage_bytes(self) -> u8 {
        u8::try_from(self.bits / 8).expect("validated runtime integer width")
    }

    fn cmp_from_binary(self, op: BinaryOp) -> Option<RuntimeCmpOp> {
        match op {
            BinaryOp::Eq => Some(RuntimeCmpOp::Eq),
            BinaryOp::Ne => Some(RuntimeCmpOp::Ne),
            BinaryOp::Lt => {
                if self.signed {
                    Some(RuntimeCmpOp::LtSigned)
                } else {
                    Some(RuntimeCmpOp::LtUnsigned)
                }
            }
            BinaryOp::Le => {
                if self.signed {
                    Some(RuntimeCmpOp::LeSigned)
                } else {
                    Some(RuntimeCmpOp::LeUnsigned)
                }
            }
            BinaryOp::Gt => {
                if self.signed {
                    Some(RuntimeCmpOp::GtSigned)
                } else {
                    Some(RuntimeCmpOp::GtUnsigned)
                }
            }
            BinaryOp::Ge => {
                if self.signed {
                    Some(RuntimeCmpOp::GeSigned)
                } else {
                    Some(RuntimeCmpOp::GeUnsigned)
                }
            }
            _ => None,
        }
    }

    fn encode_value(self, value: Value, span: Span) -> RuntimeGenericLowerResult<u64> {
        match value {
            Value::Bool(value) => {
                let bool_u = if value { 1u128 } else { 0u128 };
                if self.signed {
                    encode_signed_bits(bool_u as i128, self.bits, span)
                } else {
                    encode_unsigned_bits(bool_u, self.bits, span)
                }
            }
            Value::UInt { value, .. } => {
                if self.signed {
                    let converted = i128::try_from(value).map_err(|_| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "integer literal out of range",
                            span,
                        ))
                    })?;
                    encode_signed_bits(converted, self.bits, span)
                } else {
                    encode_unsigned_bits(value, self.bits, span)
                }
            }
            Value::Int { value, .. } => {
                if self.signed {
                    encode_signed_bits(value, self.bits, span)
                } else {
                    if value < 0 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "cannot coerce negative integer to unsigned",
                            span,
                        )));
                    }
                    let converted = u128::try_from(value).map_err(|_| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "integer literal out of range",
                            span,
                        ))
                    })?;
                    encode_unsigned_bits(converted, self.bits, span)
                }
            }
            Value::Char(value) => encode_unsigned_bits(u128::from(value as u32), self.bits, span),
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeScalarType {
    Int(RuntimeIntType),
    Float(u16),
}

impl RuntimeScalarType {
    fn display(self) -> String {
        match self {
            RuntimeScalarType::Int(int_ty) => int_ty.display(),
            RuntimeScalarType::Float(bits) => format!("f{bits}"),
        }
    }

    fn storage_bytes(self) -> u8 {
        match self {
            RuntimeScalarType::Int(int_ty) => int_ty.storage_bytes(),
            RuntimeScalarType::Float(bits) => {
                u8::try_from(bits / 8).expect("validated runtime float width")
            }
        }
    }
}

fn runtime_scalar_type_name(ty: RuntimeScalarType) -> TypeName {
    match ty {
        RuntimeScalarType::Int(RuntimeIntType { signed, bits }) => TypeName::Int { signed, bits },
        RuntimeScalarType::Float(bits) => TypeName::Float { bits },
    }
}

#[derive(Debug, Clone, Copy)]
struct RuntimeConstInt {
    encoded: u64,
    ty: RuntimeIntType,
}

#[derive(Debug, Clone)]
enum RuntimeConstContainer {
    Struct {
        struct_name: String,
        fields: HashMap<String, RuntimeConstInt>,
    },
    Array {
        elems: Vec<RuntimeConstInt>,
    },
    Dict {
        entries: HashMap<String, RuntimeConstInt>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeStructFieldLayout {
    name: String,
    ty: RuntimeScalarType,
    offset_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeStructListLayout {
    struct_name: String,
    fields: Vec<RuntimeStructFieldLayout>,
    stride_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeMapLayout {
    key_ty: RuntimeScalarType,
    value_ty: RuntimeScalarType,
    value_offset_bytes: u64,
    stride_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeEnumVariantLayout {
    name: String,
    tag: u64,
    fields: Vec<(String, RuntimeScalarType)>,
    resource: Option<RuntimeEnumResourceLayout>,
    nested: Option<RuntimeEnumNestedLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeEnumNestedLayout {
    name: String,
    layout: Box<RuntimeEnumLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeEnumResourceLayout {
    name: String,
    kind: RuntimeEnumResourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeEnumResourceKind {
    ListScalar(RuntimeScalarType),
    ListStruct(RuntimeStructListLayout),
    String,
    Map(RuntimeMapLayout),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeEnumLayout {
    enum_name: String,
    type_args: Vec<TypeName>,
    variants: Vec<RuntimeEnumVariantLayout>,
    payload_slots: usize,
}

impl RuntimeEnumLayout {
    fn owns_resources(&self) -> bool {
        self.variants.iter().any(|variant| {
            variant.resource.is_some()
                || variant
                    .nested
                    .as_ref()
                    .is_some_and(|nested| nested.layout.owns_resources())
        })
    }
}

#[derive(Debug, Clone)]
enum RuntimeFunctionParamLayout {
    Scalar {
        name: String,
        ty: RuntimeScalarType,
        slot: usize,
    },
    OwnedFile {
        name: String,
        fd_slot: usize,
    },
    BorrowedFile {
        name: String,
        fd_slot: usize,
    },
    OwnedSender {
        name: String,
        handle_slot: usize,
    },
    OwnedReceiver {
        name: String,
        handle_slot: usize,
    },
    Struct {
        name: String,
        layout: RuntimeStructListLayout,
        fields: HashMap<String, (usize, RuntimeScalarType)>,
        mutable: bool,
        by_ref: bool,
    },
    OwnedStruct {
        name: String,
        binding: RuntimeGenericBinding,
    },
    BorrowedOwnedStruct {
        name: String,
        binding: RuntimeGenericBinding,
        mutable: bool,
    },
    OwnedListScalar {
        name: String,
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        elem_ty: RuntimeScalarType,
    },
    OwnedListStruct {
        name: String,
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        layout: RuntimeStructListLayout,
    },
    OwnedString {
        name: String,
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
    },
    OwnedMap {
        name: String,
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        layout: RuntimeMapLayout,
    },
    Enum {
        name: String,
        layout: RuntimeEnumLayout,
        tag_slot: usize,
        payload_slots: Vec<usize>,
    },
}

#[derive(Debug, Clone)]
enum RuntimeFunctionReturnLayout {
    Scalar {
        ty: RuntimeScalarType,
        slot: usize,
    },
    OwnedFile {
        fd_slot: usize,
    },
    Struct {
        layout: RuntimeStructListLayout,
        fields: HashMap<String, (usize, RuntimeScalarType)>,
    },
    OwnedStruct {
        binding: RuntimeGenericBinding,
    },
    OwnedListScalar {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        elem_ty: RuntimeScalarType,
    },
    OwnedListStruct {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        layout: RuntimeStructListLayout,
    },
    OwnedString {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
    },
    OwnedMap {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        layout: RuntimeMapLayout,
    },
    Enum {
        layout: RuntimeEnumLayout,
        tag_slot: usize,
        payload_slots: Vec<usize>,
    },
}

#[derive(Debug, Clone)]
enum RuntimeOwnedStructListField {
    Scalar {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        elem_ty: RuntimeScalarType,
    },
    Struct {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        layout: RuntimeStructListLayout,
    },
}

#[derive(Debug, Clone)]
enum RuntimeGenericBinding {
    Scalar {
        slot: usize,
        mutable: bool,
        ty: RuntimeScalarType,
    },
    StructSlots {
        struct_name: String,
        fields: HashMap<String, (usize, RuntimeScalarType)>,
        mutable: bool,
    },
    OwnedMap {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        mutable: bool,
        layout: RuntimeMapLayout,
    },
    OwnedStruct {
        struct_name: String,
        scalar_fields: HashMap<String, (usize, RuntimeScalarType)>,
        list_fields: HashMap<String, RuntimeOwnedStructListField>,
        mutable: bool,
        owns_cleanup: bool,
    },
    ArraySlots {
        slots: Vec<usize>,
        len_slot: usize,
        mutable: bool,
        elem_ty: RuntimeIntType,
        full_len_known: bool,
    },
    DictSlots {
        entries: HashMap<String, usize>,
        mutable: bool,
        value_ty: RuntimeIntType,
    },
    ConstContainer {
        container: RuntimeConstContainer,
    },
    OwnedPtr {
        ptr_slot: usize,
        size_slot: usize,
    },
    OwnedFile {
        fd_slot: usize,
    },
    OwnedThread {
        handle_slot: usize,
    },
    OwnedChannel {
        handle_slot: usize,
        sender_taken: bool,
        receiver_taken: bool,
    },
    OwnedSender {
        handle_slot: usize,
    },
    OwnedReceiver {
        handle_slot: usize,
    },
    BorrowedFile {
        fd_slot: usize,
    },
    MovedResource {
        kind: &'static str,
    },
    OwnedListScalar {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        mutable: bool,
        elem_ty: RuntimeScalarType,
    },
    OwnedListStruct {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        mutable: bool,
        layout: RuntimeStructListLayout,
    },
    OwnedString {
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        mutable: bool,
        is_path: bool,
    },
    OptionScalar {
        tag_slot: usize,
        payload_slot: usize,
        mutable: bool,
        elem_ty: RuntimeScalarType,
    },
    OptionStruct {
        tag_slot: usize,
        fields: HashMap<String, (usize, RuntimeScalarType)>,
        mutable: bool,
        layout: RuntimeStructListLayout,
    },
    EnumSlots {
        layout: RuntimeEnumLayout,
        tag_slot: usize,
        payload_slots: Vec<usize>,
        mutable: bool,
        owns_cleanup: bool,
    },
}

impl RuntimeGenericBinding {
    fn as_scalar(&self) -> Option<(usize, bool, RuntimeScalarType)> {
        match self {
            RuntimeGenericBinding::Scalar { slot, mutable, ty } => Some((*slot, *mutable, *ty)),
            RuntimeGenericBinding::StructSlots { .. } => None,
            RuntimeGenericBinding::OwnedStruct { .. } => None,
            RuntimeGenericBinding::ArraySlots { .. } => None,
            RuntimeGenericBinding::DictSlots { .. } => None,
            RuntimeGenericBinding::ConstContainer { .. } => None,
            RuntimeGenericBinding::OwnedPtr { .. }
            | RuntimeGenericBinding::OwnedFile { .. }
            | RuntimeGenericBinding::OwnedThread { .. }
            | RuntimeGenericBinding::OwnedChannel { .. }
            | RuntimeGenericBinding::OwnedSender { .. }
            | RuntimeGenericBinding::OwnedReceiver { .. }
            | RuntimeGenericBinding::BorrowedFile { .. } => None,
            RuntimeGenericBinding::MovedResource { .. } => None,
            RuntimeGenericBinding::OwnedListScalar { .. } => None,
            RuntimeGenericBinding::OwnedListStruct { .. } => None,
            RuntimeGenericBinding::OwnedString { .. } => None,
            RuntimeGenericBinding::OwnedMap { .. } => None,
            RuntimeGenericBinding::OptionScalar { .. } => None,
            RuntimeGenericBinding::OptionStruct { .. } => None,
            RuntimeGenericBinding::EnumSlots { .. } => None,
        }
    }

    fn as_array_slots(&self) -> Option<(Vec<usize>, usize, bool, RuntimeIntType, bool)> {
        match self {
            RuntimeGenericBinding::ArraySlots {
                slots,
                len_slot,
                mutable,
                elem_ty,
                full_len_known,
            } => Some((
                slots.clone(),
                *len_slot,
                *mutable,
                *elem_ty,
                *full_len_known,
            )),
            RuntimeGenericBinding::Scalar { .. }
            | RuntimeGenericBinding::StructSlots { .. }
            | RuntimeGenericBinding::OwnedStruct { .. }
            | RuntimeGenericBinding::DictSlots { .. }
            | RuntimeGenericBinding::ConstContainer { .. }
            | RuntimeGenericBinding::OwnedPtr { .. }
            | RuntimeGenericBinding::OwnedFile { .. }
            | RuntimeGenericBinding::OwnedThread { .. }
            | RuntimeGenericBinding::OwnedChannel { .. }
            | RuntimeGenericBinding::OwnedSender { .. }
            | RuntimeGenericBinding::OwnedReceiver { .. }
            | RuntimeGenericBinding::BorrowedFile { .. }
            | RuntimeGenericBinding::MovedResource { .. }
            | RuntimeGenericBinding::OwnedListScalar { .. }
            | RuntimeGenericBinding::OwnedListStruct { .. }
            | RuntimeGenericBinding::OwnedString { .. }
            | RuntimeGenericBinding::OwnedMap { .. }
            | RuntimeGenericBinding::OptionScalar { .. }
            | RuntimeGenericBinding::OptionStruct { .. }
            | RuntimeGenericBinding::EnumSlots { .. } => None,
        }
    }

    fn as_dict_slots(&self) -> Option<(HashMap<String, usize>, bool, RuntimeIntType)> {
        match self {
            RuntimeGenericBinding::DictSlots {
                entries,
                mutable,
                value_ty,
            } => Some((entries.clone(), *mutable, *value_ty)),
            RuntimeGenericBinding::Scalar { .. }
            | RuntimeGenericBinding::StructSlots { .. }
            | RuntimeGenericBinding::OwnedStruct { .. }
            | RuntimeGenericBinding::ArraySlots { .. }
            | RuntimeGenericBinding::ConstContainer { .. }
            | RuntimeGenericBinding::OwnedPtr { .. }
            | RuntimeGenericBinding::OwnedFile { .. }
            | RuntimeGenericBinding::OwnedThread { .. }
            | RuntimeGenericBinding::OwnedChannel { .. }
            | RuntimeGenericBinding::OwnedSender { .. }
            | RuntimeGenericBinding::OwnedReceiver { .. }
            | RuntimeGenericBinding::BorrowedFile { .. }
            | RuntimeGenericBinding::MovedResource { .. }
            | RuntimeGenericBinding::OwnedListScalar { .. }
            | RuntimeGenericBinding::OwnedListStruct { .. }
            | RuntimeGenericBinding::OwnedString { .. }
            | RuntimeGenericBinding::OwnedMap { .. }
            | RuntimeGenericBinding::OptionScalar { .. }
            | RuntimeGenericBinding::OptionStruct { .. }
            | RuntimeGenericBinding::EnumSlots { .. } => None,
        }
    }

    fn container(&self) -> Option<&RuntimeConstContainer> {
        match self {
            RuntimeGenericBinding::ConstContainer { container } => Some(container),
            RuntimeGenericBinding::Scalar { .. }
            | RuntimeGenericBinding::StructSlots { .. }
            | RuntimeGenericBinding::OwnedStruct { .. }
            | RuntimeGenericBinding::ArraySlots { .. }
            | RuntimeGenericBinding::DictSlots { .. }
            | RuntimeGenericBinding::OwnedPtr { .. }
            | RuntimeGenericBinding::OwnedFile { .. }
            | RuntimeGenericBinding::OwnedThread { .. }
            | RuntimeGenericBinding::OwnedChannel { .. }
            | RuntimeGenericBinding::OwnedSender { .. }
            | RuntimeGenericBinding::OwnedReceiver { .. }
            | RuntimeGenericBinding::BorrowedFile { .. }
            | RuntimeGenericBinding::MovedResource { .. }
            | RuntimeGenericBinding::OwnedListScalar { .. }
            | RuntimeGenericBinding::OwnedListStruct { .. }
            | RuntimeGenericBinding::OwnedString { .. }
            | RuntimeGenericBinding::OwnedMap { .. }
            | RuntimeGenericBinding::OptionScalar { .. }
            | RuntimeGenericBinding::OptionStruct { .. }
            | RuntimeGenericBinding::EnumSlots { .. } => None,
        }
    }

    fn as_owned_list_scalar(
        &self,
    ) -> Option<(usize, usize, usize, usize, bool, RuntimeScalarType)> {
        match self {
            RuntimeGenericBinding::OwnedListScalar {
                ptr_slot,
                len_slot,
                capacity_slot,
                allocation_bytes_slot,
                mutable,
                elem_ty,
            } => Some((
                *ptr_slot,
                *len_slot,
                *capacity_slot,
                *allocation_bytes_slot,
                *mutable,
                *elem_ty,
            )),
            _ => None,
        }
    }

    fn as_owned_string(&self) -> Option<(usize, usize, usize, usize, bool)> {
        match self {
            RuntimeGenericBinding::OwnedString {
                ptr_slot,
                len_slot,
                capacity_slot,
                allocation_bytes_slot,
                mutable,
                is_path: false,
            } => Some((
                *ptr_slot,
                *len_slot,
                *capacity_slot,
                *allocation_bytes_slot,
                *mutable,
            )),
            _ => None,
        }
    }

    fn as_owned_text(&self) -> Option<(usize, usize, usize, usize, bool)> {
        match self {
            RuntimeGenericBinding::OwnedString {
                ptr_slot,
                len_slot,
                capacity_slot,
                allocation_bytes_slot,
                mutable,
                ..
            } => Some((
                *ptr_slot,
                *len_slot,
                *capacity_slot,
                *allocation_bytes_slot,
                *mutable,
            )),
            _ => None,
        }
    }

    fn as_owned_path(&self) -> Option<(usize, usize, usize, usize, bool)> {
        match self {
            RuntimeGenericBinding::OwnedString {
                ptr_slot,
                len_slot,
                capacity_slot,
                allocation_bytes_slot,
                mutable,
                is_path: true,
            } => Some((
                *ptr_slot,
                *len_slot,
                *capacity_slot,
                *allocation_bytes_slot,
                *mutable,
            )),
            _ => None,
        }
    }

    fn as_owned_file(&self) -> Option<usize> {
        match self {
            RuntimeGenericBinding::OwnedFile { fd_slot }
            | RuntimeGenericBinding::BorrowedFile { fd_slot } => Some(*fd_slot),
            _ => None,
        }
    }

    fn as_owned_map(&self) -> Option<(usize, usize, usize, usize, bool, RuntimeMapLayout)> {
        match self {
            RuntimeGenericBinding::OwnedMap {
                ptr_slot,
                len_slot,
                capacity_slot,
                allocation_bytes_slot,
                mutable,
                layout,
            } => Some((
                *ptr_slot,
                *len_slot,
                *capacity_slot,
                *allocation_bytes_slot,
                *mutable,
                layout.clone(),
            )),
            _ => None,
        }
    }

    fn as_option_scalar(&self) -> Option<(usize, usize, bool, RuntimeScalarType)> {
        match self {
            RuntimeGenericBinding::OptionScalar {
                tag_slot,
                payload_slot,
                mutable,
                elem_ty,
            } => Some((*tag_slot, *payload_slot, *mutable, *elem_ty)),
            _ => None,
        }
    }

    fn as_owned_list_struct(
        &self,
    ) -> Option<(usize, usize, usize, usize, bool, RuntimeStructListLayout)> {
        match self {
            RuntimeGenericBinding::OwnedListStruct {
                ptr_slot,
                len_slot,
                capacity_slot,
                allocation_bytes_slot,
                mutable,
                layout,
            } => Some((
                *ptr_slot,
                *len_slot,
                *capacity_slot,
                *allocation_bytes_slot,
                *mutable,
                layout.clone(),
            )),
            _ => None,
        }
    }

    fn as_option_struct(
        &self,
    ) -> Option<(
        usize,
        HashMap<String, (usize, RuntimeScalarType)>,
        bool,
        RuntimeStructListLayout,
    )> {
        match self {
            RuntimeGenericBinding::OptionStruct {
                tag_slot,
                fields,
                mutable,
                layout,
            } => Some((*tag_slot, fields.clone(), *mutable, layout.clone())),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct RuntimeGenericScopeStack {
    scopes: Vec<HashMap<String, RuntimeGenericBinding>>,
}

impl RuntimeGenericScopeStack {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        let _ = self.scopes.pop();
        if self.scopes.is_empty() {
            self.scopes.push(HashMap::new());
        }
    }

    fn depth(&self) -> usize {
        self.scopes.len()
    }

    fn owned_allocations_from(
        &self,
        depth: usize,
        include_borrowed_terminal_owners: bool,
    ) -> Vec<(usize, usize)> {
        let mut allocations = Vec::new();
        for scope in self.scopes.iter().skip(depth).rev() {
            let mut local = Vec::new();
            for binding in scope.values() {
                match binding {
                    RuntimeGenericBinding::OwnedPtr {
                        ptr_slot,
                        size_slot,
                    } => local.push((*ptr_slot, *size_slot)),
                    RuntimeGenericBinding::OwnedListScalar {
                        ptr_slot,
                        allocation_bytes_slot,
                        ..
                    }
                    | RuntimeGenericBinding::OwnedListStruct {
                        ptr_slot,
                        allocation_bytes_slot,
                        ..
                    }
                    | RuntimeGenericBinding::OwnedString {
                        ptr_slot,
                        allocation_bytes_slot,
                        ..
                    }
                    | RuntimeGenericBinding::OwnedMap {
                        ptr_slot,
                        allocation_bytes_slot,
                        ..
                    } => local.push((*ptr_slot, *allocation_bytes_slot)),
                    RuntimeGenericBinding::OwnedStruct {
                        list_fields,
                        owns_cleanup,
                        ..
                    } if *owns_cleanup || include_borrowed_terminal_owners => {
                        for field in list_fields.values() {
                            match field {
                                RuntimeOwnedStructListField::Scalar {
                                    ptr_slot,
                                    allocation_bytes_slot,
                                    ..
                                }
                                | RuntimeOwnedStructListField::Struct {
                                    ptr_slot,
                                    allocation_bytes_slot,
                                    ..
                                } => local.push((*ptr_slot, *allocation_bytes_slot)),
                            }
                        }
                    }
                    _ => {}
                }
            }
            local.sort_unstable_by_key(|(ptr_slot, _)| std::cmp::Reverse(*ptr_slot));
            allocations.extend(local);
        }
        allocations
    }

    fn owned_files_from(&self, depth: usize) -> Vec<usize> {
        let mut files = Vec::new();
        for scope in self.scopes.iter().skip(depth).rev() {
            let mut local: Vec<usize> = scope
                .values()
                .filter_map(|binding| match binding {
                    RuntimeGenericBinding::OwnedFile { fd_slot } => Some(*fd_slot),
                    _ => None,
                })
                .collect();
            local.sort_unstable_by_key(|slot| std::cmp::Reverse(*slot));
            files.extend(local);
        }
        files
    }

    fn owned_threads_from(&self, depth: usize) -> Vec<usize> {
        let mut threads = Vec::new();
        for scope in self.scopes.iter().skip(depth).rev() {
            let mut local: Vec<usize> = scope
                .values()
                .filter_map(|binding| match binding {
                    RuntimeGenericBinding::OwnedThread { handle_slot } => Some(*handle_slot),
                    _ => None,
                })
                .collect();
            local.sort_unstable_by_key(|slot| std::cmp::Reverse(*slot));
            threads.extend(local);
        }
        threads
    }

    fn owned_channels_from(&self, depth: usize) -> Vec<usize> {
        let mut out = Vec::new();
        for scope in self.scopes.iter().skip(depth).rev() {
            for binding in scope.values() {
                if let RuntimeGenericBinding::OwnedChannel { handle_slot, .. } = binding {
                    out.push(*handle_slot);
                }
            }
        }
        out.sort_unstable_by_key(|slot| std::cmp::Reverse(*slot));
        out
    }

    fn owned_channel_endpoints_from(&self, depth: usize) -> Vec<(usize, bool)> {
        let mut out = Vec::new();
        for scope in self.scopes.iter().skip(depth).rev() {
            for binding in scope.values() {
                match binding {
                    RuntimeGenericBinding::OwnedSender { handle_slot } => {
                        out.push((*handle_slot, true))
                    }
                    RuntimeGenericBinding::OwnedReceiver { handle_slot } => {
                        out.push((*handle_slot, false))
                    }
                    _ => {}
                }
            }
        }
        out.sort_unstable_by_key(|(slot, _)| std::cmp::Reverse(*slot));
        out
    }

    fn tagged_enum_allocations_from(
        &self,
        depth: usize,
        include_borrowed_terminal_owners: bool,
    ) -> Vec<(usize, u64, usize, usize)> {
        let mut allocations = Vec::new();
        for scope in self.scopes.iter().skip(depth).rev() {
            let mut local = Vec::new();
            for binding in scope.values() {
                let RuntimeGenericBinding::EnumSlots {
                    layout,
                    tag_slot,
                    payload_slots,
                    owns_cleanup,
                    ..
                } = binding
                else {
                    continue;
                };
                if !*owns_cleanup && !include_borrowed_terminal_owners {
                    continue;
                }
                if payload_slots.len() < 4 {
                    continue;
                }
                for variant in &layout.variants {
                    if variant.resource.is_some() {
                        local.push((*tag_slot, variant.tag, payload_slots[0], payload_slots[3]));
                    }
                }
            }
            local.sort_unstable_by_key(|(_, tag, ptr, _)| {
                (std::cmp::Reverse(*ptr), std::cmp::Reverse(*tag))
            });
            allocations.extend(local);
        }
        allocations
    }

    fn current_contains(&self, name: &str) -> bool {
        self.scopes
            .last()
            .map(|scope| scope.contains_key(name))
            .unwrap_or(false)
    }

    fn get_current(&self, name: &str) -> Option<&RuntimeGenericBinding> {
        self.scopes.last().and_then(|scope| scope.get(name))
    }

    fn insert(&mut self, name: String, binding: RuntimeGenericBinding) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, binding);
        }
    }

    fn get(&self, name: &str) -> Option<&RuntimeGenericBinding> {
        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.get(name) {
                return Some(binding);
            }
        }
        None
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut RuntimeGenericBinding> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(binding) = scope.get_mut(name) {
                return Some(binding);
            }
        }
        None
    }

    fn take_current(&mut self, name: &str) -> Option<RuntimeGenericBinding> {
        self.scopes.last_mut().and_then(|scope| scope.remove(name))
    }

    fn take(&mut self, name: &str) -> Option<RuntimeGenericBinding> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(binding) = scope.remove(name) {
                return Some(binding);
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
struct RuntimeGenericBuilder<'a> {
    functions: &'a HashMap<String, Function>,
    struct_layouts: &'a HashMap<String, Vec<LayoutField>>,
    enums: &'a HashMap<String, EnumDef>,
    scopes: RuntimeGenericScopeStack,
    slots: usize,
    instrs: Vec<RuntimeInstr>,
    pending_functions: VecDeque<String>,
    queued_functions: HashSet<String>,
    lowered_functions: HashSet<String>,
    function_entries: HashMap<String, usize>,
    function_param_slots: HashMap<String, Vec<RuntimeFunctionParamLayout>>,
    function_return_slots: HashMap<String, Option<RuntimeFunctionReturnLayout>>,
    active_function: Option<String>,
    call_patches: Vec<RuntimeCallPatch>,
    loop_frames: Vec<RuntimeLoopFrame>,
    unchecked_array_loop_accesses: Vec<RuntimeUncheckedArrayLoopAccess>,
    slot_unsigned_upper_bounds: HashMap<usize, u64>,
}

#[derive(Debug, Clone)]
struct RuntimeCallPatch {
    instr_index: usize,
    callee: String,
    span: Span,
}

#[derive(Debug, Clone)]
struct RuntimeLoopFrame {
    continue_target: Option<usize>,
    scope_depth: usize,
    continue_patches: Vec<usize>,
    break_patches: Vec<usize>,
}

#[derive(Debug, Clone)]
struct RuntimeUncheckedArrayLoopAccess {
    iter_name: String,
    iter_slot: usize,
    array_name: String,
}

mod builder;
mod collections;
mod control_flow;
mod expressions;

fn runtime_const_int_from_expr(
    expr: &Expr,
    expected: Option<RuntimeIntType>,
) -> RuntimeGenericLowerResult<RuntimeConstInt> {
    let value = match expr {
        Expr::Number { literal, span } => {
            parse_number_literal(literal, *span).map_err(RuntimeGenericLowerError::Diagnostic)?
        }
        Expr::Unary {
            op: UnaryOp::Plus,
            expr,
            ..
        } => return runtime_const_int_from_expr(expr, expected),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
            span,
        } => {
            let inner = runtime_const_int_from_expr(expr, None)?;
            if !inner.ty.signed {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "cannot negate unsigned literal in runtime generic lowering",
                    *span,
                )));
            }
            let signed = if inner.ty.bits == 64 {
                (inner.encoded as i64) as i128
            } else {
                let sign_bit = 1u64 << (inner.ty.bits - 1);
                let raw = inner.encoded & ((1u64 << inner.ty.bits) - 1);
                if raw & sign_bit != 0 {
                    (raw as i128) - (1i128 << inner.ty.bits)
                } else {
                    raw as i128
                }
            };
            let neg = signed.checked_neg().ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error("integer overflow", *span))
            })?;
            Value::Int {
                bits: inner.ty.bits,
                value: neg,
            }
        }
        _ => return Err(RuntimeGenericLowerError::Unsupported),
    };
    let ty = if let Some(expected_ty) = expected {
        expected_ty
    } else {
        RuntimeIntType::from_value(&value)?
    };
    let encoded = ty.encode_value(value, expr.span())?;
    Ok(RuntimeConstInt { encoded, ty })
}

fn runtime_const_array_index(expr: &Expr) -> RuntimeGenericLowerResult<usize> {
    let value = runtime_const_int_from_expr(expr, None)?;
    if value.ty.signed {
        let signed = if value.ty.bits == 64 {
            value.encoded as i64
        } else {
            let sign_bit = 1u64 << (value.ty.bits - 1);
            let raw = value.encoded & ((1u64 << value.ty.bits) - 1);
            if raw & sign_bit != 0 {
                (raw as i64) - (1i64 << value.ty.bits)
            } else {
                raw as i64
            }
        };
        if signed < 0 {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "array index must be non-negative in runtime generic lowering",
                expr.span(),
            )));
        }
        usize::try_from(signed as u64).map_err(|_| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "array index out of range in runtime generic lowering",
                expr.span(),
            ))
        })
    } else {
        usize::try_from(value.encoded).map_err(|_| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "array index out of range in runtime generic lowering",
                expr.span(),
            ))
        })
    }
}

fn runtime_const_dict_key(expr: &Expr) -> Option<String> {
    if let Expr::String { value, .. } = expr {
        Some(value.clone())
    } else {
        None
    }
}

fn max_unsigned_for_bits(bits: u16) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

fn runtime_while_body_contains_direct_loop_control(stmts: &[Stmt]) -> bool {
    for stmt in stmts {
        match stmt {
            Stmt::Break { .. } | Stmt::Continue { .. } => return true,
            Stmt::Block { stmts, .. } => {
                if runtime_while_body_contains_direct_loop_control(stmts) {
                    return true;
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                if runtime_while_body_contains_direct_loop_control(then_branch) {
                    return true;
                }
                if let Some(else_branch) = else_branch {
                    if runtime_while_body_contains_direct_loop_control(else_branch) {
                        return true;
                    }
                }
            }
            // Nested loops manage their own control-flow.
            Stmt::While { .. }
            | Stmt::Loop { .. }
            | Stmt::For { .. }
            | Stmt::ParFor { .. }
            | Stmt::ForEach { .. } => {}
            Stmt::Let { .. }
            | Stmt::Assign { .. }
            | Stmt::AssignIndex { .. }
            | Stmt::AssignStructListIndex { .. }
            | Stmt::AssignField { .. }
            | Stmt::Call { .. }
            | Stmt::MethodCall { .. }
            | Stmt::StructListMethodCall { .. }
            | Stmt::Return { .. }
            | Stmt::Print { .. }
            | Stmt::Exit { .. }
            | Stmt::BenchLoop { .. }
            | Stmt::Assert { .. }
            | Stmt::Panic { .. } => {}
        }
    }
    false
}

fn runtime_while_body_is_straight_line(stmts: &[Stmt]) -> bool {
    for stmt in stmts {
        match stmt {
            Stmt::If { .. }
            | Stmt::Block { .. }
            | Stmt::While { .. }
            | Stmt::Loop { .. }
            | Stmt::For { .. }
            | Stmt::ParFor { .. }
            | Stmt::ForEach { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. }
            | Stmt::Return { .. }
            | Stmt::Panic { .. } => return false,
            Stmt::Let { .. }
            | Stmt::Assign { .. }
            | Stmt::AssignIndex { .. }
            | Stmt::AssignStructListIndex { .. }
            | Stmt::AssignField { .. }
            | Stmt::Call { .. }
            | Stmt::MethodCall { .. }
            | Stmt::StructListMethodCall { .. }
            | Stmt::Print { .. }
            | Stmt::Exit { .. }
            | Stmt::BenchLoop { .. }
            | Stmt::Assert { .. } => {}
        }
    }
    true
}

fn collect_runtime_generic_in_place_ops<'a>(
    target_name: &str,
    expr: &'a Expr,
    ops: &mut Vec<(RuntimeBinOp, &'a Expr)>,
) -> bool {
    match expr {
        Expr::Ident { name, .. } => name == target_name,
        Expr::Unary {
            op: UnaryOp::Plus,
            expr,
            ..
        } => collect_runtime_generic_in_place_ops(target_name, expr, ops),
        Expr::Binary {
            op, left, right, ..
        } => match op {
            BinaryOp::Add => {
                let left_has = runtime_generic_expr_contains_ident(left, target_name);
                let right_has = runtime_generic_expr_contains_ident(right, target_name);
                if left_has == right_has {
                    return false;
                }
                if left_has {
                    if !collect_runtime_generic_in_place_ops(target_name, left, ops) {
                        return false;
                    }
                    ops.push((RuntimeBinOp::Add, right));
                    true
                } else {
                    if !collect_runtime_generic_in_place_ops(target_name, right, ops) {
                        return false;
                    }
                    ops.push((RuntimeBinOp::Add, left));
                    true
                }
            }
            BinaryOp::Mul => {
                let left_has = runtime_generic_expr_contains_ident(left, target_name);
                let right_has = runtime_generic_expr_contains_ident(right, target_name);
                if left_has == right_has {
                    return false;
                }
                if left_has {
                    if !collect_runtime_generic_in_place_ops(target_name, left, ops) {
                        return false;
                    }
                    ops.push((RuntimeBinOp::Mul, right));
                    true
                } else {
                    if !collect_runtime_generic_in_place_ops(target_name, right, ops) {
                        return false;
                    }
                    ops.push((RuntimeBinOp::Mul, left));
                    true
                }
            }
            BinaryOp::Sub => {
                if !runtime_generic_expr_contains_ident(left, target_name)
                    || runtime_generic_expr_contains_ident(right, target_name)
                {
                    return false;
                }
                if !collect_runtime_generic_in_place_ops(target_name, left, ops) {
                    return false;
                }
                ops.push((RuntimeBinOp::Sub, right));
                true
            }
            _ => false,
        },
        _ => false,
    }
}

fn runtime_generic_expr_contains_ident(expr: &Expr, target_name: &str) -> bool {
    match expr {
        Expr::Ident { name, .. } => name == target_name,
        Expr::Unary { expr, .. } => runtime_generic_expr_contains_ident(expr, target_name),
        Expr::Binary { left, right, .. } => {
            runtime_generic_expr_contains_ident(left, target_name)
                || runtime_generic_expr_contains_ident(right, target_name)
        }
        Expr::FieldAccess { base, .. } => runtime_generic_expr_contains_ident(base, target_name),
        Expr::Index { base, index, .. } => {
            runtime_generic_expr_contains_ident(base, target_name)
                || runtime_generic_expr_contains_ident(index, target_name)
        }
        Expr::ArrayLit { elems, .. } => elems
            .iter()
            .any(|elem| runtime_generic_expr_contains_ident(elem, target_name)),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|field| runtime_generic_expr_contains_ident(&field.expr, target_name)),
        Expr::DictLit { entries, .. } => entries
            .iter()
            .any(|entry| runtime_generic_expr_contains_ident(&entry.value, target_name)),
        Expr::MethodCall { receiver, .. } => {
            runtime_generic_expr_contains_ident(receiver, target_name)
        }
        Expr::QualifiedCall { args, .. } | Expr::EnumTupleVariant { args, .. } => args
            .iter()
            .any(|arg| runtime_generic_expr_contains_ident(arg, target_name)),
        Expr::EnumStructVariant { fields, .. } => fields
            .iter()
            .any(|field| runtime_generic_expr_contains_ident(&field.expr, target_name)),
        Expr::Match { value, arms, .. } => {
            runtime_generic_expr_contains_ident(value, target_name)
                || arms
                    .iter()
                    .any(|arm| runtime_generic_expr_contains_ident(&arm.expr, target_name))
        }
        Expr::Bool { .. }
        | Expr::String { .. }
        | Expr::Char { .. }
        | Expr::Number { .. }
        | Expr::Call { .. }
        | Expr::EnumVariant { .. } => false,
    }
}

fn runtime_generic_is_number_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Number { .. })
}

fn encode_unsigned_bits(value: u128, bits: u16, span: Span) -> RuntimeGenericLowerResult<u64> {
    let max = if bits == 64 {
        u128::from(u64::MAX)
    } else {
        (1u128 << bits) - 1
    };
    if value > max {
        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
            "integer literal out of range",
            span,
        )));
    }
    let encoded = u64::try_from(value).map_err(|_| {
        RuntimeGenericLowerError::Diagnostic(type_error("integer literal out of range", span))
    })?;
    Ok(encoded)
}

fn encode_signed_bits(value: i128, bits: u16, span: Span) -> RuntimeGenericLowerResult<u64> {
    let (min, max) = if bits == 64 {
        (i128::from(i64::MIN), i128::from(i64::MAX))
    } else {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    };
    if value < min || value > max {
        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
            "integer literal out of range",
            span,
        )));
    }

    if bits == 64 {
        let narrowed = i64::try_from(value).map_err(|_| {
            RuntimeGenericLowerError::Diagnostic(type_error("integer literal out of range", span))
        })?;
        return Ok(narrowed as u64);
    }

    let modulus = 1i128 << bits;
    let wrapped = if value < 0 { value + modulus } else { value };
    let encoded = u64::try_from(wrapped).map_err(|_| {
        RuntimeGenericLowerError::Diagnostic(type_error("integer literal out of range", span))
    })?;
    Ok(encoded)
}

fn ensure_runtime_generic_int_type(
    ty: &TypeName,
    span: Span,
) -> Result<RuntimeIntType, Diagnostic> {
    RuntimeIntType::from_type_name(ty).map_err(|_| {
        type_error(
            "runtime generic lowering supports bool/byte/integer types up to 64 bits",
            span,
        )
    })
}

fn ensure_runtime_generic_scalar_type(
    ty: &TypeName,
    span: Span,
) -> Result<RuntimeScalarType, Diagnostic> {
    match ty {
        TypeName::Float { bits } if matches!(bits, 32 | 64) => Ok(RuntimeScalarType::Float(*bits)),
        _ => ensure_runtime_generic_int_type(ty, span).map(RuntimeScalarType::Int),
    }
}

fn encode_float_bits(value: f64, bits: u16, span: Span) -> RuntimeGenericLowerResult<u64> {
    if !value.is_finite() {
        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
            "runtime generic float values must be finite",
            span,
        )));
    }
    match bits {
        32 => Ok(u64::from((value as f32).to_bits())),
        64 => Ok(value.to_bits()),
        _ => Err(RuntimeGenericLowerError::Diagnostic(type_error(
            "runtime generic lowering supports only f32 and f64",
            span,
        ))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeFallbackClass {
    CallGraph,
    UnsupportedConstruct,
    LoweringDiagnostic,
}

impl RuntimeFallbackClass {
    fn code(self) -> &'static str {
        match self {
            Self::CallGraph => "AZL001",
            Self::UnsupportedConstruct => "AZL002",
            Self::LoweringDiagnostic => "AZL003",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::CallGraph => "call-graph",
            Self::UnsupportedConstruct => "unsupported-construct",
            Self::LoweringDiagnostic => "lowering-diagnostic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeFallbackReason {
    class: RuntimeFallbackClass,
    detail: String,
    span: Span,
}

impl RuntimeFallbackReason {
    pub(super) fn new(class: RuntimeFallbackClass, detail: impl Into<String>, span: Span) -> Self {
        Self {
            class,
            detail: detail.into().replace(['\n', '\r'], " "),
            span,
        }
    }

    pub(super) fn render(&self) -> String {
        format!(
            "code={} class={} location={}:{} detail={}",
            self.class.code(),
            self.class.label(),
            self.span.line,
            self.span.column,
            self.detail
        )
    }

    fn required_diagnostic(&self) -> Diagnostic {
        Diagnostic::at_span(
            format!(
                "[{}] native execution is required but runtime lowering failed: {}",
                self.class.code(),
                self.detail
            ),
            self.span,
        )
    }
}

pub(super) enum RuntimeGenericAttempt {
    Lowered(Vec<LoweredStmt>),
    Fallback { reason: RuntimeFallbackReason },
}

fn runtime_stmts_guarantee_terminal(stmts: &[Stmt]) -> bool {
    let Some(last) = stmts.last() else {
        return false;
    };
    match last {
        Stmt::Exit { .. } | Stmt::Return { .. } | Stmt::Panic { .. } => true,
        Stmt::Block { stmts, .. } => runtime_stmts_guarantee_terminal(stmts),
        Stmt::If {
            then_branch,
            else_branch: Some(else_branch),
            ..
        } => {
            runtime_stmts_guarantee_terminal(then_branch)
                && runtime_stmts_guarantee_terminal(else_branch)
        }
        _ => false,
    }
}

pub(super) fn try_lower_runtime_generic(
    main: &Function,
    functions: &HashMap<String, Function>,
    struct_layouts: &HashMap<String, Vec<LayoutField>>,
    enums: &HashMap<String, EnumDef>,
) -> Result<RuntimeGenericAttempt, Diagnostic> {
    let runtime_required = function_contains_runtime_seed(main, functions);

    if let Err(diag) = validate_runtime_generic_call_graph(main, functions) {
        if runtime_required {
            return Err(diag);
        }
        return Ok(RuntimeGenericAttempt::Fallback {
            reason: RuntimeFallbackReason::new(
                RuntimeFallbackClass::CallGraph,
                diag.message,
                Span::in_source(diag.line, diag.column, diag.source_id),
            ),
        });
    }

    let mut builder = RuntimeGenericBuilder::new(functions, struct_layouts, enums);
    match builder
        .lower_function(&main.name, true)
        .and_then(|_| builder.lower_queued_functions())
        .and_then(|_| builder.patch_calls())
    {
        Ok(()) => Ok(RuntimeGenericAttempt::Lowered(vec![
            LoweredStmt::RuntimeGeneric {
                program: RuntimeProgram {
                    slots: builder.slots,
                    instrs: builder.instrs,
                },
            },
        ])),
        Err(RuntimeGenericLowerError::Unsupported) => {
            let reason = RuntimeFallbackReason::new(
                RuntimeFallbackClass::UnsupportedConstruct,
                format!(
                    "unsupported construct reachable from function '{}'",
                    main.name
                ),
                main.span,
            );
            if runtime_required {
                Err(reason.required_diagnostic())
            } else {
                Ok(RuntimeGenericAttempt::Fallback { reason })
            }
        }
        Err(RuntimeGenericLowerError::Diagnostic(diag)) => {
            if runtime_required {
                Err(diag)
            } else {
                Ok(RuntimeGenericAttempt::Fallback {
                    reason: RuntimeFallbackReason::new(
                        RuntimeFallbackClass::LoweringDiagnostic,
                        diag.message,
                        Span::in_source(diag.line, diag.column, diag.source_id),
                    ),
                })
            }
        }
    }
}

pub(super) fn maybe_report_runtime_generic_fallback(
    main: &Function,
    reason: &RuntimeFallbackReason,
) {
    let report_enabled = std::env::var("AZIKY_RUNTIME_FALLBACK_REPORT")
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            matches!(value.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false);
    if !report_enabled {
        return;
    }
    eprintln!(
        "runtime_generic_fallback function={} reason={}",
        main.name,
        reason.render()
    );
}

fn validate_runtime_generic_call_graph(
    main: &Function,
    functions: &HashMap<String, Function>,
) -> Result<(), Diagnostic> {
    const MAX_CALL_DEPTH: usize = 64;
    let mut visiting = Vec::new();
    let mut visited = HashSet::new();
    validate_runtime_generic_function_calls(
        &main.name,
        main.span,
        functions,
        &mut visiting,
        &mut visited,
        MAX_CALL_DEPTH,
    )
}

fn validate_runtime_generic_function_calls(
    name: &str,
    span: Span,
    functions: &HashMap<String, Function>,
    visiting: &mut Vec<String>,
    visited: &mut HashSet<String>,
    max_depth: usize,
) -> Result<(), Diagnostic> {
    if visited.contains(name) {
        return Ok(());
    }
    if visiting.iter().any(|current| current == name) {
        return Err(type_error(
            format!(
                "runtime generic lowering does not support recursive call '{}'",
                source_callable_name(name)
            )
            .as_str(),
            span,
        ));
    }
    if visiting.len() >= max_depth {
        return Err(type_error("runtime generic call depth exceeded", span));
    }
    let function = functions
        .get(name)
        .ok_or_else(|| unknown_function_diagnostic(name, span))?;

    visiting.push(name.to_string());
    for stmt in &function.body {
        validate_runtime_generic_stmt_calls(stmt, functions, visiting, visited, max_depth)?;
    }
    visiting.pop();
    visited.insert(name.to_string());
    Ok(())
}

fn validate_runtime_generic_stmt_calls(
    stmt: &Stmt,
    functions: &HashMap<String, Function>,
    visiting: &mut Vec<String>,
    visited: &mut HashSet<String>,
    max_depth: usize,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Call { name, span, args } => {
            for arg in args {
                validate_runtime_generic_expr_calls(arg, functions, visiting, visited, max_depth)?;
            }
            if let Some(diagnostic) = removed_benchmark_kernel_diagnostic(name, *span) {
                return Err(diagnostic);
            }
            if name == "runtime_seed"
                || name == "heap_alloc"
                || name == "heap_free"
                || is_file_runtime_intrinsic(name)
            {
                return Ok(());
            }
            validate_runtime_generic_function_calls(
                name, *span, functions, visiting, visited, max_depth,
            )
        }
        Stmt::Return { expr, .. } => {
            if let Some(expr) = expr {
                validate_runtime_generic_expr_calls(expr, functions, visiting, visited, max_depth)?;
            }
            Ok(())
        }
        Stmt::Block { stmts, .. } => {
            for stmt in stmts {
                validate_runtime_generic_stmt_calls(stmt, functions, visiting, visited, max_depth)?;
            }
            Ok(())
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for stmt in then_branch {
                validate_runtime_generic_stmt_calls(stmt, functions, visiting, visited, max_depth)?;
            }
            if let Some(else_branch) = else_branch {
                for stmt in else_branch {
                    validate_runtime_generic_stmt_calls(
                        stmt, functions, visiting, visited, max_depth,
                    )?;
                }
            }
            Ok(())
        }
        Stmt::While { body, .. }
        | Stmt::Loop { body, .. }
        | Stmt::For { body, .. }
        | Stmt::ParFor { body, .. }
        | Stmt::ForEach { body, .. } => {
            for stmt in body {
                validate_runtime_generic_stmt_calls(stmt, functions, visiting, visited, max_depth)?;
            }
            Ok(())
        }
        Stmt::Let { expr, .. } => {
            validate_runtime_generic_expr_calls(expr, functions, visiting, visited, max_depth)
        }
        Stmt::Assign { expr, .. } | Stmt::AssignField { expr, .. } => {
            validate_runtime_generic_expr_calls(expr, functions, visiting, visited, max_depth)
        }
        Stmt::AssignIndex { index, expr, .. } | Stmt::AssignStructListIndex { index, expr, .. } => {
            validate_runtime_generic_expr_calls(index, functions, visiting, visited, max_depth)?;
            validate_runtime_generic_expr_calls(expr, functions, visiting, visited, max_depth)
        }
        Stmt::MethodCall { args, .. } | Stmt::StructListMethodCall { args, .. } => {
            for arg in args {
                validate_runtime_generic_expr_calls(arg, functions, visiting, visited, max_depth)?;
            }
            Ok(())
        }
        Stmt::Print { expr, .. } | Stmt::Exit { expr, .. } => {
            validate_runtime_generic_expr_calls(expr, functions, visiting, visited, max_depth)
        }
        Stmt::BenchLoop { iterations, .. } => {
            validate_runtime_generic_expr_calls(iterations, functions, visiting, visited, max_depth)
        }
        Stmt::Assert { cond, message, .. } => {
            validate_runtime_generic_expr_calls(cond, functions, visiting, visited, max_depth)?;
            if let Some(message) = message {
                validate_runtime_generic_expr_calls(
                    message, functions, visiting, visited, max_depth,
                )?;
            }
            Ok(())
        }
        Stmt::Panic { message, .. } => {
            validate_runtime_generic_expr_calls(message, functions, visiting, visited, max_depth)
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => Ok(()),
    }
}

fn validate_runtime_generic_expr_calls(
    expr: &Expr,
    functions: &HashMap<String, Function>,
    visiting: &mut Vec<String>,
    visited: &mut HashSet<String>,
    max_depth: usize,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Call { name, span, args } => {
            for arg in args {
                validate_runtime_generic_expr_calls(arg, functions, visiting, visited, max_depth)?;
            }
            if let Some(diagnostic) = removed_benchmark_kernel_diagnostic(name, *span) {
                return Err(diagnostic);
            }
            if name == "runtime_seed"
                || name == "heap_alloc"
                || name == "heap_free"
                || is_file_runtime_intrinsic(name)
            {
                Ok(())
            } else {
                validate_runtime_generic_function_calls(
                    name, *span, functions, visiting, visited, max_depth,
                )
            }
        }
        Expr::Unary { expr, .. } => {
            validate_runtime_generic_expr_calls(expr, functions, visiting, visited, max_depth)
        }
        Expr::Binary { left, right, .. } => {
            validate_runtime_generic_expr_calls(left, functions, visiting, visited, max_depth)?;
            validate_runtime_generic_expr_calls(right, functions, visiting, visited, max_depth)
        }
        Expr::FieldAccess { base, .. } => {
            validate_runtime_generic_expr_calls(base, functions, visiting, visited, max_depth)
        }
        Expr::Index { base, index, .. } => {
            validate_runtime_generic_expr_calls(base, functions, visiting, visited, max_depth)?;
            validate_runtime_generic_expr_calls(index, functions, visiting, visited, max_depth)
        }
        Expr::ArrayLit { elems, .. } => {
            for elem in elems {
                validate_runtime_generic_expr_calls(elem, functions, visiting, visited, max_depth)?;
            }
            Ok(())
        }
        Expr::StructInit { fields, .. } => {
            for field in fields {
                validate_runtime_generic_expr_calls(
                    &field.expr,
                    functions,
                    visiting,
                    visited,
                    max_depth,
                )?;
            }
            Ok(())
        }
        Expr::DictLit { entries, .. } => {
            for entry in entries {
                validate_runtime_generic_expr_calls(
                    &entry.value,
                    functions,
                    visiting,
                    visited,
                    max_depth,
                )?;
            }
            Ok(())
        }
        Expr::MethodCall { receiver, .. } => {
            validate_runtime_generic_expr_calls(receiver, functions, visiting, visited, max_depth)
        }
        Expr::QualifiedCall { args, .. } | Expr::EnumTupleVariant { args, .. } => {
            for arg in args {
                validate_runtime_generic_expr_calls(arg, functions, visiting, visited, max_depth)?;
            }
            Ok(())
        }
        Expr::EnumStructVariant { fields, .. } => {
            for field in fields {
                validate_runtime_generic_expr_calls(
                    &field.expr,
                    functions,
                    visiting,
                    visited,
                    max_depth,
                )?;
            }
            Ok(())
        }
        Expr::Match { value, arms, .. } => {
            validate_runtime_generic_expr_calls(value, functions, visiting, visited, max_depth)?;
            for arm in arms {
                validate_runtime_generic_expr_calls(
                    &arm.expr, functions, visiting, visited, max_depth,
                )?;
            }
            Ok(())
        }
        Expr::Bool { .. }
        | Expr::String { .. }
        | Expr::Char { .. }
        | Expr::Number { .. }
        | Expr::Ident { .. }
        | Expr::EnumVariant { .. } => Ok(()),
    }
}

fn function_contains_runtime_seed(main: &Function, functions: &HashMap<String, Function>) -> bool {
    let mut visited = HashSet::new();
    visited.insert(main.name.clone());
    main.body
        .iter()
        .any(|stmt| stmt_contains_runtime_seed(stmt, functions, &mut visited))
}

fn stmt_contains_runtime_seed(
    stmt: &Stmt,
    functions: &HashMap<String, Function>,
    visited: &mut HashSet<String>,
) -> bool {
    match stmt {
        Stmt::Let { expr, .. } => expr_contains_runtime_seed(expr),
        Stmt::Assign { expr, .. } => expr_contains_runtime_seed(expr),
        Stmt::AssignField { expr, .. } => expr_contains_runtime_seed(expr),
        Stmt::AssignIndex { index, expr, .. } | Stmt::AssignStructListIndex { index, expr, .. } => {
            expr_contains_runtime_seed(index) || expr_contains_runtime_seed(expr)
        }
        Stmt::MethodCall { args, .. } | Stmt::StructListMethodCall { args, .. } => {
            args.iter().any(expr_contains_runtime_seed)
        }
        Stmt::Return { expr, .. } => expr
            .as_ref()
            .map(expr_contains_runtime_seed)
            .unwrap_or(false),
        Stmt::Print { expr, .. } => expr_contains_runtime_seed(expr),
        Stmt::Exit { expr, .. } => expr_contains_runtime_seed(expr),
        Stmt::BenchLoop { iterations, .. } => expr_contains_runtime_seed(iterations),
        Stmt::Block { stmts, .. } => stmts
            .iter()
            .any(|stmt| stmt_contains_runtime_seed(stmt, functions, visited)),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            expr_contains_runtime_seed(cond)
                || then_branch
                    .iter()
                    .any(|stmt| stmt_contains_runtime_seed(stmt, functions, visited))
                || else_branch
                    .as_ref()
                    .map(|branch| {
                        branch
                            .iter()
                            .any(|stmt| stmt_contains_runtime_seed(stmt, functions, visited))
                    })
                    .unwrap_or(false)
        }
        Stmt::While { cond, body, .. } => {
            expr_contains_runtime_seed(cond)
                || body
                    .iter()
                    .any(|stmt| stmt_contains_runtime_seed(stmt, functions, visited))
        }
        Stmt::Loop { body, .. } => body
            .iter()
            .any(|stmt| stmt_contains_runtime_seed(stmt, functions, visited)),
        Stmt::For {
            start, end, body, ..
        }
        | Stmt::ParFor {
            start, end, body, ..
        } => {
            expr_contains_runtime_seed(start)
                || expr_contains_runtime_seed(end)
                || body
                    .iter()
                    .any(|stmt| stmt_contains_runtime_seed(stmt, functions, visited))
        }
        Stmt::ForEach { iterable, body, .. } => {
            expr_contains_runtime_seed(iterable)
                || body
                    .iter()
                    .any(|stmt| stmt_contains_runtime_seed(stmt, functions, visited))
        }
        Stmt::Assert { cond, message, .. } => {
            expr_contains_runtime_seed(cond)
                || message
                    .as_ref()
                    .map(expr_contains_runtime_seed)
                    .unwrap_or(false)
        }
        Stmt::Panic { message, .. } => expr_contains_runtime_seed(message),
        Stmt::Call { name, args, .. } => {
            if args.iter().any(expr_contains_runtime_seed) {
                return true;
            }
            if name == "runtime_seed"
                || name == "heap_alloc"
                || name == "heap_free"
                || is_file_runtime_intrinsic(name)
                || name == "runtime_bloom_sbbf_insert"
                || name == "runtime_bloom_sbbf_maybe"
                || name == "runtime_hash_probe_grouped16"
                || name == "runtime_join_select_adaptive"
            {
                return true;
            }
            if !visited.insert(name.clone()) {
                return false;
            }
            let contains = functions
                .get(name)
                .map(|func| {
                    func.body
                        .iter()
                        .any(|stmt| stmt_contains_runtime_seed(stmt, functions, visited))
                })
                .unwrap_or(false);
            visited.remove(name);
            contains
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => false,
    }
}

fn expr_contains_runtime_seed(expr: &Expr) -> bool {
    match expr {
        Expr::Call { name, .. } => {
            name == "runtime_seed"
                || name == "heap_alloc"
                || name == "heap_free"
                || is_file_runtime_intrinsic(name)
                || name == "runtime_bloom_sbbf_insert"
                || name == "runtime_bloom_sbbf_maybe"
                || name == "runtime_hash_probe_grouped16"
                || name == "runtime_join_select_adaptive"
        }
        Expr::Unary { expr, .. } => expr_contains_runtime_seed(expr),
        Expr::Binary { left, right, .. } => {
            expr_contains_runtime_seed(left) || expr_contains_runtime_seed(right)
        }
        Expr::FieldAccess { base, .. } => expr_contains_runtime_seed(base),
        Expr::Index { base, index, .. } => {
            expr_contains_runtime_seed(base) || expr_contains_runtime_seed(index)
        }
        Expr::ArrayLit { elems, .. } => elems.iter().any(expr_contains_runtime_seed),
        Expr::StructInit { fields, .. } => fields
            .iter()
            .any(|field| expr_contains_runtime_seed(&field.expr)),
        Expr::DictLit { entries, .. } => entries
            .iter()
            .any(|entry| expr_contains_runtime_seed(&entry.value)),
        Expr::MethodCall { receiver, .. } => expr_contains_runtime_seed(receiver),
        Expr::QualifiedCall { args, .. } | Expr::EnumTupleVariant { args, .. } => {
            args.iter().any(expr_contains_runtime_seed)
        }
        Expr::EnumStructVariant { fields, .. } => fields
            .iter()
            .any(|field| expr_contains_runtime_seed(&field.expr)),
        Expr::Match { value, arms, .. } => {
            expr_contains_runtime_seed(value)
                || arms.iter().any(|arm| expr_contains_runtime_seed(&arm.expr))
        }
        Expr::Bool { .. }
        | Expr::String { .. }
        | Expr::Char { .. }
        | Expr::Number { .. }
        | Expr::Ident { .. }
        | Expr::EnumVariant { .. } => false,
    }
}
