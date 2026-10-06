/**
 * The UI test runner (Task 2.2 §4, extended in 10.2).
 *
 * ```text
 *   tsc -p tsconfig.tests.json        type-check the tests themselves
 *   esbuild tests/**                 one bundle per test file
 *   node --test dist/                run them
 * ```
 *
 * esbuild does the bundling because the tests import the real React
 * components — including `App.tsx`, which imports the WASM client. That client
 * uses Vite's `?url` import for the module itself, which no other bundler
 * understands: the plugin below resolves those to an empty stub, so an
 * App-level test can render the *shell* in Node (the client reports "no
 * WebGPU", the canvas takes its banner path) without a browser or a device.
 *
 * That is deliberate: a test that can render the whole workspace cannot go
 * stale the way a test of each panel in isolation can.
 */
import { build } from 'esbuild';
import { spawnSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const tests = readdirSync(join(root, 'tests'))
  .filter((name) => /\.test\.tsx?$/.test(name))
  .map((name) => join('tests', name));

// 1. Types first: a bundle would run code that does not type-check.
const typecheck = spawnSync('npx', ['tsc', '-p', 'tsconfig.tests.json'], {
  cwd: root,
  stdio: 'inherit',
});
if (typecheck.status !== 0) process.exit(typecheck.status ?? 1);

// 2. Bundle. `?url` (Vite's asset import) becomes a stub string; React and
//    react-dom stay external because they are CJS and Node resolves them.
await build({
  entryPoints: tests,
  outdir: 'dist',
  bundle: true,
  platform: 'node',
  format: 'esm',
  outExtension: { '.js': '.mjs' },
  external: ['react', 'react-dom', 'react-dom/server'],
  loader: { '.tsx': 'tsx' },
  plugins: [
    {
      name: 'vite-url-imports',
      setup(pluginBuild) {
        pluginBuild.onResolve({ filter: /\?url$/ }, (args) => ({
          path: args.path,
          namespace: 'vite-url',
        }));
        pluginBuild.onLoad({ filter: /.*/, namespace: 'vite-url' }, () => ({
          contents: 'export default "";',
          loader: 'js',
        }));
      },
    },
  ],
  logLevel: 'warning',
});

// 3. Run.
const run = spawnSync('node', ['--test', 'dist/'], { cwd: root, stdio: 'inherit' });
process.exit(run.status ?? 1);
