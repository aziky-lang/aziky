'use strict';

const vscode = require('vscode');
const childProcess = require('child_process');
const fs = require('fs');
const path = require('path');
const service = require('./languageService');

const LANGUAGE_ID = 'aziky';
const EXCLUDED_FILES = '**/{.git,.aziky,target,node_modules}/**';

function activate(context) {
  const output = vscode.window.createOutputChannel('Aziky');
  const diagnostics = vscode.languages.createDiagnosticCollection('aziky');
  const documentSymbols = new Map();
  const lexicalByUri = new Map();
  const compilerByUri = new Map();
  const compilerUrisByRoot = new Map();
  const timers = new Map();
  let compilerWarningShown = false;

  const configuration = (uri) => vscode.workspace.getConfiguration('aziky', uri);
  const isAziky = (document) => document.languageId === LANGUAGE_ID && document.uri.scheme === 'file';

  function publish(uri) {
    const key = uri.toString();
    diagnostics.set(uri, [...(lexicalByUri.get(key) || []), ...(compilerByUri.get(key) || [])]);
  }

  function index(document) {
    if (!isAziky(document)) return;
    indexText(document.uri, document.getText());
  }

  function indexText(uri, source) {
    documentSymbols.set(uri.toString(), service.indexDocument(uri.toString(), source));
  }

  function updateLexical(document) {
    if (!isAziky(document)) return;
    const key = document.uri.toString();
    if (!configuration(document.uri).get('diagnostics.whileTyping', true)) {
      lexicalByUri.delete(key);
      publish(document.uri);
      return;
    }
    const result = service.lexicalAnalysis(document.getText());
    const values = result.diagnostics.map((item) => {
      const diagnostic = new vscode.Diagnostic(
        new vscode.Range(document.positionAt(item.start), document.positionAt(Math.max(item.start + 1, item.end))),
        item.message,
        vscode.DiagnosticSeverity.Error
      );
      diagnostic.source = 'aziky lexer';
      diagnostic.code = item.code;
      return diagnostic;
    });
    lexicalByUri.set(key, values);
    publish(document.uri);
  }

  function scheduleLexical(document) {
    const key = document.uri.toString();
    const previous = timers.get(key);
    if (previous) clearTimeout(previous);
    const delay = configuration(document.uri).get('diagnostics.debounceMilliseconds', 250);
    timers.set(key, setTimeout(() => {
      timers.delete(key);
      updateLexical(document);
    }, delay));
  }

  async function readAndIndex(uri) {
    try {
      const source = new TextDecoder('utf-8', { fatal: true }).decode(await vscode.workspace.fs.readFile(uri));
      indexText(uri, source);
    } catch (error) {
      output.appendLine(`Unable to index ${uri.fsPath}: ${error.message}`);
    }
  }

  async function indexWorkspace() {
    const files = await vscode.workspace.findFiles('**/*.azk', EXCLUDED_FILES, 10000);
    files.sort((left, right) => left.toString().localeCompare(right.toString()));
    for (const uri of files) await readAndIndex(uri);
  }

  function compilerInvocation(uri) {
    const configured = configuration(uri).get('compiler.path', '').trim();
    if (configured) return { command: configured, prefix: [] };
    const folder = vscode.workspace.getWorkspaceFolder(uri);
    if (folder) {
      const binary = process.platform === 'win32' ? 'aziky.exe' : 'aziky';
      for (const profile of ['debug', 'release']) {
        const candidate = path.join(folder.uri.fsPath, 'target', profile, binary);
        if (fs.existsSync(candidate)) return { command: candidate, prefix: [] };
      }
      const manifest = path.join(folder.uri.fsPath, 'Cargo.toml');
      if (fs.existsSync(manifest) && fs.readFileSync(manifest, 'utf8').includes('name = "aziky"')) {
        return { command: 'cargo', prefix: ['run', '--quiet', '--'] };
      }
    }
    return { command: 'aziky', prefix: [] };
  }

  function runCompiler(uri, args, input, timeoutMs = 30000) {
    const invocation = compilerInvocation(uri);
    const cwd = vscode.workspace.getWorkspaceFolder(uri)?.uri.fsPath || path.dirname(uri.fsPath);
    return new Promise((resolve, reject) => {
      const child = childProcess.spawn(invocation.command, [...invocation.prefix, ...args], {
        cwd,
        windowsHide: true,
        shell: false,
        stdio: ['pipe', 'pipe', 'pipe']
      });
      let stdout = '';
      let stderr = '';
      let settled = false;
      const timeout = setTimeout(() => {
        child.kill();
        if (!settled) {
          settled = true;
          reject(new Error(`Aziky compiler timed out after ${timeoutMs} ms`));
        }
      }, timeoutMs);
      child.stdout.setEncoding('utf8');
      child.stderr.setEncoding('utf8');
      child.stdout.on('data', (chunk) => { stdout += chunk; });
      child.stderr.on('data', (chunk) => { stderr += chunk; });
      child.on('error', (error) => {
        clearTimeout(timeout);
        if (!settled) { settled = true; reject(error); }
      });
      child.on('close', (code) => {
        clearTimeout(timeout);
        if (!settled) { settled = true; resolve({ code: code ?? 1, stdout, stderr }); }
      });
      if (input !== undefined) child.stdin.end(input, 'utf8');
      else child.stdin.end();
    });
  }

  function findManifest(startPath) {
    let directory = path.dirname(startPath);
    while (true) {
      const candidate = path.join(directory, 'Aziky.toml');
      if (fs.existsSync(candidate)) return candidate;
      const parent = path.dirname(directory);
      if (parent === directory) return undefined;
      directory = parent;
    }
  }

  function checkInputFor(document) {
    const manifest = findManifest(document.uri.fsPath);
    if (!manifest) {
      const masked = service.lexicalAnalysis(document.getText()).masked;
      return /\bfn\s+main\s*\(/.test(masked) ? document.uri.fsPath : undefined;
    }
    try {
      const source = fs.readFileSync(manifest, 'utf8');
      const packageSection = source.match(/\[package\]([\s\S]*?)(?:\n\s*\[|$)/);
      const entry = packageSection?.[1].match(/^\s*entry\s*=\s*["']([^"']+)["']/m)?.[1];
      return entry ? path.resolve(path.dirname(manifest), entry) : undefined;
    } catch (_) {
      return undefined;
    }
  }

  function parseMachineOutput(result) {
    for (const stream of [result.stdout, result.stderr]) {
      const lines = stream.trim().split(/\r?\n/).reverse();
      for (const line of lines) {
        if (!line.startsWith('{')) continue;
        try {
          const payload = JSON.parse(line);
          if (payload.schema === 'aziky-diagnostics-v1' && Array.isArray(payload.diagnostics)) return payload;
        } catch (_) { /* Continue looking for a protocol payload. */ }
      }
    }
    return undefined;
  }

  function diagnosticUri(item, cwd) {
    if (!item.path || item.path.startsWith('<')) return undefined;
    const absolute = path.isAbsolute(item.path) ? item.path : path.resolve(cwd, item.path);
    return vscode.Uri.file(path.normalize(absolute));
  }

  function toVsDiagnostic(item) {
    const line = Math.max(0, Number(item.line || 1) - 1);
    const column = Math.max(0, Number(item.column || 1) - 1);
    const diagnostic = new vscode.Diagnostic(
      new vscode.Range(line, column, line, column + 1),
      String(item.message || 'Aziky diagnostic'),
      item.severity === 'warning' ? vscode.DiagnosticSeverity.Warning : vscode.DiagnosticSeverity.Error
    );
    diagnostic.source = item.severity === 'warning' ? 'aziky lint' : 'aziky compiler';
    if (item.code) diagnostic.code = item.code;
    return diagnostic;
  }

  async function checkDocument(document, explicit = false) {
    if (!isAziky(document) || !configuration(document.uri).get('diagnostics.enable', true)) return;
    if (document.isDirty) {
      if (explicit) vscode.window.showInformationMessage('Save the Aziky document to run project-aware semantic diagnostics.');
      return;
    }
    const checkInput = checkInputFor(document);
    const rootKey = checkInput || document.uri.fsPath;
    const cwd = vscode.workspace.getWorkspaceFolder(document.uri)?.uri.fsPath || path.dirname(document.uri.fsPath);
    try {
      const [checkResult, lintResult] = await Promise.all([
        checkInput
          ? runCompiler(document.uri, ['check', checkInput, '--diagnostic-format=json'])
          : Promise.resolve({
              code: 0,
              stdout: '{"schema":"aziky-diagnostics-v1","status":"ok","diagnostics":[]}',
              stderr: ''
            }),
        runCompiler(document.uri, ['lint', document.uri.fsPath, '--diagnostic-format=json'])
      ]);
      const checkPayload = parseMachineOutput(checkResult);
      const lintPayload = parseMachineOutput(lintResult);
      if (!checkPayload) throw new Error(checkResult.stderr.trim() || checkResult.stdout.trim() || 'compiler returned no diagnostic payload');
      if (!lintPayload) throw new Error(lintResult.stderr.trim() || lintResult.stdout.trim() || 'linter returned no diagnostic payload');

      const previousUris = compilerUrisByRoot.get(rootKey) || new Set();
      for (const key of previousUris) {
        compilerByUri.delete(key);
        publish(vscode.Uri.parse(key));
      }
      const grouped = new Map();
      for (const item of [...checkPayload.diagnostics, ...lintPayload.diagnostics]) {
        const uri = diagnosticUri(item, cwd) || document.uri;
        const key = uri.toString();
        if (!grouped.has(key)) grouped.set(key, { uri, values: [] });
        grouped.get(key).values.push(toVsDiagnostic(item));
      }
      if (!grouped.has(document.uri.toString())) grouped.set(document.uri.toString(), { uri: document.uri, values: [] });
      const currentUris = new Set();
      for (const [key, group] of grouped) {
        currentUris.add(key);
        compilerByUri.set(key, group.values);
        publish(group.uri);
      }
      compilerUrisByRoot.set(rootKey, currentUris);
      compilerWarningShown = false;
      if (explicit) vscode.window.showInformationMessage(checkPayload.status === 'ok' ? 'Aziky check passed.' : 'Aziky check completed with diagnostics.');
    } catch (error) {
      output.appendLine(`Compiler diagnostics failed for ${document.uri.fsPath}: ${error.message}`);
      if (!compilerWarningShown || explicit) {
        compilerWarningShown = true;
        vscode.window.showWarningMessage(`Aziky compiler unavailable: ${error.message}. Configure aziky.compiler.path if needed.`);
      }
    }
  }

  function allDefinitions(name) {
    const matches = [];
    for (const definitions of documentSymbols.values()) {
      for (const definition of definitions) if (!name || definition.name === name) matches.push(definition);
    }
    return matches;
  }

  async function locationFor(definition) {
    const uri = vscode.Uri.parse(definition.uri);
    const document = await vscode.workspace.openTextDocument(uri);
    const start = document.positionAt(definition.offset);
    return new vscode.Location(uri, new vscode.Range(start, document.positionAt(definition.offset + definition.length)));
  }

  const formatter = vscode.languages.registerDocumentFormattingEditProvider(LANGUAGE_ID, {
    async provideDocumentFormattingEdits(document) {
      const result = await runCompiler(document.uri, ['fmt', '--stdin'], document.getText());
      if (result.code !== 0) throw new Error(result.stderr.trim() || 'Aziky formatter failed');
      if (result.stdout === document.getText()) return [];
      const end = document.positionAt(document.getText().length);
      return [vscode.TextEdit.replace(new vscode.Range(new vscode.Position(0, 0), end), result.stdout)];
    }
  });

  const completion = vscode.languages.registerCompletionItemProvider(LANGUAGE_ID, {
    provideCompletionItems(document) {
      const items = [];
      for (const keyword of service.KEYWORDS) {
        const item = new vscode.CompletionItem(keyword, vscode.CompletionItemKind.Keyword);
        item.detail = 'Aziky keyword';
        item.documentation = service.KEYWORD_DOCS[keyword];
        item.sortText = `2-${keyword}`;
        items.push(item);
      }
      for (const type of service.TYPES) {
        const item = new vscode.CompletionItem(type, vscode.CompletionItemKind.TypeParameter);
        item.detail = 'Aziky type';
        item.documentation = service.TYPE_DOCS[type];
        item.sortText = `1-${type}`;
        items.push(item);
      }
      const seen = new Set([...service.KEYWORDS, ...service.TYPES]);
      for (const definition of allDefinitions()) {
        if (seen.has(definition.name)) continue;
        seen.add(definition.name);
        const kinds = {
          function: vscode.CompletionItemKind.Function, struct: vscode.CompletionItemKind.Struct,
          enum: vscode.CompletionItemKind.Enum, trait: vscode.CompletionItemKind.Interface,
          module: vscode.CompletionItemKind.Module, variable: vscode.CompletionItemKind.Variable,
          parameter: vscode.CompletionItemKind.Variable, alias: vscode.CompletionItemKind.Reference
        };
        const item = new vscode.CompletionItem(definition.name, kinds[definition.kind] || vscode.CompletionItemKind.Text);
        item.detail = definition.signature;
        item.documentation = definition.documentation;
        item.sortText = definition.uri === document.uri.toString() ? `0-${definition.name}` : `1-${definition.name}`;
        items.push(item);
      }
      return items;
    }
  }, ':', '.');

  const hover = vscode.languages.registerHoverProvider(LANGUAGE_ID, {
    provideHover(document, position) {
      const target = service.wordAt(document.getText(), document.offsetAt(position));
      if (!target) return undefined;
      const builtin = service.builtinHover(target.word);
      const matches = allDefinitions(target.word);
      const locals = matches
        .filter((item) => item.uri === document.uri.toString() && item.offset <= target.start)
        .sort((left, right) => right.offset - left.offset);
      const definition = locals[0] || matches[0];
      const information = definition || builtin;
      if (!information) return undefined;
      const markdown = new vscode.MarkdownString();
      markdown.appendCodeblock(information.signature || `${information.kind} ${target.word}`, 'aziky');
      if (information.documentation) markdown.appendMarkdown(`\n${information.documentation}`);
      return new vscode.Hover(markdown, new vscode.Range(document.positionAt(target.start), document.positionAt(target.end)));
    }
  });

  const definition = vscode.languages.registerDefinitionProvider(LANGUAGE_ID, {
    async provideDefinition(document, position) {
      const target = service.wordAt(document.getText(), document.offsetAt(position));
      if (!target) return undefined;

      // A module declaration resolves to either name.azk or name/mod.azk beside the declaring file.
      const before = document.getText().slice(Math.max(0, target.start - 12), target.start);
      if (/\bmod\s+$/.test(before)) {
        for (const candidate of [
          path.join(path.dirname(document.uri.fsPath), `${target.word}.azk`),
          path.join(path.dirname(document.uri.fsPath), target.word, 'mod.azk')
        ]) {
          if (fs.existsSync(candidate)) return new vscode.Location(vscode.Uri.file(candidate), new vscode.Position(0, 0));
        }
      }

      const local = allDefinitions(target.word).filter((item) => item.uri === document.uri.toString());
      const scoped = local
        .filter((item) => ['variable', 'parameter'].includes(item.kind) && item.offset <= target.start)
        .sort((left, right) => right.offset - left.offset);
      const candidates = scoped.length ? [scoped[0]] : (local.length ? local : allDefinitions(target.word));
      if (candidates.length) return Promise.all(candidates.map(locationFor));
      return undefined;
    }
  });

  const watcher = vscode.workspace.createFileSystemWatcher('**/*.azk');
  watcher.onDidCreate(readAndIndex);
  watcher.onDidChange(async (uri) => {
    const open = vscode.workspace.textDocuments.find((document) => document.uri.toString() === uri.toString());
    if (!open || !open.isDirty) await readAndIndex(uri);
  });
  watcher.onDidDelete((uri) => {
    const key = uri.toString();
    documentSymbols.delete(key);
    lexicalByUri.delete(key);
    compilerByUri.delete(key);
    diagnostics.delete(uri);
  });

  context.subscriptions.push(
    output, diagnostics, formatter, completion, hover, definition, watcher,
    vscode.workspace.onDidOpenTextDocument((document) => {
      if (!isAziky(document)) return;
      index(document); updateLexical(document); checkDocument(document);
    }),
    vscode.workspace.onDidChangeTextDocument((event) => {
      if (!isAziky(event.document)) return;
      index(event.document); scheduleLexical(event.document);
    }),
    vscode.workspace.onDidSaveTextDocument((document) => {
      if (!isAziky(document)) return;
      index(document); updateLexical(document); checkDocument(document);
    }),
    vscode.workspace.onDidCloseTextDocument((document) => {
      const timer = timers.get(document.uri.toString());
      if (timer) clearTimeout(timer);
      timers.delete(document.uri.toString());
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (!event.affectsConfiguration('aziky')) return;
      compilerWarningShown = false;
      for (const document of vscode.workspace.textDocuments.filter(isAziky)) {
        updateLexical(document); checkDocument(document);
      }
    }),
    vscode.commands.registerCommand('aziky.checkDocument', () => {
      const document = vscode.window.activeTextEditor?.document;
      if (document && isAziky(document)) return checkDocument(document, true);
      return vscode.window.showInformationMessage('Open an Aziky .azk document first.');
    }),
    vscode.commands.registerCommand('aziky.restartTooling', async () => {
      compilerWarningShown = false;
      documentSymbols.clear();
      await indexWorkspace();
      for (const document of vscode.workspace.textDocuments.filter(isAziky)) {
        index(document); updateLexical(document); await checkDocument(document);
      }
      vscode.window.showInformationMessage('Aziky language tooling restarted.');
    })
  );

  indexWorkspace();
  for (const document of vscode.workspace.textDocuments.filter(isAziky)) {
    index(document); updateLexical(document); checkDocument(document);
  }
}

function deactivate() {}

module.exports = { activate, deactivate };
