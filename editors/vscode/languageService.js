'use strict';

const KEYWORD_DOCS = Object.freeze({
  fn: 'Declares a function.', struct: 'Declares a structure type.', enum: 'Declares an enum type.',
  trait: 'Declares a trait contract.', impl: 'Implements inherent or trait methods.',
  embed: 'Embeds a structure field.', pub: 'Exports a top-level declaration or import.',
  as: 'Introduces an import alias.', mod: 'Declares a child module.', use: 'Imports a public child item.',
  let: 'Introduces a local binding.', mut: 'Marks a binding, parameter, or reference as mutable.',
  if: 'Starts a conditional expression.', else: 'Provides the alternative conditional branch.',
  match: 'Exhaustively selects an enum variant.', while: 'Runs a conditional loop.',
  loop: 'Runs an infinite loop.', for: 'Iterates over an integer range.',
  parfor: 'Runs a parallel range loop.', foreach: 'Iterates through a collection.',
  in: 'Separates a loop binding from its range or iterator.', break: 'Exits the nearest loop.',
  continue: 'Continues the nearest loop.', return: 'Returns from the current function.',
  assert: 'Checks an assertion.', panic: 'Terminates execution with an error.',
  true: 'Boolean true literal.', false: 'Boolean false literal.',
  print: 'Writes a value to standard output.', exit: 'Terminates the process with an exit code.',
  benchloop: 'Runs the compiler-recognized benchmark loop construct.'
});

const TYPE_DOCS = Object.freeze({
  u8: '8-bit unsigned integer.', u16: '16-bit unsigned integer.', u32: '32-bit unsigned integer.',
  u64: '64-bit unsigned integer.', u128: '128-bit unsigned integer.',
  i8: '8-bit signed integer.', i16: '16-bit signed integer.', i32: '32-bit signed integer.',
  i64: '64-bit signed integer.', i128: '128-bit signed integer.',
  usize: 'Pointer-sized unsigned integer.', isize: 'Pointer-sized signed integer.',
  f32: '32-bit floating-point number.', f64: '64-bit floating-point number.',
  bool: 'Boolean value.', byte: 'One byte value.', char: 'Unicode scalar value.',
  string: 'Owned UTF-8 string.', dict: 'Dictionary collection type: dict<K, V>.',
  list: 'List collection type: list<T>.', map: 'Ordered map collection type: map<K, V>.',
  Path: 'Standard-library filesystem path.', File: 'Standard-library file handle.',
  Thread: 'Standard-library thread handle.'
});

const BUILTIN_DOCS = Object.freeze({
  print: 'print(value)\n\nWrites a value to standard output.',
  exit: 'exit(code)\n\nTerminates the current process.',
  assert: 'assert(condition[, message])\n\nFails when the condition is false.',
  panic: 'panic(message)\n\nTerminates execution with an error.',
  benchloop: 'benchloop(iterations)\n\nMarks a deterministic benchmark loop.'
});

const KEYWORDS = Object.freeze(Object.keys(KEYWORD_DOCS));
const TYPES = Object.freeze(Object.keys(TYPE_DOCS));

function lexicalAnalysis(source) {
  const masked = source.split('');
  const diagnostics = [];
  let index = 0;

  const blank = (start, end) => {
    for (let i = start; i < end; i += 1) {
      if (masked[i] !== '\n' && masked[i] !== '\r') masked[i] = ' ';
    }
  };
  const report = (start, end, code, message) => diagnostics.push({ start, end, code, message });

  while (index < source.length) {
    const ch = source[index];
    const next = source[index + 1];
    if (/\s/.test(ch)) { index += 1; continue; }

    if (ch === '/' && next === '/') {
      const start = index;
      index += 2;
      while (index < source.length && source[index] !== '\n') index += 1;
      blank(start, index);
      continue;
    }
    if (ch === '/' && next === '*') {
      const start = index;
      let depth = 1;
      index += 2;
      while (index < source.length && depth > 0) {
        if (source[index] === '/' && source[index + 1] === '*') { depth += 1; index += 2; }
        else if (source[index] === '*' && source[index + 1] === '/') { depth -= 1; index += 2; }
        else index += 1;
      }
      blank(start, index);
      if (depth !== 0) report(start, Math.min(start + 2, source.length), 'AZK-ELEX', 'unterminated block comment');
      continue;
    }
    if (ch === '"') {
      const start = index++;
      let closed = false;
      while (index < source.length) {
        if (source[index] === '"') { index += 1; closed = true; break; }
        if (source[index] === '\n' || source[index] === '\r') break;
        if (source[index] === '\\') {
          const escapeStart = index;
          index += 1;
          if (index >= source.length) {
            report(escapeStart, index, 'AZK-ELEX', 'unterminated escape sequence');
            break;
          }
          if (!'nt"\\'.includes(source[index])) {
            report(escapeStart, index + 1, 'AZK-ELEX', `unsupported escape: \\${source[index]}`);
          }
        }
        index += 1;
      }
      blank(start, index);
      if (!closed) report(start, Math.min(start + 1, source.length), 'AZK-ELEX', 'unterminated string literal');
      continue;
    }
    if (ch === "'") {
      const start = index++;
      let validValue = true;
      if (index >= source.length || source[index] === '\n' || source[index] === '\r') {
        validValue = false;
      } else if (source[index] === '\\') {
        const escapeStart = index++;
        if (index >= source.length || !'nrt0\'"\\'.includes(source[index])) {
          const value = source[index] || '';
          report(escapeStart, Math.min(index + 1, source.length), 'AZK-ELEX', `unsupported char escape: \\${value}`);
        }
        index += index < source.length ? 1 : 0;
      } else {
        const point = source.codePointAt(index);
        index += point > 0xffff ? 2 : 1;
      }
      if (source[index] === "'") index += 1;
      else validValue = false;
      blank(start, index);
      if (!validValue) report(start, Math.min(start + 1, source.length), 'AZK-ELEX', 'char literal must contain exactly one Unicode scalar value');
      continue;
    }
    if (/[A-Za-z_]/.test(ch)) {
      index += 1;
      while (index < source.length && /[A-Za-z0-9_]/.test(source[index])) index += 1;
      continue;
    }
    if (/[0-9]/.test(ch)) {
      index += 1;
      while (index < source.length && /[A-Za-z0-9.]/.test(source[index])) index += 1;
      continue;
    }
    if ('(){}[];:,.=+*-/%&|^!<>'.includes(ch)) { index += 1; continue; }

    const width = source.codePointAt(index) > 0xffff ? 2 : 1;
    report(index, index + width, 'AZK-ELEX', `unexpected character: ${source.slice(index, index + width)}`);
    index += width;
  }

  return { masked: masked.join(''), diagnostics };
}

function indexDocument(uri, source) {
  const { masked } = lexicalAnalysis(source);
  const definitions = [];
  const addMatches = (regex, kind, capture = 2) => {
    let match;
    while ((match = regex.exec(masked)) !== null) {
      const name = match[capture];
      const relative = match[0].lastIndexOf(name);
      const offset = match.index + relative;
      const lineStart = source.lastIndexOf('\n', offset - 1) + 1;
      const lineEndRaw = source.indexOf('\n', offset);
      const lineEnd = lineEndRaw === -1 ? source.length : lineEndRaw;
      definitions.push({
        uri, name, kind, offset, length: name.length,
        signature: source.slice(lineStart, lineEnd).trim(),
        documentation: `${kind} ${name}`
      });
    }
  };

  addMatches(/^\s*(?:pub\s+)?(fn)\s+([A-Za-z_][A-Za-z0-9_]*)/gm, 'function');
  addMatches(/^\s*(?:pub\s+)?(struct)\s+([A-Za-z_][A-Za-z0-9_]*)/gm, 'struct');
  addMatches(/^\s*(?:pub\s+)?(enum)\s+([A-Za-z_][A-Za-z0-9_]*)/gm, 'enum');
  addMatches(/^\s*(?:pub\s+)?(trait)\s+([A-Za-z_][A-Za-z0-9_]*)/gm, 'trait');
  addMatches(/^\s*(?:pub\s+)?(mod)\s+([A-Za-z_][A-Za-z0-9_]*)/gm, 'module');
  addMatches(/\b(let)\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)/g, 'variable');
  addMatches(/^\s*(?:pub\s+)?use\s+[A-Za-z_][A-Za-z0-9_]*::[A-Za-z_][A-Za-z0-9_]*\s+(as)\s+([A-Za-z_][A-Za-z0-9_]*)/gm, 'alias');

  // Parameters are definitions local to their function and are useful for hover/navigation.
  const functionHeader = /\bfn\s+[A-Za-z_][A-Za-z0-9_]*\s*\(([^)]*)\)/g;
  let fnMatch;
  while ((fnMatch = functionHeader.exec(masked)) !== null) {
    const paramsStart = fnMatch.index + fnMatch[0].indexOf(fnMatch[1]);
    const paramPattern = /(?:^|,)\s*(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:/g;
    let paramMatch;
    while ((paramMatch = paramPattern.exec(fnMatch[1])) !== null) {
      const name = paramMatch[1];
      const offset = paramsStart + paramMatch.index + paramMatch[0].lastIndexOf(name);
      definitions.push({ uri, name, kind: 'parameter', offset, length: name.length, signature: name, documentation: `parameter ${name}` });
    }
  }

  return definitions;
}

function wordAt(source, offset) {
  if (offset < 0 || offset > source.length) return undefined;
  let start = offset;
  let end = offset;
  while (start > 0 && /[A-Za-z0-9_]/.test(source[start - 1])) start -= 1;
  while (end < source.length && /[A-Za-z0-9_]/.test(source[end])) end += 1;
  if (start === end || !/[A-Za-z_]/.test(source[start])) return undefined;
  return { word: source.slice(start, end), start, end };
}

function builtinHover(word) {
  if (TYPE_DOCS[word]) return { signature: word, documentation: TYPE_DOCS[word], kind: 'type' };
  if (BUILTIN_DOCS[word]) {
    const [signature, ...rest] = BUILTIN_DOCS[word].split('\n');
    return { signature, documentation: rest.join('\n').trim(), kind: 'function' };
  }
  if (KEYWORD_DOCS[word]) return { signature: word, documentation: KEYWORD_DOCS[word], kind: 'keyword' };
  return undefined;
}

module.exports = {
  BUILTIN_DOCS,
  KEYWORDS,
  KEYWORD_DOCS,
  TYPES,
  TYPE_DOCS,
  builtinHover,
  indexDocument,
  lexicalAnalysis,
  wordAt
};
