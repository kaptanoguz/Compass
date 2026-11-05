#!/usr/bin/env node
// Masaüstü penceresi olmadan, tarayıcıda çalıştırmak için.
import { spawn } from 'node:child_process';
import { startServer } from '../src/server.js';

const port = Number(process.env.PORT) || 4517;
const app = await startServer({ port });
console.log(`Compass hazır: ${app.url}`);
if (!process.argv.includes('--no-open')) {
  const cmd = process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'cmd' : 'xdg-open';
  const args = process.platform === 'win32' ? ['/c', 'start', '', app.url] : [app.url];
  spawn(cmd, args, { detached: true, stdio: 'ignore' }).on('error', () => {}).unref();
}
const stop = async () => { await app.close(); process.exit(0); };
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
