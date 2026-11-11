'use strict';
const test=require('node:test');const assert=require('node:assert/strict');const fs=require('node:fs');const os=require('node:os');const path=require('node:path');const H=require('./host.cjs');
const c={checkout:__dirname,userProfile:'C:/Users/trfm1',binaries:{node:{path:process.execPath}}};
test('real child process is stopped by the STOP monitor before its delayed write',async()=>{const dir=fs.mkdtempSync(path.join(os.tmpdir(),'sw64-stop-'));const file=path.join(dir,'must-not-exist');let stop=false;const timer=setTimeout(()=>stop=true,100);await assert.rejects(()=>H.execute(c,'node',['-e','setTimeout(()=>require("node:fs").writeFileSync('+JSON.stringify(file)+',"bad"),10000)'],{timeout:5000,check:()=>{if(stop)throw new Error('STOP');}}),/STOP/);clearTimeout(timer);assert(!fs.existsSync(file));});
test('real subprocess timeout is enforced',async()=>{await assert.rejects(()=>H.execute(c,'node',['-e','setTimeout(()=>{},10000)'],{timeout:200}),/timeout/);});
