'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const service = require('../languageService');

test('lexical analysis accepts nested comments and braces in literals', () => {
  const source = 'fn main() {\n/* outer { /* nested */ } */\nlet value: string = "}";\nlet mark: char = \'}\';\n}\n';
  const result = service.lexicalAnalysis(source);
  assert.deepEqual(result.diagnostics, []);
  assert.equal(result.masked.split('\n').length, source.split('\n').length);
  assert.ok(!result.masked.includes('outer'));
});

test('lexical analysis reports compiler-compatible literal failures', () => {
  const result = service.lexicalAnalysis('fn main() { let value: string = "bad\\q"; }\n/* open');
  assert.ok(result.diagnostics.some((item) => item.message === 'unsupported escape: \\q'));
  assert.ok(result.diagnostics.some((item) => item.message === 'unterminated block comment'));
});

test('document index finds declarations, locals, aliases, and parameters', () => {
  const source = [
    '// fn fake() {}',
    'pub struct Packet { value: u64, }',
    'use math::sum as add;',
    'fn classify(mut packet: Packet) -> u64 {',
    '    let result: u64 = 1u64;',
    '    return result;',
    '}'
  ].join('\n');
  const definitions = service.indexDocument('file:///main.azk', source);
  const names = definitions.map((item) => `${item.kind}:${item.name}`);
  assert.ok(names.includes('struct:Packet'));
  assert.ok(names.includes('alias:add'));
  assert.ok(names.includes('function:classify'));
  assert.ok(names.includes('parameter:packet'));
  assert.ok(names.includes('variable:result'));
  assert.ok(!names.includes('function:fake'));
});

test('word and built-in hover information are stable', () => {
  const source = 'let packet_count: usize = 0usize;';
  assert.deepEqual(service.wordAt(source, source.indexOf('count')), {
    word: 'packet_count',
    start: 4,
    end: 16
  });
  assert.equal(service.builtinHover('usize').kind, 'type');
  assert.match(service.builtinHover('match').documentation, /enum variant/);
});

test('keyword service and grammar cover the canonical Rust lexer', () => {
  const lexer = fs.readFileSync(path.resolve(__dirname, '../../../src/frontend/lexer.rs'), 'utf8');
  const lexerKeywords = [...lexer.matchAll(/^\s*"([a-z][a-z0-9_]*)"\s*=>\s*TokenKind::/gm)]
    .map((match) => match[1])
    .sort();
  assert.deepEqual([...service.KEYWORDS].sort(), lexerKeywords);

  const grammar = JSON.parse(fs.readFileSync(path.resolve(__dirname, '../syntaxes/aziky.tmLanguage.json'), 'utf8'));
  const patterns = Object.values(grammar.repository)
    .flatMap((entry) => entry.match ? [entry.match] : (entry.patterns || []).map((pattern) => pattern.match).filter(Boolean));
  for (const keyword of lexerKeywords) {
    assert.ok(patterns.some((pattern) => new RegExp(pattern).test(keyword)), `grammar does not highlight ${keyword}`);
  }
  for (const type of service.TYPES) {
    assert.ok(patterns.some((pattern) => new RegExp(pattern).test(type)), `grammar does not highlight type ${type}`);
  }
  assert.deepEqual(grammar.repository['block-comment'].patterns, [{ include: '#block-comment' }]);
});
