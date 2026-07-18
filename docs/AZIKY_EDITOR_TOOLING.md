# Aziky Editor Tooling

Status: accepted VS Code baseline (2026-07-18).

## Principles and boundary

Aziky editor tooling is offline, deterministic, cross-platform, and
compiler-owned wherever language correctness is involved. The extension does
not ship another formatter or semantic compiler. It invokes a configured local
compiler without a shell and can be installed from a local VSIX without a
Marketplace, public repository, or network service.

Fast lexical behavior stays in the editor:

- TextMate tokenization and coloring;
- brackets, comments, folding, indentation, and snippets;
- a lightweight workspace declaration index for completion, hover, and
  definition navigation;
- compiler-compatible lexical errors while an unsaved buffer is changing.

Authoritative behavior stays in `aziky`:

- formatting through `fmt --stdin`;
- package/module-aware parsing and semantics through `check`;
- naming and portable-source policy through `lint`.

Semantic diagnostics run on open and save because project checking must see a
coherent saved module graph. Unsaved buffers still receive immediate lexical
diagnostics. This avoids temporary source trees, file mutation, and a second
module resolver inside the extension.

## Machine contracts

### Formatting

```text
aziky fmt --stdin
```

The command consumes one UTF-8 source buffer and emits only the formatted UTF-8
buffer. Failure uses a nonzero exit status and stderr. The same Rust formatter
backs file-writing, check, stdout, and stdin modes.

### Diagnostics

```text
aziky check main.azk --diagnostic-format=json
aziky lint file.azk --diagnostic-format=json
```

Both commands emit one line using this stable schema:

```json
{
  "schema": "aziky-diagnostics-v1",
  "status": "error",
  "diagnostics": [
    {
      "severity": "error",
      "message": "unknown identifier 'value'",
      "path": "/project/src/main.azk",
      "line": 4,
      "column": 12
    }
  ]
}
```

Lint warnings additionally carry their stable `AZK-LNNN` code. Lines and
columns are one-based in the protocol. Errors retain nonzero process status.

## VS Code extension

The implementation is in `editors/vscode` and includes:

- `syntaxes/aziky.tmLanguage.json`: lexer-aligned grammar, including recursive
  nested block comments;
- `language-configuration.json`: comments, brackets, pairs, folding, and
  indentation;
- `languageService.js`: VS Code-independent lexical and symbol service;
- `extension.js`: formatting, diagnostics, completion, hover, and definition
  providers;
- Node regression tests and deterministic local packaging.

The compiler is resolved from an explicit setting, workspace debug/release
build, compiler-workspace Cargo fallback, or `PATH`. Windows executable naming
is handled explicitly and no command uses a shell.

## Packaging

```text
cd editors/vscode
npm install
npm test
npm run package
code --install-extension dist/aziky-language-0.1.1.vsix --force
```

The VSIX is a local artifact and `dist` remains ignored by Git.

## Later multi-editor milestone

The current completion/hover/definition service is intentionally isolated from
the VS Code API, but is still hosted in the extension process. A standalone LSP
transport remains appropriate when Aziky supports multiple editors, parser
recovery, incremental project analysis, precise reference finding/rename, and
semantic tokens. That extraction must reuse compiler spans and module identity;
it must not introduce a JavaScript semantic implementation.
