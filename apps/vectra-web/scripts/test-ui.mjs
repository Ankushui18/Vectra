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
  // `lucide-react` ships CJS *and* ESM. esbuild's Node platform default prefers
  // `main` (the CJS build), and a CJS build bundled into an ESM output does a
  // dynamic `require('react')` — which Node's ESM loader refuses. Preferring
  // `module` picks the ESM build, which imports React the way the output
  // expects: the icon set then bundles and SSR-renders like every other
  // component (Task 13.0 RULE 5).
  mainFields: ['module', 'main'],
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

// 3. Run. The bundles are named explicitly: `node --test dist/` only expands a
//    directory on some Node versions (22.22 treats the argument as a file and
//    dies with MODULE_NOT_FOUND), while one bundle per entry point is the same
//    set of tests on every version.
const bundles = tests.map((name) =>
  join('dist', name.replace(/^tests[/\\]/, '').replace(/\.tsx?$/, '.mjs')).replace(/\\/g, '/'),
);
const run = spawnSync('node', ['--test', ...bundles], { cwd: root, stdio: 'inherit' });
process.exit(run.status ?? 1);
