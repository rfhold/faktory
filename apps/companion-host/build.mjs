import { build } from 'esbuild';
import { mkdir, writeFile } from 'node:fs/promises';
const frame = await build({entryPoints:['src/frame.ts'],bundle:true,write:false,format:'iife',platform:'browser',target:'es2022'});
await mkdir('dist',{recursive:true});
await writeFile('dist/frame.js',frame.outputFiles[0].contents);
await build({entryPoints:['src/server.ts'],bundle:true,format:'esm',platform:'node',target:'node20',outfile:'dist/server.js'});
