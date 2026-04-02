import fs from 'node:fs/promises';
import path from 'node:path';
import { spawn } from 'node:child_process';

const rootDir = process.cwd();
const outDir = path.join(rootDir, 'dist');

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: rootDir,
      stdio: 'inherit',
      ...options
    });
    child.on('exit', (code) => {
      if (code === 0) {
        resolve();
        return;
      }
      reject(new Error(`${command} ${args.join(' ')} exited with code ${code}`));
    });
    child.on('error', reject);
  });
}

async function copyPath(source, target) {
  await fs.cp(path.join(rootDir, source), path.join(outDir, target), {
    recursive: true
  });
}

await fs.rm(outDir, { recursive: true, force: true });
await fs.mkdir(outDir, { recursive: true });

await copyPath('apps/web/public', 'apps/web/public');
await copyPath('apps/api/src/fixtures', 'apps/api/src/fixtures');

if (!process.argv.includes('--node-only')) {
  await run('cargo', ['build', '--manifest-path', 'apps/api-rs/Cargo.toml', '--release']);
  await fs.mkdir(path.join(outDir, 'apps/api-rs'), { recursive: true });
  await fs.copyFile(
    path.join(rootDir, 'apps/api-rs/target/release/flink-job-ui-api'),
    path.join(outDir, 'apps/api-rs/flink-job-ui-api')
  );

  await fs.copyFile(path.join(rootDir, 'package.json'), path.join(outDir, 'package.json'));
} else {
  await copyPath('apps/api/src', 'apps/api/src');
}

console.log(`Build complete: ${outDir}`);
