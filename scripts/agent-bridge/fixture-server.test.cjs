'use strict';
const test=require('node:test'); const assert=require('node:assert/strict'); const fs=require('node:fs'); const os=require('node:os'); const path=require('node:path'); const cp=require('node:child_process');
const {TASK,EXPECTED,sha}=require('./core.cjs');
function fixture(change=()=>{}) {
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'sw63-gate-test-'));const rt=path.join(root,'.runtime');fs.mkdirSync(rt);
 for(const file of ['core.cjs','fixture-server.cjs'])fs.copyFileSync(path.join(__dirname,file),path.join(root,file));
 const put=(n,v)=>fs.writeFileSync(path.join(rt,n),JSON.stringify(v)+'\n');
 put('recipe.json',{task_id:TASK,steps:[{op:'filter_positive'},{op:'square'},{op:'sum'}]});const hash=sha(fs.readFileSync(path.join(rt,'recipe.json')));
 put('state.json',{stage:'claude-executing',seq:5});put('review.json',{task_id:TASK,artifact_sha256:hash,verdict:'ACCEPT',case_results:EXPECTED,findings:[]});
 put('approval.json',{task_id:TASK,artifact_sha256:hash,review_sha256:sha(fs.readFileSync(path.join(rt,'review.json'))),scope:'synthetic-fixed-fixture-only'});
 put('event-5.json',{stage:'claude-executing'});put('ack-5.json',{event_sha256:sha(fs.readFileSync(path.join(rt,'event-5.json')))});
 change({root,rt,put,hash});
 const call={jsonrpc:'2.0',id:1,method:'tools/call',params:{name:'execute_approved_fixture',arguments:{task_id:TASK,artifact_sha256:hash}}};
 const child=cp.spawnSync(process.execPath,[path.join(root,'fixture-server.cjs')],{input:JSON.stringify(call)+'\n'+JSON.stringify({...call,id:2})+'\n',encoding:'utf8',windowsHide:true,timeout:5000});assert.equal(child.status,0);
 return {answers:child.stdout.trim().split('\n').map(JSON.parse),receipt:fs.existsSync(path.join(rt,'execution-receipt.json'))};
 // Test directories deliberately remain in OS temp: no recursive cleanup/deletion capability.
}
test('approved MCP call executes only fixed fixtures, idempotently',()=>{const r=fixture();assert(r.receipt);assert.deepEqual(r.answers[0].result,r.answers[1].result);assert.deepEqual(JSON.parse(r.answers[0].result.content[0].text).case_results,EXPECTED);});
test('STOP prevents fixture side effects',()=>{const r=fixture(({put})=>put('STOP',true));assert(!r.receipt);assert(r.answers[0].result.isError);});
test('missing review fails closed',()=>{const r=fixture(({rt})=>fs.renameSync(path.join(rt,'review.json'),path.join(rt,'withheld-review.json')));assert(!r.receipt);assert(r.answers[0].result.isError);});
test('changed approval or artifact prevents execution',()=>{const r=fixture(({put,hash})=>put('approval.json',{task_id:TASK,artifact_sha256:hash,review_sha256:'0'.repeat(64),scope:'synthetic-fixed-fixture-only'}));assert(!r.receipt);assert(r.answers[0].result.isError);});
test('unacknowledged GitHub transition prevents execution',()=>{const r=fixture(({put})=>put('ack-5.json',{event_sha256:'0'.repeat(64)}));assert(!r.receipt);assert(r.answers[0].result.isError);});
test('wrong stage prevents premature execution',()=>{const r=fixture(({put})=>put('state.json',{stage:'claude-review',seq:5}));assert(!r.receipt);assert(r.answers[0].result.isError);});
