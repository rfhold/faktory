import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';
const executablePath = process.env.CHROMIUM_EXECUTABLE_PATH || (existsSync(chromium.executablePath()) ? chromium.executablePath() : '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome');
if (!existsSync(executablePath)) throw new Error('Chromium unavailable: set CHROMIUM_EXECUTABLE_PATH (browser verification is not skipped).');
// Optional backend-emitted payloads exercise the compiler/runtime seam without a live backend.
const bench = JSON.parse(await readFile(process.env.COMPANION_BENCH_PAYLOAD || '../../packages/companion-kit/fixtures/bench.json','utf8'));
const generic = JSON.parse(await readFile(process.env.COMPANION_GENERIC_PAYLOAD || '../../packages/companion-kit/fixtures/generic.json','utf8'));
if (process.env.COMPANION_BENCH_PAYLOAD || process.env.COMPANION_GENERIC_PAYLOAD) console.log('Using backend-emitted companion payloads');
const hostile = {...generic, html:generic.html + `<script>parent.__pwned=true;fetch('https://evil.test/script')</script><img src="https://evil.test/img" onerror="parent.__pwned=true"><a href="https://evil.test/link">External</a><form action="https://evil.test/form"><input name="secret"></form><iframe src="https://evil.test/frame"></iframe><svg><foreignObject><script>alert(1)</script></foreignObject><image href="https://evil.test/svg"/><a href="javascript:alert(1)"><text>bad</text></a></svg><div style="background:url(https://evil.test/css)">Unsafe CSS</div><object data="https://evil.test/object"></object>`,css:'@import "https://evil.test/import";body{background:url(https://evil.test/style)}',assets:[...generic.assets,{id:'assets/attack.svg',mime_type:'image/svg+xml',content:`<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><image href="https://evil.test/asset"/><rect width="100" height="100" onload="alert(1)"/></svg>`}]};
hostile.html += `<img data-companion-asset="assets/attack.svg">`;
let payload = bench; let runtimeOrigin = null; const artifactRequests = []; const configRequests = []; const parentRequests = [];
const parentBundle = await build({stdin:{contents:`import {fetchCompanionOrigin,fetchCompanion,mountCompanion} from './web/src/companion/protocol.ts';globalThis.start=async(revision)=>{globalThis.connection?.destroy();document.querySelector('iframe')?.remove();globalThis.state='loading';const origin=await fetchCompanionOrigin();if(!origin){globalThis.state='disabled';return;}const c=await fetchCompanion('fixture',revision);if(!c){globalThis.state='absent';return;}const frame=document.createElement('iframe');frame.title='Model companion';frame.style.width='100%';frame.style.height='600px';frame.style.border='0';frame.setAttribute('sandbox','allow-scripts allow-modals');frame.setAttribute('credentialless','');frame.referrerPolicy='no-referrer';frame.onload=()=>{globalThis.connection=mountCompanion(frame,c,s=>globalThis.state=s)};frame.src=origin+'/frame';document.body.append(frame);};`,resolveDir:'../..',sourcefile:'browser-parent.ts'},bundle:true,write:false,format:'iife',platform:'browser'});
const parentServer = createServer((req,res) => {
  parentRequests.push(req.url);
  if (req.url === '/runtime-config') { configRequests.push(req.headers.cookie);res.writeHead(200,{'Content-Type':'application/json','Cache-Control':'private, no-store','X-Content-Type-Options':'nosniff'});res.end(JSON.stringify({companion_origin:runtimeOrigin}));return; }
  if (req.url?.startsWith('/artifacts/')) { artifactRequests.push({url:req.url,cookie:req.headers.cookie}); res.writeHead(payload ? 200 : 404,{'Content-Type':'application/json'}); res.end(payload ? JSON.stringify(payload) : ''); return; }
  if (req.url === '/parent.js') { res.writeHead(200,{'Content-Type':'text/javascript'});res.end(parentBundle.outputFiles[0].text);return; }
  res.writeHead(200,{'Content-Type':'text/html'});res.end('<!doctype html><title>Parent test</title><script src="/parent.js"></script>');
});
await new Promise(resolve => parentServer.listen(0,'127.0.0.1',resolve));
const parentOrigin = `http://127.0.0.1:${parentServer.address().port}`;
const reserve = createServer(); await new Promise(resolve => reserve.listen(0,'127.0.0.1',resolve)); const hostPort=reserve.address().port; await new Promise(resolve => reserve.close(resolve));
const hostOrigin = `http://localhost:${hostPort}`;
const host = spawn(process.execPath,['dist/server.js'],{env:{...process.env,PORT:String(hostPort),COMPANION_PARENT_ORIGINS:parentOrigin},stdio:['ignore','pipe','pipe']});
let hostLogs='';host.stderr.on('data',d=>hostLogs+=d);
let browser;
try {
  for (let i=0;i<100;i++) { try { if ((await fetch(hostOrigin+'/ready')).ok) break; } catch {} await new Promise(r=>setTimeout(r,50)); if(i===99) throw new Error('Host failed: '+hostLogs); }
  const hostResponse = await fetch(hostOrigin+'/frame'); const csp = hostResponse.headers.get('content-security-policy');
  assert.match(csp,/connect-src 'none'/);assert.match(csp,/frame-ancestors http:\/\/127\.0\.0\.1:/);assert.match(csp,/script-src 'nonce-/);
  assert.equal((await fetch(hostOrigin+'/private')).status,404);
  browser = await chromium.launch({executablePath,headless:true});
  const context = await browser.newContext(); await context.addCookies([{name:'parent_session',value:'secret-parent-only',url:parentOrigin}]);
  const page = await context.newPage(); const requests=[]; const dialogs=[]; const networkAttempts=[];
  await page.route('https://evil.test/**',route=>{networkAttempts.push(route.request().url());return route.abort();});
  page.on('request',req=>requests.push({url:req.url(),headers:req.headers()})); page.on('dialog',dialog=>{dialogs.push(dialog.message());void dialog.dismiss();});
  await page.goto(parentOrigin); await page.evaluate(origin=>globalThis.hostOrigin=origin,hostOrigin);
   await page.evaluate(revision=>globalThis.start(revision),bench.revision);assert.equal(await page.evaluate(()=>globalThis.state),'disabled');assert.equal(await page.locator('iframe').count(),0);assert.equal(artifactRequests.length,0);
   runtimeOrigin=hostOrigin;
   await page.evaluate(revision=>globalThis.start(revision),bench.revision); await page.waitForFunction(()=>globalThis.state==='ready');
  let frame=page.frames().find(f=>f.url()===hostOrigin+'/frame'); assert.ok(frame);
  assert.match(await frame.locator('h1').textContent(),/Bench/);
  assert.equal(await frame.locator('section[data-tab="Parts"]').isVisible(),false);
  await frame.getByRole('tab',{name:'Parts'}).click(); assert.equal(await frame.locator('section[data-tab="Parts"]').isVisible(),true);
  assert.match(await frame.locator('fk-cut-layout').textContent(),/not globally optimal/);
  assert.equal(await frame.locator('fk-cut-layout svg').count(),2);
  assert.equal(await frame.locator('img').getAttribute('src').then(s=>s.startsWith('blob:null/')),true);
  await frame.waitForFunction(()=>Array.from(document.images).every(image=>image.complete && image.naturalWidth>0));
  const isolation = await frame.evaluate(() => {
    const result={origin:location.origin,parentAccess:false,cookieAccess:false,storageAccess:false};
    try {parent.document.body;result.parentAccess=true;} catch {}
    try {document.cookie;result.cookieAccess=true;} catch {}
    try {localStorage.length;result.storageAccess=true;} catch {}
    return result;
  });
  assert.equal(isolation.parentAccess,false);assert.equal(isolation.cookieAccess,false);assert.equal(isolation.storageAccess,false);
   assert.equal(artifactRequests[0].cookie,'parent_session=secret-parent-only');
   assert.equal(configRequests[0],'parent_session=secret-parent-only');
  assert.equal(requests.find(r=>r.url===hostOrigin+'/frame').headers.cookie,undefined);
   console.log('PASS: two origins, authenticated parent fetch, credentialless opaque sandbox, kit tabs and local SVG');
   await page.setViewportSize({width:390,height:844});
   assert.equal(await frame.evaluate(()=>document.documentElement.scrollWidth <= innerWidth),true);
   assert.equal(await frame.locator('h1').isVisible(),true);
   await page.emulateMedia({media:'print'});
   assert.equal(await frame.locator('section[data-tab="Parts"]').isVisible(),true);
   assert.equal(await frame.locator('[role="tablist"]').isVisible(),false);
   assert.equal(await frame.locator('table').first().evaluate(el=>getComputedStyle(el).display),'table');
   const pdf=await page.pdf({format:'A4'});assert.ok(pdf.byteLength>1000);
   await page.emulateMedia({media:'screen'});await page.setViewportSize({width:1280,height:900});
   console.log('PASS: 390px mobile layout and printable document, all tab panels and PDF output');
   payload={...generic,css:generic.css+'/* authored comment */ h1 { color: r\\65 d; }',html:generic.html+'<fk-cut-layout data-companion-asset="assets/sheets.json"></fk-cut-layout>',assets:[...generic.assets,{id:'assets/sheets.json',mime_type:'application/json',content:JSON.stringify({kind:'sheet',input:{units:'mm',kerf:3,stock:[{id:'sheet',width:1000,height:1000,count:2,grain:'x'}],parts:[{id:'panel',width:300,height:400,count:4,grain:'none',rotate:true}]}})}]};
   await page.evaluate(revision=>globalThis.start(revision),generic.revision);await page.waitForFunction(()=>globalThis.state==='ready');frame=page.frames().find(f=>f.url()===hostOrigin+'/frame');
   assert.match(await frame.locator('fk-cut-layout').textContent(),/guillotine-horizontal/);assert.match(await frame.locator('fk-cut-layout').textContent(),/guillotine-vertical/);
   assert.equal(await frame.locator('h1').evaluate(el=>getComputedStyle(el).color),'rgb(255, 0, 0)');
  console.log('PASS: optional deterministic sheet-cut comparison and SVG/table output');
   payload=hostile; await page.evaluate(revision=>globalThis.start(revision),generic.revision); await page.waitForFunction(()=>globalThis.state==='ready'); frame=page.frames().find(f=>f.url()===hostOrigin+'/frame');
  assert.match(await frame.locator('h1').textContent(),/Sensor/);
  assert.equal(await frame.locator('#companion script,#companion iframe,#companion object,#companion form,#companion foreignObject').count(),0);
  assert.equal(await frame.locator('[onerror],[onload],[href]').count(),0);
  assert.equal(await page.evaluate(()=>globalThis.__pwned),undefined);
  assert.match(await frame.locator('fk-measurement').textContent(),/20 %/);
  const blocked = await frame.evaluate(async () => {
    const script=document.createElement('script');script.textContent='globalThis.__cspExecuted=true';document.body.append(script);
    const external=document.createElement('script');external.src='https://evil.test/csp-script';document.body.append(external);
    const el=document.createElement('img');el.src='https://evil.test/csp-image';document.body.append(el);
    const style=document.createElement('style');style.textContent='body{background:url(https://evil.test/csp-style)}';document.body.append(style);
    try {await fetch('/artifacts/fixture/fixture-1/companion',{credentials:'include'});return false;} catch {return globalThis.__cspExecuted!==true;}
  });
  assert.equal(blocked,true); await page.waitForTimeout(100);
  assert.equal(networkAttempts.length,0); assert.equal(dialogs.length,0);
  console.log('PASS: hostile HTML/SVG/CSS sanitization and CSP script/handler/network defense');
  // Global content, wrong nonce/revision and replay cannot mutate an established private port session.
  await frame.evaluate(()=>{globalThis.print=()=>globalThis.__printCount=(globalThis.__printCount??0)+1;});
  await page.evaluate(() => {const frame=document.querySelector('iframe');frame.contentWindow.postMessage({type:'companion:content',companion:{html:'<h1>SPOOF</h1>'}},'*');globalThis.connection.print();});
  await frame.waitForFunction(()=>globalThis.__printCount===1);
  assert.match(await frame.locator('h1').textContent(),/Sensor/);
  const disposedUrl=await frame.locator('img[src^="blob:"]').first().getAttribute('src');
  await page.evaluate(()=>globalThis.connection.destroy());await frame.waitForFunction(()=>document.querySelector('#companion').childNodes.length===0);
  assert.equal(await frame.evaluate(url=>new Promise(resolve=>{const image=new Image();image.onload=()=>resolve(false);image.onerror=()=>resolve(true);image.src=url;}),disposedUrl),true);
  console.log('PASS: explicit trusted print action and disposal closes the session/revokes asset blobs');
  // A fresh frame accepts neither self-origin spoofing nor wrong parent handshake envelopes.
  await page.evaluate(() => {globalThis.connection.destroy();document.querySelector('iframe').remove();const f=document.createElement('iframe');f.setAttribute('sandbox','allow-scripts');f.src=globalThis.hostOrigin+'/frame';document.body.append(f);});
  await page.waitForFunction(()=>document.querySelector('iframe')?.contentWindow);await page.waitForTimeout(100);frame=page.frames().find(f=>f.url()===hostOrigin+'/frame');
  await frame.evaluate(() => {const ch=new MessageChannel();globalThis.postMessage({type:'companion:init',nonce:'a'.repeat(32),revision:'fixture-1'},'*',[ch.port2]);});
  await page.evaluate(async () => {const f=document.querySelector('iframe');const ch=new MessageChannel();globalThis.badReady=false;ch.port1.onmessage=()=>globalThis.badReady=true;f.contentWindow.postMessage({type:'companion:init',nonce:'bad',revision:'fixture-1'},'*',[ch.port2]);});
  await page.waitForTimeout(50);assert.equal(await page.evaluate(()=>globalThis.badReady),false);
  // Establish a valid channel then submit wrong nonce, followed by an over-limit document.
  await page.evaluate(() => {const f=document.querySelector('iframe');const ch=new MessageChannel();globalThis.testPort=ch.port1;globalThis.testMessages=[];ch.port1.onmessage=e=>globalThis.testMessages.push(e.data);f.contentWindow.postMessage({type:'companion:init',nonce:'b'.repeat(32),revision:'fixture-1'},'*',[ch.port2]);});
  await page.waitForFunction(()=>globalThis.testMessages.length===1);
  await page.evaluate(() => globalThis.testPort.postMessage({type:'companion:content',nonce:'c'.repeat(32),revision:'fixture-1',companion:{}}));await page.waitForTimeout(50);assert.equal(await frame.locator('#companion').textContent(),'');
  await page.evaluate(() => globalThis.testPort.postMessage({type:'companion:content',nonce:'b'.repeat(32),revision:'fixture-1',companion:{format:'faktory-companion-v1',kit_version:'1',revision:'fixture-1',html:'x'.repeat(1048577),css:'',assets:[]}}));
  await page.waitForFunction(()=>globalThis.testMessages.some(m=>m.type==='companion:error'));assert.match(await frame.locator('#companion').textContent(),/invalid document/);
   console.log('PASS: spoofed global/self messages, malformed handshake, nonce mismatch, bounded document error');
   await page.addInitScript(()=>{
     globalThis.__createdBlobs=[];globalThis.__revokedBlobs=[];
     const create=URL.createObjectURL.bind(URL),revoke=URL.revokeObjectURL.bind(URL);
     URL.createObjectURL=blob=>{const url=create(blob);globalThis.__createdBlobs.push(url);return url;};
     URL.revokeObjectURL=url=>{globalThis.__revokedBlobs.push(url);return revoke(url);};
   });
   const largeBoardAsset={id:'assets/large.json',mime_type:'application/json',content:JSON.stringify({kind:'board',input:{units:'mm',kerf:0,stock:[{id:'board',length:1,count:512}],parts:[{id:'part',length:1,count:512}]}})};
   const budgetPayloads=[
     {...generic,html:'<fk-cut-layout data-companion-asset="assets/large.json"></fk-cut-layout>'.repeat(1000),assets:[largeBoardAsset]},
     {...generic,html:'<fk-cut-layout data-companion-asset="assets/large.json"></fk-cut-layout>'.repeat(2),assets:[largeBoardAsset]},
     {...generic,html:'<fk-tabs>'+ '<section data-tab="Tab"></section>'.repeat(8000)+'</fk-tabs>',assets:[]},
     {...generic,html:'<img data-companion-asset="assets/budget.svg">'.repeat(2)+'<fk-asset data-companion-asset="assets/text.json"></fk-asset>'.repeat(4),assets:[{id:'assets/budget.svg',mime_type:'image/svg+xml',content:'<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>'},{id:'assets/text.json',mime_type:'application/json',content:JSON.stringify({text:'x'.repeat(400000)})}]},
   ];
   for(const oversized of budgetPayloads){
     payload=oversized;await page.evaluate(revision=>globalThis.start(revision),generic.revision);await page.waitForFunction(()=>globalThis.state==='error');
     frame=page.frames().find(f=>f.url()===hostOrigin+'/frame');
     assert.equal(await frame.locator('#companion *').count(),0);
     assert.match(await frame.locator('#companion').textContent(),/invalid document/);
     const blobs=await frame.evaluate(()=>({created:globalThis.__createdBlobs,revoked:globalThis.__revokedBlobs}));
     assert.deepEqual(blobs.revoked,blobs.created);
     if(oversized===budgetPayloads.at(-1)) assert.equal(blobs.created.length,1);
   }
   console.log('PASS: aggregate node/widget/custom-element/JSON budgets reject small expansion attacks before attachment');
   payload=generic;await page.evaluate(revision=>globalThis.start(revision),generic.revision);await page.waitForFunction(()=>globalThis.state==='ready');
  payload={...generic,revision:'fixture-2',html:'<h1>Replacement revision</h1>'};await page.evaluate(()=>globalThis.start('fixture-2'));await page.waitForFunction(()=>globalThis.state==='ready');
  assert.equal(page.frames().filter(f=>f.url()===hostOrigin+'/frame').length,1);frame=page.frames().find(f=>f.url()===hostOrigin+'/frame');assert.equal(await frame.locator('h1').textContent(),'Replacement revision');
  payload=null;await page.evaluate(()=>globalThis.start('fixture-3'));assert.equal(await page.evaluate(()=>globalThis.state),'absent');assert.equal(await page.locator('iframe').count(),0);
  console.log('PASS: successful revision replacement, teardown and absent404 using mock artifact API');
  // frame-ancestors refuses a third origin, even if that page supplies a channel.
  const disallowed=await context.newPage();await disallowed.goto(hostOrigin+'/health');await disallowed.evaluate(origin=>{const f=document.createElement('iframe');f.src=origin+'/frame';document.body.append(f);},hostOrigin);await disallowed.waitForTimeout(100);
  assert.equal(disallowed.frames().some(f=>f.url()===hostOrigin+'/frame'),false);await disallowed.close();
  console.log('PASS: exact parent origin allowlist enforced by frame-ancestors');
} finally {await browser?.close();host.kill('SIGTERM');await new Promise(resolve=>parentServer.close(resolve));}
