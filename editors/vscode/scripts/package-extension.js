'use strict';

const childProcess = require('child_process');
const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '..');
const manifest = require('../package.json');
const output = path.join(root, 'dist', `aziky-language-${manifest.version}.vsix`);
const executable = path.join(root, 'node_modules', '.bin', process.platform === 'win32' ? 'vsce.cmd' : 'vsce');
fs.mkdirSync(path.dirname(output), { recursive: true });
const result = childProcess.spawnSync(executable, [
  'package',
  '--allow-missing-repository',
  '--no-dependencies',
  '--out',
  output
], { cwd: root, stdio: 'inherit', shell: false });
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
