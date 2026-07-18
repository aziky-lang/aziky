# Aziky Language Support for VS Code

This extension provides first-class `.azk` editing without a network service or
separate formatter implementation.

## Included language features

- TextMate highlighting for every lexer keyword, primitive type, literal,
  operator, declaration, line comment, and nested block comment;
- bracket matching, auto-closing pairs, indentation, folding, and snippets;
- compiler-owned **Format Document** and format-on-save through
  `aziky fmt --stdin`;
- immediate lexical diagnostics while typing;
- authoritative compiler and linter diagnostics on open and save through the
  versioned `aziky-diagnostics-v1` JSON protocol;
- workspace completion for keywords, types, functions, types, modules,
  parameters, aliases, and bindings;
- hover documentation and declaration signatures;
- local and workspace go-to-definition, including module files.

Semantic checks intentionally run against saved project files so module and
package resolution remains compiler-authoritative. Unsaved buffers still get
immediate lexical diagnostics and can always be formatted because formatting
uses stdin.

## Local installation

From this directory:

```text
npm install
npm test
npm run package
code --install-extension dist/aziky-language-0.1.1.vsix --force
```

Re-run the last two commands after rebuilding the extension. Marketplace
publication is intentionally separate from repository releases.

The extension looks for the compiler in this order:

1. `aziky.compiler.path`;
2. `target/debug/aziky` or `target/release/aziky` in the
   workspace;
3. `cargo run --quiet --` in an Aziky compiler workspace;
4. `aziky` on `PATH`.

On Windows the local build lookup uses `aziky.exe`. All compiler
processes are launched without a shell.

## Settings

- `aziky.compiler.path`: explicit compiler executable;
- `aziky.diagnostics.enable`: compiler and linter diagnostics;
- `aziky.diagnostics.whileTyping`: editor-side lexical diagnostics;
- `aziky.diagnostics.debounceMilliseconds`: typing diagnostic delay.

Aziky is registered as its own default formatter and format-on-save is enabled
for `[aziky]` files. Workspace or user settings can override either value.

## Architecture boundary

The grammar provides fast lexical coloring. `languageService.js` maintains the
editor symbol index used by completion, hover, and definitions. Formatting and
semantic correctness remain owned by the Rust compiler. The symbol service is
kept independent of VS Code so it can be moved behind an editor-neutral
Language Server Protocol transport when additional editors need the same
features.
