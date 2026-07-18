# The Aziky Programming Language
## Complete Language Reference Manual

**Version 1.0**  
**The Definitive Source of Truth for Developers**

---

# Table of Contents

1. [Chapter 1: Introduction and Overview](#chapter-1-introduction-and-overview)
2. [Chapter 2: Lexical Structure](#chapter-2-lexical-structure)
3. [Chapter 3: Types and Values](#chapter-3-types-and-values)
4. [Chapter 4: Expressions](#chapter-4-expressions)
5. [Chapter 5: Statements and Control Flow](#chapter-5-statements-and-control-flow)
6. [Chapter 6: Functions](#chapter-6-functions)
7. [Chapter 7: Structs and Data Types](#chapter-7-structs-and-data-types)
8. [Chapter 8: Enums](#chapter-8-enums)
9. [Chapter 9: Traits and Polymorphism](#chapter-9-traits-and-polymorphism)
10. [Chapter 10: Arrays and Dictionaries](#chapter-10-arrays-and-dictionaries)
11. [Chapter 11: Memory Model and Ownership](#chapter-11-memory-model-and-ownership)
12. [Chapter 12: Parallel Constructs](#chapter-12-parallel-constructs)
13. [Appendix A: Complete Grammar Reference](#appendix-a-complete-grammar-reference)
14. [Appendix B: Keyword Reference](#appendix-b-keyword-reference)
15. [Appendix C: Operator Precedence Table](#appendix-c-operator-precedence-table)
16. [Appendix D: Built-in Methods Reference](#appendix-d-built-in-methods-reference)
17. [Appendix E: Error Diagnostics](#appendix-e-error-diagnostics)

---

# Chapter 1: Introduction and Overview

## 1.1 What is Aziky?

Aziky is a **deterministic, memory-safe, zero-dependency systems programming language** designed for high-performance computing. Its compiler emits x86-64 machine code and executable containers directly, without an external assembler or linker. Bit-for-bit reproducibility is a required compiler invariant for identical source and explicit target options.

This manual distinguishes implemented behavior from planned end-state behavior. A feature described as planned is not part of the accepted language surface yet; runtime fallback notes identify constructs that are semantically supported but not yet lowered through the general runtime IR.

### 1.1.1 Design Philosophy

Aziky's design is guided by these core principles:

1. **Determinism**: Same source + same target triple → bit-for-bit identical binary
2. **Zero Dependencies**: No external crates for parsing, codegen, or binary packaging
3. **Memory Safety**: Rust-based compiler implementation with lexical borrow tracking; complete move and linear-resource enforcement remains in progress
4. **Static Dispatch**: All polymorphism resolves at compile time; no vtables
5. **Offline Build**: Fully offline compilation path

### 1.1.2 Key Features

- **End-to-End Compilation**: Translates source directly to x86_64 machine code
- **Embedded Structs**: Physical field flattening for deterministic layout
- **Value-Oriented Methods**: Inherent `impl Type` blocks with associated functions
  and explicit shared/mutable receivers
- **Typed Sum Types**: Generic payload enums, exhaustive `match`, and built-in
  `Option<T>`/`Result<T, E>` failure values
- **Multi-File Programs**: Deterministic `mod` loading and validated selective
  `use` declarations
- **Traits**: Static-dispatch only trait system with monomorphization
- **Parallel Loops**: Deterministic `parfor` with isolated environments
- **Planned Inline Assembly**: A constrained escape hatch with explicit clobber lists is specified but not yet parsed or lowered

## 1.2 Program Structure

An Aziky program consists of zero or more **items** at the top level:

```
item ::= function-def
       | struct-def
       | enum-def
       | trait-def
       | trait-impl
       | inherent-impl
       | module-decl
       | use-decl
       | 'pub' (function-def | struct-def | enum-def | trait-def | use-decl)
```

Every Aziky program must have a `main` function as the entry point:

```aziky
fn main() {
    // program body
    exit(0);
}
```

## 1.3 A Simple Example

```aziky
struct Point {
    x: i32;
    y: i32;
}

fn main() {
    let greeting: string = "Hello, Aziky!\n";
    let p: Point = Point { x: 10i32, y: 20i32 };
    
    print(greeting);
    print(p.x);
    print("\n");
    
    exit(0);
}
```

## 1.4 Compilation Model

Aziky compiles source code directly to x86_64 machine code. The compiler handles all stages of compilation internally, producing a standalone executable binary.

The implementation currently has three lowering outcomes: specialized runtime kernels, a general slot-based runtime IR, and deterministic semantic evaluation used as a fallback for supported constructs that are not runtime-native yet. The long-term goal is for ordinary application code to use the general runtime path without fallback.

## 1.5 Multi-File Modules

A root source file declares direct child modules and imports public items into
its lexical module scope:

```aziky
mod math;
use math::add;

fn main() {
    print(add(2i32, 3i32));
    exit(0u64);
}
```

`mod math;` resolves exactly one of `math.azk` or `math/mod.azk` beside the
declaring file. Declared modules are loaded recursively in source declaration
order. The loader rejects missing or ambiguous module files, duplicate module
declarations, module cycles, imports of absent items, and `main` functions in
non-root modules.

Declarations are private by default. `pub` exports a top-level function,
struct, enum, or trait. `use module::item;` makes one public direct-child item
available under its short name, while `use module::item as alias;` chooses a
local alias. `pub use` re-exports an imported item:

```aziky
// math.azk
fn implementation_detail() -> i32 { return 40i32; }
pub fn answer() -> i32 { return implementation_detail() + 2i32; }

// facade.azk
mod math;
pub use math::answer as final_answer;
```

Each module receives a deterministic qualified namespace. Private declarations
with the same short name can coexist in different modules; unimported exports
and private items are not visible to the parent. Import collisions are rejected
instead of being resolved by load order. `pub` is intentionally invalid on
`impl`: methods follow their statically resolved type/trait in this baseline.
Public module namespaces (`pub mod`) and glob imports are not implemented;
re-export explicit items with `pub use`.

Module discovery, parsing, cycle, and import diagnostics identify the relevant
source path. Per-file source provenance for semantic diagnostics after the
module ASTs are combined remains unfinished; those diagnostics can still be
anchored to the root source display.

## 1.6 Packages

An `Aziky.toml` manifest can map a declared module name to an exact-versioned,
SHA-256-pinned package in an offline cache. `Aziky.lock` records the complete
sorted graph and selected features without absolute host paths. Compilation and
checking require the lock to match; they never update it or fetch dependencies
implicitly. Local-module/package alias collisions, version or checksum
conflicts, dependency cycles, missing cache entries, and tampering are errors.

The full manifest grammar, cache layout, feature rules, commands, and
portability guarantees are specified in `AZIKY_PACKAGES.md`.

## 1.7 Embedded User-Language Standard Foundation

If a project declares `mod std;` and has no sibling `std.azk` or `std/mod.azk`,
the compiler loads its embedded `std`, `core`, and `alloc` Aziky sources. They
are compiled through the same lexer, parser, namespace resolver, type checker,
and lowering pipeline as project code, so the offline compiler has no host
standard-library lookup dependency.

```aziky
mod std;
use std::AzikyVersion;
use std::version;
use std::stdlib_abi_version;
use std::Ordering;
use std::ParseError;
use std::compare_i64;
use std::checked_i64;
use std::new_i32_list;
```

The current foundation exports `AzikyVersion`, `version`, the checked intrinsic
ABI version, `Ordering` with signed/unsigned comparison, checked character and
all primitive text-conversion wrappers returning `Result<T, ParseError>`, typed constructors for common list/map
shapes, and the embedded `std::fs` file facade (`create`, `open_read`,
`write_all`, `read`, and `close`). The filesystem facade is lowered through the
target's native `File` ownership ABI, with no host library fallback.
The embedded facade also exports owned access to the Linux startup argument and
environment snapshots, monotonic and wall-clock nanoseconds, process identity,
and explicit termination. Argument/environment values are host-dependent but
copied into Aziky-owned strings; their captured snapshots are immutable.
Monotonic time is for durations only, while wall time is Unix-epoch time and may
be adjusted by the host. `Path::join` provides lexical owned path composition,
while the embedded path module exposes separator queries. Child-process
ownership/spawning, allocator-backed storage, and broader generic collection APIs remain subsequent
runtime work. A project-local `std` module takes precedence when explicitly
supplied beside the declaring source file.

## 1.8 Developer Commands

The compiler includes deterministic `test`, `fmt`, and `lint` commands. Tests
are standalone source programs with zero/nonzero exit semantics and integrate
with package lockfiles and offline dependency resolution. Formatting has write,
check-only, and stdout modes. Lint diagnostics have stable codes and sorted
path/line/column output, with an explicit deny-warnings CI mode.

See `AZIKY_DEVELOPER_COMMANDS.md` for discovery, timeout, formatting, lint-rule,
portability, and benchmark-status contracts.

## 1.9 Object and Library Artifacts

`compile --emit` selects executable, relocatable object, deterministic static
archive, or shared-library output. ELF64 supports all four on the accepted
Linux x86-64 target; Mach-O64 supports its existing executable scaffold plus
relocatable objects and static archives. Darwin shared output is rejected until
its runtime/loader target is accepted.

Object and library artifacts contain exact entry/block symbols, logical source
paths, DWARF v4 source tables, and versioned declaration line/column provenance.
See `AZIKY_ARTIFACTS_AND_DEBUG.md` for the whole-program entry ABI, format
matrix, archive rules, reproducibility guarantees, and debug limitations.


# Chapter 2: Lexical Structure

## 2.1 Character Set

Aziky source files must be encoded in **UTF-8**. The lexer operates on Unicode code points but restricts identifiers and most tokens to the ASCII subset.

### 2.1.1 Whitespace

The following characters are recognized as whitespace and are ignored except as token separators:

| Character | ASCII | Description |
|-----------|-------|-------------|
| Space | 0x20 | ASCII space |
| Tab | 0x09 | Horizontal tab |
| Carriage Return | 0x0D | CR |
| Newline | 0x0A | LF |

**Important**: Newlines are significant for tracking line numbers in diagnostics but do not affect parsing semantics (Aziky is not indentation-sensitive).

### 2.1.2 Comments

Aziky supports two comment forms:

1. Line comments: `// ...` (until newline)
2. Block comments: `/* ... */` (nesting supported)

Comments are ignored by the parser and do not affect program semantics.

## 2.2 Keywords

Keywords are reserved identifiers with special meaning. They cannot be used as user-defined identifiers.

### 2.2.1 Complete Keyword List

| Keyword | Purpose |
|---------|---------|
| `fn` | Function definition |
| `struct` | Struct type definition |
| `enum` | Enum type definition |
| `trait` | Trait definition |
| `impl` | Inherent or trait implementation |
| `embed` | Embedded struct field |
| `pub` | Export a top-level declaration or selective import |
| `mod` | Declare a child module |
| `use` | Import a public child item |
| `as` | Choose a local import or re-export alias |
| `let` | Variable binding |
| `mut` | Mutable binding/parameter |
| `if` | Conditional branch |
| `else` | Alternative branch |
| `match` | Exhaustive enum selection |
| `while` | Conditional loop |
| `loop` | Infinite loop |
| `for` | Range-based loop |
| `parfor` | Parallel for loop |
| `foreach` | Iterator-based loop |
| `in` | Range/iterator membership |
| `break` | Loop exit |
| `continue` | Loop iteration skip |
| `return` | Function return |
| `assert` | Compile-time assertion |
| `panic` | Runtime error trigger |
| `true` | Boolean literal |
| `false` | Boolean literal |
| `print` | Standard output |
| `exit` | Program termination |
| `benchloop` | Benchmark loop construct |

## 2.3 Identifiers

Identifiers name variables, functions, types, and other user-defined entities.

### 2.3.1 Identifier Syntax

```
identifier ::= identifier-start identifier-continue*
identifier-start ::= 'a'..'z' | 'A'..'Z' | '_'
identifier-continue ::= identifier-start | '0'..'9'
```

### 2.3.2 Identifier Rules

1. **First character**: Must be an ASCII letter (`a`-`z`, `A`-`Z`) or underscore (`_`)
2. **Subsequent characters**: May be ASCII letters, digits, or underscores
3. **Length**: No explicit limit enforced by the lexer
4. **Case sensitivity**: Identifiers are case-sensitive (`foo` and `Foo` are distinct)
5. **Reserved words**: Keywords cannot be used as identifiers

### 2.3.3 Identifier Examples

**Valid identifiers**:
```
x
my_variable
MyStruct
_private
point2D
CONSTANT_VALUE
```

**Invalid identifiers**:
```
2d_point      // starts with digit
my-var        // contains hyphen
let           // keyword
```

## 2.4 Literals

### 2.4.1 Integer Literals

Integer literals are sequences of ASCII digits optionally followed by a type suffix.

```
integer-literal ::= digits type-suffix?
digits ::= '0'..'9'+
type-suffix ::= 'u8' | 'u16' | 'u32' | 'u64' | 'u128' | 'usize'
              | 'i8' | 'i16' | 'i32' | 'i64' | 'i128' | 'isize'
              | 'byte' | 'bool'
```

**Rules**:
1. No leading zeros restriction (except `0` itself)
2. The suffix determines the type and valid range
3. Without suffix, integer literals default to `i64` if they fit, otherwise error

**Examples**:
```aziky
42              // defaults to i64
255u8           // u8 type
1000u16         // u16 type
-100i32         // signed i32
0bool           // false
1bool           // true
```

### 2.4.2 Floating-Point Literals

Floating-point literals contain a decimal point and may have a type suffix.

```
float-literal ::= digits '.' digits? type-suffix?
               | '.' digits type-suffix?
type-suffix ::= 'f32' | 'f64'
```

**Rules**:
1. A decimal point `.` followed by digits creates a float literal
2. Without suffix, floating-point literals default to `f64`
3. The lexer distinguishes `.` (dot operator) from `.` in floats via lookahead

**Examples**:
```aziky
3.14159         // f64
2.0f32          // f32
0.5             // f64
```

### 2.4.3 String Literals

String literals are double-quote-delimited sequences of characters with escape sequences.

```
string-literal ::= '"' string-content* '"'
string-content ::= escape-sequence | regular-character
escape-sequence ::= '\n' | '\t' | '\"' | '\\'
```

**Supported Escape Sequences**:

| Escape | Meaning | ASCII |
|--------|---------|-------|
| `\n` | Newline | 0x0A |
| `\t` | Tab | 0x09 |
| `\"` | Double quote | 0x22 |
| `\\` | Backslash | 0x5C |

**Rules**:
1. String literals must be terminated on the same line (no multiline strings)
2. Any other escape sequence results in a compile error
3. Unterminated strings result in a compile error

**Examples**:
```aziky
"Hello, World!"
"Line 1\nLine 2"
"Tab\there"
"Quote: \" and backslash: \\"
```

### 2.4.4 Character Literals

Character literals contain exactly one Unicode scalar value and use single quotes.

```
char-literal ::= "'" (unicode-scalar | char-escape) "'"
char-escape ::= '\\n' | '\\r' | '\\t' | '\\0' | "\\'" | '\\"' | '\\\\'
```

```aziky
let ascii: char = 'A';
let greek: char = 'λ';
let newline: char = '\n';
let quote: char = '\'';
```

Empty literals, multiple scalar values such as `'ab'`, unknown escapes, and
unterminated literals are compile errors. A `char` is not an integer; use the
explicit `.to_u32()` conversion when the Unicode scalar number is required.

### 2.4.5 Boolean Literals

Boolean literals are the keywords `true` and `false`.

```
boolean-literal ::= 'true' | 'false'
```

**Type**: Boolean literals have type `bool`.

## 2.5 Operators and Punctuation

### 2.5.1 Single-Character Tokens

| Token | Meaning |
|-------|---------|
| `(` | Left parenthesis |
| `)` | Right parenthesis |
| `{` | Left brace |
| `}` | Right brace |
| `[` | Left bracket |
| `]` | Right bracket |
| `;` | Semicolon |
| `:` | Colon |
| `,` | Comma |
| `.` | Dot |
| `=` | Equal (assignment) |
| `+` | Plus |
| `-` | Minus |
| `*` | Star (multiply/dereference) |
| `/` | Slash (divide) |
| `%` | Percent (modulo) |
| `&` | Ampersand (bitwise AND / borrow) |
| `|` | Pipe (bitwise OR) |
| `^` | Caret (bitwise XOR) |
| `!` | Bang (logical NOT) |
| `<` | Less than |
| `>` | Greater than |

### 2.5.2 Multi-Character Tokens

| Token | Meaning | Components |
|-------|---------|------------|
| `::` | Path separator | Colon Colon |
| `..` | Range operator | Dot Dot |
| `==` | Equality | Equal Equal |
| `!=` | Inequality | Bang Equal |
| `<=` | Less or equal | Less Equal |
| `>=` | Greater or equal | Greater Equal |
| `&&` | Logical AND | Ampersand Ampersand |
| `\|\|` | Logical OR | Pipe Pipe |
| `<<` | Left shift | Less Less |
| `>>` | Right shift | Greater Greater |

### 2.5.3 Tokenization Rules for Ambiguous Sequences

The lexer uses **maximal munch** - it always takes the longest valid token:

1. `::=` → `::` + `=`, not `:` + `:` + `=`
2. `...` → `..` + `.`, not three dots as a single token
3. `==` → `==`, not `=` + `=`
4. `<<=` → `<<` + `=`, not left-shift-assign (no such operator)

## 2.6 Lexical Errors

The lexer produces errors for:

1. **Invalid characters**: Any character not matching a valid token pattern
2. **Unterminated strings**: String literal not closed before newline or EOF
3. **Invalid escape sequences**: `\x` where `x` is not `n`, `t`, `"`, or `\`

### 2.6.1 Error Format

```
error: <message> at <line>:<column>
```

**Example**:
```
error: unexpected character: @ at 5:12
```

---

# Chapter 3: Types and Values

## 3.1 Type System Overview

Aziky uses a **static type system** with explicit type annotations and type inference. All type checking occurs at compile time.

### 3.1.1 Type Categories

1. **Primitive Types**: Integers, floats, booleans, bytes, Unicode scalar
   characters, strings
2. **Composite Types**: Structs, enums, arrays, dictionaries
3. **Reference Types**: Shared and mutable references
4. **Opaque Resource Types**: Compiler-managed linear platform owners such as
   `File`

## 3.2 Primitive Types

### 3.2.1 Integer Types

Aziky provides signed and unsigned integers in multiple widths:

**Unsigned Integers**:

| Type | Minimum | Maximum | Bits |
|------|---------|---------|------|
| `u8` | 0 | 255 | 8 |
| `u16` | 0 | 65,535 | 16 |
| `u32` | 0 | 4,294,967,295 | 32 |
| `u64` | 0 | 18,446,744,073,709,551,615 | 64 |
| `u128` | 0 | 2^128 - 1 | 128 |
| `usize` | 0 | Platform-dependent (64-bit) | 64 |

**Signed Integers**:

| Type | Minimum | Maximum | Bits |
|------|---------|---------|------|
| `i8` | -128 | 127 | 8 |
| `i16` | -32,768 | 32,767 | 16 |
| `i32` | -2,147,483,648 | 2,147,483,647 | 32 |
| `i64` | -9,223,372,036,854,775,808 | 9,223,372,036,854,775,807 | 64 |
| `i128` | -2^127 | 2^127 - 1 | 128 |
| `isize` | Platform-dependent | Platform-dependent | 64 |

**Rules**:
1. Integer types do **not** implicitly convert between widths
2. Overflow/underflow is **checked at compile time** for constant expressions
3. Runtime overflow behavior is **wrapping** for the runtime-generic lowering path

### 3.2.2 Floating-Point Types

| Type | Precision | Bits |
|------|-----------|------|
| `f32` | Single precision (IEEE 754) | 32 |
| `f64` | Double precision (IEEE 754) | 64 |

**Rules**:
1. Floats must be finite for hashing and certain operations
2. NaN comparison results in compile-time error
3. Division by zero produces a compile-time error for constants

### 3.2.3 Boolean Type

The `bool` type represents a truth value with two possible values: `true` and `false`.

**Size**: 1 byte (but occupies 8 bits for alignment purposes in some contexts)

### 3.2.4 Byte Type

The `byte` type is an alias for an 8-bit unsigned integer (`u8`) with semantic intent for raw data.

### 3.2.5 Character Type

The `char` type represents exactly one Unicode scalar value. It is distinct from
`u32`: arithmetic is not available on characters, while equality, scalar-value
ordering, deterministic hashing, printing, arrays, and dictionary keys are.

`char.to_str()` encodes the scalar as UTF-8. `char.to_u32()` exposes its scalar
number explicitly. An integer's `to_char_checked()` returns `Option<char>` and
rejects values outside the Unicode scalar range; there is intentionally no
unchecked implicit integer-to-character conversion.

Current status: specified, parsed, and semantically executable. General
runtime-IR representation is not implemented, so programs using `char` use the
deterministic semantic path instead of silently lowering it as an integer.

### 3.2.6 String Type

The `string` type represents a sequence of UTF-8 encoded characters.

**Characteristics**:
- Immutable after creation
- Owned (not a reference)
- Can be concatenated with `+` operator
- UTF-8 byte length accessible via `.len()`
- Unicode scalar count accessible via `.char_count()`

## 3.3 Composite Types

### 3.3.1 Struct Types

Structs are user-defined nominal types with named fields.

```
struct-type ::= 'struct' identifier '{' field-list '}'
field-list ::= field (field-sep field)* field-sep?
field ::= identifier ':' type-name
field-sep ::= ';' | ','
```

**Example**:
```aziky
struct Point3D {
    x: f64;
    y: f64;
    z: f64;
}
```

**Rules**:
1. Field names must be unique within a struct
2. Field order is deterministic and affects memory layout
3. Structs are nominal types - different structs with identical fields are distinct types

### 3.3.2 Enum Types

Enums are user-defined types representing one of several named variants.

```
enum-type ::= 'enum' identifier type-params? '{' variant-list '}'
type-params ::= '<' identifier (',' identifier)* '>'
variant-list ::= variant (',' variant)* ','?
variant ::= identifier
          | identifier '(' type-name (',' type-name)* ','? ')'
          | identifier '{' field-list '}'
```

**Example**:
```aziky
enum Color { Red, Green, Blue }
enum Message { Quit, Move(i32, i32), Write { text: string; code: u32; } }
enum Outcome<T, E> { Ok(T), Err(E) }
```

**Rules**:
1. An enum must have at least one variant
2. Variant names must be unique within an enum
3. A variant is unit-like, tuple-like, or named-field; empty payload forms are rejected
4. Generic parameters must be unique and every applied type must supply the declared arity
5. Payload construction can infer generic arguments; any argument left ambiguous must be supplied by an expected type

### 3.3.3 Array Types

Arrays are fixed-size sequences of elements of a single type.

```
array-type ::= '[' type-name ';' integer-literal ']'
```

**Example**:
```aziky
let scores: [i32; 5] = [90i32, 85i32, 92i32, 78i32, 88i32];
let matrix: [[f64; 3]; 3] = ...;  // 3x3 matrix
```

**Rules**:
1. Array size is part of the type and must be a compile-time constant
2. Array capacity is fixed by the type; current mutable runtime arrays may track a logical length for `push`/`pop` without changing that capacity
3. Array elements are stored contiguously in memory
4. Array literals must have exactly the number of elements specified by the type

### 3.3.4 Dictionary Types

Dictionaries are typed key-value maps.

```
dict-type ::= 'dict' '<' type-name ',' type-name '>'
```

**Example**:
```aziky
let scores: dict<string, i32> = {"alice": 95i32, "bob": 87i32};
```

**Rules**:
1. Dictionary literal syntax currently uses string literal keys
2. Empty typed dictionaries can be populated with other non-reference key types through `set` in semantic fallback
3. Values must all be of the declared value type
4. Insertion order is not guaranteed, but iteration is deterministic
5. General runtime IR currently supports string-keyed, statically known entries only

### 3.3.5 Owned Dynamic Collection Types

`list<T>` and `map<K, V>` are the growable owned counterparts to fixed arrays
and dictionaries. They have distinct types even though the current literal
syntax is shared:

```aziky
let mut values: list<i32> = [];
values.push(4i32);

let mut scores: map<string, i32> = {};
scores.set("aziky", 7i32);
```

An empty `[]` or `{}` requires a type annotation because it contains no element
from which to infer its generic arguments. Dynamic collections execute through
the semantic path for all supported element types. In addition, lists of every
`bool`/`byte`/integer/`f32`/`f64` scalar type now have a runtime-native owned
representation with geometric capacity growth,
checked indexing and indexed assignment, `push`, statement-form `pop`, `len`,
`is_empty`, `foreach`, `contains`, `clear`, `reserve`, `shrink_to`,
`shrink_to_fit`, checked `get`/`first`/`last`/`peek`, value-returning `pop`, and
deterministic cleanup. Checked results use a non-allocating two-slot
typed scalar `Option<T>` tag/payload representation. Scalar elements use their
natural 1/2/4/8-byte widths for allocation, indexed access, growth copies, and
shrinking. Float membership follows IEEE equality, including equal signed zeros
and unequal NaNs. Copyable scalar-field struct elements also have a naturally
aligned native representation with the same core operations. Resource-bearing
aggregate elements and maps still remain to be lowered.

## 3.4 Reference Types

References provide borrowed access to values.

```
reference-type ::= '&' 'mut'? type-name
```

### 3.4.1 Shared References

A shared reference (`&T`) provides read-only access to a value.

```aziky
let x: i32 = 10i32;
let ref_x: &i32 = &x;
```

**Rules**:
- Multiple shared references to the same value may exist simultaneously
- The referent cannot be modified through a shared reference
- The referent must not be mutably borrowed while shared references exist

### 3.4.2 Mutable References

A mutable reference (`&mut T`) provides exclusive read-write access.

```aziky
let mut x: i32 = 10i32;
let ref_x: &mut i32 = &mut x;
```

**Rules**:
- Only **one** mutable reference to a value may exist at any time
- No shared references may exist while a mutable reference exists
- The referent may be read and modified through the mutable reference

## 3.5 Type Display Format

The string representation of types follows these conventions:

| Type | Display |
|------|---------|
| `u8`, `u16`, etc. | `u8`, `u16`, etc. |
| `i8`, `i16`, etc. | `i8`, `i16`, etc. |
| `f32`, `f64` | `f32`, `f64` |
| `bool` | `bool` |
| `byte` | `byte` |
| `char` | `char` |
| `string` | `string` |
| `[T; N]` | `[T; N]` |
| `dict<K, V>` | `dict<K, V>` |
| `list<T>` | `list<T>` |
| `map<K, V>` | `map<K, V>` |
| Generic enum | `Name<T, U>` |
| `&T` | `&T` |
| `&mut T` | `&mut T` |
| User struct | struct name |
| User enum | enum name |

## 3.6 Value Representation

### 3.6.1 Value Categories

Every expression produces a value in one of these categories:

1. **Owned values**: The expression owns its data
2. **References**: The expression is a borrowed pointer to data

### 3.6.2 Runtime Value Types

At runtime, values are represented as follows:

| Type | Representation |
|------|----------------|
| `bool` | 0 or 1 (8-bit) |
| Integers | Two's complement, width as specified |
| Floats | IEEE 754 representation |
| `char` | Unicode scalar value (semantic execution; general runtime IR pending) |
| `string` | UTF-8 byte sequence with length |
| Struct | Flattened field sequence |
| Enum | Variant discriminant + typed payload (semantic execution; general runtime IR pending) |
| Array | Contiguous element sequence |
| Dict | Hash table with deterministic iteration |
| List/Map | Owned growable collection (semantic execution; allocator-backed runtime representation pending) |

---

# Chapter 4: Expressions

## 4.1 Expression Overview

Expressions are syntactic constructs that produce values. Aziky supports a rich expression language with predictable evaluation order and precedence.

## 4.2 Literal Expressions

### 4.2.1 Integer Literals

```aziky
42              // i64 by default
255u8           // u8
-100i32         // i32 (negative)
1000000u64      // u64
```

### 4.2.2 Float Literals

```aziky
3.14159         // f64 by default
2.718f32        // f32
-0.5f64         // f64 (negative)
```

### 4.2.3 String Literals

```aziky
"Hello, World!"
"Line 1\nLine 2\nLine 3"
```

### 4.2.4 Character Literals

```aziky
'A'
'λ'
'\n'
```

### 4.2.5 Boolean Literals

```aziky
true
false
```

## 4.3 Identifier Expressions

An identifier expression refers to a previously declared binding.

```aziky
let x: i32 = 10i32;
print(x);       // x is an identifier expression
```

**Evaluation**: The identifier is looked up in the current scope chain. The expression evaluates to the current value of the binding.

## 4.4 Arithmetic Expressions

### 4.4.1 Binary Arithmetic Operators

| Operator | Operation | Types |
|----------|-----------|-------|
| `+` | Addition | Integers, Floats, Strings (concatenation) |
| `-` | Subtraction | Integers, Floats |
| `*` | Multiplication | Integers, Floats |
| `/` | Division | Integers, Floats |
| `%` | Modulo | Integers, Floats |

**Rules**:
1. Both operands must have the same type
2. Division by zero is a compile-time error for constants
3. Integer division truncates toward zero
4. String concatenation with `+` produces a new string

**Examples**:
```aziky
let sum: i32 = 10i32 + 20i32;           // 30
let diff: i64 = 100i64 - 40i64;         // 60
let prod: f64 = 3.0 * 4.0;               // 12.0
let quot: i32 = 17i32 / 5i32;           // 3
let rem: i32 = 17i32 % 5i32;            // 2
let msg: string = "Hello" + " World";   // "Hello World"
```

### 4.4.2 Unary Arithmetic Operators

| Operator | Operation | Types |
|----------|-----------|-------|
| `+` | Identity | Integers, Floats |
| `-` | Negation | Integers, Floats |

**Rules**:
1. Negation of unsigned integers is a compile-time error
2. Negation overflow (e.g., `-128i8`) is a compile-time error for constants

**Examples**:
```aziky
let pos: i32 = +10i32;      // 10
let neg: i32 = -10i32;      // -10
```

## 4.5 Bitwise Expressions

### 4.5.1 Bitwise Binary Operators

| Operator | Operation | Types |
|----------|-----------|-------|
| `&` | Bitwise AND | Integers |
| `\|` | Bitwise OR | Integers |
| `^` | Bitwise XOR | Integers |
| `<<` | Left shift | Integers |
| `>>` | Right shift | Integers |

**Shift Rules**:
1. The shift amount is masked to fit the type's bit width (shift by N mod bits)
2. Right shift on signed integers is arithmetic (sign-extending)
3. Right shift on unsigned integers is logical (zero-filling)

**Examples**:
```aziky
let and_val: u8 = 0b1100u8 & 0b1010u8;   // 0b1000 = 8
let or_val: u8 = 0b1100u8 | 0b1010u8;    // 0b1110 = 14
let xor_val: u8 = 0b1100u8 ^ 0b1010u8;   // 0b0110 = 6
let shl_val: u8 = 0b0011u8 << 2u8;       // 0b1100 = 12
let shr_val: i8 = -8i8 >> 2i8;           // -2 (arithmetic shift)
```

## 4.6 Comparison Expressions

### 4.6.1 Comparison Operators

| Operator | Operation | Types |
|----------|-----------|-------|
| `==` | Equal | All comparable types |
| `!=` | Not equal | All comparable types |
| `<` | Less than | Integers, Floats, Characters, Strings |
| `<=` | Less or equal | Integers, Floats, Characters, Strings |
| `>` | Greater than | Integers, Floats, Characters, Strings |
| `>=` | Greater or equal | Integers, Floats, Characters, Strings |

**Return Type**: All comparison operators return `bool`.

**Comparable Types**:
- Integers (same signedness and width)
- Floats (same width)
- Strings (lexicographic comparison)
- Characters (Unicode scalar-value comparison)
- Bools
- Enums (same enum type)
- Structs (same struct type, field-by-field comparison)

**Examples**:
```aziky
let eq: bool = 5i32 == 5i32;         // true
let ne: bool = 5i32 != 3i32;         // true
let lt: bool = 3i32 < 5i32;          // true
let gt: bool = "abc" > "aab";        // true (lexicographic)
```

## 4.7 Logical Expressions

### 4.7.1 Logical Operators

| Operator | Operation | Types |
|----------|-----------|-------|
| `&&` | Logical AND | bool |
| `\|\|` | Logical OR | bool |
| `!` | Logical NOT | bool |

**Short-Circuit Evaluation**:
- `a && b`: If `a` is `false`, `b` is not evaluated
- `a || b`: If `a` is `true`, `b` is not evaluated

**Examples**:
```aziky
let and_result: bool = true && false;    // false
let or_result: bool = true || false;     // true
let not_result: bool = !true;            // false
```

## 4.8 Reference Expressions

### 4.8.1 Borrow Operator

| Operator | Operation |
|----------|-----------|
| `&` | Create shared reference |
| `&mut` | Create mutable reference |

**Rules**:
1. Can only borrow identifiers (not arbitrary expressions)
2. Shared borrow requires the value not be mutably borrowed
3. Mutable borrow requires the value be declared `mut` and not borrowed at all

**Examples**:
```aziky
let x: i32 = 10i32;
let ref_x: &i32 = &x;            // shared reference

let mut y: i32 = 20i32;
let ref_y: &mut i32 = &mut y;    // mutable reference
```

## 4.9 Field Access Expressions

Access a field of a struct or embedded struct.

```
field-access ::= expression '.' identifier
```

**Examples**:
```aziky
let p: Point = Point { x: 1i32, y: 2i32 };
print(p.x);      // field access
print(p.y);      // field access
```

**Embedded Fields**: Fields from embedded structs are accessed as if they were direct fields:

```aziky
struct Base { id: u32 }
struct Derived { embed Base, value: f64 }

let d: Derived = Derived { id: 42u32, value: 3.14f64 };
print(d.id);      // accesses embedded field
```

## 4.10 Index Expressions

Access an element of an array or dictionary.

```
index-expression ::= expression '[' expression ']'
```

### 4.10.1 Array Indexing

```aziky
let arr: [i32; 3] = [10i32, 20i32, 30i32];
print(arr[0]);    // 10
print(arr[2]);    // 30
```

**Rules**:
1. Index must be an integer type
2. Out-of-bounds access results in a compile-time error for constant indices
3. Runtime bounds checking is performed in the runtime-generic lowering path

### 4.10.2 Dictionary Indexing

```aziky
let scores: dict<string, i32> = {"alice": 95i32, "bob": 87i32};
print(scores["alice"]);    // 95
```

**Rules**:
1. The index expression must match the declared key type
2. Literal-initialized and runtime-generic dictionaries currently require statically known string keys
3. Unknown key access results in an error

## 4.11 Call Expressions

Invoke a function with arguments.

```
call-expression ::= identifier '(' argument-list? ')'
argument-list ::= expression (',' expression)*
```

**Rules**:
1. The function must be defined (including `main`)
2. Argument count must match parameter count
3. Arguments must be type-compatible with parameters
4. Expression calls require the function to be pure (no side effects)
5. Call depth is limited to prevent stack overflow

**Example**:
```aziky
fn add(a: i32, b: i32) -> i32 {
    return a + b;
}

fn main() {
    let result: i32 = add(1i32, 2i32);
    print(result);    // 3
}
```

## 4.12 Method Call Expressions

Call a method on a value.

```
method-call-expression ::= expression '.' identifier '(' argument-list? ')'
```

**Built-in Methods**: See [Appendix D: Built-in Methods Reference](#appendix-d-built-in-methods-reference)

**User-Defined Methods**: Methods may be inherent to a nominal type or supplied by
a trait implementation. Inherent methods keep behavior beside the data model:

```aziky
impl Point {
    fn new(x: i32, y: i32) -> Self {
        return Self { x: x, y: y };
    }

    fn translate(self: &mut Self, dx: i32, dy: i32) {
        self.x = self.x + dx;
        self.y = self.y + dy;
    }
}

let mut point: Point = Point::new(10i32, 20i32);
point.translate(2i32, -1i32);
```

Associated calls use this grammar:

```
associated-call-expression ::= identifier '::' identifier '(' argument-list? ')'
```

Trait-provided behavior uses the same explicit receiver model:

```aziky
trait Printable {
    fn print_self(self: &Self);
}

impl Printable for Point {
    fn print_self(self: &Point) {
        print(self.x);
    }
}

fn main() {
    let p: Point = Point { x: 10i32, y: 20i32 };
    p.print_self();
}
```

Associated inherent functions use the ordinary direct-call runtime pipeline when
their parameter and return layouts are supported. User-defined receiver methods
are statically resolved in semantic lowering. An immutable `&Self` query may use
a temporary receiver, for example `Counter::new(4i32).current()`. A `&mut Self`
method requires a named mutable binding; Aziky never silently promotes an rvalue
to mutable storage. General runtime-IR lowering for receiver calls is still
pending, so programs that otherwise require that path may fall back until the
work is complete.

## 4.13 Enum Variant Expressions

Construct a unit, tuple-payload, or named-payload enum variant.

```
enum-variant-expression ::= identifier '::' identifier
                          | identifier '::' identifier '(' argument-list? ')'
                          | identifier '::' identifier '{' field-init-list '}'
```

**Example**:
```aziky
enum Message { Quit, Move(i32, i32), Write { text: string; code: u32; } }

let quit: Message = Message::Quit;
let movement: Message = Message::Move(3i32, 4i32);
let note: Message = Message::Write { text: "ready", code: 7u32 };
```

The constructor shape, field names, arity, and payload types are checked. Named
payloads must supply every declared field exactly once.

## 4.14 Match Expressions

`match` evaluates an enum value, destructures its selected payload, and returns
the value produced by the chosen arm:

```aziky
fn describe(message: Message) -> string {
    return match message {
        Message::Quit => "quit",
        Message::Move(x, y) => x.to_str() + "," + y.to_str(),
        Message::Write { text: body } => body,
    };
}
```

All arms must produce compatible types. A match must cover every variant or end
with `_`. Duplicate variant arms, arms after a wildcard, duplicate bindings,
and patterns whose shape does not match the variant are diagnosed. Pattern
bindings are scoped to their arm. `_` can also ignore individual tuple fields
or named fields.

Current status: exhaustive enum matching is parsed and semantically executed.
General runtime-IR lowering remains pending.

## 4.15 Struct Literal Expressions

Construct a struct value by specifying field values.

```
struct-literal ::= identifier '{' field-init-list '}'
field-init-list ::= field-init (',' field-init)* ','?
field-init ::= identifier ':' expression
```

**Example**:
```aziky
let p: Point = Point { x: 10i32, y: 20i32 };
```

**Rules**:
1. All fields must be specified
2. Field order in the literal need not match declaration order
3. Field types must match the struct definition

## 4.16 Array Literal Expressions

Construct an array value by specifying elements.

```
array-literal ::= '[' expression-list? ']'
expression-list ::= expression (',' expression)*
```

**Example**:
```aziky
let numbers: [i32; 3] = [1i32, 2i32, 3i32];
let empty: [i32; 0] = [];
```

**Rules**:
1. All elements must have the same type
2. Array literals cannot be empty in some contexts (type inference requires at least one element)
3. Element count must match the type's size

## 4.17 Dictionary Literal Expressions

Construct a dictionary value by specifying key-value pairs.

```
dict-literal ::= '{' entry-list? '}'
entry-list ::= entry (',' entry)* ','?
entry ::= string-literal ':' expression
```

**Example**:
```aziky
let scores: dict<string, i32> = {
    "alice": 95i32,
    "bob": 87i32,
    "charlie": 92i32
};
```

**Rules**:
1. Keys must be string literals
2. All values must have the same type
3. Duplicate keys result in an error

## 4.18 Runtime Seed Expression

The `runtime_seed()` expression provides a runtime-determined seed value.

```
runtime-seed-expression ::= 'runtime_seed' '(' ')'
```

**Purpose**: Provides non-deterministic input for benchmark kernels.

**Rules**:
1. Takes no arguments
2. Returns a `u64` value
3. Cannot be used in pure functions
4. Triggers runtime-generic lowering path when used

**Example**:
```aziky
let state: u64 = runtime_seed();
```

---

# Chapter 5: Statements and Control Flow

## 5.1 Statement Overview

Statements are executed for their side effects. Unlike expressions, statements do not produce values (except for the return statement which exits a function).

## 5.2 Variable Binding Statement

Declare and initialize a variable.

```
let-statement ::= 'let' 'mut'? identifier (':' type-name)? '=' expression ';'
```

### 5.2.1 Immutable Bindings

```aziky
let x: i32 = 10i32;
let y = 20i32;          // type inferred
```

**Rules**:
1. Immutable bindings cannot be reassigned
2. The initializer expression is evaluated once at declaration
3. Type can be explicitly annotated or inferred from the initializer

### 5.2.2 Mutable Bindings

```aziky
let mut counter: i32 = 0i32;
counter = counter + 1i32;    // allowed
```

**Rules**:
1. Mutable bindings can be reassigned
2. The `mut` keyword must precede the identifier
3. Taking a mutable reference requires a mutable binding

### 5.2.3 Shadowing

Shadowing is **not allowed**. Redeclaring a variable with the same name in the same scope is an error.

```aziky
let x: i32 = 10i32;
let x: i32 = 20i32;    // ERROR: redefinition of 'x'
```

## 5.3 Assignment Statement

Assign a new value to a mutable variable.

```
assignment-statement ::= identifier '=' expression ';'
```

**Example**:
```aziky
let mut x: i32 = 10i32;
x = 20i32;              // assignment
```

**Rules**:
1. The target must be a mutable binding
2. The expression type must match the binding's type
3. Cannot assign while the variable is borrowed

## 5.4 Index Assignment Statement

Assign a new value to an array or dictionary element.

```
index-assignment ::= identifier '[' expression ']' '=' expression ';'
```

### 5.4.1 Array Index Assignment

```aziky
let mut arr: [i32; 3] = [1i32, 2i32, 3i32];
arr[0] = 10i32;         // arr is now [10, 2, 3]
```

**Rules**:
1. The array binding must be mutable
2. The index must be within bounds
3. The value type must match the array's element type

### 5.4.2 Dictionary Index Assignment

```aziky
let mut scores: dict<string, i32> = {"alice": 95i32};
scores["bob"] = 87i32;    // adds or updates key "bob"
```

**Rules**:
1. The dictionary binding must be mutable
2. The key must be a string literal
3. The value type must match the dictionary's value type

## 5.5 Field Assignment Statement

Assign a new value to a mutable struct field.

```
field-assignment ::= identifier '.' identifier '=' expression ';'
```

**Example**:
```aziky
struct Point { x: i32; y: i32; }
let mut p: Point = Point { x: 1i32, y: 2i32 };
p.x = 10i32;
```

**Rules**:
1. The receiver binding must be mutable
2. The receiver must be a struct value
3. The field must exist on the struct
4. The assigned value must match the field type

## 5.5 Function Call Statement

Call a function for its side effects.

```
call-statement ::= identifier '(' argument-list? ')' ';'
```

**Example**:
```aziky
fn greet(name: string) {
    print("Hello, ");
    print(name);
    print("!\n");
}

fn main() {
    greet("World");     // call statement
}
```

**Rules**:
1. The function must be defined
2. Argument count and types must match
3. Functions with side effects can be called as statements

## 5.6 Method Call Statement

Call a method on a mutable container.

```
method-call-statement ::= identifier '.' identifier '(' argument-list? ')' ';'
```

### 5.6.1 Array Methods

| Method | Description | Arguments |
|--------|-------------|-----------|
| `push(value)` | Append element | value: element type |
| `pop()` | Remove last element | none |
| `sort()` | Unstable sort | none |
| `sort_stable()` | Stable sort | none |
| `sort_unstable()` | Unstable sort | none |
| `sort_radix_unstable()` | Radix sort (unstable) | none |
| `sort_radix_stable()` | Radix sort (stable) | none |
| `sort_by(comparator)` | Sort with comparator | function name |

**Examples**:
```aziky
let mut arr: [i32; 4] = [3i32, 1i32, 4i32, 1i32];
arr.sort();             // arr is now [1, 1, 3, 4]

// push/pop update the logical length within the array's fixed capacity
```

### 5.6.2 Dictionary Methods

| Method | Description | Arguments |
|--------|-------------|-----------|
| `set(key, value)` | Set key-value pair | key: declared key type, value: value type |
| `remove(key)` | Remove key | key: declared key type |

**Examples**:
```aziky
let mut scores: dict<string, i32> = {"alice": 95i32};
scores.set("bob", 87i32);
scores.remove("alice");
```

## 5.7 Print Statement

Output a value to standard output.

```
print-statement ::= 'print' '(' expression ')' ';'
```

**Example**:
```aziky
print("Hello, World!\n");
print(42);
print(3.14159f64);
```

**Rules**:
1. The expression must evaluate to a printable type
2. Printable types: `string`, `char`, `bool`, integers, floats
3. Non-printable types: `struct`, `enum`, `array`, `dict`, references
4. Adjacent print statements may be coalesced by the optimizer

## 5.8 Exit Statement

Terminate the program with an exit code.

```
exit-statement ::= 'exit' '(' expression ')' ';'
```

**Example**:
```aziky
exit(0);        // successful termination
exit(1);        // error termination
```

**Rules**:
1. The expression must evaluate to an integer type
2. The value must be non-negative
3. Exit terminates the program immediately

## 5.9 Block Statement

A sequence of statements in a new scope.

```
block-statement ::= '{' statement* '}'
```

**Example**:
```aziky
{
    let temp: i32 = 10i32;
    print(temp);
}
// temp is out of scope here
```

**Rules**:
1. A block creates a new lexical scope
2. Variables declared in the block are dropped at block exit
3. Borrows are released at block exit

## 5.10 If Statement

Conditional execution.

```
if-statement ::= 'if' expression block ('else' (block | if-statement))?
```

**Example**:
```aziky
let x: i32 = 10i32;
if x > 5i32 {
    print("greater\n");
} else if x < 5i32 {
    print("less\n");
} else {
    print("equal\n");
}
```

**Rules**:
1. The condition must be of type `bool`
2. Each branch creates a new scope
3. `else if` chains are supported
4. No parentheses required around the condition

## 5.11 While Loop

Conditional loop.

```
while-statement ::= 'while' expression block
```

**Example**:
```aziky
let mut i: i32 = 0i32;
while i < 10i32 {
    print(i);
    i = i + 1i32;
}
```

**Rules**:
1. The condition is evaluated before each iteration
2. The condition must be of type `bool`
3. The loop body creates a new scope per iteration
4. `break` and `continue` are allowed inside

## 5.12 Loop Statement

Infinite loop.

```
loop-statement ::= 'loop' block
```

**Example**:
```aziky
let mut i: i32 = 0i32;
loop {
    print(i);
    i = i + 1i32;
    if i >= 10i32 {
        break;
    }
}
```

**Rules**:
1. The loop executes indefinitely until `break` or `return`
2. A `break` or `return` is required to exit (or `exit`)
3. The loop body creates a new scope per iteration

## 5.13 For Loop

Range-based iteration.

```
for-statement ::= 'for' identifier 'in' expression '..' expression block
```

**Example**:
```aziky
for i in 0i32..10i32 {
    print(i);
}
```

**Rules**:
1. The loop variable is immutable and scoped to the loop body
2. The range is half-open: `[start, end)`
3. Start and end must have the same integer type
4. The loop iterates from `start` up to (but not including) `end`
5. `break` and `continue` are allowed inside

## 5.14 Foreach Loop

Iterator-based iteration.

```
foreach-statement ::= 'foreach' identifier 'in' expression block
```

**Example**:
```aziky
let scores: dict<string, i32> = {"alice": 95i32, "bob": 87i32};
foreach key in scores {
    print(key);
    print("\n");
}
```

**Rules**:
1. The iterable must be an array or dictionary
2. For arrays, the loop variable is each element
3. For dictionaries, the loop variable is each key (string)
4. The loop variable is immutable
5. `break` and `continue` are allowed inside

## 5.15 Break Statement

Exit the innermost loop.

```
break-statement ::= 'break' ';'
```

**Example**:
```aziky
let mut i: i32 = 0i32;
loop {
    if i >= 10i32 {
        break;
    }
    i = i + 1i32;
}
```

**Rules**:
1. Must be inside a loop (`while`, `loop`, `for`, `foreach`)
2. Not allowed inside `parfor` body

## 5.16 Continue Statement

Skip to the next iteration of the innermost loop.

```
continue-statement ::= 'continue' ';'
```

**Example**:
```aziky
for i in 0i32..10i32 {
    if i == 5i32 {
        continue;       // skip printing 5
    }
    print(i);
}
```

**Rules**:
1. Must be inside a loop (`while`, `loop`, `for`, `foreach`)
2. Not allowed inside `parfor` body

## 5.17 Return Statement

Return from a function.

```
return-statement ::= 'return' expression? ';'
```

**Example**:
```aziky
fn add(a: i32, b: i32) -> i32 {
    return a + b;
}

fn greet() {
    print("Hello\n");
    return;             // void return
}
```

**Rules**:
1. Functions with a return type must return a value
2. Functions without a return type (`void`) may have a bare `return`
3. `return` exits the function immediately
4. Not allowed inside `parfor` body

## 5.18 Assert Statement

Compile-time assertion with optional message.

```
assert-statement ::= 'assert' '(' expression (',' expression)? ')' ';'
```

**Example**:
```aziky
let x: i32 = 10i32;
assert(x > 0i32, "x must be positive");
assert(x < 100i32);     // no message
```

**Rules**:
1. The condition must be of type `bool`
2. The optional message must be a string
3. If the assertion fails, the program terminates with an error
4. Assertions are checked at compile-time for constant expressions

## 5.19 Panic Statement

Unconditional runtime error.

```
panic-statement ::= 'panic' '(' expression ')' ';'
```

**Example**:
```aziky
fn divide(a: i32, b: i32) -> i32 {
    if b == 0i32 {
        panic("division by zero");
    }
    return a / b;
}
```

**Rules**:
1. The message must be a string
2. Panic immediately terminates the program with an error

---

# Chapter 6: Functions

## 6.1 Function Definition

Functions are the primary unit of code organization in Aziky.

```
function-definition ::= 'fn' identifier '(' parameter-list? ')' ('->' type-name)? block
parameter-list ::= parameter (',' parameter)*
parameter ::= identifier ':' type-name
```

## 6.2 Function Signature

### 6.2.1 Name

Function names follow identifier rules (Section 2.3). Functions must have unique names within the program.

### 6.2.2 Parameters

Parameters are declared with name and type:

```aziky
fn greet(name: string, times: i32) {
    // ...
}
```

**Rules**:
1. Parameters are immutable by default
2. Use `mut` to allow modification (creates a local mutable copy)
3. Parameter names must be unique within the function
4. Types must be known (no generic parameters yet)

**Mutable Parameters**:
```aziky
fn increment(mut x: i32) {
    x = x + 1i32;
}
```

### 6.2.3 Return Type

Functions may optionally return a value:

```aziky
fn add(a: i32, b: i32) -> i32 {
    return a + b;
}

fn no_return() {
    print("No return value\n");
}
```

**Rules**:
1. Functions with `-> Type` must return a value of that type
2. Functions without `-> Type` cannot return a value
3. The return type must be a valid type name

## 6.3 Function Body

The function body is a block of statements:

```aziky
fn example(x: i32) -> i32 {
    let y: i32 = x * 2i32;
    print(y);
    return y;
}
```

## 6.4 The Main Function

Every Aziky program must have a `main` function as the entry point:

```aziky
fn main() {
    // program execution starts here
    exit(0);
}
```

**Rules**:
1. `main` takes no parameters
2. `main` returns no value
3. `main` must be defined exactly once

## 6.5 Function Calls

### 6.5.1 Call Statements

Call a function for side effects:

```aziky
fn greet() {
    print("Hello\n");
}

fn main() {
    greet();
}
```

### 6.5.2 Call Expressions

Call a function and use its return value:

```aziky
fn add(a: i32, b: i32) -> i32 {
    return a + b;
}

fn main() {
    let sum: i32 = add(1i32, 2i32);
    print(sum);
}
```

**Purity Requirement**: Expression calls require the function to be **pure** (no side effects).

### 6.5.3 Purity Analysis

A function is **pure** if:
1. It does not contain `print` statements
2. It does not contain `exit` statements
3. It does not contain `assert` or `panic` statements
4. It only calls other pure functions
5. It does not call `runtime_seed()`

**Example of pure function**:
```aziky
fn square(x: i32) -> i32 {
    return x * x;
}
```

**Example of impure function**:
```aziky
fn greet(name: string) {
    print("Hello, ");
    print(name);
}
```

## 6.6 Recursion

Recursion is supported but limited by call depth:

```aziky
fn factorial(n: i32) -> i32 {
    if n <= 1i32 {
        return 1i32;
    }
    return n * factorial(n - 1i32);
}
```

**Call Depth Limit**: Maximum call depth is **256**. Exceeding this limit results in a compile-time error.

## 6.7 Function Context and Borrow Tracking

Functions maintain their own scope environment:

```aziky
fn use_ref(x: &i32) {
    print(x);       // dereference happens via resolve_receiver_value
}
```

**Rules**:
1. References can be passed as parameters
2. Borrow rules are enforced across function boundaries
3. References must remain valid for the function's duration

---

# Chapter 7: Structs and Data Types

## 7.1 Struct Definition

Structs define nominal types with named fields:

```
struct-definition ::= 'struct' identifier '{' field-list '}'
field-list ::= field (field-sep field)* field-sep?
field ::= 'embed'? identifier ':' type-name
field-sep ::= ';' | ','
```

## 7.2 Basic Structs

```aziky
struct Point {
    x: f64;
    y: f64;
}

struct Person {
    name: string;
    age: u32;
    active: bool;
}
```

**Rules**:
1. Field names must be unique within a struct
2. Field types must be known types
3. Field separators can be `;` or `,` (interchangeable)
4. Trailing separator is optional

## 7.3 Struct Literals

Create a struct instance:

```aziky
let origin: Point = Point { x: 0.0f64, y: 0.0f64 };
let alice: Person = Person {
    name: "Alice",
    age: 30u32,
    active: true
};
```

**Rules**:
1. All fields must be specified
2. Field order in the literal need not match declaration order
3. Field names are matched by name, not position

## 7.4 Field Access

Access struct fields with dot notation:

```aziky
let p: Point = Point { x: 1.5f64, y: 2.5f64 };
print(p.x);      // 1.5
print(p.y);      // 2.5
```

## 7.5 Embedded Structs

The `embed` keyword flattens another struct's fields into the current struct:

```aziky
struct Base {
    id: u32;
    name: string;
}

struct Extended {
    embed Base;
    value: f64;
}
```

**Flattened Layout**: `Extended` has fields: `id`, `name`, `value`

### 7.5.1 Embedding Rules

1. **Physical Flattening**: Embedded struct fields become direct fields of the outer struct
2. **Name Collision**: Embedding that causes field name collision is an error
3. **Recursive Embedding**: Cyclic embedding chains are detected and rejected
4. **Type Requirement**: Embedded field type must be a struct

**Error Examples**:
```aziky
struct A { x: i32 }
struct B { x: i32; embed A }    // ERROR: 'x' collides

struct C { embed D }
struct D { embed C }             // ERROR: cyclic embedding
```

### 7.5.2 Embedded Field Access

Access embedded fields directly:

```aziky
struct Base { id: u32 }
struct Extended { embed Base; value: f64 }

let e: Extended = Extended { id: 42u32, value: 3.14f64 };
print(e.id);       // accesses embedded field
```

## 7.6 Struct Layout

### 7.6.1 Layout Guarantees

1. **Deterministic Order**: Fields are laid out in declaration order
2. **Embedded Flattening**: Embedded struct fields are laid out first, in their declaration order
3. **Alignment**: Platform-specific alignment rules apply

### 7.6.2 Layout Resolution

The compiler computes a flattened layout for each struct:

1. For each field:
   - If embedded, recursively flatten the embedded struct's fields
   - Otherwise, add the field to the layout
2. Detect and reject name collisions
3. Detect and reject cyclic embeddings

## 7.7 Struct Comparison

Structs of the same type can be compared for equality:

```aziky
let p1: Point = Point { x: 1.0f64, y: 2.0f64 };
let p2: Point = Point { x: 1.0f64, y: 2.0f64 };
let equal: bool = p1 == p2;    // true
```

**Rules**:
1. Only structs of the same type can be compared
2. Comparison is field-by-field
3. All fields must be comparable

## 7.8 Inherent Implementations

Inherent implementations attach constructors, queries, and mutations directly to
a struct without introducing inheritance, implicit allocation, or dynamic
dispatch.

```
inherent-implementation ::= 'impl' identifier '{' function-definition* '}'
```

```aziky
struct Counter {
    value: i32;
}

impl Counter {
    fn new(value: i32) -> Self {
        return Self { value: value };
    }

    fn current(self: &Self) -> i32 {
        return self.value;
    }

    fn add(self: &mut Self, amount: i32) {
        self.value = self.value + amount;
    }
}

fn main() {
    let mut counter: Counter = Counter::new(4i32);
    counter.add(3i32);
    print(counter.current());
    exit(0u64);
}
```

Rules:

1. The target must be a declared struct in the compilation unit.
2. An associated function has no receiver and is called as `Type::function(...)`.
3. Query methods conventionally use an explicit `self: &Self` first parameter.
4. Mutating methods use `self: &mut Self` and require a mutable caller binding.
5. Immutable query methods may borrow a temporary receiver for the duration of
   the call, as in `Counter::new(4i32).current()`.
6. `Self` is substituted in parameter types, return types, local annotations,
   nested expressions, and constructor literals.
7. Duplicate methods, including collisions across inherent and trait
   implementations, are compile errors.
8. Dispatch remains static. Aziky classes are planned as a developer-facing
   convention over value structs, opaque constructors, and traits—not as a hidden
   garbage-collected reference hierarchy.

---

# Chapter 8: Enums

## 8.1 Enum Definition

Enums define nominal sum types. A variant can carry no payload, positional
payload fields, or named payload fields:

```
enum-definition ::= 'enum' identifier type-params? '{' variant-list '}'
variant-list ::= variant (',' variant)* ','?
variant ::= identifier
          | identifier '(' type-name (',' type-name)* ','? ')'
          | identifier '{' field-list '}'
```

## 8.2 Payload and Generic Enums

```aziky
enum Color { Red, Green, Blue }
enum Message {
    Quit,
    Move(i32, i32),
    Write { text: string; code: u32; },
}
enum Outcome<T, E> { Ok(T), Err(E) }
```

**Rules**:
1. An enum must have at least one variant
2. Variant names must be unique within an enum
3. Tuple and named payloads must contain at least one field
4. Named payload fields and generic parameter names must be unique
5. Applied generic types must provide exactly the declared number of arguments
6. Generic arguments are inferred from payloads and the expected type; unresolved or conflicting inference is an error

## 8.3 Enum Variant Construction

Create an enum value with the `::` operator:

```aziky
let red: Color = Color::Red;
let moved: Message = Message::Move(3i32, 4i32);
let note: Message = Message::Write { text: "ready", code: 7u32 };
let success: Outcome<i32, string> = Outcome::Ok(42i32);
```

## 8.4 Exhaustive Matching

```aziky
fn render(value: Outcome<i32, string>) -> string {
    return match value {
        Outcome::Ok(number) => number.to_str(),
        Outcome::Err(message) => message,
    };
}
```

The compiler validates enum identity, variant existence, payload pattern shape,
field names, binding uniqueness, and exhaustiveness. A final `_` wildcard may
cover the remaining variants. A wildcard that follows complete coverage, a
repeated variant, or any arm after `_` is unreachable and rejected.

## 8.5 Built-in Failure Types

The predeclared types are equivalent to generic payload enums:

```aziky
enum Option<T> { None, Some(T) }
enum Result<T, E> { Ok(T), Err(E) }
```

`Option<T>` provides `is_some()`, `is_none()`, and `unwrap_or(fallback)`.
`Result<T, E>` provides `is_ok()`, `is_err()`, and `unwrap_or(fallback)`.
These APIs preserve expected lookup, conversion, and parsing failure as values.
Unchecked `unwrap()` is intentionally not part of this baseline.

## 8.6 Enum Comparison

Enums of the same type can be compared:

```aziky
let c1: Color = Color::Red;
let c2: Color = Color::Red;
let equal: bool = c1 == c2;    // true
```

**Rules**:
1. Only enums of the same type can be compared
2. Variants are compared deterministically by name and then by payload
3. Comparison operators `<`, `<=`, `>`, `>=` use that deterministic ordering

## 8.7 Enum Display

Enums can be converted to strings:

```aziky
let c: Color = Color::Red;
print(c.to_str());    // "Color::Red"
```

Payload formatting is deterministic. Named fields are rendered in stable field
order, and payloads participate in equality and `hash64()`.

Current status: payload enums, generic enum instantiation, built-in failure
types, and exhaustive match are semantically executable. Runtime-native enum
layout, monomorphization, and match lowering remain pending.

---

# Chapter 9: Traits and Polymorphism

## 9.1 Trait Definition

Traits define method signatures that types can implement:

```
trait-definition ::= 'trait' identifier '{' method-signature* '}'
method-signature ::= 'fn' identifier '(' parameter-list? ')' ('->' type-name)? ';'
```

## 9.2 Basic Traits

```aziky
trait Printable {
    fn print_self(self: &Self);
}

trait Numeric {
    fn abs(self: Self) -> Self;
    fn is_positive(self: &Self) -> bool;
}
```

**Rules**:
1. Traits define method signatures without bodies
2. `Self` refers to the implementing type
3. Methods can take `self` by value, `&Self`, or `&mut Self`

## 9.3 Trait Implementation

Implement a trait for a specific type:

```
trait-implementation ::= 'impl' identifier 'for' identifier '{' function-definition* '}'
```

### 9.3.1 Implementation Example

```aziky
trait Printable {
    fn print_self(self: &Self);
}

impl Printable for Point {
    fn print_self(self: &Point) {
        print(self.x);
        print(", ");
        print(self.y);
    }
}
```

### 9.3.2 Implementation Rules

1. **Signature Match**: Method signatures must exactly match the trait definition
2. **Self Substitution**: `Self` in the trait is replaced with the implementing type
3. **Complete Implementation**: All trait methods must be implemented
4. **Single Implementation**: Only one implementation per trait/type pair

## 9.4 Monomorphization

Trait methods are statically dispatched through monomorphization:

1. Method `method_name` on type `TypeName` becomes `TypeName__method_name`
2. All calls are resolved at compile time
3. No vtables or runtime dispatch

**Example**: After monomorphization:
```aziky
// Original: p.print_self()
// Becomes:  Point__print_self(&p)
```

## 9.5 Static Dispatch

All polymorphism is resolved at compile time:

```aziky
trait Shape {
    fn area(self: &Self) -> f64;
}

impl Shape for Circle {
    fn area(self: &Circle) -> f64 {
        return 3.14159f64 * self.radius * self.radius;
    }
}

fn main() {
    let c: Circle = Circle { radius: 5.0f64 };
    print(c.area());    // Direct call to Circle__area
}
```

---

# Chapter 10: Arrays and Dictionaries

## 10.1 Arrays

### 10.1.1 Array Types

Arrays have a fixed size known at compile time:

```aziky
let numbers: [i32; 5] = [1i32, 2i32, 3i32, 4i32, 5i32];
let matrix: [[f64; 3]; 3] = ...;    // 3x3 matrix
```

### 10.1.2 Array Literals

```aziky
let empty: [i32; 0] = [];
let single: [i32; 1] = [42i32];
let nested: [[i32; 2]; 2] = [[1i32, 2i32], [3i32, 4i32]];
```

**Rules**:
1. All elements must have the same type
2. Element count must match the declared size
3. Empty arrays are allowed

### 10.1.3 Array Indexing

```aziky
let arr: [i32; 3] = [10i32, 20i32, 30i32];
print(arr[0]);    // 10
print(arr[2]);    // 30
```

**Rules**:
1. Index must be an integer type
2. Out-of-bounds access is an error
3. Bounds checking is performed at runtime for dynamic indices

### 10.1.4 Array Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `len()` | `u64` | Number of elements |
| `is_empty()` | `bool` | Whether array is empty |
| `contains(v)` | `bool` | Whether an equal typed element exists |
| `get(index)` | `Option<T>` | Checked indexed lookup |
| `peek()` | element type | Last element (error if empty) |
| `push(v)` | void | Append element (mutable) |
| `pop()` | void | Remove last element (mutable) |
| `sort()` | void | Unstable sort (mutable) |
| `sort_stable()` | void | Stable sort (mutable) |
| `sort_unstable()` | void | Unstable sort (mutable) |
| `sort_radix_unstable()` | void | Radix sort (mutable) |
| `sort_radix_stable()` | void | Stable radix sort (mutable) |
| `sort_by(fn)` | void | Sort with comparator (mutable) |

**Example**:
```aziky
let mut arr: [i32; 4] = [3i32, 1i32, 4i32, 1i32];
arr.sort();
// arr is now [1, 1, 3, 4]

print(arr.len());      // 4
print(arr.is_empty()); // false
print(arr.peek());     // 4
```

### 10.1.5 Sorting

Aziky provides multiple sorting algorithms:

| Method | Stability | Complexity | Notes |
|--------|-----------|------------|-------|
| `sort()` | Unstable | O(n log n) | Default, fastest |
| `sort_stable()` | Stable | O(n log n) | Preserves order of equal elements |
| `sort_unstable()` | Unstable | O(n log n) | Same as `sort()` |
| `sort_radix_unstable()` | Unstable | O(n * k) | For i32/i64/u32/u64, k = key bits |
| `sort_radix_stable()` | Stable | O(n * k) | For i32/i64/u32/u64, k = key bits |
| `sort_by(fn)` | Unstable | O(n log n) | Custom comparator |

**Custom Comparator**:
```aziky
fn compare_desc(a: &i32, b: &i32) -> bool {
    return a > b;    // true if a should come before b
}

fn main() {
    let mut arr: [i32; 4] = [3i32, 1i32, 4i32, 1i32];
    arr.sort_by(compare_desc);
    // arr is now [4, 3, 1, 1]
}
```

## 10.2 Dictionaries

### 10.2.1 Dictionary Types

Dictionaries map a declared key type to a declared value type. Literal syntax currently uses string keys:

```aziky
let scores: dict<string, i32> = {"alice": 95i32, "bob": 87i32};
```

### 10.2.2 Dictionary Literals

```aziky
let empty: dict<string, i32> = {};
let single: dict<string, i32> = {"key": 42i32};
let multi: dict<string, f64> = {
    "pi": 3.14159f64,
    "e": 2.71828f64
};
```

**Rules**:
1. Keys must be string literals
2. All values must have the same type
3. Duplicate keys are an error
4. Non-string-key dictionaries currently start from an empty typed literal and are populated through `set` in semantic fallback

### 10.2.3 Dictionary Indexing

```aziky
let scores: dict<string, i32> = {"alice": 95i32};
print(scores["alice"]);    // 95
```

**Rules**:
1. Key expressions must match the declared key type
2. General runtime IR currently requires statically known string keys
3. Unknown key access is an error

### 10.2.4 Dictionary Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `len()` | `u64` | Number of entries |
| `is_empty()` | `bool` | Whether empty |
| `contains_key(k)` | `bool` | Whether a typed key exists |
| `get(k)` | `Option<V>` | Checked key lookup |
| `keys()` | `[string; N]` | Array of keys |
| `set(k, v)` | void | Set key-value pair (mutable) |
| `remove(k)` | void | Remove key (mutable) |

**Example**:
```aziky
let mut scores: dict<string, i32> = {"alice": 95i32};
scores.set("bob", 87i32);
print(scores["bob"]);      // 87
scores.remove("alice");
print(scores.len());       // 1
```

Both arrays and dictionaries provide `get(key) -> Option<Value>` for checked
access. Direct indexing remains the explicit trapping/erroring form.

## 10.3 Owned Lists and Maps

`list<T>` and `map<K, V>` are growable owned collections. They are distinct
from fixed `[T; N]` and `dict<K, V>` types:

```aziky
let mut values: list<i32> = [];
values.push(2i32);
values.push(4i32);
values[1u8] = 5i32;
let tail: Option<i32> = values.peek();
values.pop();

let mut scores: map<string, i32> = {};
scores.set("aziky", 7i32);
scores["compiler"] = 9i32;
let known: Option<i32> = scores.get("compiler");
scores.remove("aziky");
```

Lists support indexing, indexed assignment, `foreach`, `len`, `is_empty`,
`contains`, `get`, `first`, `last`, `peek`, `push`, `pop`, `clear`, `reserve`,
`shrink_to`, and `shrink_to_fit`. `reserve(n)` reserves room for at least `n` additional
elements; `shrink_to(n)` retains capacity for at least `max(len, n)` elements;
`shrink_to_fit()` reduces capacity to the current length. Maps support indexing,
indexed assignment, `foreach`, `len`, `is_empty`, `contains_key`, `get`, `keys`,
`set`, `remove`, and `clear`. Mutation requires a `mut` binding. `list.first()`,
`list.last()`, `list.peek()`, and collection `get()` return
`Option` instead of failing.

Current status: typed construction, mutation, iteration, and checked access are
semantically executable for the general collection surface. Generated Linux
x86-64 code supports owned lists of every integer and floating-point scalar type:
the runtime descriptor tracks pointer, length, capacity, and allocation bytes;
growth is overflow-checked and geometric; existing elements are copied before
the old allocation is released; `foreach`, `contains`, `clear`, and explicit
reserve/shrink operations lower natively; checked access and expression-form
`pop()` use a typed two-slot scalar `Option<T>` and never read outside the live
length; signed and narrow integers are normalized at storage/load boundaries;
bounds failures clean up deterministically. Scalar elements use packed natural
1/2/4/8-byte storage with width-aware x86-64 loads and stores, and float
membership preserves IEEE signed-zero/NaN semantics. Struct elements, maps,
general option/result ABIs and matching, a shared allocator, and borrowed
slice/text views are not yet fully native. The first native struct-element slice
is available for flattened integer-field structs: fields use deterministic,
naturally aligned AoS storage; construction, `push`, indexed copy-out and
replacement, `foreach`, statement-form `pop`, `clear`, `len`/`is_empty`, and
reserve/shrink operations lower natively. For these structs, `get`, `first`,
`last`, `peek`, and expression-form `pop` return a non-allocating aggregate
`Option<Struct>` represented by an explicit tag and typed field slots;
`is_some`, `is_none`, `unwrap_or`, `Some`/`None` construction, and assignment
also lower natively. Struct fields may be integer, `f32`, or `f64`; their
natural-width field storage and floating-point arithmetic/equality operations
remain native. Nested scalar aggregates flatten recursively into deterministic
naturally aligned leaf storage and remain native through list and checked-result
operations. Resource-bearing structs and arbitrary nested `list<T>` layouts are
not yet native.

---

# Chapter 11: Memory Model and Ownership

## 11.1 Ownership Overview

Aziky uses an ownership model inspired by Rust, with compile-time borrow checking.

## 11.2 Ownership Rules

### 11.2.1 Ownership Transfer (Move)

When a value is assigned to a new binding, ownership may transfer:

```aziky
let s1: string = "hello";
let s2: string = s1;    // s1 is moved to s2
// s1 is no longer valid
```

**Implementation status**: Semantic fallback currently clones many values, so complete move invalidation is not yet enforced for every value category. Reference borrow conflicts are checked; full move and linear-resource enforcement remains pending.

### 11.2.2 Borrowing

References borrow values without taking ownership:

```aziky
let x: i32 = 10i32;
let r: &i32 = &x;       // r borrows x
print(r);               // can read through r
print(x);               // x is still valid
```

## 11.3 Borrow Rules

### 11.3.1 Shared Borrows

Multiple shared borrows are allowed:

```aziky
let x: i32 = 10i32;
let r1: &i32 = &x;
let r2: &i32 = &x;      // OK: multiple shared borrows
print(r1);
print(r2);
```

### 11.3.2 Mutable Borrows

Only one mutable borrow is allowed at a time:

```aziky
let mut x: i32 = 10i32;
let r: &mut i32 = &mut x;
// cannot create another borrow while r is active
```

### 11.3.3 Borrow Conflicts

Shared and mutable borrows cannot coexist:

```aziky
let mut x: i32 = 10i32;
let r1: &i32 = &x;
let r2: &mut i32 = &mut x;    // ERROR: x already borrowed as shared
```

## 11.4 Borrow Tracking

The compiler tracks borrow state for each binding:

| State | Shared Borrows | Mutably Borrowed |
|-------|----------------|------------------|
| Free | 0 | No |
| Shared | ≥ 1 | No |
| Mutable | 0 | Yes |

**Transitions**:
- Free → Shared: Create shared reference
- Free → Mutable: Create mutable reference (requires `mut` binding)
- Shared → Free: All shared references go out of scope
- Mutable → Free: Mutable reference goes out of scope

## 11.5 Scope and Lifetime

### 11.5.1 Lexical Scoping

Variables are scoped to the block they're declared in:

```aziky
{
    let x: i32 = 10i32;
    // x is valid here
}
// x is out of scope
```

### 11.5.2 Borrow Lifetime

References must not outlive their referent:

```aziky
let r: &i32;
{
    let x: i32 = 10i32;
    r = &x;          // ERROR: x does not live long enough
}
// r would be dangling
```

## 11.6 Region Model

Aziky uses a region-based lifetime model:

| Region | Description |
|--------|-------------|
| `stack(scope_id)` | Stack-allocated values in a scope |
| `heap(owner_id)` | Linear generated-code heap ownership; the current backend still services allocations directly with `mmap`/`munmap` |
| `static` | Static values with program lifetime |

**Cleanup**: `heap_alloc(size)` must initialize a named immutable binding. That
binding is an owner, not a freely copyable integer: scalar copies, conversions,
raw-integer frees, and reassignment are rejected. The compiler freezes the
allocation size, inserts cleanup at lexical-scope exits, `break`, `continue`,
`return`, normal/failure `exit`, and validates manual `heap_free(owner, size)`
for liveness and matching size. A released pointer is cleared so later generated
cleanup is harmless, while a source-level double free is rejected as an
already-consumed owner. Within one lexical scope, a heap owner or runtime-native
scalar/struct `list<T>` owner can move into a new binding without copying its
descriptor slots; the source becomes unusable and has no cleanup obligation.
Runtime-native scalar/struct lists may also move into a function and return from
one through explicit descriptor ABIs, transferring cleanup exactly once to the
callee or caller. Moves through nested control flow, aggregate fields, borrowed
heap views, and general resource types remain pending. There is no garbage
collector.

The filesystem foundation applies the same rule to the opaque `Path`
and `File` types. `Path::new(raw)` moves a named owned `string` into an opaque
path after rejecting embedded NUL bytes. `File::create(path)` and
`File::open_read(path)` accept that validated path (and retain the original
string form during the transition) and produce named immutable
linear owners rather than copyable descriptor integers. `file.write_all(text)`
and `file.read(max_bytes)` borrow the live handle; `file.close()` consumes it.
Paths and write text are borrowed and remain usable. Owned `File` function
arguments move, `&File` arguments borrow without acquiring cleanup, and `File`
returns transfer the cleanup obligation to the caller. A borrowed handle cannot
be closed by the callee. Moving the owner invalidates the source, while normal
exits, failure exits, returns, and scope ends close live handles automatically.
Read results are owned, NUL-terminated UTF-8 byte strings allocated through the
shared generated-code allocator. Embedded NUL bytes in paths are rejected
before opening with stable code 105; open, write, and read failures use 102,
103, and 104. The final `std::fs`/`std::path` module facade remains planned.
Native filesystem syscalls are currently accepted only as a tested Linux x86-64
execution baseline; other object containers do not yet provide an equivalent
platform runtime contract.

---

# Chapter 12: Parallel Constructs

## 12.1 ParFor Statement

Execute loop iterations in parallel:

```
parfor-statement ::= 'parfor' identifier 'in' expression '..' expression block
                   | 'parfor' identifier 'in' expression '..' expression reduction
reduction ::= 'reduce' reduction-op 'into' identifier '{' expression '}'
reduction-op ::= 'sum' | 'min' | 'max'
```

## 12.2 Basic ParFor

```aziky
parfor worker in 0i32..4i32 {
    print("Worker ");
    print(worker);
    print("\n");
}
```

**Rules**:
1. Each iteration executes with an isolated environment snapshot
2. Outputs are merged by iteration index (deterministic order)
3. `break`, `continue`, and `return` are NOT allowed in parfor body
4. `exit` is NOT allowed in parfor body

## 12.3 Parallel Environment Isolation

To prevent data races:

1. **Snapshot**: Each iteration receives a copy of the environment
2. **Isolation**: Iterations cannot mutate shared state
3. **Deterministic Merge**: Results are combined in iteration order

## 12.4 Parallel Reductions

Perform parallel reductions with deterministic results:

```aziky
let mut sum_acc: i64 = 0i64;
let mut min_acc: i64 = 0i64;
let mut max_acc: i64 = 0i64;

parfor n in 1i64..6i64 reduce sum into sum_acc { n };
parfor n in -2i64..3i64 reduce min into min_acc { n };
parfor n in -2i64..3i64 reduce max into max_acc { n };
```

### 12.4.1 Reduction Operators

| Operator | Description | Identity |
|----------|-------------|----------|
| `sum` | Sum of values | 0 |
| `min` | Minimum value | Max value of type |
| `max` | Maximum value | Min value of type |

### 12.4.2 Reduction Rules

1. **Integer Only**: Target must be an integer type
2. **Deterministic**: Associative operations with fixed merge order
3. **Type Match**: Reduction expression must match target type
4. **Mutable Target**: Target must be declared `mut`

## 12.5 Parallelism Threshold

ParFor executes sequentially when:
1. Available parallelism is 1 thread
2. Iteration count is less than 64

## 12.6 Parallel Loop Safety

### 12.6.1 Prohibited Control Flow

The following are forbidden in parfor body:
- `break`
- `continue`
- `return`
- `exit`

**Rationale**: These would cause non-deterministic termination.

### 12.6.2 Side Effect Analysis

The compiler analyzes parfor bodies to:
1. Reject hidden shared-state writes
2. Enforce parallel purity contracts
3. Ensure deterministic observable output

---

# Appendix A: Complete Grammar Reference

## A.1 Program Grammar

```
program ::= item*
item ::= private-item | public-item
private-item ::= function-def | struct-def | enum-def | trait-def
               | trait-impl | inherent-impl | module-decl | use-decl
public-item ::= 'pub' (function-def | struct-def | enum-def | trait-def | use-decl)
module-decl ::= 'mod' identifier ';'
use-decl ::= 'use' identifier '::' identifier ('as' identifier)? ';'
```

## A.2 Function Grammar

```
function-def ::= 'fn' identifier '(' param-list? ')' ('->' type-name)? block
param-list ::= param (',' param)*
param ::= 'mut'? identifier ':' type-name
block ::= '{' statement* '}'
```

## A.3 Statement Grammar

```
statement ::= let-stmt
            | assign-stmt
            | assign-index-stmt
            | assign-field-stmt
            | call-stmt
            | method-call-stmt
            | return-stmt
            | print-stmt
            | exit-stmt
            | benchloop-stmt
            | block-stmt
            | if-stmt
            | while-stmt
            | loop-stmt
            | for-stmt
            | parfor-stmt
            | foreach-stmt
            | assert-stmt
            | panic-stmt
            | break-stmt
            | continue-stmt

let-stmt ::= 'let' 'mut'? identifier (':' type-name)? '=' expression ';'
assign-stmt ::= identifier '=' expression ';'
assign-index-stmt ::= identifier '[' expression ']' '=' expression ';'
assign-field-stmt ::= identifier '.' identifier '=' expression ';'
call-stmt ::= (identifier | identifier '::' identifier) '(' arg-list? ')' ';'
method-call-stmt ::= identifier '.' identifier '(' arg-list? ')' ';'
return-stmt ::= 'return' expression? ';'
print-stmt ::= 'print' '(' expression ')' ';'
exit-stmt ::= 'exit' '(' expression ')' ';'
benchloop-stmt ::= 'benchloop' '(' expression ')' ';'
block-stmt ::= '{' statement* '}'
if-stmt ::= 'if' expression block ('else' (block | if-stmt))?
while-stmt ::= 'while' expression block
loop-stmt ::= 'loop' block
for-stmt ::= 'for' identifier 'in' expression '..' expression block
parfor-stmt ::= 'parfor' identifier 'in' expression '..' expression (block | reduction)
reduction ::= 'reduce' ('sum' | 'min' | 'max') 'into' identifier '{' expression '}'
foreach-stmt ::= 'foreach' identifier 'in' expression block
assert-stmt ::= 'assert' '(' expression (',' expression)? ')' ';'
panic-stmt ::= 'panic' '(' expression ')' ';'
break-stmt ::= 'break' ';'
continue-stmt ::= 'continue' ';'
```

## A.4 Expression Grammar

```
expression ::= logical-or-expr

logical-or-expr ::= logical-and-expr ('||' logical-and-expr)*
logical-and-expr ::= comparison-expr ('&&' comparison-expr)*
comparison-expr ::= bit-or-expr (('==' | '!=' | '<' | '<=' | '>' | '>=') bit-or-expr)?
bit-or-expr ::= bit-xor-expr ('|' bit-xor-expr)*
bit-xor-expr ::= bit-and-expr ('^' bit-and-expr)*
bit-and-expr ::= shift-expr ('&' shift-expr)*
shift-expr ::= add-expr (('<<' | '>>') add-expr)*
add-expr ::= mul-expr (('+' | '-') mul-expr)*
mul-expr ::= unary-expr (('*' | '/' | '%') unary-expr)*
unary-expr ::= ('+' | '-' | '!' | '&' 'mut'?) unary-expr | postfix-expr
postfix-expr ::= primary-expr ('.' identifier '(' arg-list? ')' | '[' expression ']')*
primary-expr ::= literal
              | match-expr
              | identifier ('(' arg-list? ')'
                           | '::' identifier (('(' arg-list? ')') | ('{' field-init-list '}'))?
                           | '{' field-init-list '}')?
              | array-literal
              | dict-literal
              | '(' expression ')'

literal ::= 'true' | 'false' | string-literal | char-literal | number-literal
number-literal ::= digits type-suffix?
type-suffix ::= 'u8' | 'u16' | 'u32' | 'u64' | 'u128' | 'usize'
              | 'i8' | 'i16' | 'i32' | 'i64' | 'i128' | 'isize'
              | 'f32' | 'f64' | 'byte' | 'bool'
array-literal ::= '[' expression-list? ']'
dict-literal ::= '{' entry-list? '}'
entry ::= string-literal ':' expression
arg-list ::= expression (',' expression)*
field-init-list ::= field-init (',' field-init)* ','?
field-init ::= identifier ':' expression
expression-list ::= expression (',' expression)*
entry-list ::= entry (',' entry)* ','?
match-expr ::= 'match' expression '{' match-arm (',' match-arm)* ','? '}'
match-arm ::= match-pattern '=>' expression
match-pattern ::= '_'
                | identifier '::' identifier
                | identifier '::' identifier '(' pattern-bindings? ')'
                | identifier '::' identifier '{' named-patterns? '}'
pattern-bindings ::= pattern-binding (',' pattern-binding)* ','?
pattern-binding ::= identifier | '_'
named-patterns ::= named-pattern (',' named-pattern)* ','?
named-pattern ::= identifier | identifier ':' pattern-binding
```

## A.5 Type Grammar

```
type-name ::= 'bool' | 'byte' | 'char' | 'string'
           | 'u8' | 'u16' | 'u32' | 'u64' | 'u128' | 'usize'
           | 'i8' | 'i16' | 'i32' | 'i64' | 'i128' | 'isize'
           | 'f32' | 'f64'
           | identifier                                    // non-generic struct or enum
           | identifier '<' type-name (',' type-name)* '>'  // applied generic enum
           | '[' type-name ';' integer-literal ']'          // array
           | 'dict' '<' type-name ',' type-name '>'         // dictionary
           | 'list' '<' type-name '>'                       // owned dynamic list
           | 'map' '<' type-name ',' type-name '>'          // owned dynamic map
           | '&' 'mut'? type-name                           // reference
```

## A.6 Struct Grammar

```
struct-def ::= 'struct' identifier '{' field-list '}'
field-list ::= field (field-sep field)* field-sep?
field ::= identifier ':' type-name | 'embed' type-name
field-sep ::= ';' | ','
```

## A.7 Enum Grammar

```
enum-def ::= 'enum' identifier type-params? '{' variant-list '}'
type-params ::= '<' identifier (',' identifier)* '>'
variant-list ::= variant (',' variant)* ','?
variant ::= identifier
          | identifier '(' type-name (',' type-name)* ','? ')'
          | identifier '{' field-list '}'
```

## A.8 Trait Grammar

```
trait-def ::= 'trait' identifier '{' method-sig* '}'
method-sig ::= 'fn' identifier '(' param-list? ')' ('->' type-name)? ';'
trait-impl ::= 'impl' identifier 'for' identifier '{' function-def* '}'
inherent-impl ::= 'impl' identifier '{' function-def* '}'
```

---

# Appendix B: Keyword Reference

| Keyword | Category | Description |
|---------|----------|-------------|
| `fn` | Definition | Function definition |
| `struct` | Definition | Struct type definition |
| `enum` | Definition | Enum type definition |
| `trait` | Definition | Trait definition |
| `impl` | Definition | Inherent or trait implementation |
| `embed` | Definition | Embedded struct field |
| `pub` | Visibility | Export a declaration or selective import |
| `mod` | Module | Declare a sibling source module |
| `use` | Module | Import one public direct-child item |
| `as` | Module | Assign a local import or re-export alias |
| `let` | Binding | Variable declaration |
| `mut` | Binding | Mutable binding modifier |
| `if` | Control Flow | Conditional branch |
| `else` | Control Flow | Alternative branch |
| `match` | Control Flow | Exhaustive enum value selection |
| `while` | Control Flow | Conditional loop |
| `loop` | Control Flow | Infinite loop |
| `for` | Control Flow | Range-based loop |
| `parfor` | Control Flow | Parallel for loop |
| `foreach` | Control Flow | Iterator-based loop |
| `in` | Control Flow | Range membership |
| `break` | Control Flow | Loop exit |
| `continue` | Control Flow | Loop iteration skip |
| `return` | Control Flow | Function return |
| `assert` | Safety | Compile-time assertion |
| `panic` | Safety | Runtime error |
| `true` | Literal | Boolean true |
| `false` | Literal | Boolean false |
| `print` | I/O | Standard output |
| `exit` | I/O | Program termination |
| `benchloop` | Benchmark | Benchmark loop |

---

# Appendix C: Operator Precedence Table

Operators are listed from highest precedence (tightest binding) to lowest precedence.

| Precedence | Operator | Associativity | Description |
|------------|----------|---------------|-------------|
| 1 (highest) | `()` `[]` `.` `::` | Left to Right | Call, Index, Field, Path |
| 2 | `+` `-` `!` `&` `&mut` | Right to Left | Unary plus, negation, NOT, borrow |
| 3 | `*` `/` `%` | Left to Right | Multiplication, Division, Modulo |
| 4 | `+` `-` | Left to Right | Addition, Subtraction |
| 5 | `<<` `>>` | Left to Right | Left shift, Right shift |
| 6 | `&` | Left to Right | Bitwise AND |
| 7 | `^` | Left to Right | Bitwise XOR |
| 8 | `\|` | Left to Right | Bitwise OR |
| 9 | `==` `!=` `<` `<=` `>` `>=` | Left to Right | Comparison |
| 10 | `&&` | Left to Right | Logical AND |
| 11 | `\|\|` | Left to Right | Logical OR |
| 12 (lowest) | `=` | Right to Left | Assignment |

---

# Appendix D: Built-in Methods Reference

Unless a runtime note says otherwise, these methods are type-checked and executable
through deterministic semantic lowering. Integer/bool container `len`/`is_empty`
and supported direct associated functions have runtime-generic coverage; broad
runtime-native text, character, and dynamic-collection lowering remains planned.
Method arity is checked per API and diagnostics report expected and received
argument counts.

## D.1 Universal Methods

Available on all types:

| Method | Return Type | Description |
|--------|-------------|-------------|
| `type_name()` | `string` | Name of the type |
| `hash64()` | `u64` | FNV-1a hash of the value |

## D.2 String Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `len()` | `u64` | UTF-8 byte length |
| `char_count()` | `u64` | Unicode scalar count |
| `is_empty()` | `bool` | Whether empty |
| `trim()` | `string` | Whitespace-trimmed copy |
| `to_upper()` | `string` | ASCII-uppercase copy |
| `to_lower()` | `string` | ASCII-lowercase copy |
| `contains(pattern)` | `bool` | Contains a `string` or `char` pattern |
| `starts_with(pattern)` | `bool` | Starts with a `string` or `char` pattern |
| `ends_with(pattern)` | `bool` | Ends with a `string` or `char` pattern |
| `replace(from, to)` | `string` | Replace all string/character pattern matches |
| `repeat(count)` | `string` | Repeat with overflow and allocation checks |
| `char_at(index)` | `Option<char>` | Checked Unicode-scalar lookup |
| `parse_bool()` | `Result<bool, string>` | Checked boolean parsing |
| `parse_i8()` ... `parse_i128()` | `Result<I, string>` | Checked signed-integer parsing |
| `parse_u8()` ... `parse_u128()` | `Result<U, string>` | Checked unsigned-integer parsing |
| `parse_f32()` / `parse_f64()` | `Result<F, string>` | Checked finite floating-point parsing |
| `to_bool()` | `bool` | Parse as boolean |
| `to_i8()` | `i8` | Parse as i8 |
| `to_i16()` | `i16` | Parse as i16 |
| `to_i32()` | `i32` | Parse as i32 |
| `to_i64()` | `i64` | Parse as i64 |
| `to_i128()` | `i128` | Parse as i128 |
| `to_u8()` | `u8` | Parse as u8 |
| `to_u16()` | `u16` | Parse as u16 |
| `to_u32()` | `u32` | Parse as u32 |
| `to_u64()` | `u64` | Parse as u64 |
| `to_u128()` | `u128` | Parse as u128 |
| `to_f32()` | `f32` | Parse as f32 |
| `to_f64()` | `f64` | Parse as f64 |

The `parse_*` family preserves malformed or out-of-range input as `Err(string)`.
The native x86-64 lowering currently implements the fixed-width integer family
through 64 bits and exact lowercase `true`/`false` boolean parsing without libc
or locale state. Invalid integer and boolean input returns deterministic owned
error text. Native floating parsing remains part of the standard-library core
completion work.
The older `to_*` conversions diagnose malformed input and remain available as
explicit failure-producing conversions during the compatibility transition.

## D.3 Character Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `to_str()` / `to_string()` | `string` | UTF-8 encoding of the scalar |
| `to_u32()` | `u32` | Unicode scalar number |
| `to_upper()` | `string` | Unicode uppercase mapping, including expansion |
| `to_lower()` | `string` | Unicode lowercase mapping, including expansion |
| `to_ascii_upper()` | `char` | ASCII-only uppercase conversion |
| `to_ascii_lower()` | `char` | ASCII-only lowercase conversion |
| `is_alphabetic()` | `bool` | Unicode alphabetic classification |
| `is_alphanumeric()` | `bool` | Unicode alphanumeric classification |
| `is_numeric()` | `bool` | Unicode numeric classification |
| `is_whitespace()` | `bool` | Unicode whitespace classification |
| `is_uppercase()` | `bool` | Unicode uppercase classification |
| `is_lowercase()` | `bool` | Unicode lowercase classification |
| `is_ascii()` | `bool` | Whether the scalar is ASCII |
| `is_ascii_digit()` | `bool` | Whether it is an ASCII decimal digit |

## D.4 Numeric Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `to_str()` | `string` | String representation |
| `to_string()` | `string` | String representation |
| `to_bool()` | `bool` | Convert (0 → false, 1 → true) |
| `to_i8()` | `i8` | Convert to i8 |
| `to_i16()` | `i16` | Convert to i16 |
| `to_i32()` | `i32` | Convert to i32 |
| `to_i64()` | `i64` | Convert to i64 |
| `to_i128()` | `i128` | Convert to i128 |
| `to_u8()` | `u8` | Convert to u8 |
| `to_u16()` | `u16` | Convert to u16 |
| `to_u32()` | `u32` | Convert to u32 |
| `to_u64()` | `u64` | Convert to u64 |
| `to_u128()` | `u128` | Convert to u128 |
| `to_f32()` | `f32` | Convert to f32 |
| `to_f64()` | `f64` | Convert to f64 |
| `to_char_checked()` | `Option<char>` | Convert an integer Unicode scalar number safely |
| `abs()` | Self | Absolute value (signed only) |
| `is_positive()` | `bool` | Whether > 0 |
| `is_negative()` | `bool` | Whether < 0 |

## D.5 Boolean Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `to_str()` | `string` | "true" or "false" |
| `not()` | `bool` | Logical negation |

## D.6 Array Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `len()` | `u64` | Element count |
| `is_empty()` | `bool` | Whether empty |
| `contains(v)` | `bool` | Whether an equal typed element exists |
| `get(index)` | `Option<T>` | Checked indexed lookup |
| `peek()` | element type | Last element |
| `push(v)` | void | Append element |
| `pop()` | void | Remove last element |
| `sort()` | void | Unstable sort |
| `sort_stable()` | void | Stable sort |
| `sort_unstable()` | void | Unstable sort |
| `sort_radix_unstable()` | void | Radix sort |
| `sort_radix_stable()` | void | Stable radix sort |
| `sort_by(fn)` | void | Sort with comparator |

## D.7 Dictionary Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `len()` | `u64` | Entry count |
| `is_empty()` | `bool` | Whether empty |
| `contains_key(k)` | `bool` | Whether a typed key exists |
| `get(k)` | `Option<V>` | Checked key lookup |
| `keys()` | `[string; N]` | Array of keys |
| `set(k, v)` | void | Set key-value |
| `remove(k)` | void | Remove key |

## D.8 Owned List Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `len()` / `is_empty()` | `u64` / `bool` | Collection state |
| `contains(v)` | `bool` | Whether an equal typed element exists |
| `get(index)` | `Option<T>` | Checked indexed lookup |
| `first()` / `last()` | `Option<T>` | Checked boundary-element lookup |
| `peek()` | `Option<T>` | Checked last-element lookup |
| `push(v)` | void | Append an element |
| `pop()` | scalar `Option<T>` for native scalar-list value use | Remove and return the last element, or `None` |
| `clear()` | void | Remove every element |
| `reserve(n)` | void | Reserve at least `n` additional element slots |
| `shrink_to(n)` | void | Reduce capacity without going below `max(len, n)` |
| `shrink_to_fit()` | void | Reduce capacity to the current length |

## D.9 Owned Map Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `len()` / `is_empty()` | `u64` / `bool` | Collection state |
| `contains_key(k)` | `bool` | Whether a typed key exists |
| `get(k)` | `Option<V>` | Checked key lookup |
| `keys()` | `[K; N]` | Typed keys in deterministic map order |
| `set(k, v)` / `remove(k)` | void | Insert/update or remove an entry |
| `clear()` | void | Remove every entry |

## D.10 Option and Result Methods

| Receiver | Method | Return Type | Description |
|----------|--------|-------------|-------------|
| `Option<T>` | `is_some()` / `is_none()` | `bool` | Query the active variant |
| `Option<T>` | `unwrap_or(fallback)` | `T` | Return the payload or typed fallback |
| `Result<T, E>` | `is_ok()` / `is_err()` | `bool` | Query the active variant |
| `Result<T, E>` | `unwrap_or(fallback)` | `T` | Return `Ok` or a typed fallback for `Err` |

## D.11 Comparable Methods

Available when both values have the same comparable type:

| Method | Return Type | Description |
|--------|-------------|-------------|
| `min(other)` | Self | Lesser value |
| `max(other)` | Self | Greater value |
| `clamp(min, max)` | Self | Value restricted to inclusive bounds |

`clamp` diagnoses reversed bounds. Float comparisons reject NaN. Arguments are
coerced only through the language's existing explicit-compatible coercion rules.

## D.12 Enum Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `to_str()` | `string` | Deterministic variant and payload representation |

---

# Appendix E: Error Diagnostics

## E.1 Diagnostic Format

```
<message> at <line>:<column>
 --> <path>
<source line>
 <caret>
stack:
  1: <context>
  2: <context>
  ...
```

## E.2 Common Errors

### E.2.1 Lexical Errors

| Error | Cause |
|-------|-------|
| `unexpected character: X` | Invalid character in source |
| `unterminated string literal` | Missing closing quote |
| `unsupported escape: \X` | Unknown escape sequence |

### E.2.2 Parse Errors

| Error | Cause |
|-------|-------|
| `expected 'fn'` | Missing function keyword |
| `expected identifier` | Missing required name |
| `expected '{'` | Missing block delimiter |
| `expected ';'` | Missing statement terminator |
| `expected expression` | Invalid expression syntax |

### E.2.3 Type Errors

| Error | Cause |
|-------|-------|
| `type mismatch: expected X, got Y` | Type incompatibility |
| `cannot coerce negative to unsigned` | Negative unsigned literal |
| `integer out of range` | Value exceeds type bounds |
| `cannot convert X to Y` | Invalid type conversion |

### E.2.4 Borrow Errors

| Error | Cause |
|-------|-------|
| `cannot assign to immutable 'X'` | Assignment to immutable binding |
| `cannot take &mut of immutable 'X'` | Mutable borrow of immutable |
| `X already borrowed` | Borrow conflict |
| `X already mutably borrowed` | Borrow conflict |

### E.2.5 Semantic Errors

| Error | Cause |
|-------|-------|
| `unknown identifier 'X'` | Undefined variable |
| `unknown function 'X'` | Undefined function |
| `unknown struct 'X'` | Undefined type |
| `redefinition of 'X'` | Duplicate definition |
| `break used outside loop` | Invalid control flow |
| `return value is required` | Missing return value |

## E.3 Context Stack

When errors propagate, context is added:

```
type mismatch: expected i32, got string at 10:5
stack:
  1: statement::assign at 10:5
  2: in function 'main'
```

---

# Index

**Symbols**
- `!` (logical NOT), 49
- `!=` (not equal), 51
- `%` (modulo), 48
- `&` (bitwise AND), 50
- `&&` (logical AND), 52
- `&mut` (mutable borrow), 53
- `*` (multiply), 48
- `+` (add), 47
- `-` (subtract), 47
- `->` (return type), 97
- `.` (field access), 53
- `..` (range), 55
- `/` (divide), 48
- `::` (path), 56
- `<` (less than), 51
- `<<` (left shift), 50
- `<=` (less or equal), 51
- `=` (assignment), 69
- `==` (equal), 51
- `>` (greater than), 51
- `>=` (greater or equal), 51
- `>>` (right shift), 50
- `[]` (index), 54
- `^` (bitwise XOR), 50
- `|` (bitwise OR), 50
- `||` (logical OR), 52

**Keywords**
- `assert`, 80
- `benchloop`, 64
- `break`, 78
- `continue`, 79
- `else`, 74
- `embed`, 90
- `enum`, 85
- `exit`, 73
- `false`, 44
- `fn`, 97
- `for`, 76
- `foreach`, 77
- `if`, 74
- `impl`, 103
- `let`, 68
- `loop`, 75
- `mut`, 68
- `panic`, 81
- `parfor`, 122
- `print`, 72
- `return`, 79
- `struct`, 87
- `trait`, 102
- `true`, 44
- `while`, 75

**Types**
- `bool`, 40
- `Channel<u64>`, native linear FIFO channel owner
- `byte`, 40
- `dict<K, V>`, 43
- `f32`, 39
- `f64`, 39
- `i16`, 38
- `i32`, 38
- `i64`, 38
- `i8`, 38
- `i128`, 38
- `isize`, 38
- `string`, 40
- `Receiver<u64>`, native linear receive endpoint
- `Sender<u64>`, native linear send endpoint
- `Thread`, native linear worker owner
- `u16`, 38
- `u32`, 38
- `u64`, 38
- `u8`, 38
- `u128`, 38
- `usize`, 38
- `[T; N]` (array), 42

## Native threads and deterministic channels

The Linux x86-64 platform surface provides the opaque linear owner `Thread`
and the SPSC channel types `Channel<u64>`, `Sender<u64>`, and `Receiver<u64>`.
They cannot be constructed from integers, copied, reassigned, or used after a
consuming operation.

```aziky
fn produce(sender: Sender<u64>, value: u64) -> u64 {
    sender.send(value);
    sender.close();
    return value;
}

fn main() {
    let channel: Channel<u64> = Channel::bounded(4u64);
    let sender: Sender<u64> = channel.sender();
    let receiver: Receiver<u64> = channel.receiver();
    let worker: Thread = Thread::spawn(produce, sender, 42u64);
    let value: u64 = receiver.recv();
    receiver.close();
    let completion: u64 = worker.join();
    exit(value + completion);
}
```

`Thread::spawn` requires a statically named worker. Worker parameters may be
scalar values or moved channel endpoints; return type is `u64` or absent.
`join()` consumes the owner. Dropping a live lexical `Thread` performs a scoped
join, so native tasks cannot outlive their Aziky ownership scope. A worker panic
is observed by the joiner as stable code `101`; native launch failure is `109`.

`Channel::bounded(capacity)` requires a nonzero capacity.
`Channel::unbounded()` uses sparse virtual reservation with pages committed on
demand. Calling `sender()` and `receiver()` extracts each endpoint exactly once.
The channel is FIFO, `send` and `recv` block using native futex waits, endpoint
close wakes the peer, and scope cleanup closes live endpoints. Creation failure
uses `110`, send after receiver close uses `111`, and receive after sender close
and drain uses `112`. The accepted surface currently supports `u64` elements
and one producer/one consumer; broader generic and multi-producer channels are
not implied by these types.

---

**End of The Aziky Programming Language Reference Manual**
